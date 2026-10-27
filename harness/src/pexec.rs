//! Drives the Vx block executor (src/evm/block.vx, SPEC §6) on a synthetic
//! block and checks it against sequential execution. The executor and its
//! multi-version store live in the Vx library; this module only builds the
//! state, contributes worker threads, and verifies the outcome.

use crate::env;
use crate::ffi::*;
use crate::host::Host;
use revm::context::{cfg::CfgEnv, BlockEnv, TxEnv};
use revm::primitives::{hardfork::SpecId, Address, Bytes, TxKind, U256};
use std::collections::HashMap;
use std::ffi::c_void;
use std::time::Instant;

const SPEC: SpecId = SpecId::CANCUN;

/// 64 x (counter = sload(0) + 1; sstore(0, counter)), then STOP.
fn contract_code() -> Bytes {
    let seg = [0x60, 0x00, 0x54, 0x60, 0x01, 0x01, 0x60, 0x00, 0x55];
    let mut v = Vec::with_capacity(seg.len() * 64 + 1);
    for _ in 0..64 {
        v.extend_from_slice(&seg);
    }
    v.push(0x00);
    v.into()
}

fn addr(tag: u8, i: u64) -> Address {
    let mut b = [0u8; 20];
    b[0] = tag;
    b[12..].copy_from_slice(&i.to_be_bytes());
    Address::from(b)
}

fn build_state(nsenders: u64, ncontracts: u64) -> Host {
    let mut h = Host::default();
    let code = contract_code();
    for i in 0..ncontracts {
        h.insert(addr(0xc0, i), U256::ZERO, 1, code.clone(), HashMap::new());
    }
    for i in 0..nsenders {
        h.insert(addr(0xaa, i), U256::from(1_000_000_000_000_000_000u64), 0, Bytes::new(), HashMap::new());
    }
    h
}

fn make_tx(i: u64, ncontracts: u64) -> TxEnv {
    let mut t = TxEnv::default();
    t.caller = addr(0xaa, i);
    t.kind = TxKind::Call(addr(0xc0, i % ncontracts));
    t.gas_limit = 1_000_000;
    t.gas_price = 1;
    t.nonce = 0;
    t.chain_id = Some(1);
    t
}

struct BPtr(usize);
impl BPtr {
    fn get(&self) -> *mut c_void {
        self.0 as *mut c_void
    }
}

pub fn run(ntx: usize, nthreads: usize, ncontracts: u64) {
    let mut cfg = CfgEnv::default();
    cfg.set_spec_and_mainnet_gas_params(SPEC);
    let mut block = BlockEnv::default();
    block.number = U256::from(1);
    block.timestamp = U256::from(1000);
    block.gas_limit = 10_000_000_000;
    block.basefee = 0;
    block.beneficiary = addr(0xcc, 0);
    block.prevrandao = Some(Default::default());

    let txs: Vec<TxEnv> = (0..ntx as u64).map(|i| make_tx(i, ncontracts)).collect();
    let envs: Vec<env::EnvBuf> = txs.iter().map(|t| env::build(SPEC, &cfg, &block, t)).collect();
    let h0 = build_state(ntx as u64, ncontracts);

    // Sequential baseline, committed into plain Host maps.
    let mut hseq = h0.clone();
    let mut seq_gas = Vec::with_capacity(ntx);
    let t0 = Instant::now();
    for eb in &envs {
        let res = hseq.transact(&eb.env);
        let r = unsafe { &*res };
        assert_eq!(r.status, RES_SUCCESS, "sequential tx failed, reason {}", r.reason);
        seq_gas.push(r.gas_used);
        hseq.commit(r);
        unsafe { evmvx_result_free(res) };
    }
    let seq = t0.elapsed();

    // Parallel: the Vx block executor over a StateDb.
    let mut hpar = h0.clone();
    hpar.attach_db();
    let db = hpar.db as *mut c_void;
    let venvs: Vec<VxEnv> = envs.iter().map(|eb| unsafe { std::ptr::read(&eb.env) }).collect();
    let t0 = Instant::now();
    let b = unsafe { evmvx_block_new(db, venvs.as_ptr(), ntx as u64) };
    let bp = BPtr(b as usize);
    std::thread::scope(|s| {
        for _ in 0..nthreads {
            let bp = BPtr(bp.0);
            s.spawn(move || unsafe { evmvx_block_worker(bp.get()) });
        }
    });
    let committed = unsafe { evmvx_block_finish(b) };
    let par = t0.elapsed();
    let reexecs = unsafe { evmvx_block_reexecs(b) };
    assert_eq!(committed as usize, ntx, "block did not commit every transaction");

    // Per-transaction results must match the sequential run.
    for (i, want) in seq_gas.iter().enumerate() {
        let r = unsafe { &*evmvx_block_result(b, i as u64) };
        assert_eq!(r.status, RES_SUCCESS, "parallel tx {i} failed, reason {}", r.reason);
        assert_eq!(r.gas_used, *want, "parallel tx {i} gas differs");
    }

    // Final state in the StateDb must match the sequential Host state.
    let mut ok = true;
    let mut ha = HostAccount {
        balance: Word::default(),
        nonce: 0,
        code_hash: Word::default(),
        code: std::ptr::null(),
        code_len: 0,
    };
    for (a, acc) in &hseq.accounts {
        let aw: Word = (*a).into();
        let st = unsafe { evmvx_db_get_account(db, &aw, &mut ha) };
        if st != 0 {
            println!("MISMATCH {a}: missing in parallel state");
            ok = false;
            continue;
        }
        if ha.balance.u256() != acc.balance || ha.nonce != acc.nonce {
            println!(
                "MISMATCH {a}: balance/nonce {} / {} vs {} / {}",
                ha.balance.u256(),
                ha.nonce,
                acc.balance,
                acc.nonce
            );
            ok = false;
        }
        for (k, v) in &acc.storage {
            if v.is_zero() {
                continue;
            }
            let kw: Word = (*k).into();
            let mut out = Word::default();
            unsafe { evmvx_db_get_slot(db, &aw, &kw, &mut out) };
            if out.u256() != *v {
                println!("MISMATCH {a} slot {k:#x}: {} vs {v}", out.u256());
                ok = false;
            }
        }
    }

    let speedup = seq.as_secs_f64() / par.as_secs_f64();
    println!(
        "pexec: {ntx} txs, {ncontracts} contracts, {nthreads} threads | seq {:.1} ms, par {:.1} ms, speedup {speedup:.2}x | {reexecs} re-executions | state match: {}",
        seq.as_secs_f64() * 1e3,
        par.as_secs_f64() * 1e3,
        if ok { "OK" } else { "MISMATCH" }
    );
    assert!(ok, "parallel state differs from sequential");
}

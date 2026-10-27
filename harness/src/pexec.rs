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
use std::time::{Duration, Instant};

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

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

/// revm sequential baseline on the same block: CacheDB + transact_commit.
fn revm_seq(txs: &[TxEnv], block: &BlockEnv, cfg: &CfgEnv, ncontracts: u64, ntx: u64) -> Duration {
    use revm::database::{CacheDB, EmptyDB};
    use revm::state::AccountInfo;
    use revm::primitives::keccak256;
    use revm::{Context, ExecuteCommitEvm, MainBuilder, MainContext};
    let mut db = CacheDB::new(EmptyDB::default());
    let code = contract_code();
    let hash = keccak256(&code);
    let bytecode = revm::state::Bytecode::new_raw(code);
    for i in 0..ncontracts {
        db.insert_account_info(addr(0xc0, i), AccountInfo { balance: U256::ZERO, nonce: 1, code_hash: hash, code: Some(bytecode.clone()), ..Default::default() });
    }
    for i in 0..ntx {
        db.insert_account_info(addr(0xaa, i), AccountInfo { balance: U256::from(1_000_000_000_000_000_000u64), nonce: 0, ..Default::default() });
    }
    let t0 = Instant::now();
    let mut evm = Context::mainnet()
        .with_block(block.clone())
        .with_cfg(cfg.clone())
        .with_db(&mut db)
        .build_mainnet();
    for tx in txs {
        let r = evm.transact_commit(tx.clone()).expect("revm baseline tx failed");
        assert!(r.is_success(), "revm baseline tx not successful");
    }
    t0.elapsed()
}

pub fn run(ntx: usize, nthreads: usize, ncontracts: u64, reps: u32) {
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

    // Sequential vx baseline, committed into plain Host maps; median of reps.
    let mut seq_gas = Vec::with_capacity(ntx);
    let mut hseq = h0.clone();
    let mut seq_times = Vec::new();
    for rep in 0..reps.max(1) {
        let mut h = h0.clone();
        let mut gas = Vec::with_capacity(ntx);
        let t0 = Instant::now();
        for eb in &envs {
            let res = h.transact(&eb.env);
            let r = unsafe { &*res };
            assert_eq!(r.status, RES_SUCCESS, "sequential tx failed, reason {}", r.reason);
            gas.push(r.gas_used);
            h.commit(r);
            unsafe { evmvx_result_free(res) };
        }
        seq_times.push(t0.elapsed());
        if rep == 0 {
            seq_gas = gas;
            hseq = h;
        }
    }
    let seq = median(seq_times);
    let revm = median((0..reps.max(1)).map(|_| revm_seq(&txs, &block, &cfg, ncontracts, ntx as u64)).collect());

    // Parallel: the Vx block executor over a StateDb; median of reps.
    let venvs: Vec<VxEnv> = envs.iter().map(|eb| unsafe { std::ptr::read(&eb.env) }).collect();
    let mut par_times = Vec::new();
    let mut last: Option<(Host, usize, u64)> = None;
    for _ in 0..reps.max(1) {
        let mut hpar = h0.clone();
        hpar.attach_db();
        let db = hpar.db as *mut c_void;
        let t0 = Instant::now();
        let b = unsafe { evmvx_block_new(db, venvs.as_ptr(), ntx as u64) };
        if std::env::var_os("EVMVX_PEXEC_SETUP").is_some() {
            eprintln!("block_new: {:.2} ms", t0.elapsed().as_secs_f64() * 1e3);
        }
        let bp = BPtr(b as usize);
        std::thread::scope(|s| {
            for _ in 0..nthreads {
                let bp = BPtr(bp.0);
                std::thread::Builder::new()
                    .name("vxworker".into())
                    .spawn_scoped(s, move || unsafe { evmvx_block_worker(bp.get()) })
                    .unwrap();
            }
        });
        let t_exec = t0.elapsed();
        let committed = unsafe { evmvx_block_finish(b) };
        par_times.push(t0.elapsed());
        if std::env::var_os("EVMVX_PEXEC_SETUP").is_some() {
            eprintln!(
                "exec+commit: {:.2} ms, finish: {:.2} ms",
                t_exec.as_secs_f64() * 1e3,
                (t0.elapsed() - t_exec).as_secs_f64() * 1e3
            );
        }
        assert_eq!(committed as usize, ntx, "block did not commit every transaction");
        last = Some((hpar, b as usize, unsafe { evmvx_block_reexecs(b) }));
    }
    let par = median(par_times);
    let (hpar, b, reexecs) = last.unwrap();
    let b = b as *mut c_void;
    let db = hpar.db as *mut c_void;

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
    let vs_revm = revm.as_secs_f64() / par.as_secs_f64();
    println!(
        "pexec: {ntx} txs, {ncontracts} contracts, {nthreads} threads, median of {reps} | vx-seq {:.1} ms, revm-seq {:.1} ms, par {:.1} ms | par vs vx-seq {speedup:.2}x, par vs revm {vs_revm:.2}x | {reexecs} re-executions | state match: {}",
        seq.as_secs_f64() * 1e3,
        revm.as_secs_f64() * 1e3,
        par.as_secs_f64() * 1e3,
        if ok { "OK" } else { "MISMATCH" }
    );
    assert!(ok, "parallel state differs from sequential");
}

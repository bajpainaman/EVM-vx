//! Repeated end-to-end CPU measurements with three-way result/state checks.

use crate::env;
use crate::ffi::*;
use crate::host::Host;
use crate::observe::Observation;
use crate::pfuzz::{diff, snap_host, snap_revm};
use crate::workload::{self, addr, Workload};
use revm::context::{cfg::CfgEnv, BlockEnv, TxEnv};
use revm::database::{CacheDB, EmptyDB};
use revm::primitives::{hardfork::SpecId, Bytes, TxKind, U256};
use revm::state::{AccountInfo, Bytecode};
use revm::{Context, ExecuteCommitEvm, MainBuilder, MainContext};
use std::collections::HashMap;
use std::ffi::c_void;
use std::time::{Duration, Instant};

const SPEC: SpecId = SpecId::CANCUN;
const SENDER_BALANCE: u64 = 1_000_000_000_000_000_000;

fn build_state(ntx: usize, contracts: u64, workload: Workload) -> Host {
    let mut host = Host::default();
    let code = workload.code();
    for index in 0..contracts {
        host.insert(
            addr(0xc0, index),
            U256::ZERO,
            1,
            code.clone(),
            HashMap::new(),
        );
    }
    if workload == Workload::Calls {
        host.insert(
            workload::helper_address(),
            U256::ZERO,
            1,
            workload::helper_code(),
            HashMap::new(),
        );
    }
    for index in 0..ntx as u64 {
        host.insert(
            addr(0xaa, index),
            U256::from(SENDER_BALANCE),
            0,
            Bytes::new(),
            HashMap::new(),
        );
    }
    host
}

fn make_tx(index: u64, contracts: u64, workload: Workload) -> TxEnv {
    TxEnv {
        caller: addr(0xaa, index),
        kind: TxKind::Call(addr(0xc0, index % contracts)),
        gas_limit: 1_000_000,
        gas_price: 1,
        nonce: 0,
        chain_id: Some(1),
        value: U256::from(u64::from(workload == Workload::Transfer)),
        ..Default::default()
    }
}

struct NativeBlock(*mut c_void);

impl Drop for NativeBlock {
    fn drop(&mut self) {
        unsafe { evmvx_block_free(self.0) };
    }
}

fn median(samples: &[Duration]) -> Duration {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() / 2]
}

fn sequential(base: &Host, envs: &[env::EnvBuf]) -> (Duration, Vec<Observation>, Host) {
    let mut host = base.clone();
    let mut observations = Vec::with_capacity(envs.len());
    let start = Instant::now();
    for environment in envs {
        let result = host.transact(&environment.env);
        let view = unsafe { &*result };
        observations.push(Observation::vx(view));
        host.commit(view);
        unsafe { evmvx_result_free(result) };
    }
    (start.elapsed(), observations, host)
}

fn reference(
    base: &Host,
    txs: &[TxEnv],
    block: &BlockEnv,
    cfg: &CfgEnv,
) -> (Duration, Vec<Observation>, CacheDB<EmptyDB>) {
    let mut db = CacheDB::new(EmptyDB::default());
    for (address, account) in &base.accounts {
        db.insert_account_info(
            *address,
            AccountInfo {
                balance: account.balance,
                nonce: account.nonce,
                code_hash: account.code_hash,
                code: Some(Bytecode::new_raw(account.code.clone())),
                ..Default::default()
            },
        );
        for (key, value) in &account.storage {
            db.insert_account_storage(*address, *key, *value)
                .expect("valid benchmark prestate");
        }
    }
    let mut observations = Vec::with_capacity(txs.len());
    let start = Instant::now();
    {
        let mut evm = Context::mainnet()
            .with_block(block.clone())
            .with_cfg(cfg.clone())
            .with_db(&mut db)
            .build_mainnet();
        for transaction in txs {
            let result = evm
                .transact_commit(transaction.clone())
                .expect("revm rejected benchmark transaction");
            observations.push(Observation::revm(&result));
        }
    }
    (start.elapsed(), observations, db)
}

struct ParallelRun {
    execution: Duration,
    setup: Duration,
    cleanup: Duration,
    observations: Vec<Observation>,
    host: Host,
    reexecs: u64,
    conflicts: Vec<serde_json::Value>,
}

fn parallel(base: &Host, environments: &[VxEnv], threads: usize) -> ParallelRun {
    let setup = Instant::now();
    let mut host = base.clone();
    host.attach_db();
    let setup = setup.elapsed();
    let start = Instant::now();
    let native = NativeBlock(unsafe {
        evmvx_block_new(
            host.db as *mut c_void,
            environments.as_ptr(),
            environments.len() as u64,
        )
    });
    let address = native.0 as usize;
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(threads);
        for _ in 0..threads {
            workers.push(
                std::thread::Builder::new()
                    .name("vxworker".into())
                    .spawn_scoped(scope, move || unsafe {
                        evmvx_block_worker(address as *mut c_void)
                    })
                    .expect("could not create benchmark worker"),
            );
        }
        for worker in workers {
            worker.join().expect("Vx worker panicked");
        }
    });
    let committed = unsafe { evmvx_block_finish(native.0) } as usize;
    assert_eq!(committed, environments.len(), "parallel benchmark aborted");
    let mut observations = Vec::with_capacity(committed);
    for index in 0..committed {
        let result = unsafe { &*evmvx_block_result(native.0, index as u64) };
        observations.push(Observation::vx(result));
    }
    let execution = start.elapsed();
    let reexecs = unsafe { evmvx_block_reexecs(native.0) };
    let mut conflicts = Vec::new();
    for index in 0..committed {
        let mut conflict = BlockConflict::default();
        assert_eq!(
            unsafe { evmvx_block_conflict(native.0, index as u64, &mut conflict) },
            1
        );
        if conflict.kind != 0 {
            conflicts.push(serde_json::json!({
                "tx": index, "account": conflict.address.address().to_string(),
                "key": conflict.key.u256().to_string(), "kind": conflict.kind,
                "expected_writer": conflict.expected, "observed_writer": conflict.observed,
                "reason": conflict.reason,
            }));
        }
    }
    // Rust mirrors and native read-back are verification work, outside timers.
    for index in 0..committed {
        host.commit_excluding(
            unsafe { &*evmvx_block_result(native.0, index as u64) },
            None,
        );
    }
    let mut tail = Word::default();
    unsafe { evmvx_block_tail_fees(native.0, &mut tail) };
    host.balance_incr_rust(addr(0xcc, 0), tail.u256(), true);
    for (address, account) in &host.accounts {
        let word: Word = (*address).into();
        let mut actual = HostAccount {
            balance: Word::default(),
            nonce: 0,
            code_hash: Word::default(),
            code: std::ptr::null(),
            code_len: 0,
        };
        let status = unsafe { evmvx_db_get_account(host.db as *mut c_void, &word, &mut actual) };
        assert_eq!(status, HOST_OK, "{address}: missing native account");
        assert_eq!(
            (actual.balance.u256(), actual.nonce, actual.code_hash.b256()),
            (account.balance, account.nonce, account.code_hash),
            "{address}: native metadata differs"
        );
        for (key, value) in &account.storage {
            let mut actual = Word::default();
            unsafe {
                evmvx_db_get_slot(host.db as *mut c_void, &word, &(*key).into(), &mut actual)
            };
            assert_eq!(
                actual.u256(),
                *value,
                "{address}: native storage differs at {key}"
            );
        }
    }
    let cleanup = Instant::now();
    drop(native);
    let cleanup = cleanup.elapsed();
    ParallelRun {
        execution,
        setup,
        cleanup,
        observations,
        host,
        reexecs,
        conflicts,
    }
}

fn nanos(samples: &[Duration]) -> Vec<u128> {
    samples.iter().map(Duration::as_nanos).collect()
}

pub fn run(ntx: usize, threads: usize, contracts: u64, reps: u32, workload: Workload, json: bool) {
    let mut cfg = CfgEnv::default();
    cfg.set_spec_and_mainnet_gas_params(SPEC);
    let block = BlockEnv {
        number: U256::from(1),
        timestamp: U256::from(1000),
        gas_limit: 10_000_000_000,
        basefee: 0,
        beneficiary: addr(0xcc, 0),
        prevrandao: Some(Default::default()),
        ..Default::default()
    };
    let transactions: Vec<TxEnv> = (0..ntx as u64)
        .map(|index| make_tx(index, contracts, workload))
        .collect();
    let environments: Vec<env::EnvBuf> = transactions
        .iter()
        .map(|tx| env::build(SPEC, &cfg, &block, tx))
        .collect();
    let native_envs: Vec<VxEnv> = environments
        .iter()
        .map(|env| unsafe { std::ptr::read(&env.env) })
        .collect();
    let initial = build_state(ntx, contracts, workload);
    let mut seq_times = Vec::new();
    let mut revm_times = Vec::new();
    let mut par_times = Vec::new();
    let mut setup_times = Vec::new();
    let mut cleanup_times = Vec::new();
    let mut reexecs = Vec::new();
    let mut conflicts = Vec::new();
    let mut state_roots = Vec::new();
    let mut receipt_roots = Vec::new();
    for repetition in 0..reps {
        // Rotate order to avoid assigning every warm/cold advantage to one engine.
        let (sequential, reference, parallel) = match repetition % 3 {
            0 => {
                let s = sequential(&initial, &environments);
                let r = reference(&initial, &transactions, &block, &cfg);
                let p = parallel(&initial, &native_envs, threads);
                (s, r, p)
            }
            1 => {
                let r = reference(&initial, &transactions, &block, &cfg);
                let p = parallel(&initial, &native_envs, threads);
                let s = sequential(&initial, &environments);
                (s, r, p)
            }
            _ => {
                let p = parallel(&initial, &native_envs, threads);
                let s = sequential(&initial, &environments);
                let r = reference(&initial, &transactions, &block, &cfg);
                (s, r, p)
            }
        };
        assert_eq!(
            sequential.1, reference.1,
            "revm and Vx sequential observations differ"
        );
        assert_eq!(
            sequential.1, parallel.observations,
            "Vx parallel observations differ"
        );
        let expected = snap_host(&sequential.2);
        if let Some(error) = diff(&snap_revm(&reference.2), &expected, "revm", "vx-seq") {
            panic!("{error}");
        }
        if let Some(error) = diff(&expected, &snap_host(&parallel.host), "vx-seq", "vx-par") {
            panic!("{error}");
        }
        let root = crate::state_root(&sequential.2);
        assert_eq!(root, crate::state_root(&parallel.host));
        let mut reference_host = Host::default();
        for (address, (balance, nonce, code_hash, storage)) in snap_revm(&reference.2) {
            reference_host.accounts.insert(
                address,
                crate::host::Acct {
                    balance,
                    nonce,
                    code_hash,
                    code: Bytes::new(),
                    storage: storage.into_iter().collect(),
                },
            );
        }
        assert_eq!(root, crate::state_root(&reference_host));
        let receipts = |observations: &[Observation]| {
            let mut cumulative = 0;
            let receipts = observations
                .iter()
                .zip(&transactions)
                .map(|(o, tx)| o.receipt(tx.tx_type, &mut cumulative))
                .collect::<Vec<_>>();
            alloy_consensus::proofs::calculate_receipt_root(&receipts)
        };
        let receipt_root = receipts(&sequential.1);
        assert_eq!(receipt_root, receipts(&reference.1));
        assert_eq!(receipt_root, receipts(&parallel.observations));
        state_roots.push(root.to_string());
        receipt_roots.push(receipt_root.to_string());
        seq_times.push(sequential.0);
        revm_times.push(reference.0);
        par_times.push(parallel.execution);
        setup_times.push(parallel.setup);
        cleanup_times.push(parallel.cleanup);
        reexecs.push(parallel.reexecs);
        conflicts.push(parallel.conflicts);
    }
    let speedup = median(&revm_times).as_secs_f64() / median(&par_times).as_secs_f64();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "workload": workload.name(), "fork": "Cancun", "txs": ntx,
                "contracts": contracts, "threads": threads, "reps": reps, "valid": true,
                "scope": "execution, scheduling, validation, commit, observation copying; setup and block cleanup separate",
                "vx_seq_ns": nanos(&seq_times), "revm_seq_ns": nanos(&revm_times),
                "vx_par_ns": nanos(&par_times), "vx_par_state_setup_ns": nanos(&setup_times),
                "vx_par_block_cleanup_ns": nanos(&cleanup_times), "reexecs": reexecs,
                "speedup_vs_revm": speedup,
                "conflicts": conflicts, "state_roots": state_roots, "receipt_roots": receipt_roots,
            })
        );
    } else {
        println!("pexec: {} | {ntx} txs, {contracts} contracts, {threads} threads, median of {reps} | vx-seq {:.2} ms, revm-seq {:.2} ms, par {:.2} ms | par vs revm {speedup:.2}x | revm == vx-seq == vx-par (outcomes, gas, refunds, output, created address, logs, state)",
                 workload.name(), median(&seq_times).as_secs_f64() * 1e3,
                 median(&revm_times).as_secs_f64() * 1e3, median(&par_times).as_secs_f64() * 1e3);
    }
}

use crate::env;
use crate::ffi::*;
use crate::host::Host;
use crate::observe::Observation;
use crate::workload::addr;
use revm::context::{cfg::CfgEnv, BlockEnv, TxEnv};
use revm::database::{CacheDB, EmptyDB};
use revm::primitives::{hardfork::SpecId, Bytes, TxKind, U256};
use revm::state::{AccountInfo, Bytecode};
use revm::{Context, ExecuteCommitEvm, MainBuilder, MainContext};
use std::collections::HashMap;
use std::ffi::c_void;

struct Block(*mut c_void);

fn config() -> (CfgEnv, BlockEnv) {
    let mut cfg = CfgEnv::default();
    cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN);
    let block = BlockEnv {
        gas_limit: 100_000_000,
        basefee: 0,
        beneficiary: addr(0xcc, 0),
        prevrandao: Some(Default::default()),
        ..Default::default()
    };
    (cfg, block)
}

fn transaction(to: u64, gas_limit: u64) -> TxEnv {
    TxEnv {
        caller: addr(0xaa, to),
        kind: TxKind::Call(addr(0xc0, to)),
        gas_limit,
        gas_price: 1,
        nonce: 0,
        chain_id: Some(1),
        ..Default::default()
    }
}

impl Drop for Block {
    fn drop(&mut self) {
        unsafe { evmvx_block_free(self.0) };
    }
}

fn setup(code: Vec<u8>, gas_limit: u64) -> (Host, env::EnvBuf) {
    let mut host = Host::default();
    host.insert(
        addr(0xaa, 0),
        U256::from(10u64.pow(18)),
        0,
        Bytes::new(),
        HashMap::new(),
    );
    host.insert(addr(0xc0, 0), U256::ZERO, 1, code.into(), HashMap::new());
    host.attach_db();
    let mut cfg = CfgEnv::default();
    cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN);
    let block = BlockEnv {
        gas_limit: 100_000_000,
        basefee: 0,
        beneficiary: addr(0xcc, 0),
        prevrandao: Some(Default::default()),
        ..Default::default()
    };
    let tx = TxEnv {
        caller: addr(0xaa, 0),
        kind: TxKind::Call(addr(0xc0, 0)),
        gas_limit,
        gas_price: 1,
        nonce: 0,
        chain_id: Some(1),
        ..Default::default()
    };
    (host, env::build(SpecId::CANCUN, &cfg, &block, &tx))
}

fn slot(host: &Host, key: u64) -> U256 {
    let mut value = Word::default();
    unsafe {
        evmvx_db_get_slot(
            host.db as *mut c_void,
            &addr(0xc0, 0).into(),
            &U256::from(key).into(),
            &mut value,
        )
    };
    value.u256()
}

#[test]
fn empty_batch_accepts_a_null_environment() {
    let mut host = Host::default();
    host.attach_db();
    let block = Block(unsafe { evmvx_block_new(host.db as *mut c_void, std::ptr::null(), 0) });
    unsafe {
        evmvx_block_worker(block.0);
        assert_eq!(evmvx_block_finish(block.0), 0);
        assert_eq!(evmvx_block_aborted(block.0), 0);
    }
}

#[test]
fn unstarted_batch_publishes_nothing() {
    let (host, environment) = setup(vec![0x60, 1, 0x5f, 0x55, 0], 100_000);
    let block = Block(unsafe { evmvx_block_new(host.db as *mut c_void, &environment.env, 1) });
    unsafe {
        assert_eq!(evmvx_block_finish(block.0), 0);
        assert_ne!(evmvx_block_aborted(block.0), 0);
    }
    assert_eq!(slot(&host, 0), U256::ZERO);
}

#[test]
fn worker_results_are_private_until_finish() {
    let (host, environment) = setup(vec![0x60, 1, 0x5f, 0x55, 0], 100_000);
    let block = Block(unsafe { evmvx_block_new(host.db as *mut c_void, &environment.env, 1) });
    unsafe { evmvx_block_worker(block.0) };
    assert_eq!(slot(&host, 0), U256::ZERO);
    assert_eq!(unsafe { evmvx_block_finish(block.0) }, 1);
    assert_eq!(slot(&host, 0), U256::ONE);
}

#[test]
fn private_logs_grow_past_the_old_shared_arena_limit() {
    const DISTINCT_SLOTS: u64 = 1100;
    let mut code = Vec::new();
    for key in 0..DISTINCT_SLOTS {
        code.extend_from_slice(&[0x60, 1, 0x61, (key >> 8) as u8, key as u8, 0x55]);
    }
    code.push(0);
    let (host, environment) = setup(code, 30_000_000);
    let block = Block(unsafe { evmvx_block_new(host.db as *mut c_void, &environment.env, 1) });
    unsafe {
        evmvx_block_worker(block.0);
        assert_eq!(evmvx_block_finish(block.0), 1);
        assert_eq!(evmvx_block_aborted(block.0), 0);
    }
    for key in 0..DISTINCT_SLOTS {
        assert_eq!(slot(&host, key), U256::ONE);
    }
}

#[test]
fn cancelled_attempts_never_publish_and_finish_is_idempotent() {
    let (host, environment) = setup(vec![0x60, 1, 0x5f, 0x55, 0], 100_000);
    let block = Block(unsafe { evmvx_block_new(host.db as *mut c_void, &environment.env, 1) });
    unsafe {
        evmvx_block_worker(block.0);
        assert!(evmvx_block_result(block.0, 0).is_null());
        evmvx_block_cancel(block.0);
        assert_eq!(evmvx_block_finish(block.0), 0);
        assert_eq!(evmvx_block_finish(block.0), 0);
    }
    assert_eq!(slot(&host, 0), U256::ZERO);
}

fn check_block(
    mut host: Host,
    transactions: Vec<TxEnv>,
    threads: usize,
) -> (Host, Vec<BlockConflict>) {
    use crate::pfuzz::{snap_host, snap_revm};
    use alloy_consensus::{proofs::calculate_receipt_root, Receipt, ReceiptEnvelope, TxType};
    let (cfg, context) = config();
    let mut reference = CacheDB::new(EmptyDB::default());
    for (address, account) in &host.accounts {
        reference.insert_account_info(
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
            reference
                .insert_account_storage(*address, *key, *value)
                .unwrap();
        }
    }
    let mut cumulative = 0;
    let mut want_receipts = Vec::new();
    let expected = {
        let mut evm = Context::mainnet()
            .with_block(context.clone())
            .with_cfg(cfg.clone())
            .with_db(&mut reference)
            .build_mainnet();
        transactions
            .iter()
            .map(|tx| {
                let result = evm.transact_commit(tx.clone()).unwrap();
                cumulative += result.tx_gas_used();
                want_receipts.push(ReceiptEnvelope::from_typed(
                    TxType::try_from(tx.tx_type).unwrap(),
                    Receipt {
                        status: result.is_success().into(),
                        cumulative_gas_used: cumulative,
                        logs: result.logs().to_vec(),
                    },
                ));
                Observation::revm(&result)
            })
            .collect::<Vec<_>>()
    };
    let environments = transactions
        .iter()
        .map(|tx| env::build(SpecId::CANCUN, &cfg, &context, tx))
        .collect::<Vec<_>>();
    let mut sequential = host.clone();
    for (environment, want) in environments.iter().zip(&expected) {
        let result = sequential.transact(&environment.env);
        assert_eq!(&Observation::vx(unsafe { &*result }), want);
        sequential.commit(unsafe { &*result });
        unsafe { evmvx_result_free(result) };
    }
    host.attach_db();
    let native_envs = environments
        .iter()
        .map(|env| unsafe { std::ptr::read(&env.env) })
        .collect::<Vec<_>>();
    let block = Block(unsafe {
        evmvx_block_new(
            host.db as *mut c_void,
            native_envs.as_ptr(),
            native_envs.len() as u64,
        )
    });
    let pointer = block.0 as usize;
    std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(threads);
        for _ in 0..threads {
            workers
                .push(scope.spawn(move || unsafe { evmvx_block_worker(pointer as *mut c_void) }));
        }
        for worker in workers {
            worker.join().expect("Vx worker panicked");
        }
    });
    assert_eq!(
        unsafe { evmvx_block_finish(block.0) },
        transactions.len() as u64
    );
    assert_eq!(
        unsafe { evmvx_block_finish(block.0) },
        transactions.len() as u64
    );
    let mut actual_receipts = Vec::new();
    let mut conflicts = Vec::new();
    cumulative = 0;
    for (index, want) in expected.iter().enumerate() {
        let result = unsafe { &*evmvx_block_result(block.0, index as u64) };
        assert_eq!(&Observation::vx(result), want, "tx {index}");
        host.commit_excluding(result, None);
        cumulative += result.gas_used;
        actual_receipts.push(ReceiptEnvelope::from_typed(
            TxType::try_from(transactions[index].tx_type).unwrap(),
            Receipt {
                status: (result.status == RES_SUCCESS).into(),
                cumulative_gas_used: cumulative,
                logs: crate::host::logs_of(result),
            },
        ));
        let mut conflict = BlockConflict::default();
        assert_eq!(
            unsafe { evmvx_block_conflict(block.0, index as u64, &mut conflict) },
            1
        );
        conflicts.push(conflict);
    }
    assert_eq!(
        unsafe { evmvx_block_reexecs(block.0) } as usize,
        conflicts.iter().filter(|c| c.kind != 0).count()
    );
    let mut tail = Word::default();
    unsafe { evmvx_block_tail_fees(block.0, &mut tail) };
    host.balance_incr_rust(context.beneficiary, tail.u256(), true);
    assert_eq!(snap_host(&host), snap_host(&sequential));
    assert_eq!(snap_host(&host), snap_revm(&reference));
    assert_eq!(crate::state_root(&host), crate::state_root(&sequential));
    assert_eq!(
        calculate_receipt_root(&actual_receipts),
        calculate_receipt_root(&want_receipts)
    );
    // Read back every known address and slot, including deleted accounts and
    // slots that a discarded attempt could otherwise leave behind.
    for address in host
        .accounts
        .keys()
        .chain(sequential.accounts.keys())
        .chain(reference.cache.accounts.keys())
    {
        let mut account = HostAccount {
            balance: Word::default(),
            nonce: 0,
            code_hash: Word::default(),
            code: std::ptr::null(),
            code_len: 0,
        };
        let found = unsafe {
            evmvx_db_get_account(host.db as *mut c_void, &(*address).into(), &mut account)
        };
        if let Some(want) = sequential.accounts.get(address) {
            assert_eq!(found, HOST_OK, "{address}");
            assert_eq!(
                (
                    account.balance.u256(),
                    account.nonce,
                    account.code_hash.b256()
                ),
                (want.balance, want.nonce, want.code_hash)
            );
        } else {
            assert_ne!(found, HOST_OK, "{address}: stale account");
        }
        for key in 0..4u64 {
            let mut value = Word::default();
            unsafe {
                evmvx_db_get_slot(
                    host.db as *mut c_void,
                    &(*address).into(),
                    &U256::from(key).into(),
                    &mut value,
                )
            };
            let want = sequential
                .accounts
                .get(address)
                .and_then(|a| a.storage.get(&U256::from(key)))
                .copied()
                .unwrap_or_default();
            assert_eq!(value.u256(), want, "{address} slot {key}");
        }
    }
    (host, conflicts)
}

fn shared_contract(code: Vec<u8>, count: u64) -> (Host, Vec<TxEnv>) {
    let mut host = Host::default();
    host.insert(addr(0xc0, 0), U256::ZERO, 1, code.into(), HashMap::new());
    let mut transactions = Vec::new();
    for index in 0..count {
        host.insert(
            addr(0xaa, index),
            U256::from(10u64.pow(18)),
            0,
            Bytes::new(),
            HashMap::new(),
        );
        let mut tx = transaction(index, 1_000_000);
        tx.kind = TxKind::Call(addr(0xc0, 0));
        transactions.push(tx);
    }
    (host, transactions)
}

#[test]
fn unchanged_zero_value_callee_has_no_false_conflicts() {
    let (host, txs) = shared_contract(vec![0x60, 42, 0x5f, 0x52, 0x60, 32, 0x5f, 0xf3], 128);
    let (_, conflicts) = check_block(host, txs, 12);
    assert!(conflicts.iter().all(|c| c.kind == 0));
}

#[test]
fn hot_storage_repairs_once_with_exact_conflict_key() {
    let (host, txs) = shared_contract(vec![0x5f, 0x54, 0x60, 1, 0x01, 0x5f, 0x55, 0], 128);
    let (host, conflicts) = check_block(host, txs, 12);
    assert_eq!(slot(&host, 0), U256::from(128));
    for (index, conflict) in conflicts.iter().enumerate().skip(1) {
        assert_eq!(conflict.kind, 2);
        assert_eq!(conflict.address.address(), addr(0xc0, 0));
        assert_eq!(conflict.key.u256(), U256::ZERO);
        assert_eq!(conflict.expected, u64::MAX);
        assert_eq!(conflict.observed, index as u64 - 1);
        assert_eq!(conflict.reason, 2);
    }
}

#[test]
fn storage_clear_reports_the_applied_refund() {
    let (mut host, txs) = shared_contract(vec![0x5f, 0x5f, 0x55, 0], 2);
    host.accounts
        .get_mut(&addr(0xc0, 0))
        .unwrap()
        .storage
        .insert(U256::ZERO, U256::from(7));
    check_block(host, txs, 2);
}

#[test]
fn repair_can_discover_new_keys_and_discard_speculative_writes() {
    // If slot 0 is zero, set slots 0 and 1. Otherwise write slot 2 only.
    let (host, txs) = shared_contract(
        vec![
            0x5f, 0x54, 0x60, 15, 0x57, 0x60, 1, 0x5f, 0x55, 0x60, 7, 0x60, 1, 0x55, 0, 0x5b, 0x60,
            9, 0x60, 2, 0x55, 0,
        ],
        3,
    );
    let (host, conflicts) = check_block(host, txs, 3);
    assert_eq!(slot(&host, 1), U256::from(7));
    assert_eq!(slot(&host, 2), U256::from(9));
    assert_eq!(conflicts[1].kind, 2);
}

#[test]
fn successful_creation_returns_deployed_code() {
    let (host, mut txs) = shared_contract(vec![0], 2);
    for tx in &mut txs {
        tx.kind = TxKind::Create;
        // Init code returns the single-byte STOP runtime.
        tx.data = Bytes::from_static(&[0x5f, 0x5f, 0x53, 0x60, 1, 0x5f, 0xf3]);
    }
    check_block(host, txs, 2);
}

#[test]
fn touch_of_empty_account_and_reverted_touch_match_revm() {
    for revert in [false, true] {
        let mut code = vec![0x5f, 0x5f, 0x5f, 0x5f, 0x5f, 0x73];
        code.extend_from_slice(addr(0xdd, 0).as_slice());
        code.extend_from_slice(&[0x5a, 0xf1, 0x50]);
        if revert {
            code.extend_from_slice(&[0x5f, 0x5f, 0xfd]);
        } else {
            code.push(0);
        }
        let (mut host, txs) = shared_contract(code, 2);
        host.insert(addr(0xdd, 0), U256::ZERO, 0, Bytes::new(), HashMap::new());
        check_block(host, txs, 2);
    }
}

#[test]
fn repair_that_discovers_coinbase_reads_gets_exact_prior_fees() {
    // First call sets slot 0; later calls take the branch that reads coinbase.
    let (host, txs) = shared_contract(
        vec![
            0x5f, 0x54, 0x60, 10, 0x57, 0x60, 1, 0x5f, 0x55, 0, 0x5b, 0x41, 0x31, 0x60, 1, 0x55, 0,
        ],
        4,
    );
    check_block(host, txs, 4);
}

#[test]
fn invalid_final_transaction_publishes_no_prefix() {
    let (host, environment) = setup(vec![0x60, 1, 0x5f, 0x55, 0], 100_000);
    // Duplicate sender nonce: the second optimistic success becomes invalid.
    let environments = unsafe {
        [
            std::ptr::read(&environment.env),
            std::ptr::read(&environment.env),
        ]
    };
    let block = Block(unsafe { evmvx_block_new(host.db as *mut c_void, environments.as_ptr(), 2) });
    unsafe {
        evmvx_block_worker(block.0);
        assert_eq!(evmvx_block_finish(block.0), 0);
        assert_eq!(evmvx_block_aborted(block.0), 1);
    }
    assert_eq!(slot(&host, 0), U256::ZERO);
}

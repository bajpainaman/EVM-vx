//! Blockchain tests: a port of `revme btest` (bins/revme/src/cmd/blockchaintest.rs)
//! that executes each block's transactions and system calls through the Vx engine.

use crate::{env, ffi::*, host::{logs_of, Host}};
use alloy_consensus::{proofs::calculate_receipt_root, Receipt, ReceiptEnvelope, TxType};
use revm::{
    context::{cfg::CfgEnv, BlockEnv, TxEnv},
    context_interface::block::BlobExcessGasAndPrice,
    primitives::{address, hardfork::SpecId, Address, Bytes, TxKind, U256},
    statetest_types::blockchain::{BlockchainTest, BlockchainTestCase, ForkSpec},
};
use std::path::Path;

const SYSTEM_ADDRESS: Address = address!("0xfffffffffffffffffffffffffffffffffffffffe");
const HISTORY_STORAGE_ADDRESS: Address = address!("0x0000F90827F1C53a10cb7A02335B175320002935");
const BEACON_ROOTS_ADDRESS: Address = address!("0x000F3df6D732807Ef1319fB7B8bB8522d0Beac02");
const WITHDRAWAL_REQUEST_ADDRESS: Address = address!("0x00000961Ef480Eb55e80D19ad83579A64c007002");
const CONSOLIDATION_REQUEST_ADDRESS: Address = address!("0x0000BBdDc7CE488642fb579F8B00f3a590007251");
const SYSTEM_CALL_GAS_LIMIT: u64 = 30_000_000;
const ONE_ETHER: u128 = 1_000_000_000_000_000_000;
const ONE_GWEI: u64 = 1_000_000_000;

/// revme skip list for blockchain tests.
pub fn skip_test(path: &Path) -> bool {
    let s = path.to_str().unwrap_or_default();
    if s.contains("paris/eip7610_create_collision")
        || s.contains("cancun/eip4844_blobs")
        || s.contains("prague/eip7251_consolidations")
        || s.contains("prague/eip7685_general_purpose_el_requests")
        || s.contains("prague/eip7002_el_triggerable_withdrawals")
        || s.contains("osaka/eip7918_blob_reserve_price")
    {
        return true;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    matches!(
        name,
        "RevertInCreateInInit_Paris.json" | "RevertInCreateInInit.json"
            | "dynamicAccountOverwriteEmpty.json" | "dynamicAccountOverwriteEmpty_Paris.json"
            | "RevertInCreateInInitCreate2Paris.json" | "create2collisionStorage.json"
            | "RevertInCreateInInitCreate2.json" | "create2collisionStorageParis.json"
            | "InitCollision.json" | "InitCollisionParis.json"
            | "ValueOverflow.json" | "ValueOverflowParis.json"
            | "Call50000_sha256.json" | "static_Call50000_sha256.json" | "loopMul.json"
            | "CALLBlake2f_MaxRounds.json" | "scenarios.json"
            | "invalid_tx_max_fee_per_blob_gas.json" | "correct_increasing_blob_gas_costs.json"
            | "correct_decreasing_blob_gas_costs.json" | "block_hashes_history.json"
    )
}

fn fork_to_spec_id(fork: ForkSpec) -> SpecId {
    match fork {
        ForkSpec::Frontier => SpecId::FRONTIER,
        ForkSpec::Homestead | ForkSpec::FrontierToHomesteadAt5 => SpecId::HOMESTEAD,
        ForkSpec::EIP150 | ForkSpec::HomesteadToDaoAt5 | ForkSpec::HomesteadToEIP150At5 => SpecId::TANGERINE,
        ForkSpec::EIP158 => SpecId::SPURIOUS_DRAGON,
        ForkSpec::Byzantium | ForkSpec::EIP158ToByzantiumAt5 | ForkSpec::ByzantiumToConstantinopleFixAt5 => {
            SpecId::BYZANTIUM
        }
        ForkSpec::Constantinople | ForkSpec::ByzantiumToConstantinopleAt5 | ForkSpec::ConstantinopleFix => {
            SpecId::PETERSBURG
        }
        ForkSpec::Istanbul => SpecId::ISTANBUL,
        ForkSpec::Berlin => SpecId::BERLIN,
        ForkSpec::London | ForkSpec::BerlinToLondonAt5 => SpecId::LONDON,
        ForkSpec::Paris | ForkSpec::ParisToShanghaiAtTime15k => SpecId::MERGE,
        ForkSpec::Shanghai => SpecId::SHANGHAI,
        ForkSpec::Cancun | ForkSpec::ShanghaiToCancunAtTime15k => SpecId::CANCUN,
        ForkSpec::Prague | ForkSpec::CancunToPragueAtTime15k => SpecId::PRAGUE,
        ForkSpec::Osaka | ForkSpec::PragueToOsakaAtTime15k => SpecId::OSAKA,
        ForkSpec::Amsterdam => SpecId::AMSTERDAM,
        _ => SpecId::AMSTERDAM,
    }
}

fn block_reward(spec: SpecId) -> u128 {
    if spec.is_enabled_in(SpecId::MERGE) {
        0
    } else if spec.is_enabled_in(SpecId::PETERSBURG) {
        ONE_ETHER * 2
    } else if spec.is_enabled_in(SpecId::BYZANTIUM) {
        ONE_ETHER * 3
    } else {
        ONE_ETHER * 5
    }
}

/// What one transaction did to the block.
enum TxRun {
    /// Executed (success, revert or halt); state committed.
    Done { success: bool, gas_used: u64, logs: Vec<revm::primitives::Log> },
    /// Rejected as invalid; state unchanged.
    Invalid,
    Host(String),
}

fn run_tx(host: &mut Host, spec: SpecId, cfg: &CfgEnv, block: &BlockEnv, tx: &TxEnv) -> TxRun {
    let eb = env::build(spec, cfg, block, tx);
    let res = host.transact(&eb.env);
    let r = unsafe { &*res };
    let out = match r.status {
        RES_HOST_FAILURE => TxRun::Host(format!("host failure (reason {})", r.reason)),
        RES_INVALID_TX => TxRun::Invalid,
        st => {
            let logs = logs_of(r);
            host.commit(r);
            TxRun::Done { success: st == RES_SUCCESS, gas_used: r.gas_used, logs }
        }
    };
    unsafe { evmvx_result_free(res) };
    out
}

/// Blocks run by the parallel executor, and blocks it aborted (coinbase read
/// or table overflow) that then ran sequentially.
pub static PAR_BLOCKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static PAR_ABORTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// Transactions the committer re-ran with the exact coinbase account.
pub static PAR_CB_TXS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Runs one block's transactions through the Vx block executor. Returns the
/// cumulative gas and receipts, or None when the executor aborted (the caller
/// then runs the block sequentially; an aborted block changed no state).
fn run_block_parallel(
    host: &mut Host,
    spec: SpecId,
    cfg: &CfgEnv,
    block: &BlockEnv,
    txs: &[TxEnv],
    nthreads: usize,
    eip161: bool,
) -> Result<Option<(u64, Vec<ReceiptEnvelope>)>, String> {
    let ebs: Vec<env::EnvBuf> = txs.iter().map(|t| env::build(spec, cfg, block, t)).collect();
    let venvs: Vec<VxEnv> = ebs.iter().map(|eb| unsafe { std::ptr::read(&eb.env) }).collect();
    let db = host.db as *mut std::ffi::c_void;
    let b = unsafe { evmvx_block_new(db, venvs.as_ptr(), venvs.len() as u64) };
    let bp = b as usize;
    std::thread::scope(|s| {
        for _ in 0..nthreads {
            s.spawn(move || unsafe { evmvx_block_worker(bp as *mut std::ffi::c_void) });
        }
    });
    let committed = unsafe { evmvx_block_finish(b) };
    if unsafe { evmvx_block_aborted(b) } != 0 || committed as usize != txs.len() {
        unsafe { evmvx_block_free(b) };
        PAR_ABORTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return Ok(None);
    }
    PAR_BLOCKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut cumulative = 0u64;
    let mut receipts = Vec::with_capacity(txs.len());
    for (i, tx) in txs.iter().enumerate() {
        let r = unsafe { &*evmvx_block_result(b, i as u64) };
        match r.status {
            RES_INVALID_TX | RES_HOST_FAILURE => {
                let why = format!("parallel: tx {i} status {} (reason {})", r.status, r.reason);
                unsafe { evmvx_block_free(b) };
                return Err(why);
            }
            _ => {}
        }
        // The db already holds the block; mirror it into the Rust maps.
        host.commit_excluding(r, None);
        cumulative += r.gas_used;
        let tx_type = TxType::try_from(tx.tx_type).map_err(|e| format!("tx type: {e}"))?;
        receipts.push(ReceiptEnvelope::from_typed(
            tx_type,
            Receipt { status: (r.status == RES_SUCCESS).into(), cumulative_gas_used: cumulative, logs: logs_of(r) },
        ));
    }
    // Fees not folded into a coinbase write by a transaction, as finish paid.
    let mut tail = Word::default();
    unsafe { evmvx_block_tail_fees(b, &mut tail) };
    host.balance_incr_rust(block.beneficiary, tail.u256(), eip161);
    PAR_CB_TXS.fetch_add(unsafe { evmvx_block_coinbase_txs(b) } as usize, std::sync::atomic::Ordering::Relaxed);
    unsafe { evmvx_block_free(b) };
    Ok(Some((cumulative, receipts)))
}

/// revm system_call_commit: a 30M-gas call from SYSTEM_ADDRESS, committed.
fn system_call(host: &mut Host, spec: SpecId, cfg: &CfgEnv, block: &BlockEnv, to: Address, data: Bytes) -> Result<(), String> {
    let tx = TxEnv {
        caller: SYSTEM_ADDRESS,
        kind: TxKind::Call(to),
        data,
        gas_limit: SYSTEM_CALL_GAS_LIMIT,
        ..TxEnv::default()
    };
    let eb = env::build(spec, cfg, block, &tx);
    let res = host.system_call(&eb.env);
    let r = unsafe { &*res };
    let out = if r.status == RES_HOST_FAILURE {
        Err(format!("system call to {to} failed (reason {})", r.reason))
    } else {
        host.commit(r);
        Ok(())
    };
    unsafe { evmvx_result_free(res) };
    out
}

/// Runs one test case; Ok(true) if it ran, Ok(false) if skipped (transition fork).
fn run_case(case: &BlockchainTestCase) -> Result<bool, String> {
    if matches!(
        case.network,
        ForkSpec::ByzantiumToConstantinopleAt5
            | ForkSpec::ParisToShanghaiAtTime15k
            | ForkSpec::ShanghaiToCancunAtTime15k
            | ForkSpec::CancunToPragueAtTime15k
            | ForkSpec::PragueToOsakaAtTime15k
            | ForkSpec::BPO1ToBPO2AtTime15k
            | ForkSpec::BPO2ToAmsterdamAtTime15k
    ) {
        return Ok(false);
    }
    let mut host = Host::default();
    for (addr, acc) in &case.pre.0 {
        let storage = acc.storage.iter().map(|(k, v)| (*k, *v)).collect();
        host.insert(*addr, acc.balance, acc.nonce.to::<u64>(), acc.code.clone(), storage);
    }
    host.set_block_hash(0, case.genesis_block_header.hash);
    if crate::host::db_mode() || crate::host::block_threads() > 0 {
        host.attach_db();
    }
    let spec = fork_to_spec_id(case.network);
    let mut cfg = CfgEnv::default();
    cfg.set_spec_and_mainnet_gas_params(spec);
    let eip161 = spec.is_enabled_in(SpecId::SPURIOUS_DRAGON);

    let mut parent_block_hash = Some(case.genesis_block_header.hash);
    let mut parent_excess_blob_gas = case.genesis_block_header.excess_blob_gas.unwrap_or_default().to::<u64>();
    let mut block_env = case.genesis_block_env();

    for (bi, block) in case.blocks.iter().enumerate() {
        let should_fail = block.expect_exception.is_some();
        let transactions = block.transactions.as_deref().unwrap_or_default();
        let mut block_hash = None;
        let mut beacon_root = None;
        let mut this_excess = None;
        if let Some(h) = block.block_header.as_ref() {
            block_hash = Some(h.hash);
            beacon_root = h.parent_beacon_block_root;
            block_env = h.to_block_env(Some(BlobExcessGasAndPrice::new_with_spec(parent_excess_blob_gas, spec)));
            this_excess = h.excess_blob_gas.map(|v| v.to::<u64>());
        }

        // Pre-block system calls (EIP-2935, EIP-4788).
        if block_env.number != U256::ZERO {
            if let Some(ph) = parent_block_hash {
                if spec.is_enabled_in(SpecId::PRAGUE) {
                    system_call(&mut host, spec, &cfg, &block_env, HISTORY_STORAGE_ADDRESS, ph.0.into())
                        .map_err(|e| format!("block {bi}: {e}"))?;
                }
            }
            if let Some(root) = beacon_root {
                if spec.is_enabled_in(SpecId::CANCUN) {
                    system_call(&mut host, spec, &cfg, &block_env, BEACON_ROOTS_ADDRESS, root.0.into())
                        .map_err(|e| format!("block {bi}: {e}"))?;
                }
            }
        }

        let mut cumulative: u64 = 0;
        let mut completed = true;
        let mut receipts = Vec::with_capacity(transactions.len());
        let nthreads = crate::host::block_threads();
        let mut ran_parallel = false;
        if nthreads > 0 && !should_fail && !transactions.is_empty() && transactions.iter().all(|t| t.sender.is_some()) {
            let tx_envs: Result<Vec<TxEnv>, _> = transactions.iter().map(|t| t.to_tx_env()).collect();
            if let Ok(tx_envs) = tx_envs {
                if let Some(r) = run_block_parallel(&mut host, spec, &cfg, &block_env, &tx_envs, nthreads, eip161)? {
                    cumulative = r.0;
                    receipts = r.1;
                    ran_parallel = true;
                }
            }
        }
        for tx in transactions.iter().filter(|_| !ran_parallel) {
            if tx.sender.is_none() {
                completed = false;
                break;
            }
            let tx_env = match tx.to_tx_env() {
                Ok(t) => t,
                Err(_) if should_fail => continue,
                Err(_) => {
                    completed = false;
                    break;
                }
            };
            match run_tx(&mut host, spec, &cfg, &block_env, &tx_env) {
                TxRun::Done { success, gas_used, logs } => {
                    if should_fail {
                        completed = false;
                        break;
                    }
                    cumulative += gas_used;
                    let tx_type = TxType::try_from(tx_env.tx_type).map_err(|e| format!("tx type: {e}"))?;
                    receipts.push(ReceiptEnvelope::from_typed(
                        tx_type,
                        Receipt { status: success.into(), cumulative_gas_used: cumulative, logs },
                    ));
                }
                TxRun::Invalid => {
                    if !should_fail {
                        completed = false;
                        break;
                    }
                }
                TxRun::Host(e) => return Err(format!("block {bi}: {e}")),
            }
        }

        if completed && !should_fail {
            if let Some(h) = block.block_header.as_ref() {
                let want = h.gas_used.to::<u64>();
                if cumulative != want {
                    return Err(format!("block {bi}: gas used {cumulative} != {want}"));
                }
                if spec.is_enabled_in(SpecId::BYZANTIUM) {
                    let root = calculate_receipt_root(&receipts);
                    if root != h.receipt_trie {
                        return Err(format!("block {bi}: receipts root {root} != {}", h.receipt_trie));
                    }
                }
            }
        }

        // Post-block: reward, withdrawals, EL request system calls.
        let reward = block_reward(spec);
        if reward != 0 {
            host.balance_incr(block_env.beneficiary, U256::from(reward), eip161);
        }
        if spec.is_enabled_in(SpecId::SHANGHAI) {
            for w in block.withdrawals.as_deref().unwrap_or_default() {
                host.balance_incr(w.address, w.amount.saturating_mul(U256::from(ONE_GWEI)), eip161);
            }
        }
        if spec.is_enabled_in(SpecId::PRAGUE) {
            system_call(&mut host, spec, &cfg, &block_env, WITHDRAWAL_REQUEST_ADDRESS, Bytes::new())
                .map_err(|e| format!("block {bi}: {e}"))?;
            system_call(&mut host, spec, &cfg, &block_env, CONSOLIDATION_REQUEST_ADDRESS, Bytes::new())
                .map_err(|e| format!("block {bi}: {e}"))?;
        }
        host.set_block_hash(block_env.number.to::<u64>(), block_hash.unwrap_or_default());
        parent_block_hash = block_hash;
        if let Some(x) = this_excess {
            parent_excess_blob_gas = x;
        }
    }

    if let Some(post) = &case.post_state {
        for (addr, want) in post {
            let acc = host.accounts.get(addr).cloned().unwrap_or_default();
            if acc.balance != want.balance {
                return Err(format!("{addr}: balance {} != {}", acc.balance, want.balance));
            }
            let want_nonce = want.nonce.to::<u64>();
            if acc.nonce != want_nonce {
                return Err(format!("{addr}: nonce {} != {want_nonce}", acc.nonce));
            }
            if !want.code.is_empty() && acc.code != want.code {
                return Err(format!("{addr}: code differs"));
            }
            for (k, v) in &acc.storage {
                if !want.storage.contains_key(k) && !v.is_zero() {
                    return Err(format!("{addr}: unexpected slot {k:#x} = {v:#x}"));
                }
            }
            for (k, v) in &want.storage {
                let got = acc.storage.get(k).copied().unwrap_or_default();
                if got != *v {
                    return Err(format!("{addr}: slot {k:#x} = {got:#x}, want {v:#x}"));
                }
            }
        }
    }
    Ok(true)
}

/// Runs every case in one blockchain-test file; returns (passed, skipped, failures).
pub fn run_file(path: &Path) -> (usize, usize, Vec<String>) {
    if skip_test(path) {
        return (0, 0, Vec::new());
    }
    let text = std::fs::read_to_string(path).unwrap();
    let suite: BlockchainTest = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(e) => return (0, 0, vec![format!("{}: parse error: {e}", path.display())]),
    };
    let (mut passed, mut skipped, mut fails) = (0, 0, Vec::new());
    for (name, case) in &suite.0 {
        match run_case(case) {
            Ok(true) => passed += 1,
            Ok(false) => skipped += 1,
            Err(e) => fails.push(format!("{} {name}: {e}", path.display())),
        }
    }
    (passed, skipped, fails)
}

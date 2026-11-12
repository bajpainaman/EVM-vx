//! Independent transaction observations and differential state diagnostics.

use crate::host::Host;
use revm::{
    context::{cfg::CfgEnv, BlockEnv, TxEnv},
    database::{CacheState, State},
    primitives::{Address, KECCAK_EMPTY, U256},
    Context, ExecuteCommitEvm, MainBuilder, MainContext,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

pub fn enabled() -> bool {
    std::env::var("EVMVX_DIFF").map(|v| v != "0").unwrap_or(false)
}

type Snapshot = BTreeMap<Address, (u64, U256, revm::primitives::B256, BTreeMap<U256, U256>)>;

fn vx_snapshot(h: &Host) -> Snapshot {
    h.accounts
        .iter()
        .map(|(a, acc)| {
            let st = acc.storage.iter().filter(|(_, v)| !v.is_zero()).map(|(k, v)| (*k, *v)).collect();
            (*a, (acc.nonce, acc.balance, acc.code_hash, st))
        })
        .collect()
}

/// Returns a report of revm's result and the per-account differences.
pub fn report(cache: &CacheState, cfg: &CfgEnv, block: &BlockEnv, tx: &TxEnv, vx_post: &Host, vx_gas: u64, vx_status: u64) -> String {
    let mut state = State::builder().with_cached_prestate(cache.clone()).with_bundle_update().build();
    let res = {
        let mut evm = Context::mainnet()
            .with_block(block.clone())
            .with_tx(tx.clone())
            .with_cfg(cfg.clone())
            .with_db(&mut state)
            .build_mainnet();
        evm.transact_commit(tx.clone())
    };
    let mut out = String::new();
    match &res {
        Ok(r) => {
            let _ = write!(out, "\n    revm: gas_used={} result={:?}", r.tx_gas_used(), short(r));
        }
        Err(e) => {
            let _ = write!(out, "\n    revm: error {e}");
        }
    }
    let _ = write!(out, "\n    vx:   gas_used={vx_gas} status={vx_status}");
    let revm_snap: Snapshot = state
        .cache
        .trie_account()
        .into_iter()
        .map(|(a, acc): (Address, &revm::database::PlainAccount)| {
            let st = acc.storage.iter().filter(|(_, v)| !v.is_zero()).map(|(k, v)| (*k, *v)).collect();
            let ch = if acc.info.code_hash == revm::primitives::B256::ZERO { KECCAK_EMPTY } else { acc.info.code_hash };
            (a, (acc.info.nonce, acc.info.balance, ch, st))
        })
        .collect();
    let vx_snap = vx_snapshot(vx_post);
    let addrs: BTreeSet<Address> = revm_snap.keys().chain(vx_snap.keys()).copied().collect();
    let mut shown = 0;
    for a in addrs {
        let (r, v) = (revm_snap.get(&a), vx_snap.get(&a));
        if r == v {
            continue;
        }
        shown += 1;
        if shown > 8 {
            out.push_str("\n    ...");
            break;
        }
        match (r, v) {
            (Some(_), None) => { let _ = write!(out, "\n    {a}: only in revm {:?}", r.map(fmt_acc)); }
            (None, Some(_)) => { let _ = write!(out, "\n    {a}: only in vx {:?}", v.map(fmt_acc)); }
            (Some(r), Some(v)) => {
                if r.0 != v.0 { let _ = write!(out, "\n    {a}: nonce revm={} vx={}", r.0, v.0); }
                if r.1 != v.1 { let _ = write!(out, "\n    {a}: balance revm={} vx={}", r.1, v.1); }
                if r.2 != v.2 { let _ = write!(out, "\n    {a}: code_hash revm={} vx={}", r.2, v.2); }
                if r.3 != v.3 {
                    let keys: BTreeSet<&U256> = r.3.keys().chain(v.3.keys()).collect();
                    for k in keys.into_iter().take(6) {
                        let (rv, vv) = (r.3.get(k), v.3.get(k));
                        if rv != vv { let _ = write!(out, "\n    {a}: slot {k:#x} revm={rv:?} vx={vv:?}"); }
                    }
                }
            }
            (None, None) => {}
        }
    }
    out
}

fn fmt_acc(a: &(u64, U256, revm::primitives::B256, BTreeMap<U256, U256>)) -> String {
    format!("nonce={} balance={} slots={}", a.0, a.1, a.3.len())
}

fn short<H: core::fmt::Debug>(r: &revm::context::result::ExecutionResult<H>) -> String {
    use revm::context::result::ExecutionResult::*;
    match r {
        Success { reason, .. } => format!("Success({reason:?})"),
        Revert { .. } => "Revert".into(),
        Halt { reason, .. } => format!("Halt({reason:?})"),
    }
}

/// Public transaction observations, not interpreter-private execution traces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub status: u64,
    pub output: Vec<u8>,
    pub gas: u64,
    pub refund: u64,
    pub logs: Vec<revm::primitives::Log>,
    pub created: Option<Address>,
    pub deployed_code: Option<Vec<u8>>,
}

impl Observation {
    pub fn vx(r: &crate::ffi::VxResult) -> Self {
        let created = (r.created != 0).then(|| r.created_address.address());
        let deployed_code = created.map(|addr| {
            let account = unsafe { crate::host::slice(r.accounts, r.account_count) }
                .iter().find(|a| a.address.address() == addr).expect("created account missing from delta");
            unsafe { crate::host::slice(account.code, account.code_len) }.to_vec()
        });
        Self {
            status: r.status,
            output: unsafe { crate::host::slice(r.output, r.output_len) }.to_vec(),
            gas: r.gas_used,
            refund: r.gas_refunded,
            logs: crate::host::logs_of(r),
            created,
            deployed_code,
        }
    }

    pub fn reference(r: &revm::context::result::ExecutionResult) -> Self {
        use crate::ffi::*;
        use revm::context::result::ExecutionResult::*;
        let created = r.created_address();
        let bytes = r.output().map(|b| b.to_vec()).unwrap_or_default();
        Self {
            status: match r { Success { .. } => RES_SUCCESS, Revert { .. } => RES_REVERT, Halt { .. } => RES_HALT },
            // Vx exposes successful deployment bytes in the account delta,
            // not call returndata. Compare them separately without truncation.
            output: if created.is_some() { Vec::new() } else { bytes.clone() },
            gas: r.tx_gas_used(),
            refund: r.gas().final_refunded(),
            logs: r.logs().to_vec(),
            created,
            deployed_code: created.map(|_| bytes),
        }
    }
}

/// Use revm's state-test commit semantics, including EIP-161 deletion of
/// touched empty accounts. CacheDB deliberately retains empty cache entries.
pub fn reference_state(pre: &Host) -> State<revm::database::EmptyDB> {
    use revm::state::{AccountInfo, Bytecode};
    let mut cache = CacheState::new();
    for (a, acc) in &pre.accounts {
        let code = Bytecode::new_raw(acc.code.clone());
        cache.contracts.insert(acc.code_hash, code.clone());
        cache.insert_account_with_storage(*a, AccountInfo {
            balance: acc.balance, nonce: acc.nonce, code_hash: acc.code_hash,
            code: Some(code), ..Default::default()
        }, acc.storage.iter().map(|(k, v)| (*k, *v)).collect());
    }
    let mut db = State::builder().with_cached_prestate(cache).with_bundle_update().build();
    for (n, h) in &pre.block_hashes { db.block_hashes.insert(*n, *h); }
    db
}

/// Run runtime-supplied prestate/env/tx through revm, then compare every exposed
/// observation and the complete persistent state (including code bytes).
pub fn check(pre: &Host, cfg: &CfgEnv, block: &BlockEnv, tx: &TxEnv,
             actual: &Observation, post: &Host) -> Result<(), String> {
    let mut db = reference_state(pre);
    let reference = Context::mainnet().with_block(block.clone()).with_cfg(cfg.clone())
        .with_db(&mut db).build_mainnet().transact_commit(tx.clone());
    match reference {
        Err(e) => {
            if actual.status != crate::ffi::RES_INVALID_TX {
                return Err(format!("revm rejected ({e}); Vx returned {actual:?}"));
            }
            if vx_snapshot(pre) != vx_snapshot(post) || pre.accounts.iter().any(|(a, x)| post.accounts.get(a).map(|y| &y.code) != Some(&x.code)) {
                return Err("invalid transaction changed state".into());
            }
            return Ok(());
        }
        Ok(r) => {
            let expected = Observation::reference(&r);
            if *actual != expected { return Err(format!("observations: Vx={actual:?}; revm={expected:?}")); }
        }
    }
    check_state(&db, post)
}

/// Compare a committed sequence without re-executing its growing prefix.
pub fn check_state(db: &State<revm::database::EmptyDB>, post: &Host) -> Result<(), String> {
    use revm::primitives::B256;
    let mut expected = Host::default();
    for (a, acc) in db.cache.trie_account() {
        let hash = if acc.info.code_hash == B256::ZERO { KECCAK_EMPTY } else { acc.info.code_hash };
        let code = db.cache.contracts.get(&hash).map(|b| b.original_bytes()).unwrap_or_default();
        expected.insert(a, acc.info.balance, acc.info.nonce, code,
            acc.storage.iter().map(|(k, v)| (*k, *v)).collect());
    }
    if vx_snapshot(post) != vx_snapshot(&expected) {
        return Err(format!("post-state: Vx={:?}; revm={:?}", vx_snapshot(post), vx_snapshot(&expected)));
    }
    for (a, acc) in &expected.accounts {
        if post.accounts.get(a).map(|x| &x.code) != Some(&acc.code) { return Err(format!("code bytes differ at {a}")); }
    }
    Ok(())
}

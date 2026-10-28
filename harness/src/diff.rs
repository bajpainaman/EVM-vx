//! Differential debugging: runs revm on a failing case and reports where the
//! Vx post-state differs. Enabled with EVMVX_DIFF=1.

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

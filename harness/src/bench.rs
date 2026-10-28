//! In-process benchmark: runs the same state-test cases through the Vx engine
//! and through revm, timing execute + commit on each. Both read the same
//! parsed CfgEnv/BlockEnv/TxEnv, so parsing is outside the timers. revm runs
//! its standard statetest path (State over a cached prestate).

use crate::env;
use crate::ffi::*;
use crate::host::Host;
use revm::{
    context::cfg::CfgEnv,
    database::State,
    primitives::{hardfork::SpecId, U256},
    statetest_types::{SpecName, TestSuite},
    Context, ExecuteCommitEvm, MainBuilder, MainContext,
};
use std::path::Path;
use std::time::Instant;

#[derive(Default)]
pub struct Totals {
    pub cases: usize,
    pub vx_ns: u128,
    pub revm_ns: u128,
}

pub fn run_file(path: &Path, reps: u32, t: &mut Totals) {
    if crate::skip_test(path) {
        return;
    }
    let text = std::fs::read_to_string(path).unwrap();
    let suite: TestSuite = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(_) => return,
    };
    for (_name, unit) in suite.0 {
        let cache = unit.state();
        let mut host = Host::default();
        for (addr, info) in &unit.pre {
            host.insert(*addr, info.balance, info.nonce, info.code.clone(), info.storage.iter().map(|(k, v)| (*k, *v)).collect());
        }
        let mut cfg = CfgEnv::default();
        cfg.chain_id = unit.env.current_chain_id.unwrap_or(U256::ONE).try_into().unwrap_or(1);
        for (spec_name, tests) in &unit.post {
            if *spec_name == SpecName::Constantinople || *spec_name == SpecName::Unknown {
                continue;
            }
            let spec = spec_name.to_spec_id();
            cfg.set_spec_and_mainnet_gas_params(spec);
            if spec.is_enabled_in(SpecId::OSAKA) {
                cfg.set_max_blobs_per_tx(6);
            } else if spec.is_enabled_in(SpecId::PRAGUE) {
                cfg.set_max_blobs_per_tx(9);
            } else {
                cfg.set_max_blobs_per_tx(6);
            }
            let block = unit.block_env(&mut cfg);
            for test in tests.iter() {
                if test.expect_exception.is_some() {
                    continue;
                }
                let Ok(tx) = test.tx_env(&unit) else { continue };
                let eb = env::build(spec, &cfg, &block, &tx);

                // Validity check once, untimed: both engines must execute it.
                {
                    let mut h = host.clone();
                    let res = h.transact(&eb.env);
                    let ok = unsafe { (*res).status } != RES_INVALID_TX && unsafe { (*res).status } != RES_HOST_FAILURE;
                    unsafe { evmvx_result_free(res) };
                    if !ok {
                        continue;
                    }
                }

                for _ in 0..reps {
                    // Vx: transact + commit into the Host maps.
                    let mut h = host.clone();
                    let t0 = Instant::now();
                    let res = h.transact(&eb.env);
                    h.commit(unsafe { &*res });
                    t.vx_ns += t0.elapsed().as_nanos();
                    unsafe { evmvx_result_free(res) };

                    // revm: transact_commit into State over the same prestate.
                    let mut state = State::builder().with_cached_prestate(cache.clone()).with_bundle_update().build();
                    let t0 = Instant::now();
                    let mut evm = Context::mainnet()
                        .with_block(block.clone())
                        .with_tx(tx.clone())
                        .with_cfg(cfg.clone())
                        .with_db(&mut state)
                        .build_mainnet();
                    let _ = evm.transact_commit(tx.clone());
                    t.revm_ns += t0.elapsed().as_nanos();
                }
                t.cases += 1;
            }
        }
    }
}

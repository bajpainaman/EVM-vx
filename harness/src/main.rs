//! evmvx-test: runs Ethereum state tests against the Vx EVM, like `revme statetest`.
//!
//! Use --help for fixture, differential, and benchmark modes.

mod bctest;
mod bench;
mod cli;
mod diff;
mod env;
mod ffi;
mod host;
mod observe;
mod pexec;
mod pfuzz;
mod workload;
#[cfg(test)]
mod native_tests;

use ffi::*;
use host::Host;
use revm::{
    context::cfg::CfgEnv,
    primitives::{hardfork::SpecId, keccak256, Log, B256, U256},
    statetest_types::{SpecName, TestSuite},
};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Outcome of one test case.
enum Outcome {
    Pass,
    Fail(String),
}

fn run_case(
    host: &Host,
    spec: SpecId,
    cfg: &CfgEnv,
    block: &revm::context::BlockEnv,
    tx: &revm::context::TxEnv,
    expect_exception: bool,
    want_root: B256,
    want_logs: B256,
    cache: &revm::database::CacheState,
) -> Outcome {
    let mut host = host.clone();
    if host::db_mode() {
        host.attach_db();
    }
    let eb = env::build(spec, cfg, block, tx);
    let res = host.transact(&eb.env);
    let r = unsafe { &*res };
    let outcome = match r.status {
        RES_HOST_FAILURE => Outcome::Fail(format!("host failure (reason {})", r.reason)),
        RES_INVALID_TX if expect_exception => Outcome::Pass,
        RES_INVALID_TX => Outcome::Fail(format!("unexpected invalid tx (reason {})", r.reason)),
        _ if expect_exception => Outcome::Fail("expected exception, tx executed".into()),
        _ => {
            let logs: Vec<Log> = host::logs_of(r);
            host.commit(r);
            let logs_root = log_rlp_hash(&logs);
            let root = state_root(&host);
            if logs_root != want_logs {
                Outcome::Fail(format!("logs root {logs_root} != {want_logs}"))
            } else if root != want_root {
                let mut msg = format!("state root {root} != {want_root}");
                if diff::enabled() {
                    msg.push_str(&diff::report(cache, cfg, block, tx, &host, r.gas_used, r.status));
                }
                Outcome::Fail(msg)
            } else {
                Outcome::Pass
            }
        }
    };
    unsafe { evmvx_result_free(res) };
    host.detach_db();
    outcome
}

fn log_rlp_hash(logs: &[Log]) -> B256 {
    let mut out = Vec::new();
    alloy_rlp::encode_list(logs, &mut out);
    keccak256(&out)
}

fn state_root(host: &Host) -> B256 {
    use alloy_trie::{root::storage_root_unhashed, HashBuilder, Nibbles, TrieAccount};
    let mut v: Vec<(B256, TrieAccount)> = host
        .accounts
        .iter()
        .map(|(a, acc)| {
            let storage_root = storage_root_unhashed(
                acc.storage
                    .iter()
                    .filter(|(_, v)| !v.is_zero())
                    .map(|(k, v)| (B256::from(*k), *v)),
            );
            (
                keccak256(a),
                TrieAccount { nonce: acc.nonce, balance: acc.balance, storage_root, code_hash: acc.code_hash },
            )
        })
        .collect();
    v.sort_unstable_by_key(|(k, _)| *k);
    let mut hb = HashBuilder::default();
    let mut buf = Vec::new();
    for (k, acc) in v {
        buf.clear();
        alloy_rlp::Encodable::encode(&acc, &mut buf);
        hb.add_leaf(Nibbles::unpack(k), &buf);
    }
    hb.root()
}

/// Tests revm's own runner skips (storage-collision and slow cases).
fn skip_test(path: &Path) -> bool {
    let s = path.to_str().unwrap_or_default();
    if s.contains("paris/eip7610_create_collision") {
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
            | "test_init_collision_create_opcode.json" | "ValueOverflow.json" | "ValueOverflowParis.json"
            | "Call50000_sha256.json" | "static_Call50000_sha256.json" | "loopMul.json"
            | "CALLBlake2f_MaxRounds.json"
    )
}

/// Runs every case in one fixture file; returns (passed, failures).
fn run_file(path: &Path) -> (usize, Vec<String>) {
    if skip_test(path) {
        return (0, Vec::new());
    }
    let text = std::fs::read_to_string(path).unwrap();
    let suite: TestSuite = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(e) => return (0, vec![format!("{}: parse error: {e}", path.display())]),
    };
    let mut passed = 0;
    let mut fails = Vec::new();
    for (name, unit) in suite.0 {
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
            for (i, test) in tests.iter().enumerate() {
                let tx = match test.tx_env(&unit) {
                    Ok(tx) => tx,
                    Err(_) if test.expect_exception.is_some() => {
                        passed += 1;
                        continue;
                    }
                    Err(e) => {
                        fails.push(format!("{} {name} {spec:?}#{i}: tx env: {e}", path.display()));
                        continue;
                    }
                };
                match run_case(&host, spec, &cfg, &block, &tx, test.expect_exception.is_some(), test.hash, test.logs, &cache) {
                    Outcome::Pass => passed += 1,
                    Outcome::Fail(why) => fails.push(format!("{} {name} {spec:?}#{i}: {why}", path.display())),
                }
            }
        }
    }
    (passed, fails)
}

fn json_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for p in paths {
        for entry in WalkDir::new(p) {
            let e = entry.map_err(|error| format!("fixture path {}: {error}", p.display()))?;
            if e.path().extension().is_some_and(|x| x == "json") {
                out.push(e.path().to_path_buf());
            }
        }
    }
    out.sort();
    out.dedup();
    if !paths.is_empty() && out.is_empty() {
        return Err("fixture paths contain no JSON files".into());
    }
    Ok(out)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|value| value == "--help") {
        println!("{}", cli::USAGE);
        return;
    }
    let options = cli::parse(&args).unwrap_or_else(|error| {
        eprintln!("{error}\n{}", cli::USAGE);
        std::process::exit(2);
    });
    let files = json_files(&options.paths).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2);
    });
    let cli::Options {
        mode, show, reps, txs: ptx, threads: pthreads, contracts: pcontracts,
        blocks: pblocks, seed: pseed, workload, json, ..
    } = options;
    if let Ok(value) = std::env::var("EVMVX_BLOCK") {
        if value.parse::<usize>().map_or(true, |workers| workers == 0) {
            eprintln!("EVMVX_BLOCK must be a positive worker count");
            std::process::exit(2);
        }
    }
    // ABI layout check: Vx struct sizes must match the Rust #[repr(C)] mirrors.
    {
        let mut sz = [0u64; 7];
        unsafe { evmvx_abi_sizes(sz.as_mut_ptr()) };
        let want = [
            std::mem::size_of::<VxEnv>(), std::mem::size_of::<HostAccount>(),
            std::mem::size_of::<OutAccount>(), std::mem::size_of::<OutSlot>(),
            std::mem::size_of::<OutLog>(), std::mem::size_of::<VxResult>(),
            std::mem::size_of::<VxAuth>(),
        ];
        for (i, w) in want.iter().enumerate() {
            assert_eq!(sz[i] as usize, *w, "ABI size mismatch at struct {i}: vx={} rust={w}", sz[i]);
        }
    }
    // EVM frames use an explicit heap stack; the harness needs no giant
    // virtual reservation for nested contracts.
    let handle = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            if mode == "pfuzz" {
                return match pfuzz::run(pblocks, ptx.max(2), pseed, pthreads) {
                    Ok((txs, blocks, reexecs, cbtx)) => {
                        println!(
                            "pfuzz: {blocks} blocks, {txs} txs, seed {pseed}, {pthreads} threads: revm == vx-seq == vx-par on every tx and final state | {reexecs} re-executions, {cbtx} coinbase re-runs"
                        );
                        true
                    }
                    Err(e) => {
                        println!("pfuzz FAILED: {e}");
                        false
                    }
                };
            }
            if mode == "pexec" {
                pexec::run(ptx, pthreads, pcontracts, reps, workload, json);
                return true;
            }
            if mode == "bench" {
                let mut t = bench::Totals::default();
                for f in &files {
                    bench::run_file(f, reps, &mut t);
                }
                if t.cases == 0 {
                    eprintln!("no executable benchmark cases in the supplied fixtures");
                    return false;
                }
                let ratio = t.vx_ns as f64 / t.revm_ns.max(1) as f64;
                println!(
                    "bench: {} cases x{reps} | vx {:.1} ms, revm {:.1} ms | vx/revm = {ratio:.2}",
                    t.cases,
                    t.vx_ns as f64 / 1e6,
                    t.revm_ns as f64 / 1e6
                );
                return true;
            }
            let mut passed = 0;
            let mut fails = Vec::new();
            let mut skipped = 0;
            for f in &files {
                if mode == "btest" {
                    let (p, sk, mut fl) = bctest::run_file(f);
                    passed += p;
                    skipped += sk;
                    fails.append(&mut fl);
                } else {
                    let (p, mut fl) = run_file(f);
                    passed += p;
                    fails.append(&mut fl);
                }
            }
            if skipped > 0 {
                println!("{skipped} cases skipped (transition forks, as revme does)");
            }
            if host::block_threads() > 0 {
                use std::sync::atomic::Ordering::Relaxed;
                println!(
                    "parallel executor: {} blocks run in parallel, {} aborted to sequential, {} coinbase-touching txs re-run exactly at commit",
                    bctest::PAR_BLOCKS.load(Relaxed),
                    bctest::PAR_ABORTS.load(Relaxed),
                    bctest::PAR_CB_TXS.load(Relaxed)
                );
            }
            for f in fails.iter().take(show) {
                println!("FAIL {f}");
            }
            println!("{} files, {passed} passed, {} failed", files.len(), fails.len());
            if passed == 0 && fails.is_empty() {
                eprintln!("no executable fixture cases; check fork selection and skip policies");
                return false;
            }
            fails.is_empty()
        })
        .unwrap();
    let ok = handle.join().unwrap();
    std::process::exit(if ok { 0 } else { 1 });
}

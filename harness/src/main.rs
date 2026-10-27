//! evmvx-test: runs Ethereum state tests against the Vx EVM, like `revme statetest`.
//!
//! Usage: evmvx-test statetest <path>... [--keep-going]

mod bctest;
mod bench;
mod diff;
mod env;
mod ffi;
mod host;
mod pexec;

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

fn json_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in paths {
        for e in WalkDir::new(p).into_iter().filter_map(Result::ok) {
            if e.path().extension().is_some_and(|x| x == "json") {
                out.push(e.path().to_path_buf());
            }
        }
    }
    out.sort();
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or_default().to_string();
    if mode != "statetest" && mode != "btest" && mode != "bench" && mode != "pexec" {
        eprintln!("usage: evmvx-test statetest|btest|bench <path>... [--show N] [--reps N]");
        eprintln!("       evmvx-test pexec [--txs N] [--threads N] [--contracts N]");
        std::process::exit(2);
    }
    let mut show = 20usize;
    let mut reps = 1u32;
    let mut ptx = 2000usize;
    let mut pthreads = 8usize;
    let mut pcontracts = 2000u64;
    let mut paths = Vec::new();
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        let mut num = |d: u64| it.next().and_then(|s| s.parse().ok()).unwrap_or(d);
        match a.as_str() {
            "--show" => show = num(20) as usize,
            "--reps" => reps = num(1) as u32,
            "--txs" => ptx = num(2000) as usize,
            "--threads" => pthreads = num(8) as usize,
            "--contracts" => pcontracts = num(2000),
            _ => paths.push(PathBuf::from(a)),
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
    let files = json_files(&paths);
    // Deep call stacks recurse in the engine; run on a large stack.
    let handle = std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || {
            if mode == "pexec" {
                pexec::run(ptx, pthreads, pcontracts);
                return true;
            }
            if mode == "bench" {
                let mut t = bench::Totals::default();
                for f in &files {
                    bench::run_file(f, reps, &mut t);
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
            for f in fails.iter().take(show) {
                println!("FAIL {f}");
            }
            println!("{} files, {passed} passed, {} failed", files.len(), fails.len());
            fails.is_empty()
        })
        .unwrap();
    let ok = handle.join().unwrap();
    std::process::exit(if ok { 0 } else { 1 });
}

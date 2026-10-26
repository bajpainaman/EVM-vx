//! evmvx-test: runs Ethereum state tests against the Vx EVM, like `revme statetest`.
//!
//! Usage: evmvx-test statetest <path>... [--keep-going]

mod diff;
mod ffi;
mod host;

use ffi::*;
use host::{slice, Host};
use revm::{
    context::{cfg::CfgEnv, either::Either},
    context_interface::Cfg,
    primitives::{hardfork::SpecId, keccak256, Bytes, Log, LogData, B256, U256},
    statetest_types::{SpecName, TestSuite},
};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

fn word_ptr(v: &[Word]) -> *const Word {
    if v.is_empty() { std::ptr::null() } else { v.as_ptr() }
}

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
    let al_addrs: Vec<Word> = tx.access_list.0.iter().map(|i| i.address.into()).collect();
    let al_nslots: Vec<u64> = tx.access_list.0.iter().map(|i| i.storage_keys.len() as u64).collect();
    let al_slots: Vec<Word> = tx
        .access_list
        .0
        .iter()
        .flat_map(|i| i.storage_keys.iter().map(|k| Word::from(*k)))
        .collect();
    let blobs: Vec<Word> = tx.blob_hashes.iter().map(|h| (*h).into()).collect();
    let auths: Vec<VxAuth> = tx
        .authorization_list
        .iter()
        .filter_map(|a| match a {
            Either::Left(s) => Some(VxAuth {
                chain_id: (*s.chain_id()).into(),
                address: (*s.address()).into(),
                nonce: s.nonce(),
                y_parity: s.y_parity() as u64,
                r: s.r().into(),
                s: s.s().into(),
            }),
            Either::Right(_) => None,
        })
        .collect();
    let (blob_excess, blob_price, has_blob) = match &block.blob_excess_gas_and_price {
        Some(b) => (b.excess_blob_gas, U256::from(b.blob_gasprice), 1),
        None => (0, U256::ZERO, 0),
    };
    let mut flags = 0u64;
    if cfg.tx_chain_id_check() {
        flags |= CFG_TX_CHAIN_ID_CHECK;
    }
    let env = VxEnv {
        spec: spec as u64,
        chain_id: cfg.chain_id(),
        cfg_flags: flags,
        tx_gas_limit_cap: cfg.tx_gas_limit_cap(),
        max_blobs_per_tx: cfg.max_blobs_per_tx().unwrap_or(0),
        max_code_size: cfg.max_code_size() as u64,
        max_initcode_size: cfg.max_initcode_size() as u64,
        number: block.number.into(),
        coinbase: block.beneficiary.into(),
        timestamp: block.timestamp.into(),
        block_gas_limit: block.gas_limit,
        basefee: block.basefee,
        difficulty: block.difficulty.into(),
        prevrandao: block.prevrandao.unwrap_or_default().into(),
        has_prevrandao: block.prevrandao.is_some() as u64,
        excess_blob_gas: blob_excess,
        blob_gasprice: blob_price.into(),
        has_blob,
        tx_type: tx.tx_type as u64,
        caller: tx.caller.into(),
        gas_limit: tx.gas_limit,
        gas_price: U256::from(tx.gas_price).into(),
        priority_fee: U256::from(tx.gas_priority_fee.unwrap_or(0)).into(),
        has_priority_fee: tx.gas_priority_fee.is_some() as u64,
        is_create: tx.kind.is_create() as u64,
        to: tx.kind.to().copied().unwrap_or_default().into(),
        value: tx.value.into(),
        data: tx.data.as_ptr(),
        data_len: tx.data.len() as u64,
        nonce: tx.nonce,
        has_tx_chain_id: tx.chain_id.is_some() as u64,
        tx_chain_id: tx.chain_id.unwrap_or(0),
        al_count: al_addrs.len() as u64,
        al_addrs: word_ptr(&al_addrs),
        al_nslots: if al_nslots.is_empty() { std::ptr::null() } else { al_nslots.as_ptr() },
        al_slots: word_ptr(&al_slots),
        blob_count: blobs.len() as u64,
        blob_hashes: word_ptr(&blobs),
        max_fee_per_blob_gas: U256::from(tx.max_fee_per_blob_gas).into(),
        auth_count: auths.len() as u64,
        auths: if auths.is_empty() { std::ptr::null() } else { auths.as_ptr() },
    };
    let res = unsafe { evmvx_transact(&mut host as *mut Host as *mut _, &env) };
    let r = unsafe { &*res };
    let outcome = match r.status {
        RES_HOST_FAILURE => Outcome::Fail(format!("host failure (reason {})", r.reason)),
        RES_INVALID_TX if expect_exception => Outcome::Pass,
        RES_INVALID_TX => Outcome::Fail(format!("unexpected invalid tx (reason {})", r.reason)),
        _ if expect_exception => Outcome::Fail("expected exception, tx executed".into()),
        _ => {
            let logs: Vec<Log> = unsafe { slice(r.logs, r.log_count) }
                .iter()
                .map(|l| {
                    let topics = l.topics[..l.topic_count as usize].iter().map(|t| t.b256()).collect();
                    let data = Bytes::copy_from_slice(unsafe { slice(l.data, l.data_len) });
                    Log { address: l.address.address(), data: LogData::new_unchecked(topics, data) }
                })
                .collect();
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
    if args.first().map(String::as_str) != Some("statetest") {
        eprintln!("usage: evmvx-test statetest <path>... [--show N]");
        std::process::exit(2);
    }
    let mut show = 20usize;
    let mut paths = Vec::new();
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        if a == "--show" {
            show = it.next().and_then(|s| s.parse().ok()).unwrap_or(20);
        } else {
            paths.push(PathBuf::from(a));
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
            let mut passed = 0;
            let mut fails = Vec::new();
            for f in &files {
                let (p, mut fl) = run_file(f);
                passed += p;
                fails.append(&mut fl);
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

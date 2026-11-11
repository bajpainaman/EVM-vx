//! Cancun transactions through the actual native ABI, never a model interpreter.
use crate::{diff::{self, Observation}, env, ffi::*, host::Host};
use revm::{context::{cfg::CfgEnv, BlockEnv, TxEnv}, primitives::{hardfork::SpecId, Address, Bytes, TxKind, B256, U256}};
use std::collections::HashMap;

fn address(n: u8) -> Address { Address::from([n; 20]) }
fn bytes(s: &str) -> Bytes {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i+2], 16).unwrap()).collect::<Vec<_>>().into()
}

fn setup(code: Bytes, original: u64) -> (Host, CfgEnv, BlockEnv, TxEnv) {
    let mut h = Host::default();
    h.insert(address(1), U256::from(1_000_000_000), 7, Bytes::new(), HashMap::new());
    h.insert(address(2), U256::from(19), 1, code, [(U256::ZERO, U256::from(original))].into());
    h.insert(address(3), U256::from(23), 0, Bytes::new(), HashMap::new());
    let mut cfg = CfgEnv::default();
    cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN);
    let mut block = BlockEnv::default();
    block.basefee = 7;
    block.beneficiary = address(3);
    block.prevrandao = Some(B256::ZERO);
    let mut tx = TxEnv::default();
    tx.caller = address(1);
    tx.kind = TxKind::Call(address(2));
    tx.nonce = 7;
    tx.gas_price = 13;
    tx.gas_limit = 100_000;
    tx.value = U256::from(123);
    (h, cfg, block, tx)
}

fn execute(pre: &Host, cfg: &CfgEnv, block: &BlockEnv, tx: &TxEnv, db: bool) -> (Observation, Host) {
    let mut post = pre.clone();
    if db { post.attach_db(); }
    let eb = env::build(cfg.spec, cfg, block, tx);
    let p = post.transact(&eb.env);
    assert!(!p.is_null());
    let r = unsafe { &*p };
    let obs = Observation::vx(r);
    assert_ne!(obs.status, RES_HOST_FAILURE, "host failure {}", r.reason);
    if obs.status != RES_INVALID_TX { post.commit(r); }
    unsafe { evmvx_result_free(p) };
    diff::check(pre, cfg, block, tx, &obs, &post).unwrap();
    if db {
        // Check actual StateDb balances/nonces/slots, not only the Rust mirror.
        for (a, acc) in &post.accounts {
            let aw = Word::from(*a);
            let mut out = HostAccount { balance: Word::default(), nonce: 0, code_hash: Word::default(), code: std::ptr::null(), code_len: 0 };
            assert_eq!(unsafe { evmvx_db_get_account(post.db as _, &aw, &mut out) }, HOST_OK);
            assert_eq!((out.balance.u256(), out.nonce, out.code_hash.b256()), (acc.balance, acc.nonce, acc.code_hash));
            assert_eq!(unsafe { crate::host::slice(out.code, out.code_len) }, acc.code.as_ref());
            for (k, v) in &acc.storage {
                let mut value = Word::default();
                unsafe { evmvx_db_get_slot(post.db as _, &aw, &Word::from(*k), &mut value) };
                assert_eq!(value.u256(), *v);
            }
        }
    }
    post.detach_db();
    (obs, post)
}

#[test]
fn storage_return_revert_oog_and_settlement() {
    // Store runtime calldata, return two different words, and emit LOG1.
    let body = "60003560005560005460005260203560205260ab60406000a1";
    for db in [false, true] {
        for (tail, status) in [("60406000f3", RES_SUCCESS), ("60406000fd", RES_REVERT), ("60006201ffff5200", RES_HALT)] {
            let (h, cfg, block, mut tx) = setup(bytes(&format!("{body}{tail}")), 9);
            let mut data = vec![0u8; 64];
            data[0] = 0x81; data[31] = 0x37; data[32] = 0x19; data[63] = 0xa3;
            tx.data = data.clone().into();
            tx.gas_limit = 50_000;
            let (r, post) = execute(&h, &cfg, &block, &tx, db);
            assert_eq!(r.status, status);
            assert_eq!(r.output, if status == RES_HALT { vec![] } else { data.clone() });
            assert_eq!(r.logs.len(), usize::from(status == RES_SUCCESS));
            assert_eq!(post.accounts[&address(1)].nonce, 8);
            let value = if status == RES_SUCCESS { 123 } else { 0 };
            assert_eq!(post.accounts[&address(1)].balance, U256::from(1_000_000_000 - r.gas * 13 - value));
            assert_eq!(post.accounts[&address(2)].balance, U256::from(19 + value));
            assert_eq!(post.accounts[&address(3)].balance, U256::from(23 + r.gas * 6));
            assert_eq!(post.accounts[&address(2)].storage[&U256::ZERO], if status == RES_SUCCESS { U256::from_be_slice(&data[..32]) } else { U256::from(9) });
            if status == RES_HALT { assert_eq!(r.gas, tx.gas_limit); }
            eprintln!("native Cancun db={db} status={status} gas={} refund={} output={} bytes", r.gas, r.refund, r.output.len());
        }
    }
}

#[test]
fn storage_gas_refunds_and_memory_boundaries() {
    for db in [false, true] {
        for (code, original, gas, refund) in [
            ("600054506000545000", 9, 23_210, 0), // cold then warm SLOAD
            ("600060005500", 9, 21_206, 4_800), // clear nonzero original
            ("6000600055600960005500", 9, 23_312, 2_800), // dirty restore original
            ("6000600055600160005500", 9, 26_112, 0), // undo an earlier clear refund
            ("602a600055600060005500", 0, 34_570, 8_642), // zero restore, refund cap
            ("600960005500", 9, 23_206, 0), // cold no-op SSTORE
        ] {
            let (h, cfg, b, tx) = setup(bytes(code), original);
            let (r, _) = execute(&h, &cfg, &b, &tx, db);
            assert_eq!((r.gas, r.refund), (gas, refund), "{code}");
        }
        // EIP-2200's <=2300 sentry rejects even a cheap no-op SSTORE.
        let (h, cfg, b, mut tx) = setup(bytes("600960005500"), 9);
        tx.gas_limit = 21_006 + 2_300;
        assert_eq!(execute(&h, &cfg, &b, &tx, db).0.status, RES_HALT);
        tx.gas_limit += 1;
        assert_eq!(execute(&h, &cfg, &b, &tx, db).0.status, RES_SUCCESS);
        // MSTORE8 expansion at word and quadratic-cost boundaries.
        for offset in [0u64, 30, 31, 32, 33, 671, 672, 703, 704, 735, 736] {
            let (h, cfg, b, mut tx) = setup(bytes(&format!("60ab61{offset:04x}5300")), 0);
            let words = offset / 32 + 1;
            let gas = 21_009 + 3 * words + words * words / 512;
            let (r, _) = execute(&h, &cfg, &b, &tx, db);
            assert_eq!(r.gas, gas, "offset {offset}");
            tx.gas_limit = gas;
            assert_eq!(execute(&h, &cfg, &b, &tx, db).0.status, RES_SUCCESS);
            tx.gas_limit -= 1;
            assert_eq!(execute(&h, &cfg, &b, &tx, db).0.status, RES_HALT);
        }
    }
}

#[test]
fn validation_calldata_and_stack_boundaries() {
    for db in [false, true] {
        let (h, cfg, b, mut tx) = setup(bytes("00"), 0);
        tx.data = vec![0, 1, 0x80, 0].into();
        assert_eq!(execute(&h, &cfg, &b, &tx, db).0.gas, 21_040);
        tx.gas_limit = 21_039;
        assert_eq!(execute(&h, &cfg, &b, &tx, db).0.status, RES_INVALID_TX);
        tx.gas_limit = 100_000;
        tx.nonce = 6;
        assert_eq!(execute(&h, &cfg, &b, &tx, db).0.status, RES_INVALID_TX);
        tx.nonce = 8;
        assert_eq!(execute(&h, &cfg, &b, &tx, db).0.status, RES_INVALID_TX);
        tx.nonce = 7;
        let mut poor = h.clone();
        poor.accounts.get_mut(&address(1)).unwrap().balance = U256::from(tx.gas_limit * 13 + 122);
        assert_eq!(execute(&poor, &cfg, &b, &tx, db).0.status, RES_INVALID_TX);
        poor.accounts.get_mut(&address(1)).unwrap().balance += U256::from(1);
        assert_eq!(execute(&poor, &cfg, &b, &tx, db).0.status, RES_SUCCESS);
        tx.gas_price = 6;
        assert_eq!(execute(&h, &cfg, &b, &tx, db).0.status, RES_INVALID_TX);
        for (code, status) in [("01".to_string(), RES_HALT), ("5f".repeat(1024), RES_SUCCESS), ("5f".repeat(1025), RES_HALT)] {
            let (h, cfg, b, tx) = setup(bytes(&code), 0);
            assert_eq!(execute(&h, &cfg, &b, &tx, db).0.status, status);
        }
    }
}

#[test]
fn differential_negative_controls() {
    let (h, cfg, b, tx) = setup(bytes("600060005560ab60005260206000f3"), 9);
    let (r, post) = execute(&h, &cfg, &b, &tx, false);
    for field in 0..6 {
        let mut wrong = r.clone();
        match field {
            0 => wrong.gas += 1,
            1 => wrong.refund = 0,
            2 => wrong.output[31] ^= 1,
            3 => wrong.status = RES_REVERT,
            4 => wrong.logs.push(revm::primitives::Log::default()),
            _ => wrong.created = Some(address(4)),
        }
        assert!(diff::check(&h, &cfg, &b, &tx, &wrong, &post).is_err(), "observation control {field}");
    }
    for field in 0..5 {
        let mut wrong = post.clone();
        match field {
            0 => wrong.accounts.get_mut(&address(1)).unwrap().nonce -= 1,
            1 => wrong.accounts.get_mut(&address(3)).unwrap().balance += U256::from(1),
            2 => { wrong.accounts.get_mut(&address(2)).unwrap().storage.insert(U256::ZERO, U256::from(9)); },
            3 => wrong.accounts.get_mut(&address(2)).unwrap().code = bytes("00"),
            _ => { wrong.accounts.remove(&address(3)); },
        }
        assert!(diff::check(&h, &cfg, &b, &tx, &r, &wrong).is_err(), "state control {field}");
    }
}

#[test]
fn empty_account_existence_and_refund_floor() {
    for db in [false, true] {
        let (mut h, cfg, b, mut tx) = setup(bytes("00"), 0);
        h.accounts.remove(&address(3));
        h.insert(address(4), U256::ZERO, 0, Bytes::new(), HashMap::new());
        tx.gas_price = 7; // no beneficiary tip: the touched empty account is removed
        let (r, mut post) = execute(&h, &cfg, &b, &tx, db);
        assert!(!post.accounts.contains_key(&address(3)));
        assert!(post.accounts.contains_key(&address(4))); // untouched empty survives
        post.accounts.remove(&address(4));
        assert!(diff::check(&h, &cfg, &b, &tx, &r, &post).is_err());

        // The result-field fix must also respect the existing Prague floor.
        let (h, mut cfg, b, mut tx) = setup(bytes("600060005500"), 9);
        cfg.set_spec_and_mainnet_gas_params(SpecId::PRAGUE);
        tx.data = vec![1u8; 512].into();
        let (r, _) = execute(&h, &cfg, &b, &tx, db);
        assert_eq!((r.gas, r.refund), (41_480, 0));
    }
}

#[test]
fn reused_context_keeps_results_independent() {
    let writer = "60003560005560005460005260206000a060206000f3";
    for db in [false, true] {
        let (mut h, mut cfg, b, mut tx) = setup(bytes(writer), 9);
        if db { h.attach_db(); }
        let mut retained = Vec::new();
        // Invalid, revert and exceptional exits must reset exactly as success
        // does. Switching forks in both directions must rebuild opcode tables.
        for (code, spec, invalid, gas, status) in [
            (writer, SpecId::CANCUN, false, 100_000, RES_SUCCESS),
            ("600060005560ab60005260206000fd", SpecId::CANCUN, false, 100_000, RES_REVERT),
            ("600060005560006201ffff5200", SpecId::CANCUN, false, 50_000, RES_HALT),
            (writer, SpecId::CANCUN, true, 100_000, RES_INVALID_TX),
            ("5f00", SpecId::LONDON, false, 100_000, RES_HALT),
            ("5f00", SpecId::SHANGHAI, false, 100_000, RES_SUCCESS),
            ("5f00", SpecId::LONDON, false, 100_000, RES_HALT),
            (writer, SpecId::CANCUN, false, 100_000, RES_SUCCESS),
        ] {
            let a = h.accounts.get_mut(&address(2)).unwrap();
            a.code = bytes(code);
            a.code_hash = revm::primitives::keccak256(&a.code);
            if db {
                unsafe { evmvx_db_put_account(h.db as _, &address(2).into(), &a.balance.into(), a.nonce, a.code.as_ptr(), a.code.len() as u64) };
            }
            let pre = Host { accounts: h.accounts.clone(), block_hashes: h.block_hashes.clone(), ..Default::default() };
            cfg.set_spec_and_mainnet_gas_params(spec);
            tx.nonce = h.accounts[&address(1)].nonce + u64::from(invalid);
            tx.gas_limit = gas;
            tx.data = U256::from(tx.nonce + 41).to_be_bytes::<32>().to_vec().into();
            let p = { let eb = env::build(spec, &cfg, &b, &tx); h.transact(&eb.env) };
            let r = unsafe { &*p };
            let observed = Observation::vx(r);
            assert_eq!(observed.status, status);
            if status != RES_INVALID_TX { h.commit(r); }
            diff::check(&pre, &cfg, &b, &tx, &observed, &h).unwrap();
            let codes: Vec<_> = unsafe { crate::host::slice(r.accounts, r.account_count) }.iter()
                .map(|a| unsafe { crate::host::slice(a.code, a.code_len) }.to_vec()).collect();
            retained.push((p, observed, codes));
        }
        // No result may borrow the reset context, the replaced DB code, the
        // input environment, or even the database/host that produced it.
        drop(h);
        for (p, observed, codes) in retained {
            let r = unsafe { &*p };
            assert_eq!(Observation::vx(r), observed);
            let actual: Vec<_> = unsafe { crate::host::slice(r.accounts, r.account_count) }.iter()
                .map(|a| unsafe { crate::host::slice(a.code, a.code_len) }.to_vec()).collect();
            assert_eq!(actual, codes);
            unsafe { evmvx_result_free(p) };
        }
    }
}

#[test]
fn system_call_then_transaction_resets_state() {
    for db in [false, true] {
        let (mut h, cfg, b, mut tx) = setup(bytes("60003560005560005460005260206000f3"), 9);
        tx.value = U256::ZERO;
        tx.data = U256::from(42).to_be_bytes::<32>().to_vec().into();
        if db { h.attach_db(); }
        let p = { let eb = env::build(cfg.spec, &cfg, &b, &tx); h.system_call(&eb.env) };
        let r = unsafe { &*p };
        assert_eq!(r.status, RES_SUCCESS);
        assert_eq!(Observation::vx(r).output, tx.data.as_ref());
        h.commit(r);
        assert_eq!((h.accounts[&address(1)].nonce, h.accounts[&address(1)].balance), (7, U256::from(1_000_000_000)));
        assert_eq!(h.accounts[&address(3)].balance, U256::from(23));
        assert_eq!(h.accounts[&address(2)].storage[&U256::ZERO], U256::from(42));
        let pre = Host { accounts: h.accounts.clone(), block_hashes: h.block_hashes.clone(), ..Default::default() };
        tx.data = Bytes::new(); // clear the slot: original must be 42, not 9
        let q = { let eb = env::build(cfg.spec, &cfg, &b, &tx); h.transact(&eb.env) };
        let after = unsafe { &*q };
        h.commit(after);
        diff::check(&pre, &cfg, &b, &tx, &Observation::vx(after), &h).unwrap();
        assert_eq!(after.gas_refunded, 4_800);
        drop(h);
        assert_eq!(Observation::vx(unsafe { &*p }).output, U256::from(42).to_be_bytes::<32>());
        unsafe { evmvx_result_free(q); evmvx_result_free(p); }
    }
}

#[test]
fn host_failure_releases_execution_state() {
    for system in [false, true] {
        let (mut h, cfg, b, tx) = setup(bytes("60ab60005260206000a060005460005260206000f3"), 9);
        let pre = h.clone();
        h.fail_storage_reads = true;
        let p = {
            let eb = env::build(cfg.spec, &cfg, &b, &tx);
            if system { h.system_call(&eb.env) } else { h.transact(&eb.env) }
        };
        let r = unsafe { &*p };
        assert_eq!(r.status, RES_HOST_FAILURE); // never fabricated OOG/REVERT
        assert_eq!((r.account_count, r.slot_count, r.log_count), (0, 0, 0));
        unsafe { evmvx_result_free(p) };
        h.fail_storage_reads = false;
        let p = { let eb = env::build(cfg.spec, &cfg, &b, &tx); h.transact(&eb.env) };
        let r = unsafe { &*p };
        h.commit(r);
        diff::check(&pre, &cfg, &b, &tx, &Observation::vx(r), &h).unwrap();
        unsafe { evmvx_result_free(p) };
    }
}

#[test]
#[cfg(target_os = "linux")]
fn transaction_memory_stays_bounded() {
    fn rss() -> u64 {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        status.lines().find_map(|l| l.strip_prefix("VmRSS:"))
            .unwrap().split_whitespace().next().unwrap().parse::<u64>().unwrap() * 1024
    }
    for db in [false, true] {
        // Write and emit a 4 KiB log; odd calldata then reverts. Losing the
        // discarded log payload would exceed the bound below, even with reuse.
        let (mut h, cfg, b, mut tx) = setup(bytes("6000356000556000546000526110006000a0600035602b1460205760206000f35b60206000fd"), 9);
        h.accounts.get_mut(&address(1)).unwrap().balance = U256::from(1_000_000_000_000u64);
        if db { h.attach_db(); }
        tx.value = U256::ZERO;
        let mut before = 0;
        for i in 0..12_000 {
            if i == 2_000 { before = rss(); }
            tx.data = U256::from(42 + i % 2).to_be_bytes::<32>().to_vec().into();
            tx.nonce = h.accounts[&address(1)].nonce;
            let eb = env::build(cfg.spec, &cfg, &b, &tx);
            let p = h.transact(&eb.env);
            let r = unsafe { &*p };
            assert_eq!(r.status, if i % 2 == 0 { RES_SUCCESS } else { RES_REVERT });
            assert_eq!(r.log_count, if i % 2 == 0 { 1 } else { 0 });
            assert_eq!(unsafe { crate::host::slice(r.output, r.output_len) }, tx.data.as_ref());
            h.commit(r);
            unsafe { evmvx_result_free(p) };
        }
        let growth = rss().saturating_sub(before);
        eprintln!("after warmup, 10000 transactions db={db}: RSS growth {growth} B");
        // Permit one allocator chunk, not transaction-proportional growth.
        // The unfixed standalone path retained roughly 78553 requested B/tx.
        assert!(growth <= 8 * 1024 * 1024, "transaction memory grew by {growth} B");
        assert_eq!(h.accounts[&address(1)].nonce, 12_007);
    }
}

#[test]
#[ignore = "timing must run alone, after O0/O2 correctness"]
fn native_tx_timing() {
    use revm::{Context, ExecuteCommitEvm, MainBuilder, MainContext};
    use std::time::Instant;
    let (h, cfg, b, tx) = setup(bytes("60003560005560005460005260206000f3"), 9);
    let txs: Vec<_> = (0..1000u64).map(|i| {
        let mut t = tx.clone();
        t.nonce += i;
        t.data = U256::from(i + 1).to_be_bytes::<32>().to_vec().into();
        t
    }).collect();
    let ebs: Vec<_> = txs.iter().map(|t| env::build(cfg.spec, &cfg, &b, t)).collect();
    let mut times = [Vec::new(), Vec::new()];
    for rep in 0..12 {
        // Reset/preload both databases and build the reference context outside
        // each window. The Vx StateDb path has no per-op host callbacks.
        let mut vx = h.clone();
        vx.attach_db();
        let mut db = diff::reference_state(&h);
        let mut evm = Context::mainnet().with_block(b.clone()).with_cfg(cfg.clone())
            .with_db(&mut db).build_mainnet();
        let mut vr = Vec::with_capacity(txs.len());
        let mut rr = Vec::with_capacity(txs.len());
        let mut run_vx = || {
            let start = Instant::now();
            for eb in &ebs {
                let p = unsafe { evmvx_transact_db(vx.db as _, &eb.env) };
                unsafe { evmvx_db_apply(vx.db as _, p) };
                vr.push(p);
            }
            start.elapsed().as_secs_f64() * 1000.0
        };
        let mut run_revm = || {
            let start = Instant::now();
            for t in &txs { rr.push(evm.transact_commit(t.clone()).unwrap()); }
            start.elapsed().as_secs_f64() * 1000.0
        };
        // Serialized windows with alternating order, not concurrent timings.
        let (vt, rt) = if rep % 2 == 0 {
            let v = run_vx(); (v, run_revm())
        } else {
            let r = run_revm(); (run_vx(), r)
        };
        drop(evm);
        for (p, expected) in vr.into_iter().zip(rr) {
            let actual = unsafe { &*p };
            assert_eq!(Observation::vx(actual), Observation::reference(&expected));
            assert_eq!(actual.status, RES_SUCCESS);
            unsafe { evmvx_result_free(p) };
        }
        // Compare actual native committed state with revm, including fees and
        // nonce progression. This workload writes only slot zero.
        let mut count = 0;
        for (a, acc) in db.cache.trie_account() {
            count += 1;
            let aw = Word::from(a);
            let mut out = HostAccount { balance: Word::default(), nonce: 0, code_hash: Word::default(), code: std::ptr::null(), code_len: 0 };
            assert_eq!(unsafe { evmvx_db_get_account(vx.db as _, &aw, &mut out) }, HOST_OK);
            assert_eq!((out.balance.u256(), out.nonce, out.code_hash.b256()), (acc.info.balance, acc.info.nonce, acc.info.code_hash));
            let code = db.cache.contracts.get(&acc.info.code_hash).map(|c| c.original_bytes()).unwrap_or_default();
            assert_eq!(unsafe { crate::host::slice(out.code, out.code_len) }, code.as_ref());
            for (k, v) in &acc.storage {
                let mut value = Word::default();
                unsafe { evmvx_db_get_slot(vx.db as _, &aw, &Word::from(*k), &mut value) };
                assert_eq!(value.u256(), *v);
            }
        }
        assert_eq!(count, 3);
        if rep >= 2 {
            times[0].push(vt); times[1].push(rt);
            eprintln!("sample {}: Vx={vt:.6} ms revm={rt:.6} ms (1000 tx)", rep - 1);
        }
    }
    for (name, samples) in ["Vx", "revm"].into_iter().zip(&mut times) {
        samples.sort_by(f64::total_cmp);
        eprintln!("{name}: 10 measured windows after 2 warmups; median={:.6} ms min={:.6} max={:.6}",
            (samples[4] + samples[5]) / 2.0, samples[0], samples[9]);
    }
}

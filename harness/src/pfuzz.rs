//! Conflict stress test for the parallel block executor. Generates random
//! blocks built to conflict (one sender with several transactions, transfers
//! inside a small account set, a shared counter, a slot setter that logs, a
//! conditional reverter, coinbase tips, BALANCE reads of other accounts and of
//! the coinbase, nested calls, CREATE plus calls to the new contract) and runs
//! each block three ways: revm sequential, Vx sequential, Vx parallel. Final
//! state and every transaction's status and gas must match exactly.

use crate::env;
use crate::ffi::*;
use crate::host::Host;
use revm::context::{cfg::CfgEnv, BlockEnv, TxEnv};
use revm::database::{AccountState, CacheDB, EmptyDB};
use revm::primitives::{hardfork::SpecId, keccak256, Address, Bytes, TxKind, B256, U256};
use revm::state::{AccountInfo, Bytecode};
use revm::{Context, ExecuteCommitEvm, MainBuilder, MainContext};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::c_void;

const SPEC: SpecId = SpecId::CANCUN;
const EOAS: u64 = 8;
const SENDERS: u64 = 6;

/// xorshift64*: deterministic per seed, no dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn addr(tag: u8, i: u64) -> Address {
    let mut b = [0u8; 20];
    b[0] = tag;
    b[12..].copy_from_slice(&i.to_be_bytes());
    Address::from(b)
}

fn coinbase() -> Address {
    addr(0xcb, 0)
}

fn hex(s: &str) -> Bytes {
    let v: Vec<u8> = (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect();
    v.into()
}

/// The contracts, at addr(0xc0, i).
fn contracts() -> Vec<Bytes> {
    // 0 counter: SSTORE(0, SLOAD(0) + 1)
    let counter = "60005460010160005500".to_string();
    // 1 setter: SSTORE(cd0 % 8, cd1); LOG0(0, 0)
    let setter = "6020356008600035065560006000a000".to_string();
    // 2 reverter: SSTORE(0, 1); if cd0 & 1 { REVERT } else { SSTORE(1, 2) }
    let reverter = "6001600055600035600116601457600260015500".to_string() + "5b60006000fd";
    // 3 tipper: CALL(gas, COINBASE, cd0 % 1000)
    let tipper = "60006000600060006103e860003506415af15000".to_string();
    // 4 balance reader: SSTORE(0, BALANCE(cd0))
    let balance = "6000353160005500".to_string();
    // 5 caller: CALL(gas, counter, 0); SSTORE(0, 1)
    let caller = format!("6000600060006000600073{}5af150600160005500", hex_addr(addr(0xc0, 0)));
    [counter, setter, reverter, tipper, balance, caller].iter().map(|s| hex(s)).collect()
}

fn hex_addr(a: Address) -> String {
    a.iter().map(|b| format!("{b:02x}")).collect()
}

/// Init code that deploys the counter's runtime:
/// PUSH10 <runtime>; PUSH1 0; MSTORE; PUSH1 10; PUSH1 22; RETURN.
fn creator_init() -> Bytes {
    hex("6960005460010160005500600052600a6016f3")
}

fn word(v: U256) -> [u8; 32] {
    v.to_be_bytes()
}

struct Gen {
    rng: Rng,
    nonces: HashMap<Address, u64>,
    created: Vec<Address>,
}

impl Gen {
    fn tx(&mut self) -> TxEnv {
        let sender = addr(0xaa, self.rng.below(SENDERS));
        let nonce = *self.nonces.entry(sender).or_insert(0);
        let mut t = TxEnv::default();
        t.caller = sender;
        t.nonce = nonce;
        t.gas_limit = 300_000;
        t.gas_price = 1 + self.rng.below(20) as u128;
        t.chain_id = Some(1);
        let kind = self.rng.below(10);
        let cd = |a: U256, b: U256| -> Bytes { word(a).iter().chain(word(b).iter()).copied().collect::<Vec<u8>>().into() };
        match kind {
            0 | 1 => {
                t.kind = TxKind::Call(addr(0xaa, self.rng.below(EOAS)));
                t.value = U256::from(1 + self.rng.below(1_000_000));
            }
            2 => t.kind = TxKind::Call(addr(0xc0, 0)),
            3 => {
                t.kind = TxKind::Call(addr(0xc0, 1));
                t.data = cd(U256::from(self.rng.below(16)), U256::from(self.rng.below(5)));
            }
            4 => {
                t.kind = TxKind::Call(addr(0xc0, 2));
                t.data = cd(U256::from(self.rng.below(4)), U256::ZERO);
            }
            5 => {
                t.kind = TxKind::Call(addr(0xc0, 3));
                t.data = cd(U256::from(self.rng.below(5000)), U256::ZERO);
            }
            6 => {
                let who = if self.rng.below(3) == 0 { coinbase() } else { addr(0xaa, self.rng.below(EOAS)) };
                t.kind = TxKind::Call(addr(0xc0, 4));
                t.data = cd(U256::from_be_slice(who.as_slice()), U256::ZERO);
            }
            7 => t.kind = TxKind::Call(addr(0xc0, 5)),
            8 => {
                t.kind = TxKind::Create;
                t.data = creator_init();
                self.created.push(sender.create(nonce));
            }
            _ => {
                if let Some(c) = self.created.get(self.rng.below(self.created.len().max(1) as u64) as usize).copied() {
                    t.kind = TxKind::Call(c);
                } else {
                    t.kind = TxKind::Call(addr(0xc0, 0));
                }
            }
        }
        *self.nonces.get_mut(&sender).unwrap() += 1;
        t
    }
}

fn prestate() -> Vec<(Address, U256, u64, Bytes)> {
    let mut v = Vec::new();
    for (i, code) in contracts().into_iter().enumerate() {
        let bal = if i == 3 { U256::from(10u64.pow(18)) } else { U256::ZERO };
        v.push((addr(0xc0, i as u64), bal, 1, code));
    }
    for i in 0..EOAS.max(SENDERS) {
        v.push((addr(0xaa, i), U256::from(10u64.pow(20)), 0, Bytes::new()));
    }
    v
}

type Snap = BTreeMap<Address, (U256, u64, B256, BTreeMap<U256, U256>)>;

fn snap_host(h: &Host) -> Snap {
    h.accounts
        .iter()
        .map(|(a, x)| {
            let st: BTreeMap<U256, U256> = x.storage.iter().filter(|(_, v)| !v.is_zero()).map(|(k, v)| (*k, *v)).collect();
            (*a, (x.balance, x.nonce, x.code_hash, st))
        })
        .filter(|(_, (b, n, c, s))| !(b.is_zero() && *n == 0 && *c == revm::primitives::KECCAK_EMPTY && s.is_empty()))
        .collect()
}

fn snap_revm(db: &CacheDB<EmptyDB>) -> Snap {
    db.cache
        .accounts
        .iter()
        .filter(|(_, a)| !matches!(a.account_state, AccountState::NotExisting))
        .map(|(ad, a)| {
            let st: BTreeMap<U256, U256> = a.storage.iter().filter(|(_, v)| !v.is_zero()).map(|(k, v)| (*k, *v)).collect();
            let ch = if a.info.code_hash == B256::ZERO { revm::primitives::KECCAK_EMPTY } else { a.info.code_hash };
            (*ad, (a.info.balance, a.info.nonce, ch, st))
        })
        .filter(|(_, (b, n, c, s))| !(b.is_zero() && *n == 0 && *c == revm::primitives::KECCAK_EMPTY && s.is_empty()))
        .collect()
}

fn diff(a: &Snap, b: &Snap, an: &str, bn: &str) -> Option<String> {
    let keys: BTreeSet<&Address> = a.keys().chain(b.keys()).collect();
    for k in keys {
        if a.get(k) != b.get(k) {
            return Some(format!("{k}: {an}={:?} {bn}={:?}", a.get(k), b.get(k)));
        }
    }
    None
}

/// Returns (blocks, txs, multi-tx blocks, re-executions, coinbase re-runs).
pub fn run(blocks: usize, txs_per: usize, seed: u64, threads: usize) -> Result<(usize, usize, u64, u64), String> {
    let mut cfg = CfgEnv::default();
    cfg.set_spec_and_mainnet_gas_params(SPEC);
    let mut total_txs = 0;
    let mut reexecs = 0u64;
    let mut cbtx = 0u64;
    for bi in 0..blocks {
        let mut g = Gen { rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (bi as u64 + 1)), nonces: HashMap::new(), created: Vec::new() };
        let mut block = BlockEnv::default();
        block.number = U256::from(1 + bi);
        block.timestamp = U256::from(1000 + bi);
        block.gas_limit = u64::MAX / 4;
        block.basefee = 1;
        block.beneficiary = coinbase();
        block.prevrandao = Some(B256::ZERO);
        let n = 2 + g.rng.below(txs_per as u64 - 1) as usize;
        let txs: Vec<TxEnv> = (0..n).map(|_| g.tx()).collect();
        total_txs += n;

        // revm sequential.
        let mut rdb = CacheDB::new(EmptyDB::default());
        for (a, bal, nonce, code) in prestate() {
            let hash = if code.is_empty() { revm::primitives::KECCAK_EMPTY } else { keccak256(&code) };
            rdb.insert_account_info(a, AccountInfo { balance: bal, nonce, code_hash: hash, code: Some(Bytecode::new_raw(code)), ..Default::default() });
        }
        let mut rgas = Vec::new();
        {
            let mut evm = Context::mainnet().with_block(block.clone()).with_cfg(cfg.clone()).with_db(&mut rdb).build_mainnet();
            for t in &txs {
                match evm.transact_commit(t.clone()) {
                    Ok(r) => rgas.push((r.is_success(), r.tx_gas_used())),
                    Err(e) => return Err(format!("block {bi}: revm rejected a tx: {e}")),
                }
            }
        }

        // Vx sequential and Vx parallel from the same prestate.
        let mut h0 = Host::default();
        for (a, bal, nonce, code) in prestate() {
            h0.insert(a, bal, nonce, code, HashMap::new());
        }
        let ebs: Vec<env::EnvBuf> = txs.iter().map(|t| env::build(SPEC, &cfg, &block, t)).collect();
        let mut hs = h0.clone();
        let mut sgas = Vec::new();
        for eb in &ebs {
            let res = hs.transact(&eb.env);
            let r = unsafe { &*res };
            if r.status == RES_INVALID_TX || r.status == RES_HOST_FAILURE {
                return Err(format!("block {bi}: vx-seq status {} reason {}", r.status, r.reason));
            }
            sgas.push((r.status == RES_SUCCESS, r.gas_used));
            hs.commit(r);
            unsafe { evmvx_result_free(res) };
        }

        let mut hp = h0.clone();
        hp.attach_db();
        let venvs: Vec<VxEnv> = ebs.iter().map(|eb| unsafe { std::ptr::read(&eb.env) }).collect();
        let b = unsafe { evmvx_block_new(hp.db as *mut c_void, venvs.as_ptr(), n as u64) };
        let bp = b as usize;
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(move || unsafe { evmvx_block_worker(bp as *mut c_void) });
            }
        });
        let committed = unsafe { evmvx_block_finish(b) };
        if committed as usize != n {
            return Err(format!("block {bi}: parallel committed {committed}/{n} (aborted={})", unsafe { evmvx_block_aborted(b) }));
        }
        let mut pgas = Vec::new();
        for i in 0..n {
            let r = unsafe { &*evmvx_block_result(b, i as u64) };
            pgas.push((r.status == RES_SUCCESS, r.gas_used));
            hp.commit_excluding(r, None);
        }
        let mut tail = Word::default();
        unsafe { evmvx_block_tail_fees(b, &mut tail) };
        hp.balance_incr_rust(coinbase(), tail.u256(), true);
        reexecs += unsafe { evmvx_block_reexecs(b) };
        cbtx += unsafe { evmvx_block_coinbase_txs(b) };

        // Every transaction: same status and gas on all three.
        for i in 0..n {
            if rgas[i] != sgas[i] || sgas[i] != pgas[i] {
                return Err(format!(
                    "block {bi} tx {i} ({:?}): revm {:?} vx-seq {:?} vx-par {:?}",
                    txs[i].kind, rgas[i], sgas[i], pgas[i]
                ));
            }
        }
        // Final state: revm == vx-seq == vx-par (Rust mirror) == vx-par StateDb.
        let (sr, ss, sp) = (snap_revm(&rdb), snap_host(&hs), snap_host(&hp));
        if let Some(d) = diff(&sr, &ss, "revm", "vx-seq") {
            return Err(format!("block {bi}: {d}"));
        }
        if let Some(d) = diff(&ss, &sp, "vx-seq", "vx-par") {
            return Err(format!("block {bi}: {d}"));
        }
        let mut ha = HostAccount { balance: Word::default(), nonce: 0, code_hash: Word::default(), code: std::ptr::null(), code_len: 0 };
        for (a, (bal, nonce, _, st)) in &ss {
            let aw: Word = (*a).into();
            let found = unsafe { evmvx_db_get_account(hp.db as *mut c_void, &aw, &mut ha) } == HOST_OK;
            if !found || ha.balance.u256() != *bal || ha.nonce != *nonce {
                return Err(format!("block {bi}: {a}: StateDb differs from vx-seq"));
            }
            for (k, v) in st {
                let kw: Word = (*k).into();
                let mut out = Word::default();
                unsafe { evmvx_db_get_slot(hp.db as *mut c_void, &aw, &kw, &mut out) };
                if out.u256() != *v {
                    return Err(format!("block {bi}: {a} slot {k:#x}: StateDb {} vs {v}", out.u256()));
                }
            }
        }
        unsafe { evmvx_block_free(b) };
    }
    Ok((total_txs, blocks, reexecs, cbtx))
}

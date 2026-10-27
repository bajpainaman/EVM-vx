//! The state database the Vx engine reads through callbacks, and the commit
//! rules revm's CacheState applies after a transaction.

use crate::ffi::*;
use revm::primitives::{keccak256, Address, Bytes, B256, KECCAK_EMPTY, U256};
use std::collections::HashMap;
use std::ffi::c_void;

#[derive(Clone, Debug, Default)]
pub struct Acct {
    pub balance: U256,
    pub nonce: u64,
    pub code_hash: B256,
    pub code: Bytes,
    pub storage: HashMap<U256, U256>,
}

#[derive(Clone, Default)]
pub struct Host {
    pub accounts: HashMap<Address, Acct>,
    /// Known block hashes by number (blockchain tests). Missing numbers fall
    /// back to revm EmptyDB's keccak256 of the decimal number.
    pub block_hashes: HashMap<u64, B256>,
    /// Address of an attached Vx StateDb (EVMVX_DB=1 mode), or 0. When set,
    /// transactions run with a null host, so a stray callback would crash.
    pub db: usize,
}

/// Frees an attached db. Hosts are only cloned before attach_db, so no two
/// hosts share one.
impl Drop for Host {
    fn drop(&mut self) {
        self.detach_db();
    }
}

/// EVMVX_DB=1 runs the engine on a Vx-side StateDb instead of host callbacks.
pub fn db_mode() -> bool {
    std::env::var_os("EVMVX_DB").is_some()
}

impl Host {
    /// Loads this state into a fresh Vx StateDb and attaches it.
    pub fn attach_db(&mut self) {
        unsafe {
            let db = evmvx_db_new();
            for (a, acc) in &self.accounts {
                let aw: Word = (*a).into();
                let bw: Word = acc.balance.into();
                evmvx_db_put_account(db, &aw, &bw, acc.nonce, acc.code.as_ptr(), acc.code.len() as u64);
                for (k, v) in &acc.storage {
                    let kw: Word = (*k).into();
                    let vw: Word = (*v).into();
                    evmvx_db_put_slot(db, &aw, &kw, &vw);
                }
            }
            for (n, h) in &self.block_hashes {
                let hw: Word = (*h).into();
                evmvx_db_put_block_hash(db, *n, &hw);
            }
            self.db = db as usize;
        }
    }

    pub fn detach_db(&mut self) {
        if self.db != 0 {
            unsafe { evmvx_db_free(self.db as *mut c_void) };
            self.db = 0;
        }
    }

    /// Runs one transaction through the db or the host callbacks.
    pub fn transact(&mut self, env: &VxEnv) -> *mut VxResult {
        unsafe {
            if self.db != 0 {
                evmvx_transact_db(self.db as *mut c_void, env)
            } else {
                evmvx_transact(self as *mut Host as *mut c_void, env)
            }
        }
    }

    pub fn system_call(&mut self, env: &VxEnv) -> *mut VxResult {
        unsafe {
            if self.db != 0 {
                evmvx_system_call_db(self.db as *mut c_void, env)
            } else {
                evmvx_system_call(self as *mut Host as *mut c_void, env)
            }
        }
    }

    pub fn set_block_hash(&mut self, n: u64, h: B256) {
        self.block_hashes.insert(n, h);
        if self.db != 0 {
            let hw: Word = h.into();
            unsafe { evmvx_db_put_block_hash(self.db as *mut c_void, n, &hw) };
        }
    }

    pub fn insert(&mut self, a: Address, balance: U256, nonce: u64, code: Bytes, storage: HashMap<U256, U256>) {
        let code_hash = if code.is_empty() { KECCAK_EMPTY } else { keccak256(&code) };
        self.accounts.insert(a, Acct { balance, nonce, code_hash, code, storage });
    }

    /// revm Journal::balance_incr followed by a commit: the account is loaded
    /// and touched, so after EIP-161 an empty account is removed.
    pub fn balance_incr(&mut self, a: Address, amount: U256, eip161: bool) {
        if self.db != 0 {
            let aw: Word = a.into();
            let mw: Word = amount.into();
            unsafe { evmvx_db_balance_incr(self.db as *mut c_void, &aw, &mw, eip161 as u64) };
        }
        match self.accounts.get_mut(&a) {
            Some(acc) => {
                acc.balance += amount;
                let empty = acc.balance.is_zero() && acc.nonce == 0 && acc.code_hash == KECCAK_EMPTY;
                if empty && eip161 {
                    self.accounts.remove(&a);
                }
            }
            None => {
                if !amount.is_zero() || !eip161 {
                    self.insert(a, amount, 0, Bytes::new(), HashMap::new());
                }
            }
        }
    }

    /// Applies one transaction's account changes, following revm
    /// CacheState::apply_account_state.
    pub fn commit(&mut self, r: &VxResult) {
        self.commit_excluding(r, None);
        // Last: db_apply replaces code buffers that r's pointers can alias.
        if self.db != 0 {
            unsafe { evmvx_db_apply(self.db as *mut c_void, r) };
        }
    }

    /// Commit, skipping one account (pexec applies the coinbase as a delta).
    pub fn commit_excluding(&mut self, r: &VxResult, skip: Option<Address>) {
        let accounts = unsafe { slice(r.accounts, r.account_count) };
        let slots = unsafe { slice(r.slots, r.slot_count) };
        for a in accounts {
            if a.flags & ACC_TOUCHED == 0 {
                continue;
            }
            let addr = a.address.address();
            if Some(addr) == skip {
                continue;
            }
            if a.flags & ACC_SELFDESTRUCTED != 0 {
                self.accounts.remove(&addr);
                continue;
            }
            let changed = slots[a.slot_start as usize..(a.slot_start + a.slot_count) as usize]
                .iter()
                .filter(|s| s.original != s.present)
                .map(|s| (s.key.u256(), s.present.u256()));
            let code = Bytes::copy_from_slice(unsafe { slice(a.code, a.code_len) });
            let is_empty = a.nonce == 0
                && a.balance.u256().is_zero()
                && (a.code_hash.b256() == KECCAK_EMPTY || a.code_hash.b256() == B256::ZERO);
            if a.flags & ACC_CREATED != 0 {
                let storage = changed.collect();
                self.accounts.insert(addr, Acct {
                    balance: a.balance.u256(),
                    nonce: a.nonce,
                    code_hash: a.code_hash.b256(),
                    code,
                    storage,
                });
                continue;
            }
            if is_empty {
                self.accounts.remove(&addr);
                continue;
            }
            let entry = self.accounts.entry(addr).or_default();
            entry.balance = a.balance.u256();
            entry.nonce = a.nonce;
            entry.code_hash = a.code_hash.b256();
            entry.code = code;
            entry.storage.extend(changed);
        }
    }
}

/// The result's logs as revm Log values.
pub fn logs_of(r: &VxResult) -> Vec<revm::primitives::Log> {
    unsafe { slice(r.logs, r.log_count) }
        .iter()
        .map(|l| {
            let topics = l.topics[..l.topic_count as usize].iter().map(|t| t.b256()).collect();
            let data = Bytes::copy_from_slice(unsafe { slice(l.data, l.data_len) });
            revm::primitives::Log { address: l.address.address(), data: revm::primitives::LogData::new_unchecked(topics, data) }
        })
        .collect()
}

pub unsafe fn slice<'a, T>(p: *const T, n: u64) -> &'a [T] {
    if n == 0 || p.is_null() {
        &[]
    } else {
        std::slice::from_raw_parts(p, n as usize)
    }
}

#[no_mangle]
pub extern "C" fn evmvx_host_account(host: *mut c_void, addr: *const Word, out: *mut HostAccount) -> u64 {
    let host = unsafe { &*(host as *const Host) };
    let a = unsafe { *addr }.address();
    match host.accounts.get(&a) {
        None => HOST_MISSING,
        Some(acc) => {
            unsafe {
                *out = HostAccount {
                    balance: acc.balance.into(),
                    nonce: acc.nonce,
                    code_hash: acc.code_hash.into(),
                    code: acc.code.as_ptr(),
                    code_len: acc.code.len() as u64,
                };
            }
            HOST_OK
        }
    }
}

#[no_mangle]
pub extern "C" fn evmvx_host_storage(host: *mut c_void, addr: *const Word, key: *const Word, out: *mut Word) -> u64 {
    let host = unsafe { &*(host as *const Host) };
    let a = unsafe { *addr }.address();
    let k = unsafe { *key }.u256();
    let v = host
        .accounts
        .get(&a)
        .and_then(|acc| acc.storage.get(&k).copied())
        .unwrap_or_default();
    unsafe { *out = v.into() };
    HOST_OK
}

/// State tests have no chain history; revm's EmptyDB answers with
/// keccak256 of the decimal block number.
#[no_mangle]
pub extern "C" fn evmvx_host_block_hash(host: *mut c_void, number: u64, out: *mut Word) -> u64 {
    let host = unsafe { &*(host as *const Host) };
    let h = host
        .block_hashes
        .get(&number)
        .copied()
        .unwrap_or_else(|| keccak256(number.to_string().as_bytes()));
    unsafe { *out = h.into() };
    HOST_OK
}

/// The crypto provider for the pairing-based precompiles: bn254 (0x06-0x08),
/// KZG point evaluation (0x0a) and BLS12-381 (0x0b-0x11). It runs revm's own
/// precompile for the active fork, the backends revm itself uses (arkworks,
/// c-kzg, blst). It reads only its input, so it needs no host state.
#[no_mangle]
pub extern "C" fn evmvx_crypto_precompile(
    id: u64,
    spec: u64,
    input: *const u8,
    len: u64,
    gas_limit: u64,
    out: *mut u8,
    out_cap: u64,
    res: *mut u64,
) -> u64 {
    use revm::precompile::{PrecompileSpecId, PrecompileStatus, Precompiles};
    use revm::primitives::hardfork::SpecId;
    const PC_OK: u64 = 0;
    const PC_OOG: u64 = 1;
    const PC_ERROR: u64 = 2;
    unsafe {
        *res = 0;
        *res.add(1) = 0;
    }
    let Ok(spec) = SpecId::try_from(spec as u8) else { return PC_ERROR };
    let set = Precompiles::new(PrecompileSpecId::from_spec_id(spec));
    let Some(pc) = set.get(&Address::with_last_byte(id as u8)) else { return PC_ERROR };
    let input = unsafe { slice(input, len) };
    match pc.execute(input, gas_limit, 0) {
        Ok(o) => match o.status {
            PrecompileStatus::Success if o.bytes.len() as u64 <= out_cap => {
                unsafe {
                    std::ptr::copy_nonoverlapping(o.bytes.as_ptr(), out, o.bytes.len());
                    *res = o.gas_used;
                    *res.add(1) = o.bytes.len() as u64;
                }
                PC_OK
            }
            PrecompileStatus::Halt(h) if h.is_oog() => PC_OOG,
            _ => PC_ERROR,
        },
        Err(_) => PC_ERROR,
    }
}

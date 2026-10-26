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
}

impl Host {
    pub fn insert(&mut self, a: Address, balance: U256, nonce: u64, code: Bytes, storage: HashMap<U256, U256>) {
        let code_hash = if code.is_empty() { KECCAK_EMPTY } else { keccak256(&code) };
        self.accounts.insert(a, Acct { balance, nonce, code_hash, code, storage });
    }

    /// Applies one transaction's account changes, following revm
    /// CacheState::apply_account_state.
    pub fn commit(&mut self, r: &VxResult) {
        let accounts = unsafe { slice(r.accounts, r.account_count) };
        let slots = unsafe { slice(r.slots, r.slot_count) };
        for a in accounts {
            if a.flags & ACC_TOUCHED == 0 {
                continue;
            }
            let addr = a.address.address();
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
pub extern "C" fn evmvx_host_block_hash(_host: *mut c_void, number: u64, out: *mut Word) -> u64 {
    let h = keccak256(number.to_string().as_bytes());
    unsafe { *out = h.into() };
    HOST_OK
}

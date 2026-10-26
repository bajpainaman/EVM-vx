//! C interface to the Vx engine. Mirrors src/evm/abi.vx field for field.

use revm::primitives::{Address, B256, U256};

/// Four little-endian u64 limbs, the layout of the Vx `U256` struct.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Word(pub [u64; 4]);

impl From<U256> for Word {
    fn from(v: U256) -> Self {
        Word(*v.as_limbs())
    }
}

impl From<Address> for Word {
    fn from(a: Address) -> Self {
        U256::from_be_slice(a.as_slice()).into()
    }
}

impl From<B256> for Word {
    fn from(b: B256) -> Self {
        U256::from_be_bytes(b.0).into()
    }
}

impl Word {
    pub fn u256(self) -> U256 {
        U256::from_limbs(self.0)
    }

    pub fn address(self) -> Address {
        let b: [u8; 32] = self.u256().to_be_bytes();
        Address::from_slice(&b[12..])
    }

    pub fn b256(self) -> B256 {
        B256::from(self.u256().to_be_bytes())
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VxAuth {
    pub chain_id: Word,
    pub address: Word,
    pub nonce: u64,
    pub y_parity: u64,
    pub r: Word,
    pub s: Word,
}

pub const CFG_TX_CHAIN_ID_CHECK: u64 = 128;

#[repr(C)]
pub struct VxEnv {
    pub spec: u64,
    pub chain_id: u64,
    pub cfg_flags: u64,
    pub tx_gas_limit_cap: u64,
    pub max_blobs_per_tx: u64,
    pub max_code_size: u64,
    pub max_initcode_size: u64,
    pub number: Word,
    pub coinbase: Word,
    pub timestamp: Word,
    pub block_gas_limit: u64,
    pub basefee: u64,
    pub difficulty: Word,
    pub prevrandao: Word,
    pub has_prevrandao: u64,
    pub excess_blob_gas: u64,
    pub blob_gasprice: Word,
    pub has_blob: u64,
    pub tx_type: u64,
    pub caller: Word,
    pub gas_limit: u64,
    pub gas_price: Word,
    pub priority_fee: Word,
    pub has_priority_fee: u64,
    pub is_create: u64,
    pub to: Word,
    pub value: Word,
    pub data: *const u8,
    pub data_len: u64,
    pub nonce: u64,
    pub has_tx_chain_id: u64,
    pub tx_chain_id: u64,
    pub al_count: u64,
    pub al_addrs: *const Word,
    pub al_nslots: *const u64,
    pub al_slots: *const Word,
    pub blob_count: u64,
    pub blob_hashes: *const Word,
    pub max_fee_per_blob_gas: Word,
    pub auth_count: u64,
    pub auths: *const VxAuth,
}

#[repr(C)]
pub struct HostAccount {
    pub balance: Word,
    pub nonce: u64,
    pub code_hash: Word,
    pub code: *const u8,
    pub code_len: u64,
}

pub const HOST_OK: u64 = 0;
pub const HOST_MISSING: u64 = 1;
pub const HOST_ERROR: u64 = 2;

#[repr(C)]
pub struct OutAccount {
    pub address: Word,
    pub balance: Word,
    pub nonce: u64,
    pub code_hash: Word,
    pub code: *const u8,
    pub code_len: u64,
    pub flags: u64,
    pub slot_start: u64,
    pub slot_count: u64,
}

#[repr(C)]
pub struct OutSlot {
    pub key: Word,
    pub original: Word,
    pub present: Word,
}

#[repr(C)]
pub struct OutLog {
    pub address: Word,
    pub topic_count: u64,
    pub topics: [Word; 4],
    pub data: *const u8,
    pub data_len: u64,
}

pub const RES_SUCCESS: u64 = 0;
pub const RES_REVERT: u64 = 1;
pub const RES_HALT: u64 = 2;
pub const RES_INVALID_TX: u64 = 3;
pub const RES_HOST_FAILURE: u64 = 4;

#[repr(C)]
pub struct VxResult {
    pub status: u64,
    pub reason: u64,
    pub gas_used: u64,
    pub gas_refunded: u64,
    pub output: *const u8,
    pub output_len: u64,
    pub created: u64,
    pub created_address: Word,
    pub logs: *const OutLog,
    pub log_count: u64,
    pub accounts: *const OutAccount,
    pub account_count: u64,
    pub slots: *const OutSlot,
    pub slot_count: u64,
}

// Account flag bits (src/evm/state.vx).
pub const ACC_TOUCHED: u64 = 2;
pub const ACC_CREATED: u64 = 4;
pub const ACC_SELFDESTRUCTED: u64 = 8;
pub const ACC_NOT_EXISTING: u64 = 16;

extern "C" {
    /// Runs one transaction. `host` is passed back to the host callbacks.
    pub fn evmvx_transact(host: *mut core::ffi::c_void, env: *const VxEnv) -> *mut VxResult;
    /// Runs a system call (no validation, fees, nonce bump or refunds).
    pub fn evmvx_system_call(host: *mut core::ffi::c_void, env: *const VxEnv) -> *mut VxResult;
    pub fn evmvx_result_free(r: *mut VxResult);
    /// Struct sizes the Vx side sees, for a layout check at startup.
    pub fn evmvx_abi_sizes(out: *mut u64);
}

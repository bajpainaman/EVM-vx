//! Builds the C environment (VxEnv) for one transaction from revm's own
//! CfgEnv, BlockEnv and TxEnv, so both engines read the fixtures identically.

use crate::ffi::*;
use revm::{
    context::{cfg::CfgEnv, either::Either, BlockEnv, TxEnv},
    context_interface::Cfg,
    primitives::{hardfork::SpecId, U256},
};

/// A VxEnv plus the buffers its pointers refer to. Keep it alive while the
/// engine runs; the transaction data pointer refers to the TxEnv.
pub struct EnvBuf {
    pub env: VxEnv,
    _al_addrs: Vec<Word>,
    _al_nslots: Vec<u64>,
    _al_slots: Vec<Word>,
    _blobs: Vec<Word>,
    _auths: Vec<VxAuth>,
}

fn ptr<T>(v: &[T]) -> *const T {
    if v.is_empty() { std::ptr::null() } else { v.as_ptr() }
}

pub fn build(spec: SpecId, cfg: &CfgEnv, block: &BlockEnv, tx: &TxEnv) -> EnvBuf {
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
        al_addrs: ptr(&al_addrs),
        al_nslots: ptr(&al_nslots),
        al_slots: ptr(&al_slots),
        blob_count: blobs.len() as u64,
        blob_hashes: ptr(&blobs),
        max_fee_per_blob_gas: U256::from(tx.max_fee_per_blob_gas).into(),
        auth_count: auths.len() as u64,
        auths: ptr(&auths),
    };
    EnvBuf { env, _al_addrs: al_addrs, _al_nslots: al_nslots, _al_slots: al_slots, _blobs: blobs, _auths: auths }
}

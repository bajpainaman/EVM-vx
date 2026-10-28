//! Owned transaction observations, independent of native result lifetimes.

use crate::ffi::*;
use crate::host;
use revm::context::result::ExecutionResult;
use revm::primitives::{Address, Bytes, Log};

#[derive(Debug, PartialEq, Eq)]
pub struct Observation {
    status: u64,
    gas_used: u64,
    gas_refunded: u64,
    output: Bytes,
    created: Option<Address>,
    logs: Vec<Log>,
}

impl Observation {
    pub fn receipt(&self, tx_type: u8, cumulative: &mut u64) -> alloy_consensus::ReceiptEnvelope {
        use alloy_consensus::{Receipt, ReceiptEnvelope, TxType};
        *cumulative += self.gas_used;
        ReceiptEnvelope::from_typed(
            TxType::try_from(tx_type).expect("validated transaction type"),
            Receipt {
                status: (self.status == RES_SUCCESS).into(),
                cumulative_gas_used: *cumulative,
                logs: self.logs.clone(),
            },
        )
    }

    pub fn vx(result: &VxResult) -> Self {
        let output = if result.output_len == 0 {
            Bytes::new()
        } else {
            // Results own these buffers until evmvx_result_free/block_free.
            Bytes::copy_from_slice(unsafe {
                std::slice::from_raw_parts(result.output, result.output_len as usize)
            })
        };
        Self {
            status: result.status,
            gas_used: result.gas_used,
            gas_refunded: result.gas_refunded,
            output,
            created: (result.created != 0).then(|| result.created_address.address()),
            logs: host::logs_of(result),
        }
    }

    pub fn revm<H>(result: &ExecutionResult<H>) -> Self {
        let status = match result {
            ExecutionResult::Success { .. } => RES_SUCCESS,
            ExecutionResult::Revert { .. } => RES_REVERT,
            ExecutionResult::Halt { .. } => RES_HALT,
        };
        Self {
            status,
            gas_used: result.tx_gas_used(),
            gas_refunded: result.gas().final_refunded(),
            output: result.output().cloned().unwrap_or_default(),
            created: result.created_address(),
            logs: result.logs().to_vec(),
        }
    }
}

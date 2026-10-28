//! Deterministic bytecode workloads; all engines receive identical inputs.

use revm::primitives::{Address, Bytes};
use std::str::FromStr;

const STORAGE_ROUNDS: usize = 64;
const ARITHMETIC_ROUNDS: usize = 2048;
const HASH_ROUNDS: usize = 64;
const CALL_ROUNDS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Workload {
    Storage,
    Arithmetic,
    Keccak,
    Transfer,
    Calls,
    Revert,
}

impl FromStr for Workload {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "storage" => Ok(Self::Storage),
            "arithmetic" => Ok(Self::Arithmetic),
            "keccak" => Ok(Self::Keccak),
            "transfer" => Ok(Self::Transfer),
            "calls" => Ok(Self::Calls),
            "revert" => Ok(Self::Revert),
            _ => Err(format!("unknown workload {value:?}; use storage, arithmetic, keccak, transfer, calls, or revert")),
        }
    }
}

pub fn addr(tag: u8, index: u64) -> Address {
    let mut bytes = [0u8; 20];
    bytes[0] = tag;
    bytes[12..].copy_from_slice(&index.to_be_bytes());
    Address::from(bytes)
}

pub fn helper_address() -> Address {
    addr(0xdd, 0)
}

pub fn helper_code() -> Bytes {
    // Return a nonzero word so output comparison exercises memory copying.
    vec![0x60, 0x2a, 0x5f, 0x52, 0x60, 0x20, 0x5f, 0xf3].into()
}

impl Workload {
    pub fn name(self) -> &'static str {
        match self {
            Self::Storage => "storage",
            Self::Arithmetic => "arithmetic",
            Self::Keccak => "keccak",
            Self::Transfer => "transfer",
            Self::Calls => "calls",
            Self::Revert => "revert",
        }
    }

    pub fn code(self) -> Bytes {
        let mut code = Vec::new();
        match self {
            Self::Storage => {
                for _ in 0..STORAGE_ROUNDS {
                    code.extend_from_slice(&[0x60, 0, 0x54, 0x60, 1, 1, 0x60, 0, 0x55]);
                }
                code.push(0);
            }
            Self::Arithmetic => {
                code.extend_from_slice(&[0x60, 1]);
                for _ in 0..ARITHMETIC_ROUNDS {
                    code.extend_from_slice(&[0x60, 3, 1, 0x60, 7, 2, 0x60, 1, 0x18]);
                }
                code.extend_from_slice(&[0x5f, 0x52, 0x60, 0x20, 0x5f, 0xf3]);
            }
            Self::Keccak => {
                code.extend_from_slice(&[0x60, 1, 0x60, 0xe0, 0x52]);
                for _ in 0..HASH_ROUNDS {
                    code.extend_from_slice(&[0x61, 1, 0, 0x5f, 0x20, 0x50]);
                }
                code.push(0);
            }
            Self::Transfer => {}
            Self::Calls => {
                for _ in 0..CALL_ROUNDS {
                    code.extend_from_slice(&[0x60, 0x20, 0x5f, 0x5f, 0x5f, 0x5f, 0x73]);
                    code.extend_from_slice(helper_address().as_slice());
                    code.extend_from_slice(&[0x61, 0xff, 0xff, 0xf1, 0x50]);
                }
                code.extend_from_slice(&[0x60, 0x20, 0x5f, 0xf3]);
            }
            Self::Revert => {
                code.extend_from_slice(&[
                    0x60, 1, 0x5f, 0x55, 0x60, 0x2a, 0x5f, 0x52, 0x60, 0x20, 0x5f, 0xfd,
                ]);
            }
        }
        code.into()
    }
}

use alloy_primitives::{Address, Bytes, TxHash, U256};

#[derive(Debug, Clone)]
pub struct ClassifiedTx {
    pub nonce: u64,
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: Option<u128>,
    pub gas_limit: u64,
    pub hash: TxHash,
    pub sender: Address,
    pub to: Option<Address>,
    pub input: Bytes,
    pub value: U256,
    pub cost: U256,
    pub kind: TxCategory,
}

#[derive(Debug, Clone)]
pub enum TxCategory {
    UniV2Swap { path: Vec<Address>, amount_in: U256, min_out: U256 },
    Erc20Transfer { token: Address, to: Address, amount: U256 },
    Other,
}

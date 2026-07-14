use crate::types::{ClassifiedTx, TxKind};
use alloy_primitives::{Address, Bytes};
use reth_transaction_pool::{test_utils:: MockTransaction, NewTransactionEvent};
use alloy_sol_types::{sol, SolCall};

pub fn classify(event: &NewTransactionEvent<MockTransaction>) -> () {
    let tx: &std::sync::Arc<reth_transaction_pool::ValidPoolTransaction<MockTransaction>> = &event.transaction;

    let nonce: u64 = tx.nonce();

    let max_fee_per_gas: u128 = tx.max_fee_per_gas();
    let max_priority_fee_per_gas: Option<u128> = tx.max_priority_fee_per_gas();
    let gas_limit: u64 = tx.gas_limit();
    
    let hash: alloy_primitives::FixedBytes<32> = *tx.hash();
    let sender: Address = tx.sender();
    let to: Option<Address> = tx.to();

    let input: Bytes = tx.transaction.get_input().clone();
    let value: alloy_primitives::Uint<256, 4> = *tx.transaction.get_value();

    let cost: alloy_primitives::Uint<256, 4> = *tx.cost(); 

    let kind: TxKind = decode_input(&input.clone(), to.clone());

    let c: ClassifiedTx = ClassifiedTx {
        nonce,
        max_fee_per_gas,
        max_priority_fee_per_gas,
        gas_limit,
        hash,
        sender,
        to,
        input,
        value,
        cost,
        kind
    };
    let debug_str: String = format!("{:?}", c);
    println!("{}", debug_str);
}

sol! {
    function transfer(address to, uint256 amount) returns (bool);
    function swapExactTokensForTokens(
        uint256 amountIn,
        uint256 amountOutMin,
        address[] path,
        address to,
        uint256 deadline
    ) returns (uint256[] amounts);
}

pub fn decode_input(input: &Bytes, tx_to: Option<Address>) -> TxKind {
    if input.len() < 4 {
        return TxKind::Other;
    }

    let selector: [u8; 4] = input[0..4].try_into().unwrap();

    match selector {
        transferCall::SELECTOR => {
            let Ok(decoded) = transferCall::abi_decode(input) else {
                return TxKind::Other;
            };
            let Some(token) = tx_to else {
                return TxKind::Other;
            };
            TxKind::Erc20Transfer { token, to: decoded.to, amount: decoded.amount }
        }
        swapExactTokensForTokensCall::SELECTOR => {
            let Ok(decoded) = swapExactTokensForTokensCall::abi_decode(input) else {
                return TxKind::Other
            };
            TxKind::UniV2Swap { path: decoded.path, amount_in: decoded.amountIn, min_out: decoded.amountOutMin }
        }
        _ => TxKind::Other,
    }
}
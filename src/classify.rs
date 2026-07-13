use crate::types::{ClassifiedTx, TxKind};
use alloy_primitives::{Address, bytes::Bytes};
use alloy_consensus::Transaction;
use reth_transaction_pool::{test_utils:: MockTransaction, NewTransactionEvent};

pub fn classify(event: &NewTransactionEvent<MockTransaction>) -> () {
    let tx = &event.transaction;

    let nonce = tx.nonce();

    // let max_fee_per_gas = tx.max_fee_per_gas();
    // let max_priority_fee_per_gas = tx.max_priority_fee_per_gas();
    let gas_limit = tx.gas_limit();
    
    let hash = *tx.hash();
    let sender = tx.sender();
    let to = tx.to();
    let input = tx.transaction.get_input().clone().into();
    let value = *tx.transaction.get_value();

    let cost = *tx.cost(); 

    let c = ClassifiedTx {
        nonce,
        // max_fee_per_gas,
        // max_priority_fee_per_gas,
        gas_limit,
        hash,
        sender,
        to,
        input,
        value,
        cost
    };
    let debug_str: String = format!("{:?}", c);
    println!("{}", debug_str);
}
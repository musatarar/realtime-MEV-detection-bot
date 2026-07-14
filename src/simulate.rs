use alloy::{eips::BlockId, providers::{Provider, ProviderBuilder}, network::AnyNetwork, sol, sol_types::SolEvent};
use alloy::primitives::{Address, Bytes, U256, TxKind};
use alloy_evm::{eth::EthEvmContext, EthEvm, Evm};
use foundry_fork_db::{cache::BlockchainDbMeta, BlockchainDb, SharedBackend};
use revm::{
    context::{BlockEnv, Context, Evm as RevmEvm, Host, TxEnv}, context_interface::block::BlobExcessGasAndPrice, database::WrapDatabaseRef, handler::{EthPrecompiles, instructions::EthInstructions}, inspector::NoOpInspector, primitives::hardfork::SpecId,
};

sol! {event Transfer(address indexed from, address indexed to, uint256 value);}

pub async fn setup_fork(rpc_url: &str) -> anyhow::Result<(SharedBackend, alloy::network::AnyRpcBlock)> {
    let provider = ProviderBuilder::new().network::<AnyNetwork>().connect_http(rpc_url.parse()?);
    let block = provider.get_block(BlockId::latest()).await?.unwrap();
    let meta = BlockchainDbMeta::new(BlockEnv::default(), rpc_url.to_string());
    let db = BlockchainDb::new(meta, None);
    let shared = SharedBackend::spawn_backend(std::sync::Arc::new(provider), db, None).await;
    Ok((shared, block))
}

pub fn simulate_transfers(
    backend: SharedBackend,
    parent: &alloy::network::AnyRpcBlock,
    from: Address, to: Address, value: U256, input: Bytes,
    gas_limit: u64, gas_price: u128, nonce: u64,
) -> anyhow::Result<Vec<(Address, Address, U256)>> {
    use alloy_consensus::BlockHeader;

    let block_env = BlockEnv {
        number: U256::from(parent.header.number() + 1),  // Simulate the NEXT block
        beneficiary: parent.header.beneficiary(),
        timestamp: U256::from(parent.header.timestamp + 12),
        gas_limit: parent.header.gas_limit(),
        basefee: parent.header.base_fee_per_gas().unwrap_or(0),
        prevrandao: parent.header.mix_hash(),
        slot_num: 0, // Punted for now; TODO: Solve if affects execution
        difficulty: parent.header.difficulty(),
        blob_excess_gas_and_price: Some(BlobExcessGasAndPrice::new_with_spec(
            parent.header.excess_blob_gas().unwrap_or_default(), SpecId::PRAGUE
        )),
    };

    let ctx = EthEvmContext::new(WrapDatabaseRef(backend), SpecId::PRAGUE).with_block(block_env);
    let evm = RevmEvm::new(
        ctx, EthInstructions::new_mainnet_with_spec(SpecId::PRAGUE), EthPrecompiles::new(SpecId::PRAGUE)
    ).with_inspector(NoOpInspector);
    let mut evm = EthEvm::new(evm, false);

    let tx = TxEnv {
        caller: from,
        kind: TxKind::Call(to),
        value,
        data: input,
        gas_limit,
        gas_price,
        nonce,
        ..Default::default()
    };

    let res = evm.transact(tx)?;

    let mut transfers = Vec::new();
    for log in res.result.logs() {
        if log.topics().first() == Some(&Transfer::SIGNATURE_HASH) {
            if let Ok(ev) = Transfer::decode_log_data(&log.data) {
                transfers.push((ev.from, ev.to, ev.value));
                println!("{}, {}, {}", ev.from, ev.to, ev.value);
            }
        }
    }
    Ok(transfers)
}
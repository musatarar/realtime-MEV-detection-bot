use reth_network::{
    config::rng_secret_key, EthNetworkPrimitives, NetworkConfig, NetworkManager, PeersInfo
};
use reth_network_peers::mainnet_nodes;
use reth_storage_api::noop::NoopProvider;
use reth_tasks::Runtime;
use reth_transaction_pool::{
    test_utils::{testing_pool, MockTransaction}, 
    NewTransactionEvent, TransactionPool, 
};
use std::time::Duration;
use tokio::sync::mpsc;

pub async fn run(tx_out: mpsc::Sender<NewTransactionEvent<MockTransaction>>) -> anyhow::Result<()> {
    let client = NoopProvider::default();
    let pool = testing_pool();

    let local_key = rng_secret_key();
    let config = NetworkConfig::<_, EthNetworkPrimitives>::builder(local_key, Runtime::test())
        .boot_nodes(mainnet_nodes())
        .build(client.clone());
    let tx_config = config.transactions_manager_config.clone();

    let (handle, network, txs_manager, request_manager) = 
        NetworkManager::builder(config)
            .await?
            .transactions(pool.clone(), tx_config)
            .request_handler(client)
            .split_with_handle();

    tokio::task::spawn(network);
    tokio::task::spawn(txs_manager);
    tokio::task::spawn(request_manager);

    let monitor = handle.clone();
    tokio::task::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            println!("connected peers: {}", monitor.num_connected_peers());
        }
    });

    let mut events = pool.new_transactions_listener();
    while let Some(event) = events.recv().await {
        if tx_out.send(event).await.is_err() {
            break;
        }
    }
    Ok(())
}
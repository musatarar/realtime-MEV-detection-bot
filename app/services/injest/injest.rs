use futures::StreamExt;
use reth_network::{
    config::rng_secret_key, EthNetworkPrimitives, NetworkConfig, NetworkManager, NetworkEventListenerProvider
};
use reth_network_peers::mainnet_nodes;
use reth_storage_api::noop::NoopProvider;
use reth_tasks::Runtime;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = NoopProvider::default();

    let local_key = rng_secret_key();

    let config = NetworkConfig::<_, EthNetworkPrimitives>::builder(local_key, Runtime::test())
        .boot_nodes(mainnet_nodes())
        .build(client);

    let network = NetworkManager::new(config).await?;
    let handle = network.handle().clone();

    tokio::task::spawn(network);

    let mut events = handle.event_listener();
    while let Some(event) = events.next().await {
        println!("{event:?}");
    }
    Ok(())
}
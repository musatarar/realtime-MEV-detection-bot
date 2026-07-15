use reth_network::{config::rng_secret_key, EthNetworkPrimitives, NetworkConfig, NetworkManager, Peers, PeersConfig, PeersInfo};
use reth_network_peers::{mainnet_nodes, NodeRecord};
use reth_storage_api::noop::NoopProvider;
use reth_tasks::Runtime;
use reth_transaction_pool::{
    test_utils::{testing_pool, MockTransaction}, 
    NewTransactionEvent, TransactionPool, 
};
use std::{path::PathBuf, str::FromStr, time::Duration};
use tokio::sync::mpsc;

const PEERS_FILE: &str = "known-peers.json";

async fn save_peers(handle: &reth_network::NetworkHandle) -> anyhow::Result<()> {
    let peers = handle.get_all_peers().await?;
    // Convert PeerInfo -> NodeRecord via its enode string; that's the legacy
    // format with_basic_nodes_from_file accepts as a fallback.
    let records: Vec<NodeRecord> = peers
        .iter()
        .filter_map(|p| NodeRecord::from_str(&p.enode).ok())   // <- CONFIRM field name
        .collect();
    std::fs::write(PEERS_FILE, serde_json::to_string_pretty(&records)?)?;
    println!("saved {} peers", records.len());
    Ok(())
}

pub async fn run(tx_out: mpsc::Sender<NewTransactionEvent<MockTransaction>>) -> anyhow::Result<()> {
    let client = NoopProvider::default();
    let pool = testing_pool();
    let local_key = rng_secret_key();

    let peers_config = PeersConfig::default()
        .with_basic_nodes_from_file(Some(PathBuf::from(PEERS_FILE)))?
        .with_max_outbound(200)
        .with_max_concurrent_dials(64);

    let config = NetworkConfig::<_, EthNetworkPrimitives>::builder(local_key, Runtime::test())
        .boot_nodes(mainnet_nodes())
        .peer_config(peers_config)         
        .build(client.clone());
    let tx_config = config.transactions_manager_config.clone();

    let (handle, network, txs_manager, request_manager) =
        NetworkManager::builder(config).await?
            .transactions(pool.clone(), tx_config)
            .request_handler(client)
            .split_with_handle();

    tokio::task::spawn(network);
    tokio::task::spawn(txs_manager);
    tokio::task::spawn(request_manager);

    // periodic save — don't rely on catching ctrl-c cleanly every time
    let saver = handle.clone();
    tokio::task::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if let Err(e) = save_peers(&saver).await {
                eprintln!("peer save failed: {e}");
            }
        }
    });

    let monitor = handle.clone();
    tokio::task::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            println!("connected peers: {}", monitor.num_connected_peers());
        }
    });

    let mut events = pool.new_transactions_listener();
    loop {
        tokio::select! {
            maybe_event = events.recv() => match maybe_event {
                Some(event) => if tx_out.send(event).await.is_err() { break },
                None => break,
            },
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    // graceful exit: save, then tell the network to disconnect cleanly
    let _ = save_peers(&handle).await;
    let _ = handle.shutdown().await;
    Ok(())
}
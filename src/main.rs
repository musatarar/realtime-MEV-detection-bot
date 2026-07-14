mod ingest; mod classify; mod types; mod simulate;

use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

use std::env;

use crate::types::TxCategory;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let rpc_url = env::var("RPC_URL").unwrap();

    let (backend, block) = simulate::setup_fork(&rpc_url).await?;
    
    tracing_subscriber::fmt().with_env_filter(
        EnvFilter::from_default_env()
    )
    .init();

    let (raw_tx, mut raw_rx) = mpsc::channel(4096);

    tokio::spawn(ingest::run(raw_tx));

    while let Some(event) = raw_rx.recv().await {
        let classified_tx = classify::classify(&event)?;

        if matches!(
            classified_tx.kind,
            TxCategory::Erc20Transfer { .. } | TxCategory::UniV2Swap { .. }
        ) {
            println!("Processing tx with hash '{}'", classified_tx.hash);
            let simulated_results = simulate::simulate_transfers(
                backend.clone(),
                &block, 
                classified_tx.sender, 
                classified_tx.to.unwrap(), 
                classified_tx.value, 
                classified_tx.input, 
                classified_tx.gas_limit, 
                classified_tx.max_fee_per_gas, 
                classified_tx.nonce,
            );
        }
    }
    Ok(())
}
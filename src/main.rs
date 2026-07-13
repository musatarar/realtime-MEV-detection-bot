mod ingest; mod classify; mod types;

use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(
        EnvFilter::from_default_env()
    )
    .init();

    let (raw_tx, mut raw_rx) = mpsc::channel(4096);

    tokio::spawn(ingest::run(raw_tx));

    while let Some(event) = raw_rx.recv().await {
        classify::classify(&event)
    }
    Ok(())
}
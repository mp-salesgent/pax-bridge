//! Headless PAX bridge HTTP/WS server (no Tauri UI). Used for CI/API smoke tests.

use pax_bridge_desktop_lib::bridge;
use tokio::signal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();

    let shutdown = async {
        let _ = signal::ctrl_c().await;
        tracing::info!("shutdown signal received");
    };

    bridge::start_bridge(shutdown).await
}

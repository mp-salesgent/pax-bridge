//! Bridge server bootstrap — ports the bottom section of `bridge/index.js`:
//! load env/config, load the DB, wire CORS + routes, bind and listen.

use crate::bridge::db::Db;
use crate::bridge::ws::Broadcaster;
use crate::bridge::{config, http, AppState};
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Start the PAX bridge HTTP/WS server. Binds `0.0.0.0:<PORT>` (default 5000)
/// and stores its JSON database under `PAX_HOME`/`PAX_DATA_DIR` (see `config`).
/// Runs until `shutdown` resolves (graceful shutdown).
pub async fn start_bridge(shutdown: impl Future<Output = ()> + Send + 'static) -> anyhow::Result<()> {
    let setup = config::load_env();
    tracing::info!("[setup] runtime dir: {}", setup.dir.display());

    let data_dir = config::resolve_data_dir();
    let db = Db::load(&data_dir);

    let state = AppState { db: Arc::new(Mutex::new(db)), ws: Arc::new(Broadcaster::new()) };

    let app = http::router(state);

    let port = config::port();
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    tracing::info!("Server listening on http://localhost:{}", port);
    tracing::info!("WebSocket lifecycle events on ws://localhost:{}/ws", port);
    tracing::info!("PAX mode: live terminal");

    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown)
        .await?;

    Ok(())
}

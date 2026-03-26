//! Covalence API server.
//!
//! Thin routing layer over the engine. No business logic.

mod error;
mod handlers;
mod middleware;
mod openapi;
mod routes;
mod state;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // Try the layered figment config system first. If covalence.conf
    // exists, it takes precedence. Otherwise, fall back to the
    // legacy env-var-only path for backward compatibility.
    let config = if std::path::Path::new("covalence.conf").exists() {
        tracing::info!("loading config via figment (covalence.conf found)");
        covalence_core::config::Config::from_figment(None, None)
            .map_err(|e| anyhow::anyhow!("{e}"))?
    } else {
        tracing::info!("loading config via env vars (no covalence.conf)");
        covalence_core::config::Config::from_env().map_err(|e| anyhow::anyhow!("{e}"))?
    };

    let bind_addr = config.bind_addr.clone();
    let app_state = state::AppState::new(config).await?;
    let app = routes::router(app_state);

    tracing::info!("Covalence API listening on {bind_addr}");
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

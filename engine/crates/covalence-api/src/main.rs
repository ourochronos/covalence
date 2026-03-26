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

    let config = covalence_core::config::Config::from_figment(None, None)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let bind_addr = config.bind_addr.clone();
    let app_state = state::AppState::new(config).await?;
    let app = routes::router(app_state);

    tracing::info!("Covalence API listening on {bind_addr}");
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

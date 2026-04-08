//! Covalence API server.
//!
//! Thin routing layer over the engine. No business logic.

mod error;
mod handlers;
mod middleware;
mod openapi;
mod routes;
mod state;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use covalence_core::graph::engine::GraphEngine;
use covalence_core::storage::postgres::PgRepo;
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
    let reload_interval = config.graph_reload_interval_secs;
    let app_state = state::AppState::new(config).await?;

    // Start the background graph reload task. This is the
    // ADR-0004 polling fallback that keeps the in-process
    // sidecar in sync with PG when worker processes commit
    // edges/nodes the engine never sees (e.g. background edge
    // synthesis). Disabled when interval is 0.
    if reload_interval > 0 {
        spawn_graph_reload_task(
            Arc::clone(&app_state.graph_engine),
            Arc::clone(&app_state.repo),
            Duration::from_secs(reload_interval),
        );
    } else {
        tracing::info!("background graph reload disabled (interval = 0)");
    }

    let app = routes::router(app_state);

    tracing::info!("Covalence API listening on {bind_addr}");
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Spawn a tokio task that periodically rebuilds the in-process graph
/// sidecar from PostgreSQL.
///
/// This is the polling fallback described in ADR-0004 — the outbox /
/// LISTEN-NOTIFY primary path is not yet implemented, so this task is
/// the only mechanism keeping the engine's sidecar in sync with writes
/// committed by other processes (workers, migrations, manual SQL).
///
/// The first reload runs after one tick, not at startup, because
/// `AppState::new` already loaded the sidecar synchronously.
fn spawn_graph_reload_task(graph: Arc<dyn GraphEngine>, repo: Arc<PgRepo>, interval: Duration) {
    tracing::info!(
        interval_secs = interval.as_secs(),
        "starting background graph reload task"
    );
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // Skip the first immediate tick — startup already loaded.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            match graph.reload(repo.pool()).await {
                Ok(result) => {
                    tracing::debug!(
                        nodes = result.node_count,
                        edges = result.edge_count,
                        "background graph reload complete"
                    );
                }
                Err(e) => {
                    tracing::warn!(error = %e, "background graph reload failed");
                }
            }
        }
    });
}

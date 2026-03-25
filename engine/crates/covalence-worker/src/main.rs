//! Covalence queue worker binary.
//!
//! Processes ingestion pipeline jobs from the persistent retry queue.
//! Runs as a separate process from the API server, with its own
//! connection pool and concurrency configuration.
//!
//! Job types:
//!   - ProcessSource: chunk + embed + coref + fan-out
//!   - ExtractChunk: per-chunk entity extraction
//!   - SummarizeEntity: semantic code summaries
//!   - ComposeSourceSummary: fan-in file summary
//!   - SynthesizeEdges: co-occurrence edge creation
//!   - ReprocessSource: re-ingest from stored content
//!   - EmbedBatch: batch vector embedding

use std::sync::Arc;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

use covalence_core::config::Config;
use covalence_core::factory::ServiceFactory;
use covalence_core::storage::postgres::PgRepo;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn")),
        )
        .init();

    dotenvy::dotenv().ok();

    let config = Config::from_env()?;

    tracing::info!(
        pool_size = config.queue.reprocess_concurrency,
        "starting covalence-worker"
    );

    // Worker gets its own connection pool, sized for concurrency.
    let pool_size = (config.queue.reprocess_concurrency as u32).max(10);
    let pool = PgPoolOptions::new()
        .max_connections(pool_size + 5) // headroom for queue management
        .connect(&config.database_url)
        .await?;

    let repo = Arc::new(PgRepo::from_pool(pool));

    // Build all shared services via the factory.
    let factory = ServiceFactory::new(&config, Arc::clone(&repo))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Wire the queue service with the services it dispatches to.
    factory
        .queue_service
        .set_source_service(Arc::clone(&factory.source_service));
    factory
        .queue_service
        .set_admin_service(Arc::clone(&factory.admin_service));

    tracing::info!(
        concurrency = config.queue.reprocess_concurrency,
        job_timeout_secs = config.queue.job_timeout_secs,
        "worker ready, starting queue polling"
    );

    // Run the queue worker — this blocks forever.
    factory.queue_service.run_worker().await;

    Ok(())
}

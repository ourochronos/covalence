mod api;
mod embeddings;
mod errors;
mod graph;
mod models;
mod search;
mod services;
mod worker;

use api::AppState;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "covalence_engine=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    dotenvy::dotenv().ok();

    // Database connection
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://covalence:covalence@localhost:5434/covalence".into());

    let pool = PgPoolOptions::new()
        .max_connections(20)
        .connect(&database_url)
        .await?;

    tracing::info!("connected to database");

    // ── Auto-apply pending SQLx migrations (tracking#106) ─────────────────────
    // Migrations 001-017 were applied manually before this feature was added.
    // Run `scripts/seed-sqlx-migrations.sh` once on an existing instance to
    // register those historical migrations in _sqlx_migrations so SQLx won't
    // attempt to re-run them.  New migrations (018+) go in engine/migrations/
    // using the SQLx naming convention: {version}_{description}.sql
    sqlx::migrate!("./migrations").run(&pool).await?;
    tracing::info!("database migrations up to date");

    // Spawn slow-path background worker
    let worker_pool = pool.clone();
    tokio::spawn(async move {
        worker::run(worker_pool).await;
    });
    tracing::info!("slow-path worker spawned");

    // Build app
    let llm: std::sync::Arc<dyn worker::llm::LlmClient> = match std::env::var("OPENAI_API_KEY") {
        Ok(key) if !key.is_empty() => {
            let mut client = worker::openai::OpenAiClient::new(key);
            if let Ok(url) = std::env::var("OPENAI_BASE_URL") {
                client = client.with_base_url(url);
            }
            if let Ok(model) = std::env::var("COVALENCE_EMBED_MODEL") {
                client = client.with_embed_model(model);
            }
            tracing::info!("search using OpenAI embeddings for query auto-embed");
            std::sync::Arc::new(client)
        }
        _ => {
            tracing::warn!("OPENAI_API_KEY not set — search will skip vector dimension");
            std::sync::Arc::new(worker::llm::StubLlmClient)
        }
    };
    // Load in-memory graph from all current edges
    let all_edge_rows =
        sqlx::query("SELECT source_node_id, target_node_id, edge_type FROM covalence.edges")
            .fetch_all(&pool)
            .await
            .unwrap_or_default();

    let mut startup_graph = crate::graph::CovalenceGraph::new();
    for row in &all_edge_rows {
        use sqlx::Row as _;
        let source: uuid::Uuid = row.try_get("source_node_id").unwrap_or_default();
        let target: uuid::Uuid = row.try_get("target_node_id").unwrap_or_default();
        let edge_type: String = row.try_get("edge_type").unwrap_or_default();
        startup_graph.add_edge(source, target, edge_type);
    }
    tracing::info!(
        node_count = startup_graph.node_count(),
        edge_count = startup_graph.edge_count(),
        "in-memory graph loaded"
    );
    let shared_graph: crate::graph::SharedGraph =
        std::sync::Arc::new(tokio::sync::RwLock::new(startup_graph));

    // Optional API key — when set, all requests (except /health) must present it
    // via `Authorization: Bearer <key>` or `X-Api-Key: <key>`.
    let api_key = std::env::var("COVALENCE_API_KEY")
        .ok()
        .filter(|k| !k.is_empty());
    if api_key.is_some() {
        tracing::info!("API key authentication enabled (COVALENCE_API_KEY is set)");
    } else {
        tracing::warn!("COVALENCE_API_KEY is not set — running in unauthenticated dev mode");
    }

    let state = AppState {
        pool,
        llm,
        graph: shared_graph,
        api_key,
    };
    let app = api::routes::router()
        .with_state(state.clone())
        .layer(axum::middleware::from_fn_with_state(
            state,
            api::auth::require_api_key,
        ))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(tower_http::cors::CorsLayer::permissive());

    // Start server
    let bind = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8430".into());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("covalence engine listening on {bind}");
    axum::serve(listener, app).await?;

    Ok(())
}

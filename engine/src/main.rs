mod api;
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
    let state = AppState { pool, llm };
    let app = api::routes::router()
        .with_state(state)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(tower_http::cors::CorsLayer::permissive());

    // Start server
    let bind = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8430".into());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("covalence engine listening on {bind}");
    axum::serve(listener, app).await?;

    Ok(())
}

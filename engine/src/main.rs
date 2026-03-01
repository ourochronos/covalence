mod api;
mod graph;
mod models;
mod search;

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing (observability — per Gemini review feedback)
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "covalence_engine=debug,tower_http=debug".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let host = std::env::var("COVALENCE_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port = std::env::var("COVALENCE_PORT").unwrap_or_else(|_| "8430".into());
    let addr = format!("{}:{}", host, port);

    tracing::info!("Covalence engine starting on {}", addr);

    let app = api::routes::router();

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Listening on {}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}

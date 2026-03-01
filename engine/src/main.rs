mod api;
mod errors;
mod graph;
mod models;
mod search;
mod services;

use api::AppState;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "covalence_engine=debug,tower_http=debug".into()))
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

    // Build app
    let state = AppState { pool };
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

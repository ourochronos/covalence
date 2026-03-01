use axum::{Router, routing::get};

pub fn router() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/nodes", get(list_nodes))
}

async fn health() -> &'static str {
    "ok"
}

async fn list_nodes() -> &'static str {
    // TODO: implement
    "[]"
}

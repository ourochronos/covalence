pub mod auth;
pub mod extractors;
pub mod openapi;
pub mod routes;

use crate::graph::SharedGraph;
use crate::worker::llm::LlmClient;
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub llm: Arc<dyn LlmClient>,
    pub graph: SharedGraph,
    /// Optional API key read from `COVALENCE_API_KEY`.
    /// `None` → dev mode (no auth required).
    pub api_key: Option<String>,
}

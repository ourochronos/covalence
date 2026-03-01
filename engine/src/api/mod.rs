pub mod routes;

use crate::worker::llm::LlmClient;
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub llm: Arc<dyn LlmClient>,
}

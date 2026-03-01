pub mod routes;

use std::sync::Arc;
use sqlx::PgPool;
use crate::worker::llm::LlmClient;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub llm: Arc<dyn LlmClient>,
}

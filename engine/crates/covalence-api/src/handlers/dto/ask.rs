//! Ask (synthesis) DTOs.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

/// Request body for the ask endpoint.
#[derive(Debug, Deserialize, ToSchema, Validate)]
pub struct AskApiRequest {
    /// The natural language question to answer.
    #[validate(length(min = 1, max = 10000))]
    pub question: String,
    /// Maximum search results to include as context (default 15).
    pub max_context: Option<usize>,
    /// Search strategy (auto, balanced, precise, etc.).
    #[validate(length(max = 50))]
    pub strategy: Option<String>,
    /// LLM model override: haiku, sonnet, opus, gemini, copilot.
    #[validate(length(max = 50))]
    pub model: Option<String>,
    /// Optional session ID for multi-turn conversation context.
    pub session_id: Option<String>,
}

/// A citation backing the synthesized answer.
#[derive(Debug, Serialize, ToSchema)]
pub struct CitationResponse {
    /// Source name or URI.
    pub source: String,
    /// Relevant snippet from the source.
    pub snippet: String,
    /// Result type (chunk, statement, section, node, etc.).
    pub result_type: String,
    /// Confidence score.
    pub confidence: f64,
}

/// Response from the ask endpoint.
#[derive(Debug, Serialize, ToSchema)]
pub struct AskApiResponse {
    /// The synthesized answer.
    pub answer: String,
    /// Citations from the knowledge graph.
    pub citations: Vec<CitationResponse>,
    /// Number of search results used as context.
    pub context_used: usize,
}

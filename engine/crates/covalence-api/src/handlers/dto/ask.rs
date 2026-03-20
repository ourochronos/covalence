//! Ask (synthesis) DTOs.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Request body for the ask endpoint.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AskApiRequest {
    /// The natural language question to answer.
    pub question: String,
    /// Maximum search results to include as context (default 15).
    pub max_context: Option<usize>,
    /// Search strategy (auto, balanced, precise, etc.).
    pub strategy: Option<String>,
    /// LLM model override: haiku, sonnet, opus, gemini, copilot.
    pub model: Option<String>,
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

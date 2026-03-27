//! Memory API request/response DTOs.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

/// Request body for storing a memory.
#[derive(Debug, Deserialize, ToSchema, Validate)]
pub struct StoreMemoryRequest {
    /// The content to remember.
    #[validate(length(min = 1, max = 100_000))]
    pub content: String,
    /// Optional topic/category for organisation.
    #[validate(length(max = 500))]
    pub topic: Option<String>,
    /// Optional metadata.
    pub metadata: Option<serde_json::Value>,
    /// Confidence level (0.0 to 1.0, default 0.8).
    pub confidence: Option<f64>,
    /// Agent that owns this memory.
    #[validate(length(max = 200))]
    pub agent_id: Option<String>,
    /// External task identifier.
    #[validate(length(max = 200))]
    pub task_id: Option<String>,
}

/// Request body for recalling memories.
#[derive(Debug, Deserialize, ToSchema, Validate)]
pub struct RecallMemoryRequest {
    /// The query to search for.
    #[validate(length(min = 1, max = 10_000))]
    pub query: String,
    /// Maximum number of memories to return.
    pub limit: Option<usize>,
    /// Optional topic filter.
    pub topic: Option<String>,
    /// Minimum confidence threshold.
    pub min_confidence: Option<f64>,
    /// Filter to a specific agent's memories.
    pub agent_id: Option<String>,
}

/// Response for a recalled memory item.
#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryItemResponse {
    /// Memory identifier (source ID).
    pub id: String,
    /// The remembered content.
    pub content: String,
    /// Topic if provided.
    pub topic: Option<String>,
    /// Relevance score from search.
    pub relevance: f64,
    /// Confidence level.
    pub confidence: f64,
    /// When this memory was stored.
    pub stored_at: String,
    /// Agent that owns this memory.
    pub agent_id: Option<String>,
    /// Number of times this memory has been recalled.
    pub access_count: Option<i32>,
    /// When this memory was last accessed.
    pub last_accessed: Option<String>,
}

/// Response from storing a memory.
#[derive(Debug, Serialize, ToSchema)]
pub struct StoreMemoryResponse {
    /// ID of the stored memory.
    pub id: String,
    /// Number of entities extracted.
    pub entities_extracted: usize,
    /// Status message.
    pub status: String,
}

/// Memory status response.
#[derive(Debug, Serialize, ToSchema)]
pub struct MemoryStatusResponse {
    /// Total number of memories stored.
    pub total_memories: i64,
    /// Memories for a specific agent (if agent_id provided).
    pub agent_memories: Option<i64>,
}

/// Query parameters for memory status.
#[derive(Debug, Deserialize)]
pub struct MemoryStatusParams {
    /// Optional agent_id to scope the status.
    pub agent_id: Option<String>,
}

/// Request body for memory consolidation.
#[derive(Debug, Deserialize, ToSchema, Validate)]
pub struct ConsolidateMemoryRequest {
    /// Agent whose memories to consolidate.
    #[validate(length(min = 1, max = 200))]
    pub agent_id: String,
    /// Similarity threshold (default 0.85).
    pub threshold: Option<f64>,
}

/// Response from memory consolidation.
#[derive(Debug, Serialize, ToSchema)]
pub struct ConsolidateMemoryResponse {
    /// Number of memory groups found.
    pub groups_found: usize,
    /// Number of new merged memories created.
    pub merged: usize,
    /// Number of originals marked for expiry.
    pub expired: usize,
    /// Status message.
    pub status: String,
}

/// Request body for applying forgetting.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ForgetOldMemoryRequest {
    /// Override retention days (default 90).
    pub retention_days: Option<i64>,
}

/// Response from applying forgetting.
#[derive(Debug, Serialize, ToSchema)]
pub struct ForgetOldMemoryResponse {
    /// Number of memories deleted.
    pub deleted: usize,
    /// Status message.
    pub status: String,
}

/// Response from session reflection.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReflectMemoryResponse {
    /// Number of learnings extracted.
    pub learnings_stored: usize,
    /// Status message.
    pub status: String,
}

/// Path parameter for reflection.
#[derive(Debug, Deserialize)]
pub struct ReflectParams {
    /// Agent ID to attribute reflected learnings to.
    pub agent_id: Option<String>,
}

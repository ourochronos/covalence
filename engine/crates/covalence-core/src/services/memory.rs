//! High-level memory API for AI agent integration.
//!
//! Wraps the lower-level source/chunk/article/graph primitives with
//! a simple store/recall/forget interface. Creates `observation` type
//! sources internally.

use serde::{Deserialize, Serialize};

/// Request to store a memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStoreRequest {
    /// The content to remember.
    pub content: String,
    /// Optional topic/category for organization.
    pub topic: Option<String>,
    /// Optional metadata.
    pub metadata: Option<serde_json::Value>,
    /// Confidence level (0.0 to 1.0, default 0.8).
    pub confidence: Option<f64>,
}

/// Request to recall memories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecallRequest {
    /// The query to search for.
    pub query: String,
    /// Maximum number of memories to return.
    pub limit: Option<usize>,
    /// Optional topic filter.
    pub topic: Option<String>,
    /// Minimum confidence threshold.
    pub min_confidence: Option<f64>,
}

/// A recalled memory item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    /// Memory identifier (source ID internally).
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
}

/// Response from memory store operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStoreResponse {
    /// ID of the stored memory.
    pub id: String,
    /// Number of entities extracted (if any).
    pub entities_extracted: usize,
    /// Status message.
    pub status: String,
}

/// Memory status information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStatus {
    /// Total number of memories stored.
    pub total_memories: u64,
    /// Total number of entities in the graph.
    pub total_entities: u64,
    /// Total number of relationships.
    pub total_relationships: u64,
    /// Number of communities detected.
    pub communities: u64,
}

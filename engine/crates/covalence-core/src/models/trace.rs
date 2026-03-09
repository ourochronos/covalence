//! Search trace and feedback models for query tracing infrastructure.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A recorded search trace capturing execution details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchTrace {
    /// Unique identifier.
    pub id: Uuid,
    /// The query text that was searched.
    pub query_text: String,
    /// The search strategy used.
    pub strategy: String,
    /// Per-dimension result counts.
    pub dimension_counts: serde_json::Value,
    /// Total number of fused results returned.
    pub result_count: i32,
    /// Execution time in milliseconds.
    pub execution_ms: i32,
    /// When the trace was recorded.
    pub created_at: DateTime<Utc>,
}

impl SearchTrace {
    /// Create a new search trace.
    pub fn new(
        query_text: String,
        strategy: String,
        dimension_counts: serde_json::Value,
        result_count: i32,
        execution_ms: i32,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            query_text,
            strategy,
            dimension_counts,
            result_count,
            execution_ms,
            created_at: Utc::now(),
        }
    }
}

/// User-submitted relevance feedback on a search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchFeedback {
    /// Unique identifier.
    pub id: Uuid,
    /// The query text.
    pub query_text: String,
    /// The result entity ID being rated.
    pub result_id: Uuid,
    /// Relevance rating (0.0 to 1.0).
    pub relevance: f64,
    /// Optional free-text comment.
    pub comment: Option<String>,
    /// When the feedback was submitted.
    pub created_at: DateTime<Utc>,
}

impl SearchFeedback {
    /// Create a new search feedback entry.
    pub fn new(
        query_text: String,
        result_id: Uuid,
        relevance: f64,
        comment: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            query_text,
            result_id,
            relevance,
            comment,
            created_at: Utc::now(),
        }
    }
}

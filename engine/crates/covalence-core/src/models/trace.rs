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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_trace_new() {
        let dims = serde_json::json!({"vector": 10, "lexical": 5});
        let trace = SearchTrace::new("test query".into(), "balanced".into(), dims.clone(), 15, 42);
        assert_eq!(trace.query_text, "test query");
        assert_eq!(trace.strategy, "balanced");
        assert_eq!(trace.result_count, 15);
        assert_eq!(trace.execution_ms, 42);
        assert_eq!(trace.dimension_counts, dims);
    }

    #[test]
    fn search_trace_serde_roundtrip() {
        let trace = SearchTrace::new(
            "serde".into(),
            "precise".into(),
            serde_json::json!({}),
            0,
            0,
        );
        let json = serde_json::to_string(&trace).unwrap();
        let restored: SearchTrace = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.query_text, "serde");
        assert_eq!(restored.strategy, "precise");
    }

    #[test]
    fn search_feedback_new() {
        let result_id = Uuid::new_v4();
        let fb = SearchFeedback::new(
            "what is rust".into(),
            result_id,
            0.9,
            Some("very relevant".into()),
        );
        assert_eq!(fb.query_text, "what is rust");
        assert_eq!(fb.result_id, result_id);
        assert!((fb.relevance - 0.9).abs() < 1e-10);
        assert_eq!(fb.comment.as_deref(), Some("very relevant"));
    }

    #[test]
    fn search_feedback_no_comment() {
        let fb = SearchFeedback::new("q".into(), Uuid::new_v4(), 0.5, None);
        assert!(fb.comment.is_none());
    }

    #[test]
    fn search_feedback_serde_roundtrip() {
        let fb = SearchFeedback::new("test".into(), Uuid::new_v4(), 0.75, Some("ok".into()));
        let json = serde_json::to_string(&fb).unwrap();
        let restored: SearchFeedback = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.query_text, "test");
        assert!((restored.relevance - 0.75).abs() < 1e-10);
    }
}

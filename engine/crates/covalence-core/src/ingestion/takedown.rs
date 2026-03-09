//! Source takedown processing.
//!
//! When a source is taken down (e.g., content removed, DMCA),
//! this module handles cascading invalidation of all derived
//! graph elements. Edges extracted from the taken-down source's
//! chunks are invalidated (not deleted) to preserve temporal
//! history.

use serde::{Deserialize, Serialize};

use crate::types::ids::SourceId;

/// Result of a source takedown operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TakedownResult {
    /// Source that was taken down.
    pub source_id: SourceId,
    /// Number of edges invalidated.
    pub edges_invalidated: usize,
    /// Number of chunks affected.
    pub chunks_affected: usize,
    /// Whether articles need recompilation.
    pub articles_need_recompilation: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takedown_result_default_values() {
        let source_id = SourceId::new();
        let result = TakedownResult {
            source_id,
            edges_invalidated: 42,
            chunks_affected: 10,
            articles_need_recompilation: true,
        };
        assert_eq!(result.source_id, source_id);
        assert_eq!(result.edges_invalidated, 42);
        assert_eq!(result.chunks_affected, 10);
        assert!(result.articles_need_recompilation);
    }

    #[test]
    fn takedown_result_serialization_roundtrip() {
        let result = TakedownResult {
            source_id: SourceId::new(),
            edges_invalidated: 5,
            chunks_affected: 3,
            articles_need_recompilation: false,
        };
        let json = serde_json::to_string(&result).expect("serialize");
        let deserialized: TakedownResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.edges_invalidated, result.edges_invalidated,);
        assert_eq!(deserialized.chunks_affected, result.chunks_affected,);
    }
}

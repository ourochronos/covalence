//! Unresolved entity model — entities that missed all resolution tiers
//! and await HDBSCAN batch clustering (Tier 5).
//!
//! **Note:** Relationships involving deferred entities are intentionally
//! not stored. When an entity is deferred, its relationships are skipped
//! in the extraction pipeline. After HDBSCAN resolution creates/assigns
//! a node, edge synthesis (`/admin/edges/synthesize`) can reconstruct
//! co-occurrence edges from shared source provenance.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::ids::{NodeId, SourceId};

/// An entity extracted from a statement or chunk that failed to resolve
/// against the existing graph through all four resolution tiers (exact,
/// alias, vector, fuzzy). Held in a pool for periodic HDBSCAN clustering
/// that proposes new canonical entities or merges into existing ones.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnresolvedEntity {
    /// Unique identifier.
    pub id: Uuid,
    /// Source the entity was extracted from.
    pub source_id: SourceId,
    /// Statement the entity was extracted from (if statement pipeline).
    pub statement_id: Option<Uuid>,
    /// Chunk the entity was extracted from (if chunk pipeline).
    pub chunk_id: Option<Uuid>,
    /// Raw extracted entity name.
    pub extracted_name: String,
    /// Entity type label (e.g., "concept", "person", "method").
    pub entity_type: String,
    /// Optional description from extraction.
    pub description: Option<String>,
    /// Embedding of the entity name for HDBSCAN clustering.
    pub embedding: Option<Vec<f64>>,
    /// Extraction confidence score.
    pub confidence: f64,
    /// Node ID if this entity was later resolved by HDBSCAN.
    pub resolved_node_id: Option<NodeId>,
    /// When the entity was resolved (null if still pending).
    pub resolved_at: Option<DateTime<Utc>>,
    /// When this record was created.
    pub created_at: DateTime<Utc>,
}

impl UnresolvedEntity {
    /// Create a new unresolved entity pending HDBSCAN clustering.
    pub fn new(
        source_id: SourceId,
        extracted_name: String,
        entity_type: String,
        confidence: f64,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            source_id,
            statement_id: None,
            chunk_id: None,
            extracted_name,
            entity_type,
            description: None,
            embedding: None,
            confidence,
            resolved_node_id: None,
            resolved_at: None,
            created_at: Utc::now(),
        }
    }

    /// Whether this entity has been resolved to a graph node.
    pub fn is_resolved(&self) -> bool {
        self.resolved_node_id.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_unresolved_entity_is_pending() {
        let e = UnresolvedEntity::new(
            SourceId::new(),
            "quantum entanglement".to_string(),
            "concept".to_string(),
            0.85,
        );
        assert!(!e.is_resolved());
        assert!(e.resolved_node_id.is_none());
        assert!(e.resolved_at.is_none());
        assert!(e.statement_id.is_none());
        assert!(e.chunk_id.is_none());
        assert_eq!(e.confidence, 0.85);
    }

    #[test]
    fn serde_roundtrip() {
        let e = UnresolvedEntity::new(
            SourceId::new(),
            "test entity".to_string(),
            "concept".to_string(),
            1.0,
        );
        let json = serde_json::to_string(&e).unwrap();
        let restored: UnresolvedEntity = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.extracted_name, "test entity");
        assert_eq!(restored.entity_type, "concept");
    }
}

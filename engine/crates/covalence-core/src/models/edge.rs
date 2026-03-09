//! Edge model -- typed, directed relationship between nodes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::causal::CausalLevel;
use crate::types::clearance::ClearanceLevel;
use crate::types::ids::{EdgeId, NodeId};
use crate::types::opinion::Opinion;

/// A typed, directed relationship between two nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    /// Unique identifier.
    pub id: EdgeId,
    /// Source node of the relationship.
    pub source_node_id: NodeId,
    /// Target node of the relationship.
    pub target_node_id: NodeId,
    /// Relationship type (see edge vocabulary in spec/07).
    pub rel_type: String,
    /// Pearl's causal hierarchy level.
    pub causal_level: Option<CausalLevel>,
    /// Temporal and causal metadata.
    pub properties: serde_json::Value,
    /// Edge weight (default 1.0).
    pub weight: f64,
    /// Extraction confidence.
    pub confidence: f64,
    /// Subjective Logic opinion with per-source contributions.
    pub confidence_breakdown: Option<Opinion>,
    /// Federation clearance level.
    pub clearance_level: ClearanceLevel,
    /// Whether this edge was generated synthetically (e.g. ZK federation).
    pub is_synthetic: bool,
    /// When the fact became true in the world (extracted from text, null if unknown).
    pub valid_from: Option<DateTime<Utc>>,
    /// When the fact stopped being true (null if still current).
    pub valid_until: Option<DateTime<Utc>>,
    /// When a contradicting edge invalidated this one (null if still valid).
    pub invalid_at: Option<DateTime<Utc>>,
    /// The edge that superseded this one.
    pub invalidated_by: Option<EdgeId>,
    /// Transaction time: when the system recorded this edge.
    pub recorded_at: DateTime<Utc>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

impl Edge {
    /// Create a new edge with default weight and confidence.
    pub fn new(source_node_id: NodeId, target_node_id: NodeId, rel_type: String) -> Self {
        let now = Utc::now();
        Self {
            id: EdgeId::new(),
            source_node_id,
            target_node_id,
            rel_type,
            causal_level: None,
            properties: serde_json::Value::Object(Default::default()),
            weight: 1.0,
            confidence: 1.0,
            confidence_breakdown: None,
            clearance_level: ClearanceLevel::default(),
            is_synthetic: false,
            valid_from: None,
            valid_until: None,
            invalid_at: None,
            invalidated_by: None,
            recorded_at: now,
            created_at: now,
        }
    }

    /// Create a causal edge (L1+) with a specified causal level.
    pub fn causal(
        source_node_id: NodeId,
        target_node_id: NodeId,
        rel_type: String,
        level: CausalLevel,
    ) -> Self {
        let mut edge = Self::new(source_node_id, target_node_id, rel_type);
        edge.causal_level = Some(level);
        edge
    }

    /// Returns true if this edge has been invalidated by a newer edge.
    pub fn is_invalidated(&self) -> bool {
        self.invalid_at.is_some()
    }

    /// Returns true if this edge is temporally valid at the given point in time.
    pub fn is_valid_at(&self, t: DateTime<Utc>) -> bool {
        let after_start = self.valid_from.is_none_or(|vf| vf <= t);
        let before_end = self.valid_until.is_none_or(|vu| vu > t);
        let not_invalidated = self.invalid_at.is_none_or(|ia| ia > t);
        after_start && before_end && not_invalidated
    }
}

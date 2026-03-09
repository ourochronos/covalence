//! Node model -- entity in the knowledge graph.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::clearance::ClearanceLevel;
use crate::types::ids::NodeId;
use crate::types::opinion::Opinion;

/// An entity extracted from one or more chunks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// Unique identifier.
    pub id: NodeId,
    /// Canonical name for this entity.
    pub canonical_name: String,
    /// Dynamically assigned type (e.g. `"person"`, `"organization"`).
    pub node_type: String,
    /// Optional description text.
    pub description: Option<String>,
    /// Arbitrary JSONB properties.
    pub properties: serde_json::Value,
    /// Subjective Logic opinion tuple for confidence.
    pub confidence_breakdown: Option<Opinion>,
    /// Federation clearance level.
    pub clearance_level: ClearanceLevel,
    /// When this entity was first extracted.
    pub first_seen: DateTime<Utc>,
    /// When this entity was most recently seen.
    pub last_seen: DateTime<Utc>,
    /// Number of times this entity has been mentioned across extractions.
    pub mention_count: i32,
}

impl Node {
    /// Create a new node with default timestamps and mention count.
    pub fn new(canonical_name: String, node_type: String) -> Self {
        let now = Utc::now();
        Self {
            id: NodeId::new(),
            canonical_name,
            node_type,
            description: None,
            properties: serde_json::Value::Object(Default::default()),
            confidence_breakdown: None,
            clearance_level: ClearanceLevel::default(),
            first_seen: now,
            last_seen: now,
            mention_count: 1,
        }
    }

    /// Record a new mention, updating `last_seen` and `mention_count`.
    pub fn record_mention(&mut self) {
        self.last_seen = Utc::now();
        self.mention_count += 1;
    }

    /// Merge properties from another node into this one.
    ///
    /// Existing keys are preserved; only new keys from `other` are added.
    pub fn merge_properties(&mut self, other: &serde_json::Value) {
        if let (serde_json::Value::Object(self_map), serde_json::Value::Object(other_map)) =
            (&mut self.properties, other)
        {
            for (k, v) in other_map {
                self_map.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
    }

    /// Merge description from another node, appending if both exist.
    pub fn merge_description(&mut self, other_desc: Option<&str>) {
        match (&self.description, other_desc) {
            (None, Some(d)) => self.description = Some(d.to_string()),
            (Some(existing), Some(other)) if existing != other => {
                self.description = Some(format!("{existing}\n\n{other}"));
            }
            _ => {}
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_defaults() {
        let node = Node::new("Alice".into(), "person".into());
        assert_eq!(node.canonical_name, "Alice");
        assert_eq!(node.node_type, "person");
        assert_eq!(node.mention_count, 1);
        assert!(node.description.is_none());
        assert!(node.confidence_breakdown.is_none());
        assert_eq!(node.clearance_level, ClearanceLevel::default());
        assert_eq!(node.properties, serde_json::json!({}));
        // first_seen and last_seen should be equal at creation
        assert_eq!(node.first_seen, node.last_seen);
    }

    #[test]
    fn record_mention_increments_count() {
        let mut node = Node::new("Bob".into(), "person".into());
        let original_first_seen = node.first_seen;
        assert_eq!(node.mention_count, 1);

        node.record_mention();
        assert_eq!(node.mention_count, 2);
        // first_seen should not change
        assert_eq!(node.first_seen, original_first_seen);
        // last_seen should be updated (>= first_seen)
        assert!(node.last_seen >= node.first_seen);

        node.record_mention();
        assert_eq!(node.mention_count, 3);
    }

    #[test]
    fn merge_properties_adds_new_keys() {
        let mut node = Node::new("Test".into(), "entity".into());
        node.properties = serde_json::json!({"color": "blue"});

        let other = serde_json::json!({"size": "large", "color": "red"});
        node.merge_properties(&other);

        // "size" should be added, "color" should stay as "blue"
        assert_eq!(node.properties["color"], "blue");
        assert_eq!(node.properties["size"], "large");
    }

    #[test]
    fn merge_properties_noop_for_non_objects() {
        let mut node = Node::new("Test".into(), "entity".into());
        node.properties = serde_json::json!({"a": 1});

        // Merging a non-object should do nothing
        node.merge_properties(&serde_json::json!("not an object"));
        assert_eq!(node.properties, serde_json::json!({"a": 1}));

        // Merging into a non-object property should do nothing
        node.properties = serde_json::json!(42);
        node.merge_properties(&serde_json::json!({"b": 2}));
        assert_eq!(node.properties, serde_json::json!(42));
    }

    #[test]
    fn merge_description_into_none() {
        let mut node = Node::new("Test".into(), "entity".into());
        assert!(node.description.is_none());

        node.merge_description(Some("A description"));
        assert_eq!(node.description.as_deref(), Some("A description"));
    }

    #[test]
    fn merge_description_appends_different() {
        let mut node = Node::new("Test".into(), "entity".into());
        node.description = Some("First".into());

        node.merge_description(Some("Second"));
        assert_eq!(node.description.as_deref(), Some("First\n\nSecond"));
    }

    #[test]
    fn merge_description_noop_for_same() {
        let mut node = Node::new("Test".into(), "entity".into());
        node.description = Some("Same text".into());

        node.merge_description(Some("Same text"));
        assert_eq!(node.description.as_deref(), Some("Same text"));
    }

    #[test]
    fn merge_description_noop_for_none_other() {
        let mut node = Node::new("Test".into(), "entity".into());
        node.description = Some("Existing".into());

        node.merge_description(None);
        assert_eq!(node.description.as_deref(), Some("Existing"));
    }

    #[test]
    fn serde_roundtrip() {
        let node = Node::new("Rust".into(), "language".into());
        let json = serde_json::to_string(&node).unwrap();
        let restored: Node = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.canonical_name, "Rust");
        assert_eq!(restored.node_type, "language");
        assert_eq!(restored.mention_count, 1);
    }
}

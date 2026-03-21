//! Node model -- entity in the knowledge graph.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::clearance::ClearanceLevel;
use crate::types::ids::NodeId;
use crate::types::opinion::Opinion;

/// Entity class classification for nodes.
///
/// Groups the 47+ ad-hoc `node_type` values into four controlled
/// categories. Stored on the node for indexing and filtering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityClass {
    /// Entities extracted from source code (function, struct, trait, etc.).
    Code,
    /// Domain concepts from any textual source (concept, technology, etc.).
    Domain,
    /// People, organizations, locations.
    Actor,
    /// System-generated entities (component, community_summary).
    Analysis,
}

impl EntityClass {
    /// String representation for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Domain => "domain",
            Self::Actor => "actor",
            Self::Analysis => "analysis",
        }
    }

    /// Parse from database string.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "code" => Some(Self::Code),
            "domain" => Some(Self::Domain),
            "actor" => Some(Self::Actor),
            "analysis" => Some(Self::Analysis),
            _ => None,
        }
    }

    /// Map from ontology category to EntityClass.
    ///
    /// Ontology categories (concept, process, artifact, agent,
    /// property, collection) are more granular than EntityClass.
    /// This provides the backward-compatible mapping.
    pub fn from_category(category: &str) -> Self {
        match category {
            "agent" => Self::Actor,
            _ => Self::Domain, // concept, process, artifact, property, collection
        }
    }
}

/// Derive entity class from a node_type string.
///
/// Uses the ontology category mapping when available (passed as
/// a HashMap from type → category). Falls back to hardcoded
/// defaults for backward compatibility.
pub fn derive_entity_class(node_type: &str) -> EntityClass {
    derive_entity_class_with_ontology(node_type, None)
}

/// Derive entity class using an optional ontology type→category map.
///
/// When `type_to_category` is provided, looks up the node_type in
/// the ontology. When not provided (or type not found), falls back
/// to hardcoded defaults.
pub fn derive_entity_class_with_ontology(
    node_type: &str,
    type_to_category: Option<&std::collections::HashMap<String, String>>,
) -> EntityClass {
    // Try ontology lookup first.
    if let Some(map) = type_to_category {
        if let Some(category) = map.get(node_type) {
            return EntityClass::from_category(category);
        }
    }

    // Hardcoded fallback (backward compatibility).
    match node_type {
        "function" | "struct" | "trait" | "enum" | "impl_block" | "constant" | "module"
        | "class" | "macro" | "code_function" | "code_struct" | "code_trait" | "code_module"
        | "code_impl" | "code_type" | "code_test" => EntityClass::Code,
        "person" | "organization" | "location" | "role" => EntityClass::Actor,
        "component" => EntityClass::Analysis,
        _ => EntityClass::Domain,
    }
}

/// Derive entity class considering the source domain context.
///
/// Code-typed entities (struct, function, etc.) extracted from
/// non-code sources (research papers, specs) are classified as
/// `Domain` rather than `Code`, since they're *mentions* of code
/// concepts, not actual code entities. This prevents research
/// papers that discuss "structs" from polluting the code entity
/// class.
///
/// Falls back to [`derive_entity_class`] for code-domain sources
/// or when source_domain is unknown.
pub fn derive_entity_class_with_context(
    node_type: &str,
    source_domain: Option<&str>,
) -> EntityClass {
    let base = derive_entity_class(node_type);

    // If the base class is Code but the source isn't a code source,
    // demote to Domain — it's a mention, not an actual code entity.
    if base == EntityClass::Code {
        match source_domain {
            Some("code") | None => base,
            Some(_) => EntityClass::Domain,
        }
    } else {
        base
    }
}

/// An entity extracted from one or more chunks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// Unique identifier.
    pub id: NodeId,
    /// Canonical name for this entity.
    pub canonical_name: String,
    /// Dynamically assigned type (e.g. `"person"`, `"organization"`).
    pub node_type: String,
    /// Entity class: code, domain, actor, analysis.
    pub entity_class: Option<String>,
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
    /// Shannon entropy of domain distribution across extraction provenance.
    /// Low = internal concept (one domain), high = cross-cutting (many domains).
    pub domain_entropy: Option<f32>,
    /// The domain where this entity is most frequently mentioned.
    pub primary_domain: Option<String>,
}

impl Node {
    /// Create a new node with default timestamps and mention count.
    pub fn new(canonical_name: String, node_type: String) -> Self {
        let now = Utc::now();
        let entity_class = Some(derive_entity_class(&node_type).as_str().to_string());
        Self {
            id: NodeId::new(),
            canonical_name,
            node_type,
            entity_class,
            description: None,
            properties: serde_json::Value::Object(Default::default()),
            confidence_breakdown: None,
            clearance_level: ClearanceLevel::default(),
            first_seen: now,
            last_seen: now,
            mention_count: 1,
            domain_entropy: None,
            primary_domain: None,
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
    fn entity_class_roundtrip() {
        let classes = [
            EntityClass::Code,
            EntityClass::Domain,
            EntityClass::Actor,
            EntityClass::Analysis,
        ];
        for ec in &classes {
            let s = ec.as_str();
            let parsed = EntityClass::from_str_opt(s);
            assert_eq!(parsed, Some(ec.clone()), "roundtrip failed for {s}");
        }
    }

    #[test]
    fn entity_class_from_str_unknown() {
        assert!(EntityClass::from_str_opt("unknown").is_none());
        assert!(EntityClass::from_str_opt("").is_none());
    }

    #[test]
    fn derive_entity_class_code_types() {
        for t in &[
            "function",
            "struct",
            "trait",
            "enum",
            "impl_block",
            "constant",
            "module",
            "class",
            "macro",
            "code_function",
            "code_struct",
            "code_trait",
        ] {
            assert_eq!(
                derive_entity_class(t),
                EntityClass::Code,
                "{t} should be code"
            );
        }
    }

    #[test]
    fn derive_entity_class_actor_types() {
        for t in &["person", "organization", "location", "role"] {
            assert_eq!(
                derive_entity_class(t),
                EntityClass::Actor,
                "{t} should be actor"
            );
        }
    }

    #[test]
    fn derive_entity_class_analysis_types() {
        assert_eq!(derive_entity_class("component"), EntityClass::Analysis);
    }

    #[test]
    fn derive_entity_class_with_context_code_from_code_source() {
        assert_eq!(
            derive_entity_class_with_context("struct", Some("code")),
            EntityClass::Code
        );
        assert_eq!(
            derive_entity_class_with_context("function", Some("code")),
            EntityClass::Code
        );
    }

    #[test]
    fn derive_entity_class_with_context_code_from_research_demoted() {
        // A "struct" mentioned in a research paper should be domain, not code
        assert_eq!(
            derive_entity_class_with_context("struct", Some("research")),
            EntityClass::Domain
        );
        assert_eq!(
            derive_entity_class_with_context("function", Some("spec")),
            EntityClass::Domain
        );
    }

    #[test]
    fn derive_entity_class_with_context_none_source() {
        // Unknown source defaults to base derivation
        assert_eq!(
            derive_entity_class_with_context("struct", None),
            EntityClass::Code
        );
    }

    #[test]
    fn derive_entity_class_with_context_non_code_types_unaffected() {
        // Non-code types are not affected by source domain
        assert_eq!(
            derive_entity_class_with_context("person", Some("research")),
            EntityClass::Actor
        );
        assert_eq!(
            derive_entity_class_with_context("concept", Some("code")),
            EntityClass::Domain
        );
    }

    #[test]
    fn derive_entity_class_domain_fallback() {
        for t in &[
            "concept",
            "technology",
            "algorithm",
            "dataset",
            "event",
            "unknown_type",
        ] {
            assert_eq!(
                derive_entity_class(t),
                EntityClass::Domain,
                "{t} should be domain"
            );
        }
    }

    #[test]
    fn new_sets_defaults() {
        let node = Node::new("Alice".into(), "person".into());
        assert_eq!(node.canonical_name, "Alice");
        assert_eq!(node.node_type, "person");
        assert_eq!(node.entity_class.as_deref(), Some("actor"));
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

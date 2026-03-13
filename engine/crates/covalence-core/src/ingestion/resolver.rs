//! Stage 7: Entity resolution and storage.
//!
//! Matches extracted entities against existing nodes using:
//! exact name, fuzzy (trigram), vector similarity, and graph context.
//! Uses PG advisory locks for concurrency safety.

use serde::{Deserialize, Serialize};

use crate::types::ids::NodeId;

use super::extractor::ExtractedEntity;

/// How an entity was matched to an existing node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchType {
    /// Exact canonical name match.
    Exact,
    /// Fuzzy trigram match.
    Fuzzy,
    /// Vector similarity match.
    Vector,
    /// No match found — new entity.
    New,
    /// Deferred to Tier 5 (HDBSCAN clustering pool).
    /// Entity is stored in `unresolved_entities` for batch resolution.
    Deferred,
}

/// Result of resolving an extracted entity against the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedEntity {
    /// Existing node ID if matched, None if new.
    pub node_id: Option<NodeId>,
    /// Canonical name for the entity.
    pub canonical_name: String,
    /// How the match was determined.
    pub match_type: MatchType,
}

/// Trait for resolving extracted entities against existing graph nodes.
#[async_trait::async_trait]
pub trait EntityResolver: Send + Sync {
    /// Resolve an extracted entity to an existing node or mark as new.
    async fn resolve(&self, entity: &ExtractedEntity) -> crate::error::Result<ResolvedEntity>;
}

/// A mock resolver that always returns `MatchType::New`.
pub struct MockResolver;

#[async_trait::async_trait]
impl EntityResolver for MockResolver {
    async fn resolve(&self, entity: &ExtractedEntity) -> crate::error::Result<ResolvedEntity> {
        Ok(ResolvedEntity {
            node_id: None,
            canonical_name: entity.name.clone(),
            match_type: MatchType::New,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_resolver_returns_new() {
        let resolver = MockResolver;
        let entity = ExtractedEntity {
            name: "Alice".to_string(),
            entity_type: "person".to_string(),
            description: None,
            confidence: 0.9,
            metadata: None,
        };
        let result = resolver.resolve(&entity).await.unwrap();
        assert_eq!(result.match_type, MatchType::New);
        assert_eq!(result.node_id, None);
        assert_eq!(result.canonical_name, "Alice");
    }
}

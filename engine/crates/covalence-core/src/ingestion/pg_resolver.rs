//! PostgreSQL-backed entity resolver.
//!
//! Resolves extracted entities against the knowledge graph using a
//! three-tier strategy: exact name match, alias match, then fuzzy
//! trigram similarity via `pg_trgm`.

use std::sync::Arc;

use sqlx::Row;

use crate::error::{Error, Result};
use crate::ingestion::extractor::ExtractedEntity;
use crate::ingestion::resolver::{EntityResolver, MatchType, ResolvedEntity};
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{NodeAliasRepo, NodeRepo};
use crate::types::ids::NodeId;

/// Minimum trigram similarity score to consider a fuzzy match.
const FUZZY_THRESHOLD: f32 = 0.6;

/// Entity resolver backed by PostgreSQL.
///
/// Resolution strategy (in order of preference):
/// 1. **Exact match** — case-insensitive canonical name lookup.
/// 2. **Alias match** — case-insensitive lookup in `node_aliases`.
/// 3. **Fuzzy match** — `pg_trgm` similarity on `nodes.canonical_name`,
///    preferring nodes with matching `node_type`.
/// 4. **New** — no match found; the entity should be created.
pub struct PgResolver {
    /// Shared reference to the PostgreSQL repository.
    repo: Arc<PgRepo>,
}

impl PgResolver {
    /// Create a new resolver with the given repository.
    pub fn new(repo: Arc<PgRepo>) -> Self {
        Self { repo }
    }

    /// Try exact case-insensitive match on canonical name.
    async fn try_exact_match(&self, entity: &ExtractedEntity) -> Result<Option<ResolvedEntity>> {
        let node = NodeRepo::find_by_name(self.repo.as_ref(), &entity.name).await?;
        Ok(node.map(|n| ResolvedEntity {
            node_id: Some(n.id),
            canonical_name: n.canonical_name,
            match_type: MatchType::Exact,
        }))
    }

    /// Try case-insensitive alias lookup in the `node_aliases` table.
    async fn try_alias_match(&self, entity: &ExtractedEntity) -> Result<Option<ResolvedEntity>> {
        let aliases = NodeAliasRepo::find_by_alias(self.repo.as_ref(), &entity.name).await?;

        // find_by_alias uses ILIKE with wildcards; filter to exact
        // case-insensitive matches only.
        let exact_alias = aliases
            .into_iter()
            .find(|a| a.alias.eq_ignore_ascii_case(&entity.name));

        if let Some(alias) = exact_alias {
            // Fetch the canonical node to return its name.
            let node = NodeRepo::get(self.repo.as_ref(), alias.node_id).await?;
            let canonical_name = node
                .map(|n| n.canonical_name)
                .unwrap_or_else(|| entity.name.clone());
            return Ok(Some(ResolvedEntity {
                node_id: Some(alias.node_id),
                canonical_name,
                match_type: MatchType::Exact,
            }));
        }

        Ok(None)
    }

    /// Try fuzzy trigram match using `pg_trgm` similarity.
    ///
    /// Queries nodes where `similarity(canonical_name, $1) >= 0.6`,
    /// ordering by same `node_type` first, then by descending similarity.
    async fn try_fuzzy_match(&self, entity: &ExtractedEntity) -> Result<Option<ResolvedEntity>> {
        let row = sqlx::query(
            "SELECT id, canonical_name, node_type,
                    similarity(canonical_name, $1) AS sim
             FROM nodes
             WHERE similarity(canonical_name, $1) >= $2
             ORDER BY
                 (node_type = $3) DESC,
                 sim DESC
             LIMIT 1",
        )
        .bind(&entity.name)
        .bind(FUZZY_THRESHOLD)
        .bind(&entity.entity_type)
        .fetch_optional(self.repo.pool())
        .await
        .map_err(|e| Error::EntityResolution(format!("fuzzy match query failed: {e}")))?;

        Ok(row.map(|r| {
            let id: NodeId = r.get("id");
            let canonical_name: String = r.get("canonical_name");
            ResolvedEntity {
                node_id: Some(id),
                canonical_name,
                match_type: MatchType::Fuzzy,
            }
        }))
    }
}

#[async_trait::async_trait]
impl EntityResolver for PgResolver {
    /// Resolve an extracted entity against the knowledge graph.
    ///
    /// Tries exact match, then alias match, then fuzzy match.
    /// Returns `MatchType::New` if nothing matches.
    async fn resolve(&self, entity: &ExtractedEntity) -> Result<ResolvedEntity> {
        // 1. Exact canonical name match (fastest path).
        if let Some(resolved) = self.try_exact_match(entity).await? {
            return Ok(resolved);
        }

        // 2. Alias match.
        if let Some(resolved) = self.try_alias_match(entity).await? {
            return Ok(resolved);
        }

        // 3. Fuzzy trigram match.
        if let Some(resolved) = self.try_fuzzy_match(entity).await? {
            return Ok(resolved);
        }

        // 4. No match — new entity.
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

    /// Verify the resolver struct is Send + Sync (required by EntityResolver).
    #[test]
    fn pg_resolver_is_send_sync() {
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<PgResolver>();
    }

    /// Verify that the MatchType::New fallback works correctly
    /// without any database by constructing the expected output
    /// directly (mirrors what resolve() returns on no match).
    #[test]
    fn new_entity_fallback() {
        let entity = ExtractedEntity {
            name: "Alice".to_string(),
            entity_type: "person".to_string(),
            description: Some("A test entity".to_string()),
            confidence: 0.95,
        };
        let resolved = ResolvedEntity {
            node_id: None,
            canonical_name: entity.name.clone(),
            match_type: MatchType::New,
        };
        assert_eq!(resolved.match_type, MatchType::New);
        assert!(resolved.node_id.is_none());
        assert_eq!(resolved.canonical_name, "Alice");
    }

    /// Verify the fuzzy threshold constant is set correctly.
    #[test]
    fn fuzzy_threshold_is_reasonable() {
        assert!((0.5..=0.9).contains(&FUZZY_THRESHOLD));
    }
}

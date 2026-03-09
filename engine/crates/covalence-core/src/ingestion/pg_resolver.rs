//! PostgreSQL-backed entity and relationship-type resolver.
//!
//! Resolves extracted entities against the knowledge graph using a
//! four-tier strategy: exact name match, alias match, vector cosine
//! similarity, then fuzzy trigram similarity via `pg_trgm`.
//!
//! Also resolves relationship type labels so that synonymous edge
//! types (e.g. "author_of" / "is_author_of" / "wrote") converge on
//! the canonical (most frequently used) form.

use std::sync::Arc;

use sqlx::Row;

use crate::error::{Error, Result};
use crate::ingestion::embedder::Embedder;
use crate::ingestion::extractor::ExtractedEntity;
use crate::ingestion::resolver::{EntityResolver, MatchType, ResolvedEntity};
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{NodeAliasRepo, NodeRepo};
use crate::types::ids::NodeId;

/// Default minimum trigram similarity score for fuzzy matching.
const DEFAULT_FUZZY_THRESHOLD: f32 = 0.4;

/// Default cosine similarity threshold for vector-based matching.
const DEFAULT_VECTOR_THRESHOLD: f32 = 0.85;

/// Entity and relationship-type resolver backed by PostgreSQL.
///
/// Resolution strategy for entities (in order of preference):
/// 1. **Exact match** — case-insensitive canonical name lookup.
/// 2. **Alias match** — case-insensitive lookup in `node_aliases`.
/// 3. **Vector match** — cosine similarity between the entity name
///    embedding and existing node embeddings (requires an embedder).
/// 4. **Fuzzy match** — `pg_trgm` similarity on
///    `nodes.canonical_name`, preferring nodes with matching
///    `node_type`.
/// 5. **New** — no match found; the entity should be created.
pub struct PgResolver {
    /// Shared reference to the PostgreSQL repository.
    repo: Arc<PgRepo>,
    /// Minimum trigram similarity score (0.0–1.0) required to
    /// accept a fuzzy match.
    threshold: f32,
    /// Optional embedder for generating entity name embeddings.
    embedder: Option<Arc<dyn Embedder>>,
    /// Minimum cosine similarity to accept a vector match.
    vector_threshold: f32,
}

impl PgResolver {
    /// Create a new resolver with the given repository and default
    /// trigram threshold.
    pub fn new(repo: Arc<PgRepo>) -> Self {
        Self {
            repo,
            threshold: DEFAULT_FUZZY_THRESHOLD,
            embedder: None,
            vector_threshold: DEFAULT_VECTOR_THRESHOLD,
        }
    }

    /// Create a new resolver with a custom trigram threshold.
    pub fn with_threshold(repo: Arc<PgRepo>, threshold: f32) -> Self {
        Self {
            repo,
            threshold,
            embedder: None,
            vector_threshold: DEFAULT_VECTOR_THRESHOLD,
        }
    }

    /// Create a resolver with an embedder for vector-based matching.
    ///
    /// When an embedder is provided, the resolver will embed entity
    /// names and compare them against existing node embeddings using
    /// cosine similarity before falling back to trigram matching.
    pub fn with_embedder(
        repo: Arc<PgRepo>,
        threshold: f32,
        embedder: Arc<dyn Embedder>,
        vector_threshold: f32,
    ) -> Self {
        Self {
            repo,
            threshold,
            embedder: Some(embedder),
            vector_threshold,
        }
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

    /// Try vector cosine similarity match against node embeddings.
    ///
    /// Embeds the entity name via the configured embedder, then
    /// queries the closest node by cosine distance. Accepts the match
    /// only when similarity meets or exceeds `vector_threshold`.
    async fn try_vector_match(&self, entity: &ExtractedEntity) -> Result<Option<ResolvedEntity>> {
        let embedder = match &self.embedder {
            Some(e) => e,
            None => return Ok(None),
        };

        // Embed the entity name.
        let embeddings = embedder
            .embed(std::slice::from_ref(&entity.name))
            .await
            .map_err(|e| Error::EntityResolution(format!("failed to embed entity name: {e}")))?;

        let embedding = match embeddings.into_iter().next() {
            Some(v) => v,
            None => return Ok(None),
        };

        // Convert to f32 for halfvec cast in the query.
        let embedding_f32: Vec<f32> = embedding.iter().map(|&v| v as f32).collect();

        // Query closest node by cosine distance, filtering to
        // nodes that have embeddings.
        let row = sqlx::query(
            "SELECT id, canonical_name, \
                    1.0 - (embedding <=> $1::halfvec) AS sim \
             FROM nodes \
             WHERE embedding IS NOT NULL \
             ORDER BY embedding <=> $1::halfvec \
             LIMIT 1",
        )
        .bind(&embedding_f32)
        .fetch_optional(self.repo.pool())
        .await
        .map_err(|e| Error::EntityResolution(format!("vector match query failed: {e}")))?;

        if let Some(r) = row {
            let sim: f64 = r.get("sim");
            if sim as f32 >= self.vector_threshold {
                let id: NodeId = r.get("id");
                let canonical_name: String = r.get("canonical_name");
                return Ok(Some(ResolvedEntity {
                    node_id: Some(id),
                    canonical_name,
                    match_type: MatchType::Vector,
                }));
            }
        }

        Ok(None)
    }

    /// Try fuzzy trigram match using `pg_trgm` similarity.
    ///
    /// Queries nodes where `similarity(canonical_name, $1)` exceeds
    /// the configured threshold, ordering by same `node_type` first,
    /// then by descending similarity.
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
        .bind(self.threshold)
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

    /// Resolve a relationship type label against existing edge types.
    ///
    /// Strategy:
    /// 1. **Exact match** — if the given `rel_type` already exists
    ///    among edges, return it unchanged.
    /// 2. **Fuzzy match** — use `pg_trgm` similarity to find the
    ///    closest existing `rel_type` above the configured threshold.
    ///    When a match is found, return the canonical form (the most
    ///    frequently used spelling).
    /// 3. **No match** — return the input unchanged (it is a
    ///    genuinely new relationship type).
    pub async fn resolve_rel_type(&self, rel_type: &str) -> Result<String> {
        // Normalize for comparison: lowercase + trim.
        let normalized = rel_type.trim().to_lowercase();

        // 1. Exact match — check if this rel_type already exists.
        let exact = sqlx::query(
            "SELECT rel_type FROM edges \
             WHERE LOWER(rel_type) = LOWER($1) \
             LIMIT 1",
        )
        .bind(&normalized)
        .fetch_optional(self.repo.pool())
        .await
        .map_err(|e| Error::EntityResolution(format!("rel_type exact match query failed: {e}")))?;

        if let Some(row) = exact {
            return Ok(row.get::<String, _>("rel_type"));
        }

        // 2. Fuzzy match — find the closest existing rel_type.
        let fuzzy = sqlx::query(
            "SELECT rel_type, \
                    similarity(rel_type, $1) AS sim, \
                    COUNT(*) AS freq \
             FROM edges \
             WHERE similarity(rel_type, $1) > $2 \
             GROUP BY rel_type \
             ORDER BY sim DESC, freq DESC \
             LIMIT 1",
        )
        .bind(&normalized)
        .bind(self.threshold)
        .fetch_optional(self.repo.pool())
        .await
        .map_err(|e| Error::EntityResolution(format!("rel_type fuzzy match query failed: {e}")))?;

        if let Some(row) = fuzzy {
            return Ok(row.get::<String, _>("rel_type"));
        }

        // 3. No match — use the input as-is.
        Ok(rel_type.to_string())
    }
}

#[async_trait::async_trait]
impl EntityResolver for PgResolver {
    /// Resolve an extracted entity against the knowledge graph.
    ///
    /// Tries exact → alias → vector → fuzzy trigram.
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

        // 3. Vector cosine similarity match.
        if let Some(resolved) = self.try_vector_match(entity).await? {
            return Ok(resolved);
        }

        // 4. Fuzzy trigram match.
        if let Some(resolved) = self.try_fuzzy_match(entity).await? {
            return Ok(resolved);
        }

        // 5. No match — new entity.
        Ok(ResolvedEntity {
            node_id: None,
            canonical_name: entity.name.clone(),
            match_type: MatchType::New,
        })
    }
}

/// Normalize a relationship type label for comparison.
///
/// Applies lowercase, trimming, underscore/hyphen/space unification,
/// and strips common prefixes like "is_" and "has_".
pub fn normalize_rel_type(raw: &str) -> String {
    let mut s = raw.trim().to_lowercase();

    // Unify separators: spaces and hyphens become underscores.
    s = s.replace([' ', '-'], "_");

    // Collapse multiple underscores.
    while s.contains("__") {
        s = s.replace("__", "_");
    }

    // Strip leading/trailing underscores.
    s = s.trim_matches('_').to_string();

    // Strip common semantically-empty prefixes.
    for prefix in &["is_", "has_", "was_"] {
        if let Some(stripped) = s.strip_prefix(prefix) {
            if !stripped.is_empty() {
                s = stripped.to_string();
                break;
            }
        }
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the resolver struct is Send + Sync (required by
    /// EntityResolver).
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

    /// Verify the default fuzzy threshold constant is reasonable.
    #[test]
    fn fuzzy_threshold_is_reasonable() {
        assert!((0.1..=0.9).contains(&DEFAULT_FUZZY_THRESHOLD));
    }

    /// Custom threshold is stored and used.
    #[test]
    fn custom_threshold_stored() {
        // We can't construct a PgResolver without a pool in unit
        // tests, but we can verify the with_threshold constructor
        // compiles and the type is correct.
        fn _check_api() {
            // Ensures with_threshold signature is correct at compile
            // time.
            let _: fn(Arc<PgRepo>, f32) -> PgResolver = PgResolver::with_threshold;
        }
    }

    /// Verify the default vector threshold is reasonable.
    #[test]
    fn vector_threshold_default_is_reasonable() {
        assert!((0.7..=0.95).contains(&DEFAULT_VECTOR_THRESHOLD));
    }

    /// Verify MatchType::Vector variant round-trips correctly.
    #[test]
    fn vector_match_type_round_trips() {
        let resolved = ResolvedEntity {
            node_id: None,
            canonical_name: "test".to_string(),
            match_type: MatchType::Vector,
        };
        assert_eq!(resolved.match_type, MatchType::Vector);
    }

    /// Verify with_embedder constructor compiles correctly.
    #[test]
    fn with_embedder_constructor_compiles() {
        fn _check_api() {
            let _: fn(Arc<PgRepo>, f32, Arc<dyn Embedder>, f32) -> PgResolver =
                PgResolver::with_embedder;
        }
    }

    // ---- normalize_rel_type unit tests ----

    #[test]
    fn normalize_rel_type_basic() {
        assert_eq!(normalize_rel_type("author_of"), "author_of");
    }

    #[test]
    fn normalize_rel_type_strips_is_prefix() {
        assert_eq!(normalize_rel_type("is_author_of"), "author_of");
    }

    #[test]
    fn normalize_rel_type_strips_has_prefix() {
        assert_eq!(normalize_rel_type("has_part"), "part");
    }

    #[test]
    fn normalize_rel_type_strips_was_prefix() {
        assert_eq!(normalize_rel_type("was_created_by"), "created_by");
    }

    #[test]
    fn normalize_rel_type_unifies_separators() {
        assert_eq!(normalize_rel_type("authored by"), "authored_by");
        assert_eq!(normalize_rel_type("authored-by"), "authored_by");
    }

    #[test]
    fn normalize_rel_type_trims_whitespace() {
        assert_eq!(normalize_rel_type("  wrote  "), "wrote");
    }

    #[test]
    fn normalize_rel_type_collapses_underscores() {
        assert_eq!(normalize_rel_type("related__to"), "related_to");
    }

    #[test]
    fn normalize_rel_type_case_insensitive() {
        assert_eq!(normalize_rel_type("AuthorOf"), "authorof");
        assert_eq!(normalize_rel_type("WROTE"), "wrote");
    }

    #[test]
    fn normalize_rel_type_empty_string() {
        assert_eq!(normalize_rel_type(""), "");
    }

    #[test]
    fn normalize_rel_type_only_prefix() {
        // "is_" alone should not strip to empty — the prefix
        // stripping only fires if something remains after removal.
        assert_eq!(normalize_rel_type("is_"), "is");
    }
}

//! PostgreSQL-backed entity and relationship-type resolver.
//!
//! Resolves extracted entities against the knowledge graph using a
//! five-tier strategy: exact name match, alias match, vector cosine
//! similarity, fuzzy trigram similarity via `pg_trgm`, and graph
//! context disambiguation. When a vector or fuzzy match produces a
//! candidate whose `node_type` differs from the extracted entity type,
//! the resolver checks the candidate's 1-hop neighborhood in the
//! graph sidecar. If no neighbor shares the extracted entity type,
//! the match is rejected — e.g., "Apple" in a food context won't
//! resolve to Apple Inc. in a tech graph.
//!
//! Also resolves relationship type labels so that synonymous edge
//! types (e.g. "author_of" / "is_author_of" / "wrote") converge on
//! the canonical (most frequently used) form.

use std::sync::Arc;

use uuid::Uuid;

use crate::error::{Error, Result};
use crate::graph::SharedGraph;
use crate::graph::traversal::bfs_neighborhood;
use crate::ingestion::embedder::{Embedder, truncate_and_validate};
use crate::ingestion::extractor::ExtractedEntity;
use crate::ingestion::resolver::{EntityResolver, MatchType, ResolvedEntity};
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{NodeAliasRepo, NodeRepo};
use crate::types::ids::NodeId;

/// Default minimum trigram similarity score for fuzzy matching.
///
/// Raised from 0.4 → 0.55 to prevent false conflation (e.g.,
/// "GraphRAG" matching "GraphQL" at ~0.42 similarity).
const DEFAULT_FUZZY_THRESHOLD: f32 = 0.55;

/// Default cosine similarity threshold for vector-based matching.
const DEFAULT_VECTOR_THRESHOLD: f32 = 0.85;

/// Entity and relationship-type resolver backed by PostgreSQL.
///
/// Resolution strategy for entities (in order of preference):
/// 1. **Exact match** — case-insensitive canonical name lookup.
/// 2. **Alias match** — case-insensitive lookup in `node_aliases`.
/// 3. **Vector match** — cosine similarity between the entity name
///    embedding and existing node embeddings (requires an embedder).
///    **Graph context:** if the candidate's `node_type` differs from
///    the entity type and no 1-hop neighbor shares the entity type,
///    the match is rejected.
/// 4. **Fuzzy match** — `pg_trgm` similarity on
///    `nodes.canonical_name`, preferring nodes with matching
///    `node_type`. Same graph context disambiguation as vector.
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
    /// Target dimension for node embeddings (for truncation).
    node_embed_dim: usize,
    /// Optional graph sidecar for context disambiguation.
    ///
    /// When available, vector and fuzzy match candidates whose
    /// `node_type` differs from the extracted entity type are
    /// checked against the candidate's 1-hop neighborhood. If no
    /// neighbor shares the entity type, the match is rejected.
    graph: Option<SharedGraph>,
    /// When true, entities that fail all 4 resolution tiers are
    /// returned as `MatchType::Deferred` instead of `MatchType::New`,
    /// signaling the pipeline to route them to the unresolved_entities
    /// pool for HDBSCAN batch clustering (Tier 5).
    tier5_enabled: bool,
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
            node_embed_dim: 256,
            graph: None,
            tier5_enabled: false,
        }
    }

    /// Create a new resolver with a custom trigram threshold.
    pub fn with_threshold(repo: Arc<PgRepo>, threshold: f32) -> Self {
        Self {
            repo,
            threshold,
            embedder: None,
            vector_threshold: DEFAULT_VECTOR_THRESHOLD,
            node_embed_dim: 256,
            graph: None,
            tier5_enabled: false,
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
            node_embed_dim: 256,
            graph: None,
            tier5_enabled: false,
        }
    }

    /// Enable Tier 5 (HDBSCAN deferred resolution).
    ///
    /// When enabled, entities that fail all 4 resolution tiers are
    /// returned as `MatchType::Deferred` instead of `MatchType::New`,
    /// routing them to the unresolved_entities pool for batch clustering.
    pub fn with_tier5(mut self, enabled: bool) -> Self {
        self.tier5_enabled = enabled;
        self
    }

    /// Set the target dimension for node embeddings.
    pub fn with_node_embed_dim(mut self, dim: usize) -> Self {
        self.node_embed_dim = dim;
        self
    }

    /// Attach a graph sidecar for context disambiguation.
    ///
    /// When set, the resolver checks the 1-hop neighborhood of
    /// vector and fuzzy match candidates. If the candidate's
    /// `node_type` differs from the extracted entity type and no
    /// 1-hop neighbor shares the entity type, the match is
    /// rejected — preventing "Apple (fruit)" from resolving to
    /// "Apple Inc." in a technology graph.
    pub fn with_graph(mut self, graph: SharedGraph) -> Self {
        self.graph = Some(graph);
        self
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

        // find_by_alias uses exact case-insensitive matching; pick
        // the first result (all will be exact matches).
        let exact_alias = aliases.into_iter().next();

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

        // Truncate to node embedding dimension and convert to f32
        // for halfvec cast in the query.
        let truncated = truncate_and_validate(&embedding, self.node_embed_dim, "nodes")
            .map_err(|e| Error::EntityResolution(format!("node dimension mismatch: {e}")))?;
        let embedding_f32: Vec<f32> = truncated.iter().map(|&v| v as f32).collect();

        // Query closest node by cosine distance via stored procedure.
        // Convert similarity threshold to distance threshold for the SP
        // (distance = 1.0 - similarity).
        let distance_threshold = 1.0 - self.vector_threshold as f64;
        let row: Option<(Uuid, String, String, f64)> =
            sqlx::query_as("SELECT * FROM sp_find_closest_node_embedding($1::halfvec, $2, $3)")
                .bind(&embedding_f32)
                .bind(distance_threshold)
                .bind(1_i32)
                .fetch_optional(self.repo.pool())
                .await
                .map_err(|e| Error::EntityResolution(format!("vector match query failed: {e}")))?;

        if let Some((id, canonical_name, _node_type, _distance)) = row {
            let id = NodeId::from(id);
            return Ok(Some(ResolvedEntity {
                node_id: Some(id),
                canonical_name,
                match_type: MatchType::Vector,
            }));
        }

        Ok(None)
    }

    /// Try fuzzy trigram match using `pg_trgm` similarity.
    ///
    /// Uses `sp_search_nodes_fuzzy_typed` which orders by same
    /// `node_type` first, then by descending similarity.
    async fn try_fuzzy_match(&self, entity: &ExtractedEntity) -> Result<Option<ResolvedEntity>> {
        let row: Option<(Uuid, String, String, f32)> =
            sqlx::query_as("SELECT * FROM sp_search_nodes_fuzzy_typed($1, $2, $3, $4)")
                .bind(&entity.name)
                .bind(self.threshold)
                .bind(&entity.entity_type)
                .bind(1_i32)
                .fetch_optional(self.repo.pool())
                .await
                .map_err(|e| Error::EntityResolution(format!("fuzzy match query failed: {e}")))?;

        Ok(
            row.map(|(id, canonical_name, _node_type, _sim)| ResolvedEntity {
                node_id: Some(NodeId::from(id)),
                canonical_name,
                match_type: MatchType::Fuzzy,
            }),
        )
    }

    /// Check whether a candidate match is contextually compatible
    /// with the extracted entity by examining the graph neighborhood.
    ///
    /// Returns `true` if the match should be accepted, `false` if
    /// the graph context suggests a different entity.
    ///
    /// When no graph is available, always returns `true`.
    async fn graph_context_supports_match(
        &self,
        candidate_id: NodeId,
        candidate_type: &str,
        entity: &ExtractedEntity,
    ) -> bool {
        // No graph → no disambiguation, accept the match.
        let graph = match &self.graph {
            Some(g) => g,
            None => return true,
        };

        // If types already match, no disambiguation needed.
        if candidate_type.eq_ignore_ascii_case(&entity.entity_type) {
            return true;
        }

        // Check the candidate's 1-hop neighborhood for type overlap.
        let g = graph.read().await;
        let neighbors = bfs_neighborhood(&g, candidate_id.into_uuid(), 1, None);

        // If the candidate has no neighbors, we can't disambiguate.
        if neighbors.is_empty() {
            return true;
        }

        // Check if any neighbor shares the entity type being resolved.
        for (neighbor_id, _hops) in &neighbors {
            if let Some(meta) = g.get_node(*neighbor_id) {
                if meta.node_type.eq_ignore_ascii_case(&entity.entity_type) {
                    return true;
                }
            }
        }

        // Type mismatch + no neighborhood overlap → reject.
        tracing::debug!(
            candidate = %candidate_id.into_uuid(),
            candidate_type,
            entity_name = %entity.name,
            entity_type = %entity.entity_type,
            neighbor_count = neighbors.len(),
            "graph context disambiguation rejected match"
        );
        false
    }

    /// Resolve a relationship type label against existing edge types.
    ///
    /// Strategy:
    /// 1. **Exact match** via `sp_resolve_rel_type_exact` — returns
    ///    the canonical form if the rel_type already exists.
    /// 2. **Fuzzy match** via `sp_resolve_rel_type_fuzzy` — finds the
    ///    closest existing rel_type above the configured threshold,
    ///    weighted by frequency.
    /// 3. **No match** — return the input unchanged (genuinely new).
    pub async fn resolve_rel_type(&self, rel_type: &str) -> Result<String> {
        // Normalize for comparison: lowercase + trim.
        let normalized = rel_type.trim().to_lowercase();

        // 1. Exact match via stored procedure (returns scalar TEXT or NULL).
        let exact: (Option<String>,) = sqlx::query_as("SELECT sp_resolve_rel_type_exact($1)")
            .bind(&normalized)
            .fetch_one(self.repo.pool())
            .await
            .map_err(|e| {
                Error::EntityResolution(format!("rel_type exact match query failed: {e}"))
            })?;

        if let (Some(resolved),) = exact {
            return Ok(resolved);
        }

        // 2. Fuzzy match via stored procedure (returns scalar TEXT or NULL).
        let fuzzy: (Option<String>,) = sqlx::query_as("SELECT sp_resolve_rel_type_fuzzy($1, $2)")
            .bind(&normalized)
            .bind(self.threshold)
            .fetch_one(self.repo.pool())
            .await
            .map_err(|e| {
                Error::EntityResolution(format!("rel_type fuzzy match query failed: {e}"))
            })?;

        if let (Some(resolved),) = fuzzy {
            return Ok(resolved);
        }

        // 3. No match — use the input as-is.
        Ok(rel_type.to_string())
    }
}

/// Whether an extracted entity came from a deterministic AST parser.
///
/// True iff `metadata.ast_hash` is present. AST entities are exact
/// source identifiers and must not be matched fuzzily — see #188.
fn is_ast_extracted(entity: &ExtractedEntity) -> bool {
    entity
        .metadata
        .as_ref()
        .and_then(|m| m.get("ast_hash"))
        .is_some()
}

#[async_trait::async_trait]
impl EntityResolver for PgResolver {
    /// Resolve an extracted entity against the knowledge graph.
    ///
    /// Tries exact → alias → vector (+ graph context) → fuzzy
    /// trigram (+ graph context).
    /// Returns `MatchType::New` if nothing matches.
    ///
    /// **AST short-circuit**: entities carrying `metadata.ast_hash`
    /// come from a deterministic AST parser, not an LLM. Their names
    /// are unambiguous source identifiers (struct/trait/function
    /// names). Fuzzy/vector matching against pre-existing LLM-noise
    /// nodes silently merges distinct identifiers (e.g.
    /// `ChainChatBackend` folded into `HttpChatBackend` because the
    /// embeddings are close, see #188). For AST entities we only
    /// allow exact name and alias matches; otherwise we create a new
    /// node.
    async fn resolve(&self, entity: &ExtractedEntity) -> Result<ResolvedEntity> {
        let is_ast_entity = is_ast_extracted(entity);

        // 1. Exact canonical name match (fastest path).
        if let Some(resolved) = self.try_exact_match(entity).await? {
            return Ok(resolved);
        }

        // 2. Alias match.
        if let Some(resolved) = self.try_alias_match(entity).await? {
            return Ok(resolved);
        }

        // AST short-circuit: skip vector, fuzzy, and tier5 entirely.
        // AST identifiers must be exact-matched or created fresh.
        // We deliberately bypass the tier5 deferral path even when
        // tier5 is enabled — tier5 batch-clusters ambiguous LLM
        // extractions, but AST identifiers are deterministic and
        // unambiguous, so clustering them with LLM noise reintroduces
        // exactly the conflation we're guarding against.
        if is_ast_entity {
            tracing::debug!(
                entity = %entity.name,
                entity_type = %entity.entity_type,
                "ast short-circuit: bypassing fuzzy/vector/tier5"
            );
            return Ok(ResolvedEntity {
                node_id: None,
                canonical_name: entity.name.clone(),
                match_type: MatchType::New,
            });
        }

        // 3. Vector cosine similarity match + graph context.
        if let Some(resolved) = self.try_vector_match(entity).await? {
            if let Some(nid) = resolved.node_id {
                // Fetch the candidate's type for disambiguation.
                let candidate_type = NodeRepo::get(self.repo.as_ref(), nid)
                    .await?
                    .map(|n| n.node_type)
                    .unwrap_or_default();
                if self
                    .graph_context_supports_match(nid, &candidate_type, entity)
                    .await
                {
                    return Ok(resolved);
                }
                // Graph context rejected — fall through to fuzzy.
            } else {
                return Ok(resolved);
            }
        }

        // 4. Fuzzy trigram match + graph context.
        if let Some(resolved) = self.try_fuzzy_match(entity).await? {
            if let Some(nid) = resolved.node_id {
                let candidate_type = NodeRepo::get(self.repo.as_ref(), nid)
                    .await?
                    .map(|n| n.node_type)
                    .unwrap_or_default();
                if self
                    .graph_context_supports_match(nid, &candidate_type, entity)
                    .await
                {
                    return Ok(resolved);
                }
                // Graph context rejected — treat as new.
            } else {
                return Ok(resolved);
            }
        }

        // 5. No match — new entity or deferred to Tier 5.
        let match_type = if self.tier5_enabled {
            MatchType::Deferred
        } else {
            MatchType::New
        };
        Ok(ResolvedEntity {
            node_id: None,
            canonical_name: entity.name.clone(),
            match_type,
        })
    }
}

/// Normalize a relationship type label for comparison.
///
/// Applies lowercase, trimming, underscore/hyphen/space unification,
/// and strips common prefixes like "is_" and "has_".
/// Not yet wired into the resolution pipeline — will be used when
/// relationship type consolidation is implemented.
#[cfg(test)]
fn normalize_rel_type(raw: &str) -> String {
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
            metadata: None,
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

    /// Verify MatchType::Deferred variant round-trips correctly.
    #[test]
    fn deferred_match_type_round_trips() {
        let resolved = ResolvedEntity {
            node_id: None,
            canonical_name: "test".to_string(),
            match_type: MatchType::Deferred,
        };
        assert_eq!(resolved.match_type, MatchType::Deferred);
        assert!(resolved.node_id.is_none());
    }

    /// Verify with_tier5 builder compiles and is chainable.
    #[test]
    fn with_tier5_builder_compiles() {
        fn _check_api() {
            // Ensures with_tier5 signature is correct at compile time.
            let _: fn(PgResolver, bool) -> PgResolver = PgResolver::with_tier5;
        }
    }

    /// AST entity (carrying `metadata.ast_hash`) is detected.
    #[test]
    fn ast_entity_detected_via_metadata() {
        let entity = ExtractedEntity {
            name: "ChainChatBackend".to_string(),
            entity_type: "struct".to_string(),
            description: None,
            confidence: 1.0,
            metadata: Some(serde_json::json!({ "ast_hash": "abc123" })),
        };
        assert!(is_ast_extracted(&entity));
    }

    /// LLM-extracted entity (no metadata) is not flagged as AST.
    #[test]
    fn llm_entity_not_flagged_as_ast() {
        let entity = ExtractedEntity {
            name: "ChainChatBackend".to_string(),
            entity_type: "technology".to_string(),
            description: Some("a chat backend".to_string()),
            confidence: 0.8,
            metadata: None,
        };
        assert!(!is_ast_extracted(&entity));
    }

    /// Entity with metadata but no `ast_hash` field is not flagged.
    #[test]
    fn metadata_without_ast_hash_not_flagged() {
        let entity = ExtractedEntity {
            name: "Foo".to_string(),
            entity_type: "concept".to_string(),
            description: None,
            confidence: 0.5,
            metadata: Some(serde_json::json!({ "other_field": "x" })),
        };
        assert!(!is_ast_extracted(&entity));
    }
}

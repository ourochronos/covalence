//! Search service — multi-dimensional fused search orchestration.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use petgraph::visit::EdgeRef;

use crate::config::TableDimensions;
use crate::error::Result;
use crate::graph::SharedGraph;
use crate::ingestion::embedder::Embedder;
use crate::search::abstention::{AbstentionConfig, check_abstention};
use crate::search::cache::{CacheConfig, QueryCache};
use crate::search::context::{AssembledContext, ContextConfig, RawContextItem, assemble_context};
use crate::search::dimensions::{
    GlobalDimension, GraphDimension, LexicalDimension, SearchDimension, SearchQuery,
    StructuralDimension, TemporalDimension, VectorDimension,
};
use crate::search::expansion::spreading_activation;
use crate::search::fusion::{self, FusedResult, RelatedEntity};
use crate::search::rerank::{NoopReranker, Reranker};
use crate::search::skewroute::{detect_intent, select_strategy};
use crate::search::strategy::SearchStrategy;
use crate::search::trace::QueryTrace;
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{ArticleRepo, ChunkRepo, NodeRepo, SourceRepo};
use crate::types::ids::{ArticleId, ChunkId, NodeId};

use super::search_helpers::{
    ENTITY_DEMOTION_FACTOR, derive_chunk_name_qualified, kwic_snippet, strategy_name,
    truncate_with_ellipsis,
};

/// Post-fusion filters for narrowing search results.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchFilters {
    /// Minimum epistemic confidence (projected probability).
    pub min_confidence: Option<f64>,
    /// Restrict to specific node types.
    pub node_types: Option<Vec<String>>,
    /// Restrict to a temporal date range.
    pub date_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
    /// Restrict to specific source types (e.g. "document", "code").
    /// Applies only to chunk and source results.
    pub source_types: Option<Vec<String>>,
    /// Restrict to specific source layers derived from source URI.
    /// Layers: "spec", "design", "code", "research".
    /// Applies only to chunk and source results.
    pub source_layers: Option<Vec<String>>,
}

/// Derive a source layer from a source URI.
///
/// Layers classify sources by their role in the project:
/// - `"spec"` — specification documents (`file://spec/`)
/// - `"design"` — architecture decision records (`file://docs/adr/`)
/// - `"code"` — source code (`file://engine/` or `file://cli/`)
/// - `"research"` — external research (HTTP/HTTPS URLs)
///
/// Returns `None` if the URI doesn't match a known pattern.
pub fn source_layer_from_uri(uri: &str) -> Option<&'static str> {
    if uri.starts_with("file://spec/") {
        Some("spec")
    } else if uri.starts_with("file://docs/adr/") {
        Some("design")
    } else if uri.starts_with("file://engine/") || uri.starts_with("file://cli/") {
        Some("code")
    } else if uri.starts_with("http://") || uri.starts_with("https://") {
        Some("research")
    } else {
        None
    }
}

/// Service for orchestrating multi-dimensional search and RRF fusion.
///
/// Integrates the full search pipeline: cache lookup, adaptive
/// strategy selection, dimension search, RRF fusion, abstention
/// detection, query expansion, reranking, and trace recording.
pub struct SearchService {
    repo: Arc<PgRepo>,
    embedder: Option<Arc<dyn Embedder>>,
    graph: SharedGraph,
    vector: VectorDimension,
    lexical: LexicalDimension,
    temporal: TemporalDimension,
    graph_dim: GraphDimension,
    structural: StructuralDimension,
    global: GlobalDimension,
    reranker: Arc<dyn Reranker>,
    cache: Option<QueryCache>,
    abstention_config: AbstentionConfig,
    /// Use Convex Combination fusion instead of RRF.
    /// CC preserves score magnitude; RRF uses only rank.
    use_cc_fusion: bool,
}

impl SearchService {
    /// Create a new search service with default table dimensions.
    pub fn new(repo: Arc<PgRepo>, graph: SharedGraph) -> Self {
        Self::with_embedder(repo, graph, None)
    }

    /// Create a new search service with an optional embedder for
    /// vector search.
    pub fn with_embedder(
        repo: Arc<PgRepo>,
        graph: SharedGraph,
        embedder: Option<Arc<dyn Embedder>>,
    ) -> Self {
        Self::with_config(repo, graph, embedder, TableDimensions::default())
    }

    /// Create a new search service with per-table embedding
    /// dimensions.
    pub fn with_config(
        repo: Arc<PgRepo>,
        graph: SharedGraph,
        embedder: Option<Arc<dyn Embedder>>,
        table_dims: TableDimensions,
    ) -> Self {
        let pool = repo.pool().clone();
        let graph_clone = Arc::clone(&graph);
        Self {
            repo,
            embedder,
            vector: VectorDimension::new(pool.clone(), table_dims.clone()),
            lexical: LexicalDimension::new(pool.clone()),
            temporal: TemporalDimension::new(pool.clone()),
            graph_dim: GraphDimension::new(Arc::clone(&graph)),
            structural: StructuralDimension::new(Arc::clone(&graph)),
            global: GlobalDimension::new(pool.clone(), table_dims),
            graph: graph_clone,
            reranker: Arc::new(NoopReranker),
            cache: None,
            abstention_config: AbstentionConfig::default(),
            use_cc_fusion: true,
        }
    }

    /// Use Convex Combination fusion instead of RRF.
    ///
    /// CC preserves score magnitude information that RRF discards
    /// by normalizing scores within each dimension to \[0, 1\] and
    /// computing a weighted sum. Bruch et al. (2210.11934) showed
    /// CC consistently outperforms RRF in hybrid retrieval.
    pub fn with_cc_fusion(mut self, enabled: bool) -> Self {
        self.use_cc_fusion = enabled;
        self
    }

    /// Enable the semantic query cache with the given config.
    ///
    /// When enabled, semantically similar queries within the TTL
    /// window will return cached results instead of re-executing
    /// the full search pipeline.
    pub fn with_cache(mut self, config: CacheConfig) -> Self {
        let pool = self.repo.pool().clone();
        self.cache = Some(QueryCache::new(pool, config));
        self
    }

    /// Set a custom reranker implementation.
    ///
    /// By default, `NoopReranker` is used which preserves the
    /// original RRF ordering. Provide a cross-encoder reranker
    /// (e.g. Voyage rerank-2.5) for improved result relevance.
    pub fn with_reranker(mut self, reranker: Arc<dyn Reranker>) -> Self {
        self.reranker = reranker;
        self
    }

    /// Set a custom abstention config.
    pub fn with_abstention_config(mut self, config: AbstentionConfig) -> Self {
        self.abstention_config = config;
        self
    }

    /// Clear the semantic query cache. Returns the number of entries
    /// removed, or 0 if the cache is not enabled.
    pub async fn clear_cache(&self) -> Result<u64> {
        match &self.cache {
            Some(cache) => cache.clear().await,
            None => Ok(0),
        }
    }

    /// Execute a fused search across all dimensions.
    ///
    /// Pipeline:
    /// 1. Check semantic query cache (if enabled)
    /// 2. Embed the query for vector search
    /// 3. Auto-select strategy if not specified (SkewRoute)
    /// 4. Run all 6 dimensions concurrently
    /// 5. RRF fusion
    /// 6. Abstention detection
    /// 7. Query expansion (spreading activation from top-K)
    /// 8. Enrichment (node/article/chunk metadata)
    /// 9. Reranking
    /// 10. Post-fusion filtering and truncation
    /// 11. Cache population + trace recording
    pub async fn search(
        &self,
        query: &str,
        strategy: SearchStrategy,
        limit: usize,
        filters: Option<SearchFilters>,
    ) -> Result<Vec<FusedResult>> {
        let start = Instant::now();
        let time_range = filters.as_ref().and_then(|f| f.date_range);

        // --- Step 1: Embed the query ---
        let query_embedding = if let Some(ref embedder) = self.embedder {
            match embedder.embed(&[query.to_string()]).await {
                Ok(mut vecs) if !vecs.is_empty() => Some(vecs.remove(0)),
                Ok(_) => None,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "failed to embed query, \
                         vector search disabled"
                    );
                    None
                }
            }
        } else {
            None
        };

        tracing::debug!(
            has_embedding = query_embedding.is_some(),
            embedding_dim = query_embedding.as_ref().map(|e| e.len()),
            "query embedding status"
        );

        // --- Step 2: Cache lookup ---
        if let (Some(cache), Some(emb)) = (&self.cache, &query_embedding) {
            let strategy_str = strategy_name(&strategy);
            match cache.lookup(emb, strategy_str).await {
                Ok(Some(cached_results)) => {
                    tracing::debug!("cache hit for query");
                    let mut trace = QueryTrace::new(query, &strategy);
                    trace.cache_hit = true;
                    trace.final_count = cached_results.len();
                    trace.set_duration(start.elapsed());
                    trace.emit();
                    return Ok(cached_results);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "cache lookup failed, proceeding \
                         without cache"
                    );
                }
            }
        }

        // --- Step 3: Adaptive strategy selection ---
        let effective_strategy = if strategy == SearchStrategy::Auto {
            // First, check for keyword-based intent signals.
            // These are strong indicators that override
            // score-based selection (e.g., "latest" → Recent).
            if let Some(intent_strategy) = detect_intent(query) {
                tracing::debug!(?intent_strategy, "intent detection selected strategy");
                intent_strategy
            } else if let Some(ref emb) = query_embedding {
                // Fall back to SkewRoute score-based selection.
                let probe_query = SearchQuery {
                    text: query.to_string(),
                    strategy: SearchStrategy::Balanced,
                    limit: 20,
                    time_range,
                    embedding: Some(emb.clone()),
                    ..SearchQuery::default()
                };
                match self.vector.search(&probe_query).await {
                    Ok(results) => {
                        let scores: Vec<f64> = results.iter().map(|r| r.score).collect();
                        let mut selected = select_strategy(&scores);
                        // Guard: Global strategy relies on
                        // community summaries (articles). If
                        // none exist, fall back to Exploratory
                        // which is the right choice for broad
                        // queries (graph-heavy traversal).
                        if selected == SearchStrategy::Global {
                            let has_articles = self
                                .global
                                .search(&probe_query)
                                .await
                                .is_ok_and(|r| !r.is_empty());
                            if !has_articles {
                                selected = SearchStrategy::Exploratory;
                                tracing::debug!(
                                    "skewroute: Global → \
                                     Exploratory (no articles)"
                                );
                            }
                        }
                        if selected != SearchStrategy::Balanced {
                            tracing::debug!(
                                ?selected,
                                "skewroute auto-selected \
                                     strategy"
                            );
                        }
                        selected
                    }
                    Err(_) => strategy.clone(),
                }
            } else {
                strategy.clone()
            }
        } else {
            strategy.clone()
        };

        // --- Step 4: Run all 6 dimensions concurrently ---
        let search_query = SearchQuery {
            text: query.to_string(),
            strategy: effective_strategy.clone(),
            limit,
            time_range,
            embedding: query_embedding.clone(),
            ..SearchQuery::default()
        };

        let (vec_r, lex_r, tmp_r, grp_r, str_r, glb_r) = tokio::join!(
            self.vector.search(&search_query),
            self.lexical.search(&search_query),
            self.temporal.search(&search_query),
            self.graph_dim.search(&search_query),
            self.structural.search(&search_query),
            self.global.search(&search_query),
        );

        // --- Step 5: Collect and fuse ---
        let mut trace = QueryTrace::new(query, &effective_strategy);

        let dimensions: [(&str, std::result::Result<Vec<fusion::SearchResult>, _>, f64); 6] = {
            let w = effective_strategy.weights();
            [
                ("vector", vec_r, w.vector),
                ("lexical", lex_r, w.lexical),
                ("temporal", tmp_r, w.temporal),
                ("graph", grp_r, w.graph),
                ("structural", str_r, w.structural),
                ("global", glb_r, w.global),
            ]
        };

        // Entity demotion: for content-focused strategies, push bare
        // entity nodes to the end of each dimension's ranked list
        // BEFORE fusion. This prevents nodes from crowding out
        // chunks in high-weight dimensions like vector search.
        let demote_entities = !matches!(
            effective_strategy,
            SearchStrategy::Exploratory | SearchStrategy::GraphFirst | SearchStrategy::Global
        );

        let mut ranked_lists = Vec::new();
        let mut weights = Vec::new();
        let mut snippets: HashMap<Uuid, String> = HashMap::new();
        let mut result_types: HashMap<Uuid, String> = HashMap::new();

        for (name, result, weight) in dimensions {
            match result {
                Ok(mut results) => {
                    tracing::debug!(
                        dimension = name,
                        count = results.len(),
                        weight,
                        "search dimension returned results"
                    );
                    trace.record_dimension(name, results.len());
                    for r in &results {
                        if let Some(s) = &r.snippet {
                            snippets.entry(r.id).or_insert_with(|| s.clone());
                        }
                        if let Some(rt) = &r.result_type {
                            result_types.entry(r.id).or_insert_with(|| rt.clone());
                        }
                    }

                    // Per-dimension entity demotion: move bare entity
                    // nodes to the back of the ranked list so they
                    // get worse RRF ranks. This is more principled
                    // than post-fusion score multiplication because
                    // it affects the rank input to RRF directly.
                    if demote_entities && !results.is_empty() {
                        let (mut content, entities): (Vec<_>, Vec<_>) = results
                            .drain(..)
                            .partition(|r| r.result_type.as_deref() != Some("node"));
                        // Re-rank: content items keep their positions,
                        // entity nodes follow at the end.
                        content.extend(entities);
                        for (i, r) in content.iter_mut().enumerate() {
                            r.rank = i + 1;
                        }
                        results = content;
                    }

                    ranked_lists.push(results);
                    weights.push(weight);
                }
                Err(e) => {
                    tracing::warn!(
                        dimension = name,
                        error = %e,
                        "search dimension failed, skipping"
                    );
                    trace.record_dimension(name, 0);
                }
            }
        }

        // --- Step 5b: Per-dimension quality gating ---
        // A non-discriminating dimension (where all results have
        // nearly identical scores) adds noise to fusion without
        // providing ranking signal. Reduce its weight proportionally
        // to its discrimination power (score spread).
        // Based on: "Balancing the Blend" (2508.01405) — a weak
        // retrieval path substantially degrades fused accuracy.
        let mut dampened_weight = 0.0_f64;
        for (list, weight) in ranked_lists.iter().zip(weights.iter_mut()) {
            if list.len() < 2 {
                continue;
            }
            let max_score = list
                .iter()
                .map(|r| r.score)
                .fold(f64::NEG_INFINITY, f64::max);
            let min_score = list.iter().map(|r| r.score).fold(f64::INFINITY, f64::min);
            let spread = if max_score > 0.0 {
                (max_score - min_score) / max_score
            } else {
                0.0
            };
            // If spread < 0.05 (less than 5% relative difference
            // between best and worst), the dimension isn't
            // discriminating. Scale its weight by the spread ratio.
            if spread < 0.05 {
                let dampening = spread / 0.05;
                let original = *weight;
                *weight *= dampening;
                dampened_weight += original - *weight;
                tracing::debug!(
                    spread,
                    dampening,
                    original_weight = original,
                    new_weight = *weight,
                    "quality gate dampened non-discriminating dimension"
                );
            }
        }

        // --- Step 5b2: Clear results from zero-weight dimensions ---
        // Dimensions dampened to zero weight would contribute nothing
        // to fusion scores, but their results would still enter the
        // fused result set (with score 0.0) and inflate the reranker's
        // result pool.  Clear them so only meaningful results survive.
        for (list, weight) in ranked_lists.iter_mut().zip(weights.iter()) {
            if *weight < 1e-12 && !list.is_empty() {
                tracing::debug!(
                    cleared = list.len(),
                    "cleared results from zero-weight dimension"
                );
                list.clear();
            }
        }

        // --- Step 5c: Redistribute weight from empty/dampened dims ---
        // Weight from empty dimensions (no results) and dampened
        // dimensions (quality-gated to near-zero) is redistributed
        // proportionally to active dimensions so effective weights
        // still sum to the strategy's intended total.
        let empty_weight: f64 = ranked_lists
            .iter()
            .zip(weights.iter())
            .filter(|(list, _)| list.is_empty())
            .map(|(_, &w)| w)
            .sum();
        let total_redistribute = empty_weight + dampened_weight;
        if total_redistribute > 0.0 {
            let active_weight: f64 = weights
                .iter()
                .zip(ranked_lists.iter())
                .filter(|(_, list)| !list.is_empty())
                .map(|(&w, _)| w)
                .sum();
            if active_weight > 0.0 {
                for (w, list) in weights.iter_mut().zip(ranked_lists.iter()) {
                    if !list.is_empty() {
                        *w += total_redistribute * (*w / active_weight);
                    }
                }
                tracing::debug!(
                    redistributed = total_redistribute,
                    from_empty = empty_weight,
                    from_dampened = dampened_weight,
                    active_dimensions = ranked_lists.iter().filter(|l| !l.is_empty()).count(),
                    "redistributed weight from empty/dampened dimensions"
                );
            }
        }

        // Log effective weights and top-1 item per dimension for
        // diagnosing score anomalies.
        let dim_names = [
            "vector",
            "lexical",
            "temporal",
            "graph",
            "structural",
            "global",
        ];
        let weight_summary: String = dim_names
            .iter()
            .zip(weights.iter())
            .filter(|(_, w)| **w > 1e-6)
            .map(|(name, w)| {
                let abbr = &name[..name.len().min(3)];
                format!("{abbr}={w:.3}")
            })
            .collect::<Vec<_>>()
            .join(" ");
        tracing::debug!(weights = %weight_summary, "effective fusion weights");

        let mut fused = if self.use_cc_fusion {
            fusion::cc_fuse(&ranked_lists, &weights)
        } else {
            fusion::rrf_fuse(&ranked_lists, &weights, fusion::DEFAULT_K)
        };
        trace.fused_count = fused.len();

        // --- Step 6: Abstention detection ---
        let scores: Vec<f64> = fused.iter().map(|r| r.fused_score).collect();
        let abstention_check = check_abstention(&scores, &self.abstention_config);
        if abstention_check.should_abstain {
            trace.abstained = true;
            tracing::info!(
                reason = ?abstention_check.reason,
                "search abstention triggered"
            );
        }

        // --- Step 7: Query expansion (spreading activation) ---
        if !fused.is_empty() {
            let top_k = 5.min(fused.len());
            let seed_ids: Vec<Uuid> = fused[..top_k].iter().map(|r| r.id).collect();
            let spread = spreading_activation(&seed_ids, &self.graph, None).await;
            if !spread.expanded_ids.is_empty() {
                tracing::debug!(
                    expanded = spread.expanded_ids.len(),
                    seeds = spread.seeds_used,
                    "spreading activation found neighbors"
                );
                // Merge expanded IDs as low-score entries if
                // they aren't already in fused results.
                let existing: std::collections::HashSet<Uuid> =
                    fused.iter().map(|r| r.id).collect();
                for eid in &spread.expanded_ids {
                    if !existing.contains(eid) {
                        let w = spread.weights.get(eid).copied().unwrap_or(0.0);
                        // Use a small fraction of the lowest
                        // fused score scaled by activation
                        // weight so expanded items rank below
                        // direct hits.
                        let base = fused.last().map(|r| r.fused_score * 0.5).unwrap_or(0.01);
                        fused.push(FusedResult {
                            id: *eid,
                            fused_score: base * w.min(1.0),
                            confidence: None,
                            entity_type: None,
                            name: None,
                            snippet: None,
                            content: None,
                            source_uri: None,
                            source_title: None,
                            source_type: None,
                            result_type: None,
                            created_at: None,
                            dimension_scores: HashMap::new(),
                            dimension_ranks: HashMap::new(),
                            graph_context: None,
                        });
                    }
                }
                // Re-sort after expansion merge.
                fused.sort_by(|a, b| {
                    b.fused_score
                        .partial_cmp(&a.fused_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }

        // --- Step 8: Enrichment ---
        for result in &mut fused {
            result.snippet = snippets.remove(&result.id);
            let rtype = result_types.get(&result.id).map(|s| s.as_str());
            result.result_type = rtype.map(|s| s.to_string());

            match rtype {
                Some("node") => {
                    if let Ok(Some(node)) =
                        NodeRepo::get(&*self.repo, NodeId::from_uuid(result.id)).await
                    {
                        result.name = Some(node.canonical_name);
                        result.entity_type = Some(node.node_type);
                        result.content = node.description.clone();
                        result.confidence =
                            node.confidence_breakdown.map(|o| o.projected_probability());
                    }
                }
                Some("article") => {
                    if let Ok(Some(article)) =
                        ArticleRepo::get(&*self.repo, ArticleId::from_uuid(result.id)).await
                    {
                        result.name = Some(article.title);
                        result.entity_type = Some("article".to_string());
                        result.content = Some(article.body.clone());
                        result.confidence = article
                            .confidence_breakdown
                            .map(|o| o.projected_probability());
                        if result.snippet.is_none() {
                            result.snippet = Some(kwic_snippet(&article.body, query, 300));
                        }
                    }
                }
                Some("source") => {
                    if let Ok(Some(source)) = SourceRepo::get(
                        &*self.repo,
                        crate::types::ids::SourceId::from_uuid(result.id),
                    )
                    .await
                    {
                        result.name = source.title.clone();
                        result.entity_type = Some("source".to_string());
                        result.source_uri = source.uri;
                        result.source_title = source.title;
                        result.source_type = Some(source.source_type.clone());
                        result.created_at = Some(source.ingested_at.to_rfc3339());
                        // Use truncated raw content for snippet.
                        if let Some(ref raw) = source.raw_content {
                            result.content = Some(truncate_with_ellipsis(raw, 500));
                        }
                    }
                }
                _ => {
                    // Chunk or unknown — try chunk lookup for
                    // source_uri and content, then node lookup
                    // for compat.
                    if let Ok(Some(chunk)) =
                        ChunkRepo::get(&*self.repo, ChunkId::from_uuid(result.id)).await
                    {
                        result.content = Some(chunk.content.clone());
                        result.entity_type = Some("chunk".to_string());
                        // Look up source first so we can qualify
                        // generic chunk headings with the source title.
                        let src_title = if let Ok(Some(source)) =
                            SourceRepo::get(&*self.repo, chunk.source_id).await
                        {
                            result.source_uri = source.uri;
                            result.source_title = source.title.clone();
                            result.source_type = Some(source.source_type.clone());
                            result.created_at = Some(source.ingested_at.to_rfc3339());
                            source.title
                        } else {
                            None
                        };
                        result.name = Some(derive_chunk_name_qualified(
                            &chunk.content,
                            src_title.as_deref(),
                        ));
                        // Content-based snippet fallback: if no lexical
                        // snippet exists, extract a keyword-in-context
                        // window around query terms. Falls back to the
                        // first 200 chars if no terms match.
                        if result.snippet.is_none() {
                            result.snippet = Some(kwic_snippet(&chunk.content, query, 200));
                        }
                        // Parent-context injection: for paragraph-level
                        // chunks, prepend truncated parent content to
                        // the snippet (avoids a second chunk fetch).
                        if chunk.level == "paragraph" {
                            if let Some(parent_id) = chunk.parent_chunk_id {
                                if let Ok(Some(parent)) =
                                    ChunkRepo::get(&*self.repo, parent_id).await
                                {
                                    let parent_ctx = truncate_with_ellipsis(&parent.content, 200);
                                    let prefix = format!("[{}: {}]", parent.level, parent_ctx);
                                    result.snippet = Some(match result.snippet.take() {
                                        Some(s) => format!("{} {}", prefix, s),
                                        None => prefix,
                                    });
                                }
                            }
                        }
                    }
                    // If still no entity_type, try source lookup
                    // (vector dimension produces source results
                    // but result_type may not propagate through
                    // fusion).
                    if result.entity_type.is_none() {
                        if let Ok(Some(source)) = SourceRepo::get(
                            &*self.repo,
                            crate::types::ids::SourceId::from_uuid(result.id),
                        )
                        .await
                        {
                            result.name = source.title.clone();
                            result.entity_type = Some("source".to_string());
                            result.source_uri = source.uri;
                            result.source_title = source.title;
                            result.source_type = Some(source.source_type.clone());
                            result.created_at = Some(source.ingested_at.to_rfc3339());
                            if let Some(ref raw) = source.raw_content {
                                result.content = Some(truncate_with_ellipsis(raw, 500));
                            }
                        }
                    }
                    if let Ok(Some(node)) =
                        NodeRepo::get(&*self.repo, NodeId::from_uuid(result.id)).await
                    {
                        result.name = Some(node.canonical_name);
                        result.entity_type = Some(node.node_type);
                        // Only set content from node if not
                        // already set from chunk.
                        if result.content.is_none() {
                            result.content = node.description.clone();
                        }
                        result.confidence =
                            node.confidence_breakdown.map(|o| o.projected_probability());
                    }
                }
            }
        }

        // --- Step 8a: Graph context enrichment ---
        // For node-type results, attach 1-hop graph neighbors to
        // provide relationship context. Uses the in-memory sidecar
        // (no DB queries) so this is fast.
        {
            const MAX_NEIGHBORS: usize = 10;
            let graph = self.graph.read().await;
            for result in &mut fused {
                if result
                    .entity_type
                    .as_deref()
                    .is_none_or(|t| t == "chunk" || t == "article" || t == "source")
                {
                    continue;
                }
                let Some(idx) = graph.node_index(result.id) else {
                    continue;
                };
                let mut related = Vec::new();
                // Outgoing edges
                for edge in graph.graph().edges(idx) {
                    let target = &graph.graph()[edge.target()];
                    let meta = edge.weight();
                    if meta.is_synthetic {
                        continue;
                    }
                    related.push(RelatedEntity {
                        name: target.canonical_name.clone(),
                        rel_type: meta.rel_type.clone(),
                        direction: "outgoing".to_string(),
                    });
                    if related.len() >= MAX_NEIGHBORS {
                        break;
                    }
                }
                // Incoming edges (if room)
                if related.len() < MAX_NEIGHBORS {
                    for edge in graph
                        .graph()
                        .edges_directed(idx, petgraph::Direction::Incoming)
                    {
                        let source = &graph.graph()[edge.source()];
                        let meta = edge.weight();
                        if meta.is_synthetic {
                            continue;
                        }
                        related.push(RelatedEntity {
                            name: source.canonical_name.clone(),
                            rel_type: meta.rel_type.clone(),
                            direction: "incoming".to_string(),
                        });
                        if related.len() >= MAX_NEIGHBORS {
                            break;
                        }
                    }
                }
                if !related.is_empty() {
                    result.graph_context = Some(related);
                }
            }
        }

        // --- Step 8b: Post-fusion entity demotion ---
        // Secondary demotion pass: entity nodes that appear in many
        // dimensions can accumulate high fused scores. Apply a score
        // multiplier that scales with dimension evidence: nodes found
        // by 3+ dimensions get lighter demotion (they're clearly
        // relevant), while single-dimension nodes get full demotion.
        //
        // Exception: entities whose name appears in the query text
        // are exempt from demotion — the user is likely asking about
        // that entity specifically.
        let query_lower = query.to_lowercase();
        if demote_entities {
            let mut demoted_count = 0usize;
            for result in &mut fused {
                let is_bare_entity = result.result_type.as_deref() == Some("node")
                    && result
                        .entity_type
                        .as_deref()
                        .is_none_or(|t| t != "community_summary" && t != "article");
                if is_bare_entity {
                    // Skip demotion if the entity name appears in
                    // the query (case-insensitive).
                    let name_in_query = result.name.as_ref().is_some_and(|name| {
                        let name_lower = name.to_lowercase();
                        name_lower.len() >= 3 && query_lower.contains(&name_lower)
                    });
                    if name_in_query {
                        continue;
                    }

                    let num_dims = result.dimension_scores.len();
                    // Scale demotion: 1 dim → 0.3, 2 → 0.5, 3+ → 0.7
                    let factor = match num_dims {
                        0 | 1 => ENTITY_DEMOTION_FACTOR,
                        2 => 0.5,
                        _ => 0.7,
                    };
                    result.fused_score *= factor;
                    demoted_count += 1;
                }
            }
            if demoted_count > 0 {
                tracing::debug!(
                    demoted_count,
                    factor = ENTITY_DEMOTION_FACTOR,
                    "demoted bare entity nodes in search results"
                );
                trace.entities_demoted = demoted_count;
                // Re-sort after demotion.
                fused.sort_by(|a, b| {
                    b.fused_score
                        .partial_cmp(&a.fused_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }

        // --- Step 9: Reranking ---
        // Build documents for the reranker. Prefer snippet, then
        // name, then truncated content. Vector-only chunk results
        // have no snippet or name — without content fallback they
        // would be reranked against empty strings and always lose.
        let documents: Vec<String> = fused
            .iter()
            .map(|r| {
                r.snippet
                    .clone()
                    .or_else(|| r.name.clone())
                    .or_else(|| r.content.as_ref().map(|c| truncate_with_ellipsis(c, 500)))
                    .unwrap_or_default()
            })
            .collect();

        if !documents.is_empty() && !self.reranker.is_noop() {
            match self.reranker.rerank(query, &documents).await {
                Ok(reranked) => {
                    // Blend reranker scores with fusion scores rather
                    // than letting the reranker completely override
                    // multi-dimensional evidence. The reranker provides
                    // text-level relevance; fusion provides multi-signal
                    // evidence.
                    let max_rerank = reranked
                        .iter()
                        .map(|r| r.relevance_score)
                        .fold(0.0f64, f64::max);
                    if max_rerank > 0.0 {
                        for rr in &reranked {
                            if rr.index < fused.len() {
                                let norm_score = rr.relevance_score / max_rerank;
                                // Blend: 60% fusion score + 40% reranker
                                fused[rr.index].fused_score =
                                    fused[rr.index].fused_score * 0.6 + norm_score * 0.4;
                            }
                        }
                        fused.sort_by(|a, b| {
                            b.fused_score
                                .partial_cmp(&a.fused_score)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "reranking failed, \
                         keeping fusion order"
                    );
                }
            }
        }

        // --- Step 9b: Low-quality chunk demotion ---
        // Runs AFTER reranking so the reranker can't blend demoted
        // scores back up. Catches bibliography entries, reference
        // sections, boilerplate, and metadata-only chunks.
        {
            use super::chunk_quality::{
                is_author_block, is_bibliography_entry, is_boilerplate_heavy, is_metadata_only,
                is_reference_section, is_title_only,
            };

            let mut demoted = 0usize;
            for result in &mut fused {
                if result.entity_type.as_deref() != Some("chunk") {
                    continue;
                }
                let content = match result.content.as_deref() {
                    Some(c) => c,
                    None => continue,
                };
                if is_bibliography_entry(content)
                    || is_reference_section(content)
                    || is_boilerplate_heavy(content)
                    || is_metadata_only(content)
                    || is_title_only(content)
                    || is_author_block(content)
                {
                    result.fused_score *= 0.1;
                    demoted += 1;
                }
            }
            if demoted > 0 {
                tracing::debug!(demoted, "demoted low-quality chunks after reranking");
                trace.chunks_quality_demoted = demoted;
                fused.sort_by(|a, b| {
                    b.fused_score
                        .partial_cmp(&a.fused_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }

        // --- Step 10: Post-fusion filters ---
        if let Some(ref f) = filters {
            if let Some(min_conf) = f.min_confidence {
                fused.retain(|r| r.confidence.is_some_and(|c| c >= min_conf));
            }
            if let Some(ref types) = f.node_types {
                fused.retain(|r| r.entity_type.as_ref().is_some_and(|t| types.contains(t)));
            }
            if let Some(ref types) = f.source_types {
                fused.retain(|r| {
                    // Pass through non-source/chunk results (nodes,
                    // articles) since they have no source_type.
                    r.source_type.as_ref().is_none_or(|st| types.contains(st))
                });
            }
            if let Some(ref layers) = f.source_layers {
                fused.retain(|r| {
                    // When filtering by layer, only keep results
                    // that positively match. Results without a
                    // source_uri or with an unrecognized URI are
                    // excluded — the user wants a specific layer.
                    r.source_uri
                        .as_ref()
                        .and_then(|uri| source_layer_from_uri(uri))
                        .is_some_and(|layer| layers.iter().any(|l| l == layer))
                });
            }
        }

        // --- Step 10b: Source diversification + content dedup ---
        // Hierarchical chunkers produce chunks at multiple levels
        // (source, section, paragraph) with overlapping content.
        // Without dedup, the same text appears multiple times.
        //
        // Three-pass approach:
        // 1. Content dedup (within source): same source, shared
        //    first 100 chars → keep highest-scored.
        // 2. Content dedup (cross source): different sources with
        //    identical content prefixes → keep highest-scored.
        //    This catches code files ingested multiple times.
        // 3. Source cap: max 2 chunks per source URI.
        const MAX_CHUNKS_PER_SOURCE: usize = 2;
        const CONTENT_PREFIX_LEN: usize = 100;
        {
            // Pass 1: content prefix dedup within same source.
            let mut seen_prefixes: HashMap<(String, String), usize> = HashMap::new();
            let pre_dedup = fused.len();
            fused.retain(|r| {
                if let (Some(uri), Some(content)) = (&r.source_uri, &r.content) {
                    let prefix_end = content
                        .char_indices()
                        .nth(CONTENT_PREFIX_LEN)
                        .map(|(i, _)| i)
                        .unwrap_or(content.len());
                    let prefix = content[..prefix_end].to_string();
                    let key = (uri.clone(), prefix);
                    let count = seen_prefixes.entry(key).or_insert(0);
                    *count += 1;
                    *count <= 1 // Keep only first occurrence
                } else {
                    true
                }
            });
            let deduped = pre_dedup - fused.len();

            // Pass 1b: cross-source content dedup. Same content
            // prefix from different sources (e.g., codebase
            // re-ingested multiple times) → keep first (highest
            // scored since results are sorted).
            let mut cross_prefixes: HashMap<String, usize> = HashMap::new();
            let pre_cross = fused.len();
            fused.retain(|r| {
                if r.result_type.as_deref() == Some("chunk") {
                    if let Some(content) = &r.content {
                        let prefix_end = content
                            .char_indices()
                            .nth(CONTENT_PREFIX_LEN)
                            .map(|(i, _)| i)
                            .unwrap_or(content.len());
                        let prefix = content[..prefix_end].to_string();
                        let count = cross_prefixes.entry(prefix).or_insert(0);
                        *count += 1;
                        *count <= 1
                    } else {
                        true
                    }
                } else {
                    true
                }
            });
            let cross_deduped = pre_cross - fused.len();

            // Pass 2: per-source count cap.
            let mut source_counts: HashMap<String, usize> = HashMap::new();
            let pre_diverse = fused.len();
            fused.retain(|r| {
                if let Some(ref uri) = r.source_uri {
                    let count = source_counts.entry(uri.clone()).or_insert(0);
                    *count += 1;
                    *count <= MAX_CHUNKS_PER_SOURCE
                } else {
                    true
                }
            });
            let capped = pre_diverse - fused.len();

            // Pass 3: article title dedup — different communities
            // can produce articles with the same title. Keep the
            // highest-scored instance.
            let mut seen_titles: HashMap<String, usize> = HashMap::new();
            let pre_title = fused.len();
            fused.retain(|r| {
                if r.entity_type.as_deref() == Some("article") {
                    if let Some(name) = &r.name {
                        let count = seen_titles.entry(name.clone()).or_insert(0);
                        *count += 1;
                        *count <= 1
                    } else {
                        true
                    }
                } else {
                    true
                }
            });
            let title_deduped = pre_title - fused.len();

            if deduped + cross_deduped + capped + title_deduped > 0 {
                tracing::debug!(
                    content_deduped = deduped,
                    cross_source_deduped = cross_deduped,
                    source_capped = capped,
                    article_title_deduped = title_deduped,
                    max_per_source = MAX_CHUNKS_PER_SOURCE,
                    "source diversification"
                );
            }
        }

        fused.truncate(limit);

        // --- Step 11: Cache population + trace ---
        for result in &fused {
            let rtype = result.result_type.as_deref().unwrap_or("unknown");
            trace.record_result_type(rtype);
        }
        trace.final_count = fused.len();
        trace.set_duration(start.elapsed());
        trace.emit();

        // Persist trace to DB for offline analysis.
        let db_trace = crate::models::trace::SearchTrace::new(
            trace.query_text.clone(),
            trace.strategy.clone(),
            serde_json::to_value(&trace.dimension_counts).unwrap_or_default(),
            trace.final_count as i32,
            trace.execution_ms as i32,
        );
        if let Err(e) =
            crate::storage::traits::SearchTraceRepo::create(&*self.repo, &db_trace).await
        {
            tracing::warn!(error = %e, "failed to persist search trace");
        }

        if let (Some(cache), Some(emb)) = (&self.cache, &query_embedding) {
            let strategy_str = strategy_name(&strategy);
            if let Err(e) = cache.store(query, emb, strategy_str, &fused).await {
                tracing::warn!(
                    error = %e,
                    "failed to store search results in cache"
                );
            }
        }

        Ok(fused)
    }

    /// Execute a search and return the full response including
    /// Assemble fused results into a context string suitable
    /// for LLM generation.
    ///
    /// Applies deduplication (cosine > 0.95), source diversity
    /// (max 3 per source), and token budget (8K default).
    /// Returns an `AssembledContext` with numbered references.
    pub async fn assemble_context(
        &self,
        results: &[FusedResult],
        config: Option<ContextConfig>,
    ) -> AssembledContext {
        let config = config.unwrap_or_default();

        let raw_items: Vec<RawContextItem> = results
            .iter()
            .map(|r| {
                let content = r
                    .content
                    .clone()
                    .or_else(|| r.snippet.clone())
                    .or_else(|| r.name.clone())
                    .unwrap_or_default();
                // Rough token estimate: ~4 chars per token.
                let token_count = content.len().div_ceil(4);
                RawContextItem {
                    content,
                    source_id: r.source_uri.clone().or_else(|| r.entity_type.clone()),
                    source_title: r.source_title.clone().or_else(|| r.name.clone()),
                    score: r.fused_score,
                    token_count,
                    embedding: None,
                    parent_context: None,
                }
            })
            .collect();

        assemble_context(raw_items, &config)
    }

    /// Format an assembled context into a single string with
    /// numbered references suitable for LLM prompts.
    pub fn format_context(context: &AssembledContext) -> String {
        let mut out = String::new();
        for item in &context.items {
            out.push_str(&format!("[{}] {}\n\n", item.ref_number, item.content));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_layer_spec() {
        assert_eq!(
            source_layer_from_uri("file://spec/01-architecture.md"),
            Some("spec")
        );
        assert_eq!(
            source_layer_from_uri("file://spec/05-ingestion.md"),
            Some("spec")
        );
    }

    #[test]
    fn source_layer_design() {
        assert_eq!(
            source_layer_from_uri("file://docs/adr/0001-hybrid-property-graph.md"),
            Some("design")
        );
    }

    #[test]
    fn source_layer_code() {
        assert_eq!(
            source_layer_from_uri("file://engine/crates/covalence-core/src/search/fusion.rs"),
            Some("code")
        );
        assert_eq!(
            source_layer_from_uri("file://cli/cmd/search.go"),
            Some("code")
        );
    }

    #[test]
    fn source_layer_research() {
        assert_eq!(
            source_layer_from_uri("https://arxiv.org/html/2501.00309"),
            Some("research")
        );
        assert_eq!(
            source_layer_from_uri("http://example.com/paper.pdf"),
            Some("research")
        );
    }

    #[test]
    fn source_layer_unknown() {
        assert_eq!(source_layer_from_uri("file://README.md"), None);
        assert_eq!(source_layer_from_uri("covalence://internal"), None);
    }
}

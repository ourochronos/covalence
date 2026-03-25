//! Search service — multi-dimensional fused search orchestration.
//!
//! Split into focused submodules:
//! - [`filters`] — `SearchFilters` struct, `source_layer_from_uri`,
//!   and all post-fusion filtering/diversification logic
//! - [`enrichment`] — result enrichment (node/chunk/article/source
//!   metadata lookup, graph context)
//! - [`pipeline`] — the core `search_inner` pipeline orchestration
//! - [`tests`] — unit tests

mod enrichment;
pub mod filters;
mod pipeline;
mod tests;

use std::sync::Arc;

use crate::config::TableDimensions;
use crate::error::Result;
use crate::graph::SharedGraph;
use crate::graph::engine::GraphEngine;
use crate::ingestion::embedder::Embedder;
use crate::search::abstention::AbstentionConfig;
use crate::search::cache::{CacheConfig, QueryCache};
use crate::search::context::{AssembledContext, ContextConfig, RawContextItem, assemble_context};
use crate::search::dimensions::{
    GlobalDimension, GraphDimension, LexicalDimension, StructuralDimension, TemporalDimension,
    VectorDimension,
};
use crate::search::fusion::FusedResult;
use crate::search::rerank::{NoopReranker, Reranker};
use crate::search::strategy::SearchStrategy;
use crate::storage::postgres::PgRepo;

// Re-export public types so `services::search::SearchFilters` and
// `services::search::SearchService` still resolve.
pub use filters::{SearchFilters, source_layer_from_uri};

/// Service for orchestrating multi-dimensional search and RRF fusion.
///
/// Integrates the full search pipeline: cache lookup, adaptive
/// strategy selection, dimension search, RRF fusion, abstention
/// detection, query expansion, reranking, and trace recording.
pub struct SearchService {
    pub(super) repo: Arc<PgRepo>,
    pub(super) embedder: Option<Arc<dyn Embedder>>,
    /// Graph engine trait for graph context enrichment.
    pub(super) graph_engine: Arc<dyn GraphEngine>,
    /// Raw shared graph for search dimensions and query expansion.
    pub(super) graph: SharedGraph,
    pub(super) vector: VectorDimension,
    pub(super) lexical: LexicalDimension,
    pub(super) temporal: TemporalDimension,
    pub(super) graph_dim: GraphDimension,
    pub(super) structural: StructuralDimension,
    pub(super) global: GlobalDimension,
    pub(super) reranker: Arc<dyn Reranker>,
    pub(super) cache: Option<QueryCache>,
    pub(super) abstention_config: AbstentionConfig,
    /// Use Convex Combination fusion instead of RRF.
    /// CC preserves score magnitude; RRF uses only rank.
    pub(super) use_cc_fusion: bool,
    /// Internal domains for DDSS boost (from ontology).
    pub(super) internal_domains: std::collections::HashSet<String>,
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
        let graph_engine: Arc<dyn GraphEngine> =
            Arc::new(crate::graph::PetgraphEngine::new(Arc::clone(&graph)));
        Self {
            repo,
            embedder,
            vector: VectorDimension::new(pool.clone(), table_dims.clone()),
            lexical: LexicalDimension::new(pool.clone()),
            temporal: TemporalDimension::new(pool.clone()),
            graph_dim: GraphDimension::new(Arc::clone(&graph)),
            structural: StructuralDimension::new(Arc::clone(&graph_engine)),
            global: GlobalDimension::new(pool.clone(), table_dims),
            graph_engine,
            graph: Arc::clone(&graph),
            reranker: Arc::new(NoopReranker),
            cache: None,
            abstention_config: AbstentionConfig::default(),
            use_cc_fusion: true,
            internal_domains: ["code", "spec", "design"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }

    /// Set internal domains from the ontology (replaces hardcoded
    /// INTERNAL_DOMAINS).
    pub fn with_internal_domains(mut self, domains: std::collections::HashSet<String>) -> Self {
        self.internal_domains = domains;
        self
    }

    /// Set view → edge type mappings from the ontology.
    pub fn with_view_edges(
        mut self,
        view_edges: std::collections::HashMap<String, std::collections::HashSet<String>>,
    ) -> Self {
        self.graph_dim = self.graph_dim.with_view_edges(view_edges);
        self
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

    /// Execute a fused search across all dimensions (standard flat).
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
        self.search_inner(query, strategy, limit, filters, false)
            .await
    }

    /// Execute a hierarchical (coarse-to-fine) search.
    ///
    /// Finds relevant sources first via source embeddings, then
    /// restricts chunk retrieval to those sources. This eliminates
    /// "right paragraph, wrong document" mismatches.
    pub async fn search_hierarchical(
        &self,
        query: &str,
        strategy: SearchStrategy,
        limit: usize,
        filters: Option<SearchFilters>,
    ) -> Result<Vec<FusedResult>> {
        self.search_inner(query, strategy, limit, filters, true)
            .await
    }

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

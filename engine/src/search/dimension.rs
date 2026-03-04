//! DimensionAdaptor trait — the SING cascade search pattern (SPEC §4.2).
//!
//! Each adaptor represents one retrieval dimension (vector, lexical, graph).
//! Adaptors produce scored results that are fused by the search service.

use crate::models::SearchIntent;
use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

/// A single scored result from one dimension.
#[derive(Debug, Clone)]
pub struct DimensionResult {
    pub node_id: Uuid,
    /// Raw score from this dimension (unnormalized).
    pub raw_score: f64,
    /// Normalized score in [0.0, 1.0] (set by normalize_scores).
    pub normalized_score: f64,
    /// For graph results: the hop distance from the nearest anchor node.
    /// `None` for vector and lexical results.
    pub hop: Option<u32>,
}

/// Query parameters for a dimension search.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DimensionQuery {
    /// The search text (used by lexical).
    pub text: String,
    /// Pre-computed query embedding (used by vector).
    pub embedding: Option<Vec<f32>>,
    /// Search intent (affects graph edge weighting).
    pub intent: Option<SearchIntent>,
    /// Session scope filter.
    pub session_id: Option<Uuid>,
    /// Node type filter.
    pub node_types: Option<Vec<String>>,
    /// Maximum number of graph traversal hops (1–3). Only used by
    /// [`GraphAdaptor`]; other adaptors ignore this field.
    pub max_hops: Option<u32>,
    /// Minimum causal weight filter for graph traversal (covalence#75).
    ///
    /// When set, the graph dimension only traverses edges whose
    /// `causal_weight >= min_causal_weight`.  This restricts BFS to
    /// high-causal-strength edge types (e.g. 0.6 excludes RELATES_TO at 0.15
    /// and CONTRADICTS at 0.50 but keeps CONFIRMS at 0.60 and above).
    ///
    /// Default: `None` (all edges traversed — backward compatible).
    pub min_causal_weight: Option<f32>,
    /// Namespace filter — only nodes whose `namespace` column matches this
    /// value are returned.  Defaults to `"default"`.
    pub namespace: String,
}

#[async_trait]
pub trait DimensionAdaptor: Send + Sync {
    /// Human-readable name for this dimension.
    #[allow(dead_code)]
    fn name(&self) -> &'static str;

    /// Check if this dimension's backend is available.
    /// Called once at startup. Failure is a hard error (except lexical → ts_rank fallback).
    async fn check_availability(&self, pool: &PgPool) -> bool;

    /// Execute this dimension's search.
    /// `candidates`: if Some, restrict search to these node IDs (cascade pre-filter).
    async fn search(
        &self,
        pool: &PgPool,
        query: &DimensionQuery,
        candidates: Option<&[Uuid]>,
        limit: usize,
    ) -> anyhow::Result<Vec<DimensionResult>>;

    /// Normalize raw scores to [0.0, 1.0] (higher = better).
    fn normalize_scores(&self, results: &mut [DimensionResult]);

    /// Static estimate of selectivity [0.0, 1.0]. Lower = more selective.
    /// Used by the query planner for cascade ordering.
    #[allow(dead_code)]
    fn estimate_selectivity(&self, query: &DimensionQuery) -> f64;

    /// Can this dimension run in parallel with others?
    #[allow(dead_code)]
    fn parallelizable(&self) -> bool;
}

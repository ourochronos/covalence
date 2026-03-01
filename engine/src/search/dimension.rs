use async_trait::async_trait;
use uuid::Uuid;

/// A scored candidate from a dimension search.
#[derive(Debug, Clone)]
pub struct ScoredCandidate {
    pub node_id: Uuid,
    /// Raw score from this dimension (will be normalized to 0-1).
    pub raw_score: f32,
}

/// Each search dimension implements this trait.
/// Graph, semantic, lexical — all behind the same interface.
/// The `candidates` parameter enables cascade pre-filtering:
/// pass results from a cheaper dimension to constrain an expensive one.
#[async_trait]
pub trait DimensionAdaptor: Send + Sync {
    /// Human-readable name for logging/explain.
    fn name(&self) -> &str;

    /// Estimated base cost (relative units). Used for query planning.
    fn base_cost(&self) -> f32;

    /// Search this dimension, optionally constrained to a candidate set.
    async fn search(
        &self,
        query: &str,
        candidates: Option<&[Uuid]>,
        limit: usize,
    ) -> anyhow::Result<Vec<ScoredCandidate>>;

    /// Normalize raw scores to [0, 1].
    fn normalize(&self, candidates: &mut [ScoredCandidate]);

    /// Estimate selectivity (0-1) for query planning.
    /// Lower = more selective = fewer results = run first.
    async fn estimate_selectivity(&self, query: &str) -> anyhow::Result<f32>;
}

//! Reciprocal Rank Fusion (RRF) implementation.
//!
//! Merges ranked lists from multiple search dimensions without requiring
//! score normalization. RRF_score(d) = Σ weight_i / (K + rank_i(d))
//! where K defaults to 60.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Default K parameter for RRF. Higher values reduce the impact of
/// high-ranking items relative to lower-ranking ones.
pub const DEFAULT_K: f64 = 60.0;

/// A single result from one search dimension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// ID of the matched entity.
    pub id: Uuid,
    /// Score assigned by the dimension (for reference only; RRF uses rank).
    pub score: f64,
    /// 1-based rank within this dimension's result list.
    pub rank: usize,
    /// Which dimension produced this result.
    pub dimension: String,
    /// Optional text snippet for display.
    pub snippet: Option<String>,
    /// The type of result: "chunk", "node", or "article".
    pub result_type: Option<String>,
}

/// A fused result combining evidence from multiple dimensions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusedResult {
    /// ID of the matched entity.
    pub id: Uuid,
    /// Combined RRF score across all dimensions.
    pub fused_score: f64,
    /// Epistemic confidence (projected probability from Subjective Logic).
    pub confidence: Option<f64>,
    /// Entity type (e.g. "node").
    pub entity_type: Option<String>,
    /// Canonical name of the entity.
    pub name: Option<String>,
    /// Best available text snippet.
    pub snippet: Option<String>,
    /// The type of result: "chunk", "node", or "article".
    pub result_type: Option<String>,
    /// Per-dimension scores (original scores, not RRF contributions).
    pub dimension_scores: HashMap<String, f64>,
    /// Per-dimension ranks.
    pub dimension_ranks: HashMap<String, usize>,
}

/// Fuse multiple ranked lists using Reciprocal Rank Fusion.
///
/// Each list in `ranked_lists` corresponds to a search dimension. The
/// `weights` slice must have the same length as `ranked_lists` (one
/// weight per dimension). The `k` parameter controls rank saturation.
///
/// Formula: fused_score(d) = Σ weight_i / (k + rank_i(d))
///
/// Results are sorted by fused_score descending.
pub fn rrf_fuse(ranked_lists: &[Vec<SearchResult>], weights: &[f64], k: f64) -> Vec<FusedResult> {
    if ranked_lists.is_empty() {
        return Vec::new();
    }

    // Accumulate scores per entity ID.
    let mut fused: HashMap<Uuid, FusedResult> = HashMap::new();

    for (i, list) in ranked_lists.iter().enumerate() {
        let weight = weights.get(i).copied().unwrap_or(1.0);
        for result in list {
            let entry = fused.entry(result.id).or_insert_with(|| FusedResult {
                id: result.id,
                fused_score: 0.0,
                confidence: None,
                entity_type: None,
                name: None,
                snippet: None,
                result_type: None,
                dimension_scores: HashMap::new(),
                dimension_ranks: HashMap::new(),
            });
            entry.fused_score += weight / (k + result.rank as f64);
            if entry.result_type.is_none() {
                entry.result_type.clone_from(&result.result_type);
            }
            entry
                .dimension_scores
                .insert(result.dimension.clone(), result.score);
            entry
                .dimension_ranks
                .insert(result.dimension.clone(), result.rank);
        }
    }

    let mut results: Vec<FusedResult> = fused.into_values().collect();
    results.sort_by(|a, b| {
        b.fused_score
            .partial_cmp(&a.fused_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_result(id: Uuid, score: f64, rank: usize, dimension: &str) -> SearchResult {
        SearchResult {
            id,
            score,
            rank,
            dimension: dimension.to_string(),
            snippet: None,
            result_type: None,
        }
    }

    #[test]
    fn empty_input_returns_empty() {
        let results = rrf_fuse(&[], &[], DEFAULT_K);
        assert!(results.is_empty());
    }

    #[test]
    fn single_list() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let list = vec![
            make_result(id1, 0.9, 1, "vector"),
            make_result(id2, 0.5, 2, "vector"),
        ];
        let results = rrf_fuse(&[list], &[1.0], DEFAULT_K);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, id1);
        assert_eq!(results[1].id, id2);
        assert!(results[0].fused_score > results[1].fused_score);
    }

    #[test]
    fn multiple_lists_combine_scores() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let list_a = vec![
            make_result(id1, 0.9, 1, "vector"),
            make_result(id2, 0.5, 2, "vector"),
        ];
        let list_b = vec![
            make_result(id2, 0.8, 1, "lexical"),
            make_result(id1, 0.3, 2, "lexical"),
        ];
        let results = rrf_fuse(&[list_a, list_b], &[1.0, 1.0], DEFAULT_K);
        assert_eq!(results.len(), 2);
        // Both appear in both lists, scores should be combined.
        for r in &results {
            assert_eq!(r.dimension_scores.len(), 2);
            assert_eq!(r.dimension_ranks.len(), 2);
        }
    }

    #[test]
    fn weight_influence() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        // id1 is rank 1 in low-weight dimension,
        // id2 is rank 1 in high-weight dimension.
        let list_a = vec![make_result(id1, 0.9, 1, "vector")];
        let list_b = vec![make_result(id2, 0.9, 1, "lexical")];

        let results = rrf_fuse(&[list_a, list_b], &[0.1, 10.0], DEFAULT_K);
        assert_eq!(results.len(), 2);
        // id2 should rank higher due to much larger weight.
        assert_eq!(results[0].id, id2);
    }

    #[test]
    fn k_parameter_effect() {
        let id = Uuid::new_v4();
        let list = vec![make_result(id, 1.0, 1, "vector")];

        let small_k = rrf_fuse(&[list.clone()], &[1.0], 1.0);
        let large_k = rrf_fuse(&[list], &[1.0], 1000.0);

        // Smaller K gives higher score: 1/(1+1) = 0.5 vs 1/(1000+1).
        assert!(small_k[0].fused_score > large_k[0].fused_score);
    }

    #[test]
    fn duplicate_ids_across_lists() {
        let id = Uuid::new_v4();
        let list_a = vec![make_result(id, 0.9, 1, "vector")];
        let list_b = vec![make_result(id, 0.8, 3, "lexical")];
        let results = rrf_fuse(&[list_a, list_b], &[1.0, 1.0], DEFAULT_K);
        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert_eq!(r.id, id);
        // Should have contributions from both dimensions.
        let expected = 1.0 / (DEFAULT_K + 1.0) + 1.0 / (DEFAULT_K + 3.0);
        assert!((r.fused_score - expected).abs() < 1e-10);
    }
}

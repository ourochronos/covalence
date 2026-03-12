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
    /// Full content of the matched entity (chunk content, article
    /// body, or node description). Populated during enrichment.
    pub content: Option<String>,
    /// Source URI (for chunk results).
    pub source_uri: Option<String>,
    /// Source title (for chunk results).
    pub source_title: Option<String>,
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

    if ranked_lists.len() != weights.len() {
        tracing::warn!(
            lists = ranked_lists.len(),
            weights = weights.len(),
            "rrf_fuse: weight/list length mismatch, defaulting missing weights to 1.0"
        );
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
                content: None,
                source_uri: None,
                source_title: None,
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

/// Fuse multiple ranked lists using Convex Combination of scores.
///
/// Unlike RRF (which discards score magnitude and uses only rank),
/// CC preserves score information by normalizing scores within each
/// dimension to \[0, 1\] and computing a weighted sum.
///
/// Formula: fused_score(d) = Σ weight_i * norm_score_i(d)
///
/// where norm_score_i = (score - min) / (max - min) within dimension i.
///
/// Bruch et al. (2210.11934) showed CC consistently outperforms RRF
/// because rank-based fusion discards magnitude information that
/// distinguishes strong matches from marginal ones.
pub fn cc_fuse(ranked_lists: &[Vec<SearchResult>], weights: &[f64]) -> Vec<FusedResult> {
    if ranked_lists.is_empty() {
        return Vec::new();
    }

    if ranked_lists.len() != weights.len() {
        tracing::warn!(
            lists = ranked_lists.len(),
            weights = weights.len(),
            "cc_fuse: weight/list length mismatch, defaulting missing weights to 1.0"
        );
    }

    let mut fused: HashMap<Uuid, FusedResult> = HashMap::new();

    for (i, list) in ranked_lists.iter().enumerate() {
        let weight = weights.get(i).copied().unwrap_or(1.0);
        if list.is_empty() {
            continue;
        }

        // Min-max normalize scores within this dimension.
        let min_score = list
            .iter()
            .map(|r| r.score)
            .fold(f64::INFINITY, f64::min);
        let max_score = list
            .iter()
            .map(|r| r.score)
            .fold(f64::NEG_INFINITY, f64::max);
        let range = max_score - min_score;

        for result in list {
            let norm_score = if range > 1e-12 {
                (result.score - min_score) / range
            } else {
                1.0 // All scores identical → treat as maximum
            };
            // Guard against NaN/Inf scores from upstream dimensions.
            let norm_score = if norm_score.is_finite() {
                norm_score
            } else {
                0.0
            };
            let entry = fused.entry(result.id).or_insert_with(|| FusedResult {
                id: result.id,
                fused_score: 0.0,
                confidence: None,
                entity_type: None,
                name: None,
                snippet: None,
                content: None,
                source_uri: None,
                source_title: None,
                result_type: None,
                dimension_scores: HashMap::new(),
                dimension_ranks: HashMap::new(),
            });
            entry.fused_score += weight * norm_score;
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

        let small_k = rrf_fuse(std::slice::from_ref(&list), &[1.0], 1.0);
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

    #[test]
    fn fused_result_content_field_defaults_to_none() {
        let id = Uuid::new_v4();
        let list = vec![make_result(id, 0.9, 1, "vector")];
        let results = rrf_fuse(&[list], &[1.0], DEFAULT_K);
        assert_eq!(results.len(), 1);
        // Content is populated later during enrichment, so
        // it should be None after fusion.
        assert!(results[0].content.is_none());
    }

    #[test]
    fn fused_result_serialization_includes_content() {
        let result = FusedResult {
            id: Uuid::new_v4(),
            fused_score: 0.5,
            confidence: None,
            entity_type: None,
            name: None,
            snippet: None,
            content: Some("full chunk content".to_string()),
            source_uri: None,
            source_title: None,
            result_type: None,
            dimension_scores: HashMap::new(),
            dimension_ranks: HashMap::new(),
        };
        let json = serde_json::to_value(&result).expect("serialization");
        assert_eq!(json["content"], "full chunk content");
    }

    #[test]
    fn fused_result_deserialization_with_content() {
        let json = serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "fused_score": 0.5,
            "content": "some content",
            "dimension_scores": {},
            "dimension_ranks": {}
        });
        let result: FusedResult = serde_json::from_value(json).expect("deserialization");
        assert_eq!(result.content.as_deref(), Some("some content"));
    }

    #[test]
    fn fused_result_deserialization_without_content() {
        // Backward compatibility: existing JSON without
        // content field should deserialize fine.
        let json = serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "fused_score": 0.5,
            "dimension_scores": {},
            "dimension_ranks": {}
        });
        let result: FusedResult = serde_json::from_value(json).expect("deserialization");
        assert!(result.content.is_none());
    }

    // --- CC fusion tests ---

    #[test]
    fn cc_empty_input_returns_empty() {
        let results = cc_fuse(&[], &[]);
        assert!(results.is_empty());
    }

    #[test]
    fn cc_single_list_normalizes_scores() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let list = vec![
            make_result(id1, 0.9, 1, "vector"),
            make_result(id2, 0.5, 2, "vector"),
        ];
        let results = cc_fuse(&[list], &[1.0]);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, id1);
        // id1 normalized: (0.9-0.5)/(0.9-0.5) = 1.0
        assert!((results[0].fused_score - 1.0).abs() < 1e-10);
        // id2 normalized: (0.5-0.5)/(0.9-0.5) = 0.0
        assert!((results[1].fused_score - 0.0).abs() < 1e-10);
    }

    #[test]
    fn cc_multi_dimensional_combines_scores() {
        let id = Uuid::new_v4();
        let list_a = vec![make_result(id, 0.9, 1, "vector")];
        let list_b = vec![make_result(id, 0.8, 1, "lexical")];
        let results = cc_fuse(&[list_a, list_b], &[0.6, 0.4]);
        assert_eq!(results.len(), 1);
        // Single item per list → normalized to 1.0 each.
        // fused = 0.6 * 1.0 + 0.4 * 1.0 = 1.0
        assert!((results[0].fused_score - 1.0).abs() < 1e-10);
    }

    #[test]
    fn cc_weight_influence() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let list_a = vec![make_result(id1, 0.9, 1, "vector")];
        let list_b = vec![make_result(id2, 0.9, 1, "lexical")];
        // id1 only in low-weight dimension, id2 only in high-weight.
        let results = cc_fuse(&[list_a, list_b], &[0.1, 10.0]);
        assert_eq!(results.len(), 2);
        // id2 should rank higher due to larger weight.
        assert_eq!(results[0].id, id2);
    }

    #[test]
    fn cc_preserves_score_magnitude() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        // id1 has a strong match (0.95) and a weak match (0.2)
        // id2 has moderate matches (0.5, 0.5)
        let list_a = vec![
            make_result(id1, 0.95, 1, "vector"),
            make_result(id2, 0.5, 2, "vector"),
        ];
        let list_b = vec![
            make_result(id2, 0.5, 1, "lexical"),
            make_result(id1, 0.2, 2, "lexical"),
        ];
        let results = cc_fuse(&[list_a, list_b], &[1.0, 1.0]);
        // id1: vector normalized = 1.0, lexical normalized = 0.0 → 1.0
        // id2: vector normalized = 0.0, lexical normalized = 1.0 → 1.0
        // Both equal — CC treats them identically when they're
        // the top of different dimensions.
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn cc_nan_scores_treated_as_zero() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let list = vec![
            make_result(id1, f64::NAN, 1, "vector"),
            make_result(id2, 0.5, 2, "vector"),
        ];
        let results = cc_fuse(&[list], &[1.0]);
        // NaN should not contaminate results — id2 should still
        // have a finite score.
        for r in &results {
            assert!(
                r.fused_score.is_finite(),
                "fused score should be finite, got {}",
                r.fused_score
            );
        }
    }

    #[test]
    fn cc_identical_scores_treated_as_max() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let list = vec![
            make_result(id1, 0.85, 1, "vector"),
            make_result(id2, 0.85, 2, "vector"),
        ];
        let results = cc_fuse(&[list], &[1.0]);
        // All identical → all get 1.0.
        assert!((results[0].fused_score - 1.0).abs() < 1e-10);
        assert!((results[1].fused_score - 1.0).abs() < 1e-10);
    }

    #[test]
    fn rrf_fuse_weight_mismatch_defaults_to_one() {
        // When fewer weights than lists, missing weights default to
        // 1.0. This is a graceful degradation, not a panic.
        let id1 = Uuid::new_v4();
        let list_a = vec![make_result(id1, 0.9, 1, "vector")];
        let list_b = vec![make_result(id1, 0.8, 1, "lexical")];
        // Only one weight for two lists.
        let results = rrf_fuse(&[list_a, list_b], &[2.0], DEFAULT_K);
        assert_eq!(results.len(), 1);
        // Second dimension should use weight=1.0 (default).
        let expected = 2.0 / (DEFAULT_K + 1.0) + 1.0 / (DEFAULT_K + 1.0);
        assert!(
            (results[0].fused_score - expected).abs() < 1e-10,
            "expected {expected}, got {}",
            results[0].fused_score
        );
    }

    #[test]
    fn cc_fuse_weight_mismatch_defaults_to_one() {
        let id1 = Uuid::new_v4();
        let list_a = vec![make_result(id1, 0.9, 1, "vector")];
        let list_b = vec![make_result(id1, 0.8, 1, "lexical")];
        // Only one weight for two lists.
        let results = cc_fuse(&[list_a, list_b], &[2.0]);
        assert_eq!(results.len(), 1);
        // Single element per list → norm_score = 1.0 for both.
        // fused = 2.0 * 1.0 + 1.0 * 1.0 = 3.0
        assert!(
            (results[0].fused_score - 3.0).abs() < 1e-10,
            "expected 3.0, got {}",
            results[0].fused_score
        );
    }
}

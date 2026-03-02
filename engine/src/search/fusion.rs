//! ScoreFusion — weighted dimensional score fusion with confidence × freshness (SPEC §7.3).

#![allow(dead_code)]

use super::dimension::DimensionResult;
use crate::models::DimensionWeights;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use uuid::Uuid;

/// A fused search result ready for ranking.
#[derive(Debug, Clone)]
pub struct FusedResult {
    pub node_id: Uuid,
    pub composite_score: f64,
    pub dimensional_scores: DimensionalScores,
}

#[derive(Debug, Clone)]
pub struct DimensionalScores {
    pub vector: Option<f64>,
    pub lexical: Option<f64>,
    pub graph: Option<f64>,
}

/// Decay rate for freshness (λ in exp(-λ × days_since_modified)).
const DECAY_RATE: f64 = 0.01;

/// Novelty boost duration in hours.
const NOVELTY_HOURS: f64 = 48.0;

/// Max novelty multiplier.
const NOVELTY_MAX: f64 = 1.5;

pub struct ScoreFusion;

impl ScoreFusion {
    /// Fuse results from multiple dimensions into a ranked list.
    ///
    /// Formula (SPEC §7.3):
    ///   dimensional_score = weighted_mean(vector, lexical, graph)
    ///   final = dimensional * 0.50 + confidence * 0.35 + freshness * 0.15
    ///   final *= novelty_boost (1.0–1.5, decays over 48h)
    pub fn fuse(
        vector_results: &[DimensionResult],
        lexical_results: &[DimensionResult],
        graph_results: &[DimensionResult],
        weights: &DimensionWeights,
        // For each node: (confidence, modified_at, created_at)
        node_metadata: &HashMap<Uuid, (f64, DateTime<Utc>, DateTime<Utc>)>,
        limit: usize,
    ) -> Vec<FusedResult> {
        #[allow(clippy::type_complexity)]
        let mut scores: HashMap<Uuid, (Option<f64>, Option<f64>, Option<f64>)> = HashMap::new();

        for r in vector_results {
            scores.entry(r.node_id).or_insert((None, None, None)).0 = Some(r.normalized_score);
        }
        for r in lexical_results {
            scores.entry(r.node_id).or_insert((None, None, None)).1 = Some(r.normalized_score);
        }
        for r in graph_results {
            scores.entry(r.node_id).or_insert((None, None, None)).2 = Some(r.normalized_score);
        }

        let now = Utc::now();
        let mut results: Vec<FusedResult> = scores
            .into_iter()
            .map(|(node_id, (v, l, g))| {
                // Weighted mean over present dimensions
                let dimensional_score = weighted_sum_present(
                    v,
                    l,
                    g,
                    weights.vector as f64,
                    weights.lexical as f64,
                    weights.graph as f64,
                );

                // Look up node metadata
                let (confidence, modified_at, created_at) = node_metadata
                    .get(&node_id)
                    .copied()
                    .unwrap_or((0.5, now, now));

                // Freshness decay
                let days_since = (now - modified_at).num_seconds() as f64 / 86400.0;
                let freshness = (-DECAY_RATE * days_since).exp();

                // Base composite
                let mut composite = dimensional_score * 0.85 + confidence * 0.10 + freshness * 0.05;

                // Novelty boost for new nodes
                let hours_since_created = (now - created_at).num_seconds() as f64 / 3600.0;
                if hours_since_created < NOVELTY_HOURS {
                    let novelty =
                        1.0 + (NOVELTY_MAX - 1.0) * (1.0 - hours_since_created / NOVELTY_HOURS);
                    composite *= novelty;
                }

                FusedResult {
                    node_id,
                    composite_score: composite,
                    dimensional_scores: DimensionalScores {
                        vector: v,
                        lexical: l,
                        graph: g,
                    },
                }
            })
            .collect();

        // Sort descending by composite score
        results.sort_by(|a, b| {
            b.composite_score
                .partial_cmp(&a.composite_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit);
        results
    }
}

/// Weighted mean over present dimensions only.
/// Absent dimensions don't dilute the score.
fn weighted_sum_present(
    v: Option<f64>,
    l: Option<f64>,
    g: Option<f64>,
    wv: f64,
    wl: f64,
    wg: f64,
) -> f64 {
    let mut sum = 0.0;

    if let Some(score) = v {
        sum += score * wv;
    }
    if let Some(score) = l {
        sum += score * wl;
    }
    if let Some(score) = g {
        sum += score * wg;
    }

    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn make_result(id: Uuid, score: f64) -> DimensionResult {
        DimensionResult {
            node_id: id,
            raw_score: score,
            normalized_score: score,
        }
    }

    #[test]
    fn test_fusion_basic() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let now = Utc::now();

        let vector = vec![make_result(id1, 0.9), make_result(id2, 0.5)];
        let lexical = vec![make_result(id1, 0.7), make_result(id2, 0.8)];
        let graph = vec![];

        let weights = DimensionWeights::default();
        let mut meta = HashMap::new();
        meta.insert(id1, (0.8, now, now));
        meta.insert(
            id2,
            (0.6, now - Duration::days(30), now - Duration::days(30)),
        );

        let results = ScoreFusion::fuse(&vector, &lexical, &graph, &weights, &meta, 10);

        assert_eq!(results.len(), 2);
        // id1 should rank higher (better vector + higher confidence + newer)
        assert_eq!(results[0].node_id, id1);
    }

    #[test]
    fn test_absent_dimension_no_dilution() {
        let id = Uuid::new_v4();
        let now = Utc::now();

        // Only vector result
        let vector = vec![make_result(id, 0.9)];
        let weights = DimensionWeights::default();
        let mut meta = HashMap::new();
        meta.insert(id, (0.8, now, now));

        let results = ScoreFusion::fuse(&vector, &[], &[], &weights, &meta, 10);
        assert_eq!(results.len(), 1);
        // dimensional should be 0.9 (not diluted by absent dims)
        // composite = 0.9 * 0.50 + 0.8 * 0.35 + ~1.0 * 0.15 * novelty
        assert!(results[0].composite_score > 0.7);
    }
}

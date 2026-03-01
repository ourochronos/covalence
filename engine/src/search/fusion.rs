use std::collections::HashMap;
use uuid::Uuid;
use super::dimension::ScoredCandidate;

/// Fused result after combining scores from multiple dimensions.
#[derive(Debug, Clone)]
pub struct FusedResult {
    pub node_id: Uuid,
    pub total_score: f32,
    pub dimension_scores: HashMap<String, f32>,
}

/// Configurable weights for score fusion.
#[derive(Debug, Clone)]
pub struct DimensionWeights {
    pub graph: f32,
    pub semantic: f32,
    pub lexical: f32,
}

impl Default for DimensionWeights {
    fn default() -> Self {
        Self {
            graph: 0.3,
            semantic: 0.4,
            lexical: 0.3,
        }
    }
}

/// Score fusion: combine normalized scores from multiple dimensions.
pub struct ScoreFusion;

impl ScoreFusion {
    /// Weighted mean fusion. Missing dimension scores are treated as absent
    /// (weight redistributed), not zero — this is the Option<f32> insight from SING.
    pub fn fuse(
        dimension_results: &[(&str, Vec<ScoredCandidate>)],
        weights: &DimensionWeights,
    ) -> Vec<FusedResult> {
        let mut scores: HashMap<Uuid, HashMap<String, f32>> = HashMap::new();

        for (name, candidates) in dimension_results {
            for c in candidates {
                scores
                    .entry(c.node_id)
                    .or_default()
                    .insert(name.to_string(), c.raw_score);
            }
        }

        let weight_map: HashMap<&str, f32> = [
            ("graph", weights.graph),
            ("semantic", weights.semantic),
            ("lexical", weights.lexical),
        ]
        .into_iter()
        .collect();

        let mut results: Vec<FusedResult> = scores
            .into_iter()
            .map(|(node_id, dim_scores)| {
                let mut weighted_sum = 0.0;
                let mut total_weight = 0.0;

                for (dim, score) in &dim_scores {
                    if let Some(&w) = weight_map.get(dim.as_str()) {
                        weighted_sum += w * score;
                        total_weight += w;
                    }
                }

                let total_score = if total_weight > 0.0 {
                    weighted_sum / total_weight
                } else {
                    0.0
                };

                FusedResult {
                    node_id,
                    total_score,
                    dimension_scores: dim_scores,
                }
            })
            .collect();

        results.sort_by(|a, b| b.total_score.partial_cmp(&a.total_score).unwrap());
        results
    }
}

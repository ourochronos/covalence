//! Search result types and query strategy definitions.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::ids::SourceId;

/// A fused search result with per-dimension score breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// ID of the matched entity (chunk, node, or article).
    pub entity_id: uuid::Uuid,

    /// Type of the matched entity.
    pub entity_type: String,

    /// Content of the match.
    pub content: String,

    /// Fused RRF score.
    pub score: f64,

    /// Composite confidence (opinion projected probability * topo).
    pub confidence: f64,

    /// Per-dimension score breakdown.
    pub dimension_scores: HashMap<String, f64>,

    /// Provenance summary.
    pub source: Option<SourceSummary>,

    /// Parent chunk content for context injection.
    pub context: Option<String>,
}

/// Brief provenance summary included with search results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSummary {
    /// Provenance source identifier.
    pub source_id: SourceId,
    /// Human-readable title if available.
    pub title: Option<String>,
    /// Type of source (e.g. "web", "document").
    pub source_type: String,
    /// Reliability score for this source.
    pub reliability_score: f64,
}

/// Pre-configured search strategy with per-dimension weights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchStrategy {
    /// Equal weight across all dimensions.
    Balanced,
    /// Favors vector and lexical similarity.
    Precise,
    /// Favors graph traversal and structural features.
    Exploratory,
    /// Strongly favors temporal recency.
    Recent,
    /// Graph traversal dominant with structural support.
    GraphFirst,
    /// Favors community summary search for global/thematic queries.
    Global,
    /// User-supplied weights.
    Custom,
}

/// Weights for each search dimension.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DimensionWeights {
    /// Semantic vector similarity weight.
    pub vector: f64,
    /// Full-text lexical search weight.
    pub lexical: f64,
    /// Temporal recency/range weight.
    pub temporal: f64,
    /// Graph traversal weight.
    pub graph: f64,
    /// Structural centrality weight.
    pub structural: f64,
    /// Community summary search for global/thematic queries.
    pub global: f64,
}

impl SearchStrategy {
    /// Get the default weights for this strategy.
    pub fn weights(self) -> DimensionWeights {
        match self {
            Self::Balanced => DimensionWeights {
                vector: 0.28,
                lexical: 0.22,
                temporal: 0.13,
                graph: 0.18,
                structural: 0.09,
                global: 0.10,
            },
            Self::Precise => DimensionWeights {
                vector: 0.38,
                lexical: 0.33,
                temporal: 0.05,
                graph: 0.14,
                structural: 0.05,
                global: 0.05,
            },
            Self::Exploratory => DimensionWeights {
                vector: 0.18,
                lexical: 0.09,
                temporal: 0.09,
                graph: 0.32,
                structural: 0.22,
                global: 0.10,
            },
            Self::Recent => DimensionWeights {
                vector: 0.23,
                lexical: 0.18,
                temporal: 0.33,
                graph: 0.14,
                structural: 0.05,
                global: 0.07,
            },
            Self::GraphFirst => DimensionWeights {
                vector: 0.14,
                lexical: 0.09,
                temporal: 0.05,
                graph: 0.42,
                structural: 0.22,
                global: 0.08,
            },
            Self::Global => DimensionWeights {
                vector: 0.10,
                lexical: 0.05,
                temporal: 0.05,
                graph: 0.10,
                structural: 0.10,
                global: 0.60,
            },
            Self::Custom => DimensionWeights {
                vector: 1.0 / 6.0,
                lexical: 1.0 / 6.0,
                temporal: 1.0 / 6.0,
                graph: 1.0 / 6.0,
                structural: 1.0 / 6.0,
                global: 1.0 / 6.0,
            },
        }
    }
}

impl DimensionWeights {
    /// Sum of all weights. Should be 1.0 for normalized strategies.
    pub fn total(&self) -> f64 {
        self.vector + self.lexical + self.temporal + self.graph + self.structural + self.global
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategy_weights_sum_to_one() {
        let strategies = [
            SearchStrategy::Balanced,
            SearchStrategy::Precise,
            SearchStrategy::Exploratory,
            SearchStrategy::Recent,
            SearchStrategy::GraphFirst,
            SearchStrategy::Global,
            SearchStrategy::Custom,
        ];

        for strategy in &strategies {
            let w = strategy.weights();
            let total = w.total();
            assert!(
                (total - 1.0).abs() < 0.01,
                "{:?} weights sum to {total}, expected ~1.0",
                strategy
            );
        }
    }

    #[test]
    fn strategy_weights_all_non_negative() {
        let strategies = [
            SearchStrategy::Balanced,
            SearchStrategy::Precise,
            SearchStrategy::Exploratory,
            SearchStrategy::Recent,
            SearchStrategy::GraphFirst,
            SearchStrategy::Global,
        ];

        for strategy in &strategies {
            let w = strategy.weights();
            assert!(w.vector >= 0.0, "{strategy:?} vector < 0");
            assert!(w.lexical >= 0.0, "{strategy:?} lexical < 0");
            assert!(w.temporal >= 0.0, "{strategy:?} temporal < 0");
            assert!(w.graph >= 0.0, "{strategy:?} graph < 0");
            assert!(w.structural >= 0.0, "{strategy:?} structural < 0");
            assert!(w.global >= 0.0, "{strategy:?} global < 0");
        }
    }

    #[test]
    fn precise_favors_vector_and_lexical() {
        let w = SearchStrategy::Precise.weights();
        assert!(
            w.vector > w.graph,
            "Precise should favor vector over graph"
        );
        assert!(
            w.lexical > w.temporal,
            "Precise should favor lexical over temporal"
        );
    }

    #[test]
    fn exploratory_favors_graph() {
        let w = SearchStrategy::Exploratory.weights();
        assert!(
            w.graph > w.vector,
            "Exploratory should favor graph over vector"
        );
    }

    #[test]
    fn recent_favors_temporal() {
        let w = SearchStrategy::Recent.weights();
        assert!(
            w.temporal > w.vector,
            "Recent should favor temporal over vector"
        );
        assert!(
            w.temporal > w.graph,
            "Recent should favor temporal over graph"
        );
    }

    #[test]
    fn global_favors_global_dimension() {
        let w = SearchStrategy::Global.weights();
        assert!(
            w.global > 0.5,
            "Global should give global dimension majority weight"
        );
    }

    #[test]
    fn strategy_serde_roundtrip() {
        let strategy = SearchStrategy::Precise;
        let json = serde_json::to_string(&strategy).unwrap();
        assert_eq!(json, "\"precise\"");
        let parsed: SearchStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SearchStrategy::Precise);
    }

    #[test]
    fn search_result_serde_roundtrip() {
        let result = SearchResult {
            entity_id: uuid::Uuid::new_v4(),
            entity_type: "chunk".into(),
            content: "test content".into(),
            score: 0.85,
            confidence: 0.9,
            dimension_scores: HashMap::from([
                ("vector".into(), 0.8),
                ("lexical".into(), 0.7),
            ]),
            source: None,
            context: Some("parent context".into()),
        };
        let json = serde_json::to_string(&result).unwrap();
        let restored: SearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.entity_type, "chunk");
        assert!((restored.score - 0.85).abs() < 1e-10);
    }
}

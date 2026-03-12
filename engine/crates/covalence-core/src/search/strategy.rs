//! Query strategy selection and weight configuration.
//!
//! Pre-configured strategies: balanced, precise, exploratory, recent,
//! graph_first, global. Users can also provide custom weights.

use serde::{Deserialize, Serialize};

/// Pre-configured search strategy with tuned dimension weights.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SearchStrategy {
    /// Auto-detect strategy via SkewRoute. This is the default
    /// when the caller does not specify a strategy.
    #[default]
    Auto,
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
    Custom(DimensionWeights),
}

/// Per-dimension weight configuration for search fusion.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
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

impl DimensionWeights {
    /// Return weights as a fixed-size array in canonical order:
    /// [vector, lexical, temporal, graph, structural, global].
    pub fn as_slice(&self) -> [f64; 6] {
        [
            self.vector,
            self.lexical,
            self.temporal,
            self.graph,
            self.structural,
            self.global,
        ]
    }

    /// Normalize weights so they sum to 1.0.
    ///
    /// If all weights are zero, returns equal weights
    /// (1/6 each).
    pub fn normalize(&self) -> DimensionWeights {
        let sum: f64 = self.as_slice().iter().sum();
        let equal = 1.0 / 6.0;
        if sum == 0.0 {
            return DimensionWeights {
                vector: equal,
                lexical: equal,
                temporal: equal,
                graph: equal,
                structural: equal,
                global: equal,
            };
        }
        DimensionWeights {
            vector: self.vector / sum,
            lexical: self.lexical / sum,
            temporal: self.temporal / sum,
            graph: self.graph / sum,
            structural: self.structural / sum,
            global: self.global / sum,
        }
    }
}

impl SearchStrategy {
    /// Get the dimension weights for this strategy.
    pub fn weights(&self) -> DimensionWeights {
        match self {
            Self::Auto | Self::Balanced => DimensionWeights {
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
            Self::Custom(w) => *w,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_weights() {
        let w = SearchStrategy::Balanced.weights();
        assert_eq!(w.as_slice(), [0.28, 0.22, 0.13, 0.18, 0.09, 0.10]);
    }

    #[test]
    fn precise_favors_vector_lexical() {
        let w = SearchStrategy::Precise.weights();
        assert!(w.vector > w.temporal);
        assert!(w.lexical > w.graph);
    }

    #[test]
    fn exploratory_favors_graph_structural() {
        let w = SearchStrategy::Exploratory.weights();
        assert!(w.graph > w.vector);
        assert!(w.structural > w.lexical);
    }

    #[test]
    fn recent_favors_temporal() {
        let w = SearchStrategy::Recent.weights();
        assert!(w.temporal > w.vector);
        assert!(w.temporal > w.lexical);
        assert!(w.temporal > w.graph);
        assert!(w.temporal > w.structural);
    }

    #[test]
    fn graph_first_favors_graph() {
        let w = SearchStrategy::GraphFirst.weights();
        assert!(w.graph > w.vector);
        assert!(w.graph > w.lexical);
        assert!(w.graph > w.temporal);
        assert!(w.graph > w.structural);
    }

    #[test]
    fn global_favors_community_summary() {
        let w = SearchStrategy::Global.weights();
        assert!(w.global > w.vector);
        assert!(w.global > w.lexical);
        assert!(w.global > w.temporal);
        assert!(w.global > w.graph);
        assert!(w.global > w.structural);
    }

    #[test]
    fn custom_passthrough() {
        let custom = DimensionWeights {
            vector: 0.1,
            lexical: 0.2,
            temporal: 0.3,
            graph: 0.4,
            structural: 0.5,
            global: 0.6,
        };
        let strategy = SearchStrategy::Custom(custom);
        let w = strategy.weights();
        assert_eq!(w.vector, 0.1);
        assert_eq!(w.structural, 0.5);
        assert_eq!(w.global, 0.6);
    }

    #[test]
    fn default_is_auto() {
        assert_eq!(SearchStrategy::default(), SearchStrategy::Auto);
    }

    #[test]
    fn normalize_sums_to_one() {
        let w = SearchStrategy::Precise.weights().normalize();
        let sum: f64 = w.as_slice().iter().sum();
        assert!((sum - 1.0).abs() < 1e-10);
    }

    #[test]
    fn normalize_zero_weights() {
        let zero = DimensionWeights {
            vector: 0.0,
            lexical: 0.0,
            temporal: 0.0,
            graph: 0.0,
            structural: 0.0,
            global: 0.0,
        };
        let n = zero.normalize();
        let equal = 1.0 / 6.0;
        for &w in &n.as_slice() {
            assert!((w - equal).abs() < 1e-10, "expected {equal}, got {w}");
        }
    }
}

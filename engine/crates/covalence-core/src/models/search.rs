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

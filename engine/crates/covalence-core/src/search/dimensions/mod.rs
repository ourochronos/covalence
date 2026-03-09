//! Individual search dimensions.
//!
//! Each dimension independently produces a ranked list of results:
//! - Vector: semantic similarity via pgvector HNSW
//! - Lexical: full-text search via tsvector
//! - Temporal: recency or time-range filtering
//! - Graph: traversal from seed nodes with hop-decay
//! - Structural: centrality, community membership
//! - Global: community summary embeddings for thematic queries

pub mod global;
pub mod graph;
pub mod lexical;
pub mod structural;
pub mod temporal;
pub mod vector;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::fusion::SearchResult;
use super::strategy::SearchStrategy;

pub use self::global::GlobalDimension;
pub use self::graph::GraphDimension;
pub use self::lexical::LexicalDimension;
pub use self::structural::StructuralDimension;
pub use self::temporal::TemporalDimension;
pub use self::vector::VectorDimension;

/// The six search dimensions supported by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DimensionKind {
    /// Semantic similarity via vector embeddings.
    Vector,
    /// Full-text lexical search.
    Lexical,
    /// Temporal recency or time-range filtering.
    Temporal,
    /// Graph traversal from seed nodes.
    Graph,
    /// Structural centrality and community features.
    Structural,
    /// Community summary search for global/thematic queries.
    Global,
}

impl std::fmt::Display for DimensionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vector => write!(f, "vector"),
            Self::Lexical => write!(f, "lexical"),
            Self::Temporal => write!(f, "temporal"),
            Self::Graph => write!(f, "graph"),
            Self::Structural => write!(f, "structural"),
            Self::Global => write!(f, "global"),
        }
    }
}

/// A multi-dimensional search query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    /// The text query string.
    pub text: String,
    /// Optional pre-computed embedding for the query.
    pub embedding: Option<Vec<f64>>,
    /// Optional time range filter (start, end).
    pub time_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
    /// Seed node IDs for graph-based search.
    pub seed_nodes: Vec<Uuid>,
    /// Which strategy (and thus weights) to use.
    pub strategy: SearchStrategy,
    /// Maximum number of results to return per dimension.
    pub limit: usize,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            text: String::new(),
            embedding: None,
            time_range: None,
            seed_nodes: Vec::new(),
            strategy: SearchStrategy::default(),
            limit: 10,
        }
    }
}

/// Trait for individual search dimension implementations.
///
/// Each dimension independently retrieves and ranks candidates,
/// returning `SearchResult` items that are later fused via RRF.
#[allow(async_fn_in_trait)]
pub trait SearchDimension {
    /// Execute this dimension's search against the given query.
    async fn search(&self, query: &SearchQuery) -> crate::error::Result<Vec<SearchResult>>;

    /// Which dimension kind this implementation covers.
    fn kind(&self) -> DimensionKind;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimension_kind_display() {
        assert_eq!(DimensionKind::Vector.to_string(), "vector");
        assert_eq!(DimensionKind::Lexical.to_string(), "lexical");
        assert_eq!(DimensionKind::Temporal.to_string(), "temporal");
        assert_eq!(DimensionKind::Graph.to_string(), "graph");
        assert_eq!(DimensionKind::Structural.to_string(), "structural");
        assert_eq!(DimensionKind::Global.to_string(), "global");
    }

    #[test]
    fn search_query_default() {
        let q = SearchQuery::default();
        assert!(q.text.is_empty());
        assert!(q.embedding.is_none());
        assert!(q.time_range.is_none());
        assert!(q.seed_nodes.is_empty());
        assert_eq!(q.strategy, SearchStrategy::Balanced);
        assert_eq!(q.limit, 10);
    }

    #[test]
    fn search_query_builder_style() {
        let q = SearchQuery {
            text: "knowledge graph".to_string(),
            limit: 20,
            ..SearchQuery::default()
        };
        assert_eq!(q.text, "knowledge graph");
        assert_eq!(q.limit, 20);
        assert!(q.embedding.is_none());
    }

    #[test]
    fn dimension_kind_serde_roundtrip() {
        let kind = DimensionKind::Graph;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"graph\"");
        let back: DimensionKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, kind);
    }
}

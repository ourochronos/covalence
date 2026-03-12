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

/// Minimum term length for query matching. Short terms like "in",
/// "of", "to" match too many node names and pollute results.
pub const MIN_QUERY_TERM_LEN: usize = 3;

/// Common stopwords filtered from query matching across dimensions.
pub const STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all",
    "can", "has", "her", "was", "one", "our", "out", "how",
    "its", "may", "use", "who", "did", "get", "let", "say",
    "she", "too", "via", "from", "with", "this", "that",
    "what", "when", "will", "been", "have", "each", "make",
    "like", "does", "into", "them", "then", "than", "more",
    "some", "such", "also", "about", "which", "their",
    "would", "there", "these", "other", "could", "should",
];

/// Extract filtered query terms from raw text.
///
/// Splits on whitespace, lowercases, removes terms shorter than
/// `MIN_QUERY_TERM_LEN`, and removes stopwords. Used by graph
/// and structural dimensions for node name matching.
pub fn extract_query_terms(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|t| t.to_lowercase())
        .filter(|t| t.len() >= MIN_QUERY_TERM_LEN)
        .filter(|t| !STOPWORDS.contains(&t.as_str()))
        .collect()
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
        assert_eq!(q.strategy, SearchStrategy::Auto);
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
    fn extract_query_terms_filters_stopwords_and_short() {
        let terms = extract_query_terms("the Rust for async runtime");
        // "the" (stopword), "for" (stopword) filtered;
        // "rust", "async", "runtime" survive.
        assert_eq!(terms, vec!["rust", "async", "runtime"]);
    }

    #[test]
    fn extract_query_terms_empty() {
        assert!(extract_query_terms("").is_empty());
        assert!(extract_query_terms("  ").is_empty());
    }

    #[test]
    fn extract_query_terms_all_stopwords() {
        assert!(extract_query_terms("the and for").is_empty());
    }

    #[test]
    fn extract_query_terms_short_terms() {
        // "in" and "of" are < MIN_QUERY_TERM_LEN.
        assert!(extract_query_terms("in of").is_empty());
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

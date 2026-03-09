//! Search engine — multi-dimensional retrieval with RRF fusion.

pub mod abstention;
pub mod cache;
pub mod context;
pub mod dimensions;
pub mod expansion;
pub mod fusion;
pub mod rerank;
pub mod skewroute;
pub mod strategy;

pub use cache::{CacheConfig, CachedResponse};
pub use context::{AssembledContext, ContextConfig, ContextItem, RawContextItem, assemble_context};
pub use dimensions::{DimensionKind, SearchDimension, SearchQuery};
pub use expansion::{ExpandedQuery, expand_query};
pub use fusion::{DEFAULT_K, FusedResult, SearchResult, rrf_fuse};
pub use rerank::{NoopReranker, RerankConfig, RerankedResult, Reranker};
pub use skewroute::{gini_coefficient, select_strategy};
pub use strategy::{DimensionWeights, SearchStrategy};

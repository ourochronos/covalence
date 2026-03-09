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
pub mod trace;

pub use abstention::{AbstentionCheck, AbstentionConfig, check_abstention};
pub use cache::{CacheConfig, CachedResponse, QueryCache};
pub use context::{AssembledContext, ContextConfig, ContextItem, RawContextItem, assemble_context};
pub use dimensions::{DimensionKind, SearchDimension, SearchQuery};
pub use expansion::{ExpandedQuery, SpreadingResult, expand_query, spreading_activation};
pub use fusion::{DEFAULT_K, FusedResult, SearchResult, rrf_fuse};
pub use rerank::{HttpReranker, NoopReranker, RerankConfig, RerankedResult, Reranker};
pub use skewroute::{gini_coefficient, select_strategy};
pub use strategy::{DimensionWeights, SearchStrategy};
pub use trace::QueryTrace;

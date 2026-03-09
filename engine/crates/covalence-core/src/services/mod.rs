//! Service layer — business logic orchestrating storage and graph operations.

pub mod admin;
pub mod article;
pub mod consolidation;
pub mod edge;
pub mod memory;
pub mod node;
pub mod search;
pub mod source;

pub use admin::AdminService;
pub use article::ArticleService;
pub use consolidation::GraphDeepConsolidator;
pub use edge::EdgeService;
pub use node::NodeService;
pub use search::{SearchFilters, SearchService};
pub use source::SourceService;

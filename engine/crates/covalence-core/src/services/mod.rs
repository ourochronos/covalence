//! Service layer — business logic orchestrating storage and graph operations.

pub mod admin;
pub mod analysis;
pub mod article;
pub mod ask;
pub mod chunk_quality;
pub mod consolidation;
pub mod edge;
pub mod health;
pub(crate) mod ingestion_helpers;
pub mod memory;
pub mod node;
pub(crate) mod noise_filter;
pub(crate) mod pipeline;
pub(crate) mod prompts;
pub mod queue;
pub mod search;
pub(crate) mod search_helpers;
pub mod source;
pub(crate) mod statement_pipeline;

pub use admin::{AdminService, CooccurrenceResult, GcResult, InvalidatedEdgeStats};
pub use analysis::AnalysisService;
pub use article::ArticleService;
pub use ask::{AskResponse, AskService, Citation};
pub use consolidation::GraphDeepConsolidator;
pub use edge::EdgeService;
pub use health::{ConfigAudit, SidecarHealth};
pub use node::{NodeExplanation, NodeService};
pub use queue::RetryQueueService;
pub use search::{SearchFilters, SearchService};
pub use source::SourceService;

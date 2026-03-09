//! Consolidation pipeline — three-timescale knowledge maturation.
//!
//! - Online (seconds): per-source ingestion processing
//! - Batch (hours): topic clustering, article compilation
//! - Deep (daily+): TrustRank, community detection, BMR forgetting

pub mod batch;
pub mod compiler;
pub mod contention;
pub mod deep;
pub mod graph_batch;
pub mod scheduler;
pub mod summary;
pub mod topic;

pub use batch::{BatchConsolidator, BatchJob, BatchStatus};
pub use compiler::{ArticleCompiler, ConcatCompiler, LlmCompiler};
pub use contention::{Contention, detect_contentions};
pub use deep::{DeepConfig, DeepConsolidator, DeepReport};
pub use graph_batch::GraphBatchConsolidator;
pub use scheduler::ConsolidationScheduler;
pub use summary::{
    CommunitySummary, CommunitySummaryInput, ConcatSummaryGenerator, SummaryGenerator,
};
pub use topic::{SourceNodes, cluster_sources_by_community};

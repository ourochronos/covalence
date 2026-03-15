//! Cross-domain analysis service — bridges research, spec, and code domains.
//!
//! Implements the Component model from spec/12-code-ingestion.md and the
//! six analysis capabilities from spec/13-cross-domain-analysis.md.

use std::sync::Arc;

use crate::graph::SharedGraph;
use crate::ingestion::ChatBackend;
use crate::ingestion::Embedder;
use crate::storage::postgres::PgRepo;

mod bootstrap;
mod constants;
mod health;
mod intelligence;
mod tests;

// Re-export all public types so `use crate::services::analysis::*` works.
pub use health::{
    CoverageItem, CoverageResult, DivergentNode, ErosionItem, ErosionResult, WhitespaceGap,
    WhitespaceNode, WhitespaceResult,
};
pub use intelligence::{
    AffectedNode, BlastRadiusHop, BlastRadiusResult, CounterArgument, CritiqueEvidence,
    CritiqueResult, CritiqueSynthesis, SupportingArgument, TargetInfo, VerificationMatch,
    VerificationResult,
};

/// Result of Component bootstrapping.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BootstrapResult {
    /// Components created as new nodes.
    pub components_created: u64,
    /// Components that already existed (skipped).
    pub components_existing: u64,
    /// Components that were embedded.
    pub components_embedded: u64,
}

/// Result of cross-domain linking.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LinkingResult {
    /// PART_OF_COMPONENT edges created (code -> component).
    pub part_of_edges: u64,
    /// IMPLEMENTS_INTENT edges created (component -> spec).
    pub intent_edges: u64,
    /// THEORETICAL_BASIS edges created (component -> research).
    pub basis_edges: u64,
    /// Edges skipped because they already exist.
    pub skipped_existing: u64,
}

/// Cross-domain analysis service.
pub struct AnalysisService {
    repo: Arc<PgRepo>,
    graph: SharedGraph,
    embedder: Option<Arc<dyn Embedder>>,
    chat_backend: Option<Arc<dyn ChatBackend>>,
    node_embed_dim: usize,
}

impl AnalysisService {
    /// Create a new analysis service.
    pub fn new(repo: Arc<PgRepo>, graph: SharedGraph) -> Self {
        Self {
            repo,
            graph,
            embedder: None,
            chat_backend: None,
            node_embed_dim: 256,
        }
    }

    /// Set the embedder for component description embedding.
    pub fn with_embedder(mut self, embedder: Option<Arc<dyn Embedder>>) -> Self {
        self.embedder = embedder;
        self
    }

    /// Set the chat backend for LLM-driven analysis.
    pub fn with_chat_backend(mut self, backend: Option<Arc<dyn ChatBackend>>) -> Self {
        self.chat_backend = backend;
        self
    }

    /// Set the target node embedding dimension.
    pub fn with_node_embed_dim(mut self, dim: usize) -> Self {
        self.node_embed_dim = dim;
        self
    }
}

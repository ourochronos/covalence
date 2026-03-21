//! Cross-domain analysis service — bridges research, spec, and code domains.
//!
//! Implements the Component model from spec/12-code-ingestion.md and the
//! six analysis capabilities from spec/13-cross-domain-analysis.md.

use std::sync::Arc;

use crate::graph::GraphEngine;
use crate::ingestion::ChatBackend;
use crate::ingestion::Embedder;
use crate::storage::postgres::PgRepo;

mod alignment;
mod bootstrap;
mod constants;
mod health;
mod intelligence;
mod tests;

// Re-export all public types so `use crate::services::analysis::*` works.
pub use alignment::{AlignmentItem, AlignmentReport, AlignmentRequest};
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
/// Configurable domain groupings for cross-domain analysis.
///
/// Defines which domains are "internal" (spec-like) vs "external"
/// (research-like) and which entity class represents code entities.
#[derive(Debug, Clone)]
pub struct DomainConfig {
    /// Entity class for code entities.
    pub code_entity_class: String,
    /// Domains that contain specification/design intent.
    pub spec_domains: Vec<String>,
    /// Domains that contain research/external knowledge.
    pub research_domains: Vec<String>,
    /// Domain for code sources.
    pub code_domain: String,
}

impl Default for DomainConfig {
    fn default() -> Self {
        Self {
            code_entity_class: "code".to_string(),
            spec_domains: vec!["spec".to_string(), "design".to_string()],
            research_domains: vec!["research".to_string(), "external".to_string()],
            code_domain: "code".to_string(),
        }
    }
}

/// Configurable bridge relationship types for cross-domain analysis.
///
/// These define how domains are connected in the knowledge graph.
/// Defaults match the current Covalence ontology but can be
/// overridden per-project via the ontology tables.
#[derive(Debug, Clone)]
pub struct BridgeConfig {
    /// Edge type linking code entities to components.
    pub part_of_component: String,
    /// Edge type linking components to spec concepts.
    pub implements_intent: String,
    /// Edge type linking components to research concepts.
    pub theoretical_basis: String,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            part_of_component: "PART_OF_COMPONENT".to_string(),
            implements_intent: "IMPLEMENTS_INTENT".to_string(),
            theoretical_basis: "THEORETICAL_BASIS".to_string(),
        }
    }
}

pub struct AnalysisService {
    repo: Arc<PgRepo>,
    graph: Arc<dyn GraphEngine>,
    embedder: Option<Arc<dyn Embedder>>,
    chat_backend: Option<Arc<dyn ChatBackend>>,
    node_embed_dim: usize,
    /// Configurable bridge relationship types.
    pub(crate) bridges: BridgeConfig,
    /// Configurable domain groupings.
    pub(crate) domains: DomainConfig,
}

impl AnalysisService {
    /// Create a new analysis service.
    pub fn new(repo: Arc<PgRepo>, graph: Arc<dyn GraphEngine>) -> Self {
        Self {
            repo,
            graph,
            embedder: None,
            chat_backend: None,
            node_embed_dim: 256,
            bridges: BridgeConfig::default(),
            domains: DomainConfig::default(),
        }
    }

    /// Set bridge relationship types from the ontology.
    pub fn with_bridges(mut self, bridges: BridgeConfig) -> Self {
        self.bridges = bridges;
        self
    }

    /// Set domain groupings from the ontology.
    pub fn with_domains(mut self, domains: DomainConfig) -> Self {
        self.domains = domains;
        self
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

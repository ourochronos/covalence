//! Admin service — health checks, graph reload, consolidation, metrics.

mod backfill;
mod consolidation;
mod edge_ops;
mod graph_ops;
mod health;
mod knowledge;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use crate::error::Result;
use crate::graph::SharedGraph;
use crate::graph::engine::GraphEngine;
use crate::ingestion::Embedder;
use crate::ingestion::chat_backend::ChatBackend;
use crate::storage::postgres::PgRepo;

// Re-export all public types from submodules.
pub use backfill::{
    BackfillResult, CodeSummaryResult, GcResult, NoiseCleanupResult, NoiseEntityInfo,
    SeedOpinionsResult,
};
pub use consolidation::Metrics;
pub use edge_ops::{BridgeResult, CooccurrenceResult};
pub use graph_ops::{GraphStats, InvalidatedEdgeNode, InvalidatedEdgeStats, InvalidatedEdgeType};
pub use health::{DataHealthReport, HealthStatus};
pub use knowledge::KnowledgeGap;

/// Compute knowledge gap candidates from the graph sidecar.
///
/// Returns tuples of `(uuid, name, type, in_degree, out_degree)` for
/// nodes whose in-degree exceeds `min_in_degree` and out-degree, with
/// labels at least `min_label_length` characters and not in
/// `exclude_types`. Results are sorted by gap score descending and
/// truncated to `limit`.
#[cfg(test)]
pub(crate) fn compute_gap_candidates(
    graph: &petgraph::stable_graph::StableDiGraph<
        crate::graph::sidecar::NodeMeta,
        crate::graph::sidecar::EdgeMeta,
    >,
    min_in_degree: usize,
    min_label_length: usize,
    exclude_types: &[String],
    limit: usize,
) -> Vec<(uuid::Uuid, String, String, usize, usize)> {
    let mut candidates: Vec<(uuid::Uuid, String, String, usize, usize)> = Vec::new();

    for idx in graph.node_indices() {
        let meta = &graph[idx];

        if meta.canonical_name.len() < min_label_length {
            continue;
        }

        if exclude_types.iter().any(|t| t == &meta.node_type) {
            continue;
        }

        let in_deg = graph
            .edges_directed(idx, petgraph::Direction::Incoming)
            .count();
        let out_deg = graph.edges(idx).count();

        if in_deg >= min_in_degree && in_deg > out_deg {
            candidates.push((
                meta.id,
                meta.canonical_name.clone(),
                meta.node_type.clone(),
                in_deg,
                out_deg,
            ));
        }
    }

    candidates.sort_by(|a, b| {
        let score_a = a.3 as f64 - a.4 as f64;
        let score_b = b.3 as f64 - b.4 as f64;
        score_b
            .partial_cmp(&score_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.truncate(limit);
    candidates
}

/// Service for administrative operations.
pub struct AdminService {
    pub(super) repo: Arc<PgRepo>,
    pub(super) graph: Arc<dyn GraphEngine>,
    /// Raw shared graph for write-path operations (consolidation, edge synthesis).
    pub(super) shared_graph: SharedGraph,
    pub(super) embedder: Option<Arc<dyn Embedder>>,
    pub(super) chat_backend: Option<Arc<dyn ChatBackend>>,
    pub(super) config: Option<crate::config::Config>,
}

impl AdminService {
    /// Create a new admin service.
    pub fn new(repo: Arc<PgRepo>, graph: Arc<dyn GraphEngine>, shared_graph: SharedGraph) -> Self {
        Self {
            repo,
            graph,
            shared_graph,
            embedder: None,
            chat_backend: None,
            config: None,
        }
    }

    /// Set the embedder for ontology clustering.
    pub fn with_embedder(mut self, embedder: Option<Arc<dyn Embedder>>) -> Self {
        self.embedder = embedder;
        self
    }

    /// Set the chat backend for LLM operations.
    pub fn with_chat_backend(mut self, backend: Option<Arc<dyn ChatBackend>>) -> Self {
        self.chat_backend = backend;
        self
    }

    /// Set the application configuration for config audit.
    pub fn with_config(mut self, config: crate::config::Config) -> Self {
        self.config = Some(config);
        self
    }

    /// List recent audit log entries.
    pub async fn audit_log(&self, limit: i64) -> Result<Vec<crate::models::audit::AuditLog>> {
        crate::storage::traits::AuditLogRepo::list_recent(&*self.repo, limit).await
    }

    /// List recent search traces.
    pub async fn list_traces(&self, limit: i64) -> Result<Vec<crate::models::trace::SearchTrace>> {
        crate::storage::traits::SearchTraceRepo::list_recent(&*self.repo, limit).await
    }

    /// Get a single search trace by ID.
    pub async fn get_trace(
        &self,
        id: uuid::Uuid,
    ) -> Result<Option<crate::models::trace::SearchTrace>> {
        crate::storage::traits::SearchTraceRepo::get(&*self.repo, id).await
    }

    /// Submit search feedback and log to audit.
    pub async fn submit_feedback(
        &self,
        feedback: crate::models::trace::SearchFeedback,
    ) -> Result<()> {
        use crate::models::audit::{AuditAction, AuditLog};
        use crate::storage::traits::{AuditLogRepo, SearchFeedbackRepo};

        let result_id = feedback.result_id;
        let query_text = feedback.query_text.clone();
        SearchFeedbackRepo::create(&*self.repo, &feedback).await?;

        let audit = AuditLog::new(
            AuditAction::SearchFeedback,
            "api:feedback".to_string(),
            serde_json::json!({
                "query_text": query_text,
                "result_id": result_id,
                "relevance": feedback.relevance,
            }),
        );
        AuditLogRepo::create(&*self.repo, &audit).await?;

        Ok(())
    }
}

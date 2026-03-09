//! Admin service — health checks, graph reload, consolidation, metrics.

use std::sync::Arc;

use std::collections::HashSet;

use petgraph::stable_graph::NodeIndex;
use petgraph::visit::EdgeRef;

use crate::error::Result;
use crate::graph::SharedGraph;
use crate::graph::sync::full_reload;
use crate::models::audit::{AuditAction, AuditLog};
use crate::models::trace::{SearchFeedback, SearchTrace};
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{AuditLogRepo, SearchFeedbackRepo, SearchTraceRepo, SourceRepo};

/// Graph statistics snapshot.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphStats {
    /// Number of nodes in the sidecar.
    pub node_count: usize,
    /// Number of edges in the sidecar.
    pub edge_count: usize,
    /// Graph density (edges / max possible edges).
    pub density: f64,
    /// Number of weakly connected components.
    pub component_count: usize,
}

/// Service metrics snapshot.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Metrics {
    /// Number of nodes in the graph sidecar.
    pub graph_nodes: usize,
    /// Number of edges in the graph sidecar.
    pub graph_edges: usize,
    /// Number of sources in the database.
    pub source_count: i64,
}

/// Health status of the system.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HealthStatus {
    /// Whether the database is reachable.
    pub pg_healthy: bool,
    /// Number of nodes in the sidecar.
    pub sidecar_node_count: usize,
    /// Number of edges in the sidecar.
    pub sidecar_edge_count: usize,
}

/// Count weakly connected components in a `StableDiGraph`.
///
/// `petgraph::algo::connected_components` requires `NodeCompactIndexable`
/// which `StableDiGraph` does not implement (indices may be sparse after
/// removals). This BFS-based implementation works with any graph that
/// supports `node_indices()` and directed edge iteration.
fn count_weak_components(
    graph: &petgraph::stable_graph::StableDiGraph<
        crate::graph::sidecar::NodeMeta,
        crate::graph::sidecar::EdgeMeta,
    >,
) -> usize {
    let mut visited: HashSet<NodeIndex> = HashSet::with_capacity(graph.node_count());
    let mut components = 0usize;

    for start in graph.node_indices() {
        if !visited.insert(start) {
            continue;
        }
        components += 1;
        let mut stack = vec![start];
        while let Some(v) = stack.pop() {
            // Outgoing neighbours
            for edge in graph.edges(v) {
                if visited.insert(edge.target()) {
                    stack.push(edge.target());
                }
            }
            // Incoming neighbours (weak connectivity)
            for edge in graph.edges_directed(v, petgraph::Direction::Incoming) {
                if visited.insert(edge.source()) {
                    stack.push(edge.source());
                }
            }
        }
    }

    components
}

/// Service for administrative operations.
pub struct AdminService {
    repo: Arc<PgRepo>,
    graph: SharedGraph,
}

impl AdminService {
    /// Create a new admin service.
    pub fn new(repo: Arc<PgRepo>, graph: SharedGraph) -> Self {
        Self { repo, graph }
    }

    /// Get graph statistics from the sidecar.
    pub async fn graph_stats(&self) -> GraphStats {
        let g = self.graph.read().await;
        let n = g.node_count();
        let e = g.edge_count();
        let density = if n > 1 {
            e as f64 / (n as f64 * (n as f64 - 1.0))
        } else {
            0.0
        };
        let component_count = count_weak_components(g.graph());
        GraphStats {
            node_count: n,
            edge_count: e,
            density,
            component_count,
        }
    }

    /// Reload the graph sidecar from PG.
    pub async fn reload_graph(&self) -> Result<GraphStats> {
        full_reload(self.repo.pool(), self.graph.clone()).await?;
        Ok(self.graph_stats().await)
    }

    /// Check system health.
    pub async fn health(&self) -> HealthStatus {
        let pg_healthy = sqlx::query("SELECT 1")
            .execute(self.repo.pool())
            .await
            .is_ok();

        let g = self.graph.read().await;
        HealthStatus {
            pg_healthy,
            sidecar_node_count: g.node_count(),
            sidecar_edge_count: g.edge_count(),
        }
    }

    /// Trigger batch consolidation.
    // TODO: Wire to consolidation::batch module.
    pub async fn trigger_consolidation(&self) -> Result<()> {
        Ok(())
    }

    /// Get service metrics: graph node/edge counts and source count.
    pub async fn metrics(&self) -> Result<Metrics> {
        let stats = self.graph_stats().await;
        let source_count = SourceRepo::count(&*self.repo).await?;
        Ok(Metrics {
            graph_nodes: stats.node_count,
            graph_edges: stats.edge_count,
            source_count,
        })
    }

    /// List recent audit log entries.
    pub async fn audit_log(&self, limit: i64) -> Result<Vec<AuditLog>> {
        AuditLogRepo::list_recent(&*self.repo, limit).await
    }

    /// List recent search traces.
    pub async fn list_traces(&self, limit: i64) -> Result<Vec<SearchTrace>> {
        SearchTraceRepo::list_recent(&*self.repo, limit).await
    }

    /// Get a single search trace by ID.
    pub async fn get_trace(&self, id: uuid::Uuid) -> Result<Option<SearchTrace>> {
        SearchTraceRepo::get(&*self.repo, id).await
    }

    /// Submit search feedback and log to audit.
    pub async fn submit_feedback(&self, feedback: SearchFeedback) -> Result<()> {
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

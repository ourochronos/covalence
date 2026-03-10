//! Admin service — health checks, graph reload, consolidation, metrics.

use std::sync::Arc;

use std::collections::HashSet;

use petgraph::stable_graph::NodeIndex;
use petgraph::visit::EdgeRef;

use crate::consolidation::batch::BatchJob;
use crate::consolidation::graph_batch::GraphBatchConsolidator;
use crate::consolidation::ontology::{
    self, ClusterLevel, ClusterResult, build_entity_clusters, build_rel_type_clusters,
    build_type_clusters,
};
use crate::consolidation::{BatchConsolidator, BatchStatus};
use crate::error::{Error, Result};
use crate::graph::SharedGraph;
use crate::graph::sync::full_reload;
use crate::ingestion::Embedder;
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
    embedder: Option<Arc<dyn Embedder>>,
}

impl AdminService {
    /// Create a new admin service.
    pub fn new(repo: Arc<PgRepo>, graph: SharedGraph) -> Self {
        Self {
            repo,
            graph,
            embedder: None,
        }
    }

    /// Set the embedder for ontology clustering.
    pub fn with_embedder(mut self, embedder: Option<Arc<dyn Embedder>>) -> Self {
        self.embedder = embedder;
        self
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

    /// Trigger batch consolidation over all sources.
    ///
    /// Collects all source IDs, constructs a `BatchJob`, and runs
    /// it through the `GraphBatchConsolidator`.
    pub async fn trigger_consolidation(&self) -> Result<()> {
        let sources = SourceRepo::list(&*self.repo, 1000, 0).await?;
        if sources.is_empty() {
            return Ok(());
        }
        let source_ids: Vec<_> = sources.iter().map(|s| s.id).collect();
        let mut job = BatchJob {
            id: uuid::Uuid::new_v4(),
            source_ids,
            status: BatchStatus::Pending,
            created_at: chrono::Utc::now(),
            completed_at: None,
        };
        let consolidator = GraphBatchConsolidator::new(
            Arc::clone(&self.repo),
            Arc::clone(&self.graph),
            None,
            None,
        );
        consolidator.run_batch(&mut job).await?;
        tracing::info!(
            job_id = %job.id,
            status = ?job.status,
            "batch consolidation completed"
        );
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

    /// Run ontology clustering at the specified level(s) using
    /// HDBSCAN.
    ///
    /// When `dry_run` is true, returns discovered clusters without
    /// writing them back to the database. When false, stores cluster
    /// definitions and updates canonical labels on nodes/edges, then
    /// reloads the graph sidecar.
    ///
    /// `min_cluster_size` controls the minimum number of labels
    /// required to form a cluster (default: 2). Labels that don't
    /// belong to any cluster are returned as noise.
    pub async fn cluster_ontology(
        &self,
        level: Option<ClusterLevel>,
        min_cluster_size: usize,
        dry_run: bool,
    ) -> Result<ClusterResult> {
        let embedder = self.embedder.as_ref().ok_or_else(|| {
            Error::Config("no embedder configured for ontology clustering".into())
        })?;

        let pool = self.repo.pool();
        let mut combined = ClusterResult {
            clusters: Vec::new(),
            noise_labels: Vec::new(),
        };

        let do_entity = level.is_none() || level == Some(ClusterLevel::Entity);
        let do_type = level.is_none() || level == Some(ClusterLevel::EntityType);
        let do_rel = level.is_none() || level == Some(ClusterLevel::RelationType);

        if do_entity {
            let r = build_entity_clusters(pool, embedder.as_ref(), min_cluster_size).await?;
            tracing::info!(
                clusters = r.clusters.len(),
                noise = r.noise_labels.len(),
                "entity name clusters discovered"
            );
            combined.clusters.extend(r.clusters);
            combined.noise_labels.extend(r.noise_labels);
        }
        if do_type {
            let r = build_type_clusters(pool, embedder.as_ref(), min_cluster_size).await?;
            tracing::info!(
                clusters = r.clusters.len(),
                noise = r.noise_labels.len(),
                "entity type clusters discovered"
            );
            combined.clusters.extend(r.clusters);
            combined.noise_labels.extend(r.noise_labels);
        }
        if do_rel {
            let r = build_rel_type_clusters(pool, embedder.as_ref(), min_cluster_size).await?;
            tracing::info!(
                clusters = r.clusters.len(),
                noise = r.noise_labels.len(),
                "relationship type clusters discovered"
            );
            combined.clusters.extend(r.clusters);
            combined.noise_labels.extend(r.noise_labels);
        }

        if !dry_run && !combined.clusters.is_empty() {
            ontology::apply_clusters(pool, &combined.clusters, min_cluster_size).await?;
            tracing::info!(
                total = combined.clusters.len(),
                "ontology clusters applied, reloading graph"
            );
            full_reload(self.repo.pool(), self.graph.clone()).await?;
        }

        Ok(combined)
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

//! Admin service — health checks, graph reload, consolidation, metrics.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use petgraph::stable_graph::NodeIndex;
use petgraph::visit::EdgeRef;
use sqlx::Row;

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
use crate::storage::traits::{
    AuditLogRepo, EdgeRepo, NodeAliasRepo, NodeRepo, SearchFeedbackRepo, SearchTraceRepo,
    SourceRepo,
};

/// Graph statistics snapshot.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphStats {
    /// Number of nodes in the sidecar.
    pub node_count: usize,
    /// Number of edges in the sidecar.
    pub edge_count: usize,
    /// Number of semantic (non-synthetic) edges.
    pub semantic_edge_count: usize,
    /// Number of synthetic (co-occurrence) edges.
    pub synthetic_edge_count: usize,
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
    /// Number of semantic (non-synthetic) edges.
    pub semantic_edge_count: usize,
    /// Number of synthetic (co-occurrence) edges.
    pub synthetic_edge_count: usize,
    /// Number of weakly connected components.
    pub component_count: usize,
    /// Number of sources in the database.
    pub source_count: i64,
    /// Number of chunks in the database.
    pub chunk_count: i64,
    /// Number of articles in the database.
    pub article_count: i64,
    /// Number of search traces in the database.
    pub search_trace_count: i64,
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

/// Result of a provenance-based garbage collection pass.
///
/// Nodes that lost all active (non-superseded) extraction grounding
/// are evicted along with their edges and aliases.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GcResult {
    /// Number of ungrounded nodes evicted.
    pub nodes_evicted: u64,
    /// Number of edges removed (from evicted nodes).
    pub edges_removed: u64,
    /// Number of aliases removed (from evicted nodes).
    pub aliases_removed: u64,
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

/// A knowledge gap — an entity frequently referenced but never explained.
#[derive(Debug, Clone, serde::Serialize)]
pub struct KnowledgeGap {
    /// Node UUID.
    pub node_id: uuid::Uuid,
    /// Canonical entity name.
    pub canonical_name: String,
    /// Entity type (e.g. "concept", "entity").
    pub node_type: String,
    /// Number of incoming edges (references to this entity).
    pub in_degree: usize,
    /// Number of outgoing edges (explanations from this entity).
    pub out_degree: usize,
    /// Gap score: in_degree - out_degree (higher = bigger gap).
    pub gap_score: f64,
    /// Source URIs that reference this entity.
    pub referenced_by: Vec<String>,
}

/// Result of co-occurrence edge synthesis.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CooccurrenceResult {
    /// Number of synthetic edges created.
    pub edges_created: u64,
    /// Number of candidate pairs evaluated.
    pub candidates_evaluated: u64,
}

/// Compute knowledge gap candidates from the graph sidecar.
///
/// Returns tuples of `(uuid, name, type, in_degree, out_degree)` for
/// nodes whose in-degree exceeds `min_in_degree` and out-degree, with
/// labels at least `min_label_length` characters and not in
/// `exclude_types`. Results are sorted by gap score descending and
/// truncated to `limit`.
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
    repo: Arc<PgRepo>,
    graph: SharedGraph,
    embedder: Option<Arc<dyn Embedder>>,
    config: Option<crate::config::Config>,
}

impl AdminService {
    /// Create a new admin service.
    pub fn new(repo: Arc<PgRepo>, graph: SharedGraph) -> Self {
        Self {
            repo,
            graph,
            embedder: None,
            config: None,
        }
    }

    /// Set the embedder for ontology clustering.
    pub fn with_embedder(mut self, embedder: Option<Arc<dyn Embedder>>) -> Self {
        self.embedder = embedder;
        self
    }

    /// Set the application configuration for config audit.
    pub fn with_config(mut self, config: crate::config::Config) -> Self {
        self.config = Some(config);
        self
    }

    /// Run a full configuration audit.
    ///
    /// Checks sidecar health, summarizes the current configuration,
    /// and generates warnings for potential issues. Returns
    /// `Error::Config` if no configuration has been set.
    pub async fn config_audit(&self) -> Result<super::health::ConfigAudit> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| Error::Config("no configuration set on AdminService".into()))?;
        Ok(super::health::run_config_audit(config).await)
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

        // Count synthetic vs semantic edges
        let synthetic_edge_count = g.graph().edge_weights().filter(|e| e.is_synthetic).count();
        let semantic_edge_count = e - synthetic_edge_count;

        GraphStats {
            node_count: n,
            edge_count: e,
            semantic_edge_count,
            synthetic_edge_count,
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
        // Wire up LLM compiler if chat API keys are configured.
        let compiler: Option<Arc<dyn crate::consolidation::compiler::ArticleCompiler>> =
            self.config.as_ref().and_then(|cfg| {
                cfg.chat_api_key.as_ref().map(|key| {
                    let base = cfg
                        .chat_base_url
                        .clone()
                        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
                    Arc::new(crate::consolidation::compiler::LlmCompiler::new(
                        base,
                        key.clone(),
                        cfg.chat_model.clone(),
                    ))
                        as Arc<dyn crate::consolidation::compiler::ArticleCompiler>
                })
            });
        let mut consolidator = GraphBatchConsolidator::new(
            Arc::clone(&self.repo),
            Arc::clone(&self.graph),
            compiler,
            self.embedder.clone(),
        );
        if let Some(ref cfg) = self.config {
            consolidator = consolidator.with_table_dims(cfg.embedding.table_dims.clone());
        }
        consolidator.run_batch(&mut job).await?;
        tracing::info!(
            job_id = %job.id,
            status = ?job.status,
            "batch consolidation completed"
        );
        Ok(())
    }

    /// Get service metrics: graph stats, entity counts, trace count.
    pub async fn metrics(&self) -> Result<Metrics> {
        let stats = self.graph_stats().await;
        let source_count = SourceRepo::count(&*self.repo).await?;

        let chunk_row = sqlx::query("SELECT COUNT(*) as count FROM chunks")
            .fetch_one(self.repo.pool())
            .await?;
        let chunk_count: i64 = chunk_row.get("count");

        let article_row = sqlx::query("SELECT COUNT(*) as count FROM articles")
            .fetch_one(self.repo.pool())
            .await?;
        let article_count: i64 = article_row.get("count");

        let trace_row = sqlx::query("SELECT COUNT(*) as count FROM search_traces")
            .fetch_one(self.repo.pool())
            .await?;
        let search_trace_count: i64 = trace_row.get("count");

        Ok(Metrics {
            graph_nodes: stats.node_count,
            graph_edges: stats.edge_count,
            semantic_edge_count: stats.semantic_edge_count,
            synthetic_edge_count: stats.synthetic_edge_count,
            component_count: stats.component_count,
            source_count,
            chunk_count,
            article_count,
            search_trace_count,
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

    /// Identify knowledge gaps — entities with high in-degree but low
    /// out-degree. These are concepts the system references frequently
    /// but has no source material explaining.
    ///
    /// Gap score = `in_degree - out_degree`. Entities with zero
    /// out-degree and high in-degree represent the biggest blind spots.
    pub async fn knowledge_gaps(
        &self,
        min_in_degree: usize,
        min_label_length: usize,
        exclude_types: &[String],
        limit: usize,
    ) -> Result<Vec<KnowledgeGap>> {
        // Phase 1: compute in/out degree from graph sidecar.
        let g = self.graph.read().await;
        let candidates = compute_gap_candidates(
            g.graph(),
            min_in_degree,
            min_label_length,
            exclude_types,
            limit,
        );
        drop(g);

        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        // Phase 2: batch-fetch source URIs for the gap nodes.
        let node_ids: Vec<uuid::Uuid> = candidates.iter().map(|c| c.0).collect();
        let rows = sqlx::query_as::<_, (uuid::Uuid, Option<String>, Option<String>)>(
            "SELECT DISTINCT e.entity_id, s.uri, s.title \
             FROM extractions e \
             JOIN chunks c ON c.id = e.chunk_id \
             JOIN sources s ON s.id = c.source_id \
             WHERE e.entity_type = 'node' \
               AND e.entity_id = ANY($1) \
               AND e.is_superseded = false",
        )
        .bind(&node_ids)
        .fetch_all(self.repo.pool())
        .await?;

        let mut refs_map: HashMap<uuid::Uuid, Vec<String>> = HashMap::new();
        for (node_id, uri, title) in rows {
            let label = uri.or(title).unwrap_or_else(|| node_id.to_string());
            refs_map.entry(node_id).or_default().push(label);
        }

        // Build final results.
        let gaps = candidates
            .into_iter()
            .map(|(id, name, ntype, in_deg, out_deg)| {
                let gap_score = in_deg as f64 - out_deg as f64;
                let referenced_by = refs_map.remove(&id).unwrap_or_default();
                KnowledgeGap {
                    node_id: id,
                    canonical_name: name,
                    node_type: ntype,
                    in_degree: in_deg,
                    out_degree: out_deg,
                    gap_score,
                    referenced_by,
                }
            })
            .collect();

        Ok(gaps)
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

    /// Synthesize co-occurrence edges from extraction provenance.
    ///
    /// Entities extracted from the same chunk co-occur in the source
    /// text. This method creates `co_occurs` edges between entity
    /// pairs that share at least `min_cooccurrences` chunks and where
    /// at least one entity has degree ≤ `max_degree` (poorly connected).
    ///
    /// Edges are marked `is_synthetic = true` with weight proportional
    /// to co-occurrence frequency. Existing edges (of any type) between
    /// the pair are respected — no duplicates are created.
    ///
    /// Returns counts of edges created vs skipped.
    pub async fn synthesize_cooccurrence_edges(
        &self,
        min_cooccurrences: i64,
        max_degree: i64,
    ) -> Result<CooccurrenceResult> {
        // Single SQL query: find co-occurring entity pairs where at
        // least one entity is poorly connected. This avoids pulling
        // all 151K pairs into Rust — the DB does the heavy filtering.
        let rows: Vec<(uuid::Uuid, uuid::Uuid, i64)> = sqlx::query_as(
            "WITH pair_freq AS ( \
                SELECT e1.entity_id AS n1, e2.entity_id AS n2, \
                       count(DISTINCT e1.chunk_id) AS freq \
                FROM extractions e1 \
                JOIN extractions e2 \
                  ON e1.chunk_id = e2.chunk_id \
                 AND e1.entity_id < e2.entity_id \
                WHERE e1.entity_type = 'node' \
                  AND e2.entity_type = 'node' \
                  AND e1.is_superseded = false \
                  AND e2.is_superseded = false \
                GROUP BY e1.entity_id, e2.entity_id \
                HAVING count(DISTINCT e1.chunk_id) >= $1 \
            ), \
            node_degree AS ( \
                SELECT n.id, \
                       (SELECT count(*) FROM edges e \
                        WHERE e.source_node_id = n.id \
                           OR e.target_node_id = n.id) AS deg \
                FROM nodes n \
            ) \
            SELECT pf.n1, pf.n2, pf.freq \
            FROM pair_freq pf \
            JOIN node_degree d1 ON d1.id = pf.n1 \
            JOIN node_degree d2 ON d2.id = pf.n2 \
            WHERE (d1.deg <= $2 OR d2.deg <= $2) \
              AND NOT EXISTS ( \
                  SELECT 1 FROM edges e \
                  WHERE (e.source_node_id = pf.n1 AND e.target_node_id = pf.n2) \
                     OR (e.source_node_id = pf.n2 AND e.target_node_id = pf.n1) \
              )",
        )
        .bind(min_cooccurrences)
        .bind(max_degree)
        .fetch_all(self.repo.pool())
        .await?;

        let total_candidates = rows.len() as u64;
        let mut edges_created: u64 = 0;

        for (n1, n2, freq) in &rows {
            let source_id = crate::types::ids::NodeId::from_uuid(*n1);
            let target_id = crate::types::ids::NodeId::from_uuid(*n2);

            let mut edge =
                crate::models::edge::Edge::new(source_id, target_id, "co_occurs".to_string());
            edge.is_synthetic = true;
            // Weight: normalized co-occurrence frequency, capped at 1.0.
            edge.weight = (*freq as f64 / 5.0).min(1.0);
            // Confidence: proportional to frequency, lower baseline.
            edge.confidence = (0.3 + (*freq as f64 * 0.1)).min(0.9);
            edge.properties = serde_json::json!({
                "cooccurrence_count": freq,
                "synthesis_method": "extraction_provenance",
            });

            EdgeRepo::create(&*self.repo, &edge).await?;
            edges_created += 1;
        }

        if edges_created > 0 {
            tracing::info!(
                edges_created,
                total_candidates,
                min_cooccurrences,
                max_degree,
                "co-occurrence edge synthesis complete, reloading graph"
            );
            full_reload(self.repo.pool(), self.graph.clone()).await?;
        } else {
            tracing::info!("co-occurrence synthesis: no new edges to create");
        }

        // Log the operation.
        let audit = AuditLog::new(
            AuditAction::AdminAction,
            "admin:synthesize_cooccurrence".to_string(),
            serde_json::json!({
                "edges_created": edges_created,
                "total_candidates": total_candidates,
                "min_cooccurrences": min_cooccurrences,
                "max_degree": max_degree,
            }),
        );
        AuditLogRepo::create(&*self.repo, &audit).await?;

        Ok(CooccurrenceResult {
            edges_created,
            candidates_evaluated: total_candidates,
        })
    }

    /// Run provenance-based garbage collection.
    ///
    /// Finds all nodes where every extraction has been superseded
    /// (no active extractions remain) and evicts them along with
    /// their edges and aliases. Returns counts of evicted entities.
    pub async fn garbage_collect_nodes(&self) -> Result<GcResult> {
        let ungrounded = NodeRepo::list_ungrounded(&*self.repo).await?;

        if ungrounded.is_empty() {
            tracing::info!("gc: no ungrounded nodes found");
            return Ok(GcResult {
                nodes_evicted: 0,
                edges_removed: 0,
                aliases_removed: 0,
            });
        }

        tracing::info!(count = ungrounded.len(), "gc: evicting ungrounded nodes");

        let mut nodes_evicted: u64 = 0;
        let mut edges_removed: u64 = 0;
        let mut aliases_removed: u64 = 0;

        for node in &ungrounded {
            // Delete aliases first (no FK constraints from aliases
            // to edges, but clean up before node deletion).
            aliases_removed += NodeAliasRepo::delete_by_node(&*self.repo, node.id).await?;

            // Delete edges involving this node.
            edges_removed += EdgeRepo::delete_by_node(&*self.repo, node.id).await?;

            // Delete the node itself.
            if NodeRepo::delete(&*self.repo, node.id).await? {
                nodes_evicted += 1;
            }
        }

        tracing::info!(
            nodes_evicted,
            edges_removed,
            aliases_removed,
            "gc: provenance-based garbage collection complete"
        );

        Ok(GcResult {
            nodes_evicted,
            edges_removed,
            aliases_removed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::sidecar::{EdgeMeta, GraphSidecar, NodeMeta};

    fn make_node(name: &str, ntype: &str) -> NodeMeta {
        NodeMeta {
            id: uuid::Uuid::new_v4(),
            node_type: ntype.into(),
            canonical_name: name.into(),
            clearance_level: 0,
        }
    }

    fn make_edge() -> EdgeMeta {
        EdgeMeta {
            id: uuid::Uuid::new_v4(),
            rel_type: "related_to".into(),
            weight: 1.0,
            confidence: 0.9,
            causal_level: None,
            clearance_level: 0,
            is_synthetic: false,
        }
    }

    /// Build a graph with a clear knowledge gap: "Subjective Logic"
    /// has 4 incoming edges (referenced by A, B, C, D) but 0
    /// outgoing edges.
    fn build_gap_graph() -> GraphSidecar {
        let mut g = GraphSidecar::new();

        let gap_node = make_node("Subjective Logic", "concept");
        let gap_id = gap_node.id;
        g.add_node(gap_node).unwrap();

        // 4 nodes that reference the gap node.
        for name in &[
            "Epistemic Model",
            "Opinion Fusion",
            "Trust Framework",
            "Dempster-Shafer",
        ] {
            let n = make_node(name, "concept");
            let nid = n.id;
            g.add_node(n).unwrap();
            g.add_edge(nid, gap_id, make_edge()).unwrap();
        }

        // A well-explained node with both in and out edges.
        let explained = make_node("Bayesian Inference", "concept");
        let explained_id = explained.id;
        g.add_node(explained).unwrap();
        g.add_edge(explained_id, gap_id, make_edge()).unwrap();

        // Give "Bayesian Inference" outgoing edges so it's NOT a gap.
        let target = make_node("Probability Theory", "concept");
        let target_id = target.id;
        g.add_node(target).unwrap();
        g.add_edge(explained_id, target_id, make_edge()).unwrap();

        g
    }

    #[test]
    fn detect_knowledge_gap() {
        let g = build_gap_graph();
        let candidates = compute_gap_candidates(
            g.graph(),
            3, // min_in_degree
            4, // min_label_length
            &[],
            20,
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].1, "Subjective Logic");
        assert_eq!(candidates[0].3, 5); // in_degree
        assert_eq!(candidates[0].4, 0); // out_degree
    }

    #[test]
    fn min_in_degree_filter() {
        let g = build_gap_graph();

        // Require 6 in-degree — no gaps qualify.
        let candidates = compute_gap_candidates(g.graph(), 6, 4, &[], 20);
        assert!(candidates.is_empty());

        // Require 5 — exactly matches.
        let candidates = compute_gap_candidates(g.graph(), 5, 4, &[], 20);
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn exclude_types_filter() {
        let g = build_gap_graph();

        // Exclude "concept" — no gaps.
        let exclude = vec!["concept".to_string()];
        let candidates = compute_gap_candidates(g.graph(), 3, 4, &exclude, 20);
        assert!(candidates.is_empty());
    }

    #[test]
    fn min_label_length_filter() {
        let mut g = GraphSidecar::new();
        let short = make_node("AI", "concept");
        let short_id = short.id;
        g.add_node(short).unwrap();

        // 3 nodes referencing "AI".
        for name in &["Machine Learning", "Deep Learning", "Neural Networks"] {
            let n = make_node(name, "concept");
            let nid = n.id;
            g.add_node(n).unwrap();
            g.add_edge(nid, short_id, make_edge()).unwrap();
        }

        // "AI" has 3 in-degree but name length < 4.
        let candidates = compute_gap_candidates(g.graph(), 3, 4, &[], 20);
        assert!(candidates.is_empty());

        // With min_label_length=2, it shows up.
        let candidates = compute_gap_candidates(g.graph(), 3, 2, &[], 20);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].1, "AI");
    }

    #[test]
    fn limit_truncates_results() {
        let mut g = GraphSidecar::new();

        // Create 5 gap nodes each with 3 incoming edges.
        for gap_name in &[
            "Alpha Gap",
            "Beta Gap",
            "Gamma Gap",
            "Delta Gap",
            "Epsilon Gap",
        ] {
            let gap = make_node(gap_name, "concept");
            let gap_id = gap.id;
            g.add_node(gap).unwrap();
            for i in 0..3 {
                let src = make_node(&format!("{gap_name}-ref-{i}"), "entity");
                let src_id = src.id;
                g.add_node(src).unwrap();
                g.add_edge(src_id, gap_id, make_edge()).unwrap();
            }
        }

        let candidates = compute_gap_candidates(g.graph(), 3, 4, &[], 2);
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn sorted_by_gap_score_descending() {
        let mut g = GraphSidecar::new();

        // "Small Gap" has 3 in-degree.
        let small = make_node("Small Gap Node", "concept");
        let small_id = small.id;
        g.add_node(small).unwrap();
        for i in 0..3 {
            let src = make_node(&format!("small-ref-{i}"), "entity");
            let src_id = src.id;
            g.add_node(src).unwrap();
            g.add_edge(src_id, small_id, make_edge()).unwrap();
        }

        // "Big Gap" has 6 in-degree.
        let big = make_node("Big Gap Node", "concept");
        let big_id = big.id;
        g.add_node(big).unwrap();
        for i in 0..6 {
            let src = make_node(&format!("big-ref-{i}"), "entity");
            let src_id = src.id;
            g.add_node(src).unwrap();
            g.add_edge(src_id, big_id, make_edge()).unwrap();
        }

        let candidates = compute_gap_candidates(g.graph(), 3, 4, &[], 20);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].1, "Big Gap Node");
        assert_eq!(candidates[1].1, "Small Gap Node");
    }

    #[test]
    fn no_gap_when_out_degree_matches() {
        let mut g = GraphSidecar::new();

        // Node with 3 in and 3 out — not a gap.
        let balanced = make_node("Balanced Node", "concept");
        let balanced_id = balanced.id;
        g.add_node(balanced).unwrap();

        for i in 0..3 {
            let src = make_node(&format!("src-{i}"), "entity");
            let src_id = src.id;
            g.add_node(src).unwrap();
            g.add_edge(src_id, balanced_id, make_edge()).unwrap();

            let tgt = make_node(&format!("tgt-{i}"), "entity");
            let tgt_id = tgt.id;
            g.add_node(tgt).unwrap();
            g.add_edge(balanced_id, tgt_id, make_edge()).unwrap();
        }

        let candidates = compute_gap_candidates(g.graph(), 3, 4, &[], 20);
        assert!(candidates.is_empty());
    }

    #[test]
    fn empty_graph_returns_no_gaps() {
        let g = GraphSidecar::new();
        let candidates = compute_gap_candidates(g.graph(), 3, 4, &[], 20);
        assert!(candidates.is_empty());
    }

    // --- GcResult tests ---

    #[test]
    fn gc_result_serializes_all_fields() {
        let result = GcResult {
            nodes_evicted: 5,
            edges_removed: 12,
            aliases_removed: 8,
        };
        let json = serde_json::to_value(&result).expect("serialize");
        assert_eq!(json["nodes_evicted"], 5);
        assert_eq!(json["edges_removed"], 12);
        assert_eq!(json["aliases_removed"], 8);
    }

    #[test]
    fn gc_result_zero_counts() {
        let result = GcResult {
            nodes_evicted: 0,
            edges_removed: 0,
            aliases_removed: 0,
        };
        let json = serde_json::to_value(&result).expect("serialize");
        assert_eq!(json["nodes_evicted"], 0);
        assert_eq!(json["edges_removed"], 0);
        assert_eq!(json["aliases_removed"], 0);
    }

    #[test]
    fn gc_result_debug_impl() {
        let result = GcResult {
            nodes_evicted: 3,
            edges_removed: 7,
            aliases_removed: 2,
        };
        let debug = format!("{result:?}");
        assert!(debug.contains("nodes_evicted: 3"));
        assert!(debug.contains("edges_removed: 7"));
        assert!(debug.contains("aliases_removed: 2"));
    }

    #[test]
    fn gc_result_clone() {
        let result = GcResult {
            nodes_evicted: 10,
            edges_removed: 20,
            aliases_removed: 5,
        };
        let cloned = result.clone();
        assert_eq!(cloned.nodes_evicted, result.nodes_evicted);
        assert_eq!(cloned.edges_removed, result.edges_removed);
        assert_eq!(cloned.aliases_removed, result.aliases_removed);
    }
}

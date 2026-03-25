//! Admin service — health checks, graph reload, consolidation, metrics.

use std::collections::HashMap;
use std::sync::Arc;

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
use crate::graph::engine::GraphEngine;
use crate::ingestion::Embedder;
use crate::ingestion::chat_backend::ChatBackend;
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
    /// Number of RAPTOR summary chunks.
    pub summary_chunk_count: i64,
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

/// Data health report — preview of what's stale, orphaned, or
/// duplicated without modifying anything.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DataHealthReport {
    /// Sources superseded by newer versions.
    pub superseded_sources: u64,
    /// Chunks belonging to superseded sources.
    pub superseded_chunks: u64,
    /// Nodes with no extraction provenance.
    pub orphan_nodes: u64,
    /// Orphan nodes that still have edges (load-bearing).
    pub orphan_nodes_with_edges: u64,
    /// Duplicate sources (same title, same domain).
    pub duplicate_sources: u64,
    /// Nodes with no embedding.
    pub unembedded_nodes: u64,
    /// Code entities missing semantic summaries.
    pub unsummarized_code_entities: u64,
    /// Sources missing summaries.
    pub unsummarized_sources: u64,
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

/// A noise entity identified for cleanup.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NoiseEntityInfo {
    /// Node UUID.
    pub node_id: uuid::Uuid,
    /// Canonical entity name.
    pub canonical_name: String,
    /// Entity type.
    pub node_type: String,
    /// Number of edges connected to this node.
    pub edge_count: u64,
}

/// Result of noise entity cleanup.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NoiseCleanupResult {
    /// Number of noise nodes identified.
    pub nodes_identified: u64,
    /// Number of nodes actually deleted (0 in dry-run mode).
    pub nodes_deleted: u64,
    /// Number of edges removed (0 in dry-run mode).
    pub edges_removed: u64,
    /// Number of aliases removed (0 in dry-run mode).
    pub aliases_removed: u64,
    /// Whether this was a dry run.
    pub dry_run: bool,
    /// Details of identified noise entities.
    pub entities: Vec<NoiseEntityInfo>,
}

/// Result of backfilling node embeddings.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BackfillResult {
    /// Total nodes found without embeddings.
    pub total_missing: u64,
    /// Nodes successfully embedded.
    pub embedded: u64,
    /// Nodes that failed to embed.
    pub failed: u64,
}

/// Result of seeding epistemic opinions.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SeedOpinionsResult {
    /// Nodes that received computed opinions from extractions.
    pub nodes_seeded: u64,
    /// Nodes set to vacuous opinion (no extractions).
    pub nodes_vacuous: u64,
    /// Edges that received computed opinions from extractions.
    pub edges_seeded: u64,
    /// Edges set to vacuous opinion (no extractions).
    pub edges_vacuous: u64,
}

/// Result of LLM code node summarization.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CodeSummaryResult {
    /// Code nodes found without semantic summaries.
    pub nodes_found: u64,
    /// Nodes successfully summarized and re-embedded.
    pub summarized: u64,
    /// Nodes where LLM summary failed.
    pub failed: u64,
}

/// Result of code-to-concept bridge edge creation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BridgeResult {
    /// Code-type nodes checked for bridging.
    pub code_nodes_checked: u64,
    /// New bridge edges created.
    pub edges_created: u64,
    /// Pairs skipped because an edge already exists.
    pub skipped_existing: u64,
}

/// A relationship type with its count of invalidated edges.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InvalidatedEdgeType {
    /// Relationship type (e.g. "RELATED_TO", "co_occurs").
    pub rel_type: String,
    /// Number of invalidated edges with this type.
    pub count: i64,
}

/// A node with a high count of invalidated edges (controversy indicator).
#[derive(Debug, Clone, serde::Serialize)]
pub struct InvalidatedEdgeNode {
    /// Node UUID.
    pub node_id: uuid::Uuid,
    /// Canonical node name.
    pub canonical_name: String,
    /// Node type.
    pub node_type: String,
    /// Number of invalidated edges touching this node.
    pub invalidated_edge_count: i64,
}

/// Statistics about invalidated edges in the graph.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InvalidatedEdgeStats {
    /// Total number of invalidated edges.
    pub total_invalidated: i64,
    /// Total number of valid (non-invalidated) edges.
    pub total_valid: i64,
    /// Top relationship types by invalidated edge count.
    pub top_types: Vec<InvalidatedEdgeType>,
    /// Nodes with the highest number of invalidated edges.
    pub top_nodes: Vec<InvalidatedEdgeNode>,
}

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
    repo: Arc<PgRepo>,
    graph: Arc<dyn GraphEngine>,
    /// Raw shared graph for write-path operations (consolidation, edge synthesis).
    shared_graph: SharedGraph,
    embedder: Option<Arc<dyn Embedder>>,
    chat_backend: Option<Arc<dyn ChatBackend>>,
    config: Option<crate::config::Config>,
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
        match self.graph.stats().await {
            Ok(engine_stats) => GraphStats {
                node_count: engine_stats.node_count,
                edge_count: engine_stats.edge_count,
                semantic_edge_count: engine_stats.semantic_edge_count,
                synthetic_edge_count: engine_stats.synthetic_edge_count,
                density: engine_stats.density,
                component_count: engine_stats.component_count,
            },
            Err(e) => {
                tracing::warn!(error = %e, "failed to get graph stats");
                GraphStats {
                    node_count: 0,
                    edge_count: 0,
                    semantic_edge_count: 0,
                    synthetic_edge_count: 0,
                    density: 0.0,
                    component_count: 0,
                }
            }
        }
    }

    /// Reload the graph sidecar from PG.
    pub async fn reload_graph(&self) -> Result<GraphStats> {
        self.graph.reload(self.repo.pool()).await?;
        Ok(self.graph_stats().await)
    }

    /// Retrieve statistics about invalidated edges.
    ///
    /// Returns the total count, top relationship types, and nodes
    /// with the highest invalidated edge counts (controversy
    /// indicators). These edges are normally invisible to the graph
    /// sidecar (`full_reload` filters `WHERE invalid_at IS NULL`).
    pub async fn invalidated_edge_stats(
        &self,
        type_limit: usize,
        node_limit: usize,
    ) -> Result<InvalidatedEdgeStats> {
        // Total invalidated + valid via SP.
        let stats_row: (i64, i64) = sqlx::query_as("SELECT * FROM sp_invalidated_edge_stats()")
            .fetch_one(self.repo.pool())
            .await?;
        let total_invalidated = stats_row.0;
        let total_valid = stats_row.1;

        // Top invalidated edge types via SP.
        let top_types: Vec<(String, i64)> =
            sqlx::query_as("SELECT * FROM sp_top_invalidated_rel_types($1)")
                .bind(type_limit as i32)
                .fetch_all(self.repo.pool())
                .await?;

        // Nodes with the highest invalidated-edge count.
        //
        // We UNION source and target sides so a node touching many
        // invalidated edges on either direction is surfaced.
        let top_nodes: Vec<(uuid::Uuid, String, String, i64)> = sqlx::query_as(
            "SELECT n.id, n.canonical_name, n.node_type, cnt \
             FROM ( \
                 SELECT node_id, SUM(c) AS cnt FROM ( \
                     SELECT source_node_id AS node_id, COUNT(*) AS c \
                     FROM edges WHERE invalid_at IS NOT NULL \
                     GROUP BY 1 \
                     UNION ALL \
                     SELECT target_node_id AS node_id, COUNT(*) AS c \
                     FROM edges WHERE invalid_at IS NOT NULL \
                     GROUP BY 1 \
                 ) sub \
                 GROUP BY node_id \
             ) agg \
             JOIN nodes n ON n.id = agg.node_id \
             ORDER BY cnt DESC \
             LIMIT $1",
        )
        .bind(node_limit as i64)
        .fetch_all(self.repo.pool())
        .await?;

        Ok(InvalidatedEdgeStats {
            total_invalidated,
            total_valid,
            top_types: top_types
                .into_iter()
                .map(|(rel_type, count)| InvalidatedEdgeType { rel_type, count })
                .collect(),
            top_nodes: top_nodes
                .into_iter()
                .map(
                    |(node_id, canonical_name, node_type, count)| InvalidatedEdgeNode {
                        node_id,
                        canonical_name,
                        node_type,
                        invalidated_edge_count: count,
                    },
                )
                .collect(),
        })
    }

    /// Check system health.
    pub async fn health(&self) -> HealthStatus {
        let pg_healthy = sqlx::query("SELECT 1")
            .execute(self.repo.pool())
            .await
            .is_ok();

        let sidecar_node_count = self.graph.node_count().await.unwrap_or(0);
        let sidecar_edge_count = self.graph.edge_count().await.unwrap_or(0);
        HealthStatus {
            pg_healthy,
            sidecar_node_count,
            sidecar_edge_count,
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
            Arc::clone(&self.shared_graph),
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

    /// Trigger RAPTOR recursive summarization across all sources.
    ///
    /// Builds hierarchical summary chunks that enable multi-resolution
    /// retrieval. Requires chat API keys to be configured.
    pub async fn trigger_raptor(&self) -> Result<crate::consolidation::raptor::RaptorReport> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| Error::Config("no configuration set on AdminService".into()))?;
        let chat_key = config
            .chat_api_key
            .as_ref()
            .ok_or_else(|| Error::Config("RAPTOR requires CHAT_API_KEY to be set".into()))?;
        let chat_base = config
            .chat_base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let embedder = self
            .embedder
            .as_ref()
            .ok_or_else(|| Error::Config("RAPTOR requires an embedder to be configured".into()))?;

        let consolidator = crate::consolidation::raptor::RaptorConsolidator::new(
            Arc::clone(&self.repo),
            Arc::clone(embedder),
            chat_base,
            chat_key.clone(),
            config.chat_model.clone(),
        )
        .with_table_dims(config.embedding.table_dims.clone());

        consolidator.run_all_sources().await
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

        let summary_row =
            sqlx::query("SELECT COUNT(*) as count FROM chunks WHERE level LIKE 'summary_%'")
                .fetch_one(self.repo.pool())
                .await?;
        let summary_chunk_count: i64 = summary_row.get("count");

        Ok(Metrics {
            graph_nodes: stats.node_count,
            graph_edges: stats.edge_count,
            semantic_edge_count: stats.semantic_edge_count,
            synthetic_edge_count: stats.synthetic_edge_count,
            component_count: stats.component_count,
            source_count,
            chunk_count,
            summary_chunk_count,
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
            self.graph.reload(self.repo.pool()).await?;
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
        // Phase 1: compute in/out degree via the graph engine trait.
        let exclude_refs: Vec<&str> = exclude_types.iter().map(|s| s.as_str()).collect();
        let engine_gaps = self
            .graph
            .knowledge_gaps(min_in_degree, min_label_length, &exclude_refs, limit)
            .await?;
        let candidates: Vec<(uuid::Uuid, String, String, usize, usize)> = engine_gaps
            .into_iter()
            .map(|g| (g.id, g.name, g.node_type, g.in_degree, g.out_degree))
            .collect();

        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        // Phase 2: batch-fetch source URIs for the gap nodes via SP.
        let node_ids: Vec<uuid::Uuid> = candidates.iter().map(|c| c.0).collect();
        let rows = sqlx::query_as::<_, (uuid::Uuid, Option<String>, Option<String>)>(
            "SELECT * FROM sp_get_node_provenance_sources($1)",
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
        // Find co-occurring entity pairs via SP. The SP filters by
        // min co-occurrences, max degree, and excludes existing edges.
        let rows: Vec<(uuid::Uuid, uuid::Uuid, i64)> =
            sqlx::query_as("SELECT * FROM sp_find_cooccurrence_pairs($1, $2)")
                .bind(min_cooccurrences as i32)
                .bind(max_degree as i32)
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
                "co-occurrence edge synthesis complete"
            );
            // Only reload if graph is active (has been loaded).
            // Workers run with an empty graph — reloading would
            // wastefully load the entire graph into memory.
            let stats = self.graph.stats().await?;
            if stats.node_count > 0 {
                self.graph.reload(self.repo.pool()).await?;
                tracing::info!("graph sidecar reloaded after edge synthesis");
            }
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

    /// Preview data health — shows what's stale, orphaned, or
    /// duplicated without modifying anything.
    pub async fn data_health_report(&self) -> Result<DataHealthReport> {
        let row: (i64, i64, i64, i64, i64, i64, i64, i64) =
            sqlx::query_as("SELECT * FROM sp_data_health_report()")
                .fetch_one(self.repo.pool())
                .await?;

        Ok(DataHealthReport {
            superseded_sources: row.0 as u64,
            superseded_chunks: row.1 as u64,
            orphan_nodes: row.2 as u64,
            orphan_nodes_with_edges: row.3 as u64,
            duplicate_sources: row.4 as u64,
            unembedded_nodes: row.5 as u64,
            unsummarized_code_entities: row.6 as u64,
            unsummarized_sources: row.7 as u64,
        })
    }

    /// Retroactively clean noise entities from the graph.
    ///
    /// Scans all nodes through the `is_noise_entity()` filter and
    /// optionally removes matches along with their edges and aliases.
    /// In dry-run mode (default), only reports what would be deleted.
    pub async fn cleanup_noise_entities(&self, dry_run: bool) -> Result<NoiseCleanupResult> {
        use super::noise_filter::is_noise_entity;

        // Fetch all nodes (id, canonical_name, node_type).
        let rows: Vec<(uuid::Uuid, String, String)> =
            sqlx::query_as("SELECT id, canonical_name, node_type FROM nodes")
                .fetch_all(self.repo.pool())
                .await?;

        // Identify noise entities.
        let mut noise: Vec<(uuid::Uuid, String, String)> = Vec::new();
        for (id, name, ntype) in &rows {
            if is_noise_entity(name, ntype) {
                noise.push((*id, name.clone(), ntype.clone()));
            }
        }

        if noise.is_empty() {
            tracing::info!("noise cleanup: no noise entities found");
            return Ok(NoiseCleanupResult {
                nodes_identified: 0,
                nodes_deleted: 0,
                edges_removed: 0,
                aliases_removed: 0,
                dry_run,
                entities: Vec::new(),
            });
        }

        tracing::info!(
            count = noise.len(),
            dry_run,
            "noise cleanup: identified noise entities"
        );

        // Count edges per noise node for reporting.
        let mut entities: Vec<NoiseEntityInfo> = Vec::new();
        for (id, name, ntype) in &noise {
            let edge_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM edges \
                 WHERE source_node_id = $1 OR target_node_id = $1",
            )
            .bind(id)
            .fetch_one(self.repo.pool())
            .await?;

            entities.push(NoiseEntityInfo {
                node_id: *id,
                canonical_name: name.clone(),
                node_type: ntype.clone(),
                edge_count: edge_count as u64,
            });
        }

        // Sort by edge count descending for visibility.
        entities.sort_by(|a, b| b.edge_count.cmp(&a.edge_count));

        let nodes_identified = entities.len() as u64;

        if dry_run {
            return Ok(NoiseCleanupResult {
                nodes_identified,
                nodes_deleted: 0,
                edges_removed: 0,
                aliases_removed: 0,
                dry_run: true,
                entities,
            });
        }

        // Delete: aliases → edges → nodes (FK order).
        let mut nodes_deleted: u64 = 0;
        let mut edges_removed: u64 = 0;
        let mut aliases_removed: u64 = 0;

        for entity in &entities {
            aliases_removed += NodeAliasRepo::delete_by_node(
                &*self.repo,
                crate::types::ids::NodeId::from_uuid(entity.node_id),
            )
            .await?;

            // Nullify invalidated_by FK references pointing at edges
            // we are about to delete, to avoid FK violation.
            sqlx::query(
                "UPDATE edges SET invalidated_by = NULL \
                 WHERE invalidated_by IN ( \
                     SELECT id FROM edges \
                     WHERE source_node_id = $1 OR target_node_id = $1 \
                 )",
            )
            .bind(entity.node_id)
            .execute(self.repo.pool())
            .await?;

            edges_removed += EdgeRepo::delete_by_node(
                &*self.repo,
                crate::types::ids::NodeId::from_uuid(entity.node_id),
            )
            .await?;

            // Clear unresolved_entities FK references before deletion
            // to avoid FK violation if this node was a Tier 5 target.
            sqlx::query(
                "UPDATE unresolved_entities SET resolved_node_id = NULL \
                 WHERE resolved_node_id = $1",
            )
            .bind(entity.node_id)
            .execute(self.repo.pool())
            .await?;

            if NodeRepo::delete(
                &*self.repo,
                crate::types::ids::NodeId::from_uuid(entity.node_id),
            )
            .await?
            {
                nodes_deleted += 1;
            }
        }

        tracing::info!(
            nodes_deleted,
            edges_removed,
            aliases_removed,
            "noise cleanup: retroactive cleanup complete"
        );

        // Reload graph sidecar to reflect deletions.
        if nodes_deleted > 0 {
            self.graph.reload(self.repo.pool()).await?;
        }

        // Audit log.
        let audit = AuditLog::new(
            AuditAction::AdminAction,
            "admin:cleanup_noise_entities".to_string(),
            serde_json::json!({
                "nodes_identified": nodes_identified,
                "nodes_deleted": nodes_deleted,
                "edges_removed": edges_removed,
                "aliases_removed": aliases_removed,
            }),
        );
        AuditLogRepo::create(&*self.repo, &audit).await?;

        Ok(NoiseCleanupResult {
            nodes_identified,
            nodes_deleted,
            edges_removed,
            aliases_removed,
            dry_run: false,
            entities,
        })
    }

    /// Run Tier 5 HDBSCAN batch resolution on the unresolved_entities pool.
    ///
    /// Fetches pending entities, embeds names, clusters with HDBSCAN,
    /// and resolves each cluster to a canonical node. Noise entities
    /// are promoted to individual nodes.
    pub async fn resolve_tier5(
        &self,
        min_cluster_size: Option<usize>,
    ) -> Result<crate::consolidation::tier5::Tier5Report> {
        let embedder = self
            .embedder
            .as_ref()
            .ok_or_else(|| Error::Config("no embedder configured for Tier 5 resolution".into()))?;

        let node_embed_dim = self
            .config
            .as_ref()
            .map(|c| c.embedding.table_dims.node)
            .unwrap_or(256);

        let config = crate::consolidation::tier5::Tier5Config {
            min_cluster_size: min_cluster_size.unwrap_or(2),
            node_embed_dim,
        };

        crate::consolidation::tier5::resolve_tier5(&self.repo, embedder.as_ref(), &config).await
    }

    /// Backfill embeddings for nodes that are missing them.
    ///
    /// Fetches all node IDs with `embedding IS NULL`, generates
    /// embeddings from `canonical_name: description` text, and
    /// stores them via `update_embedding`.
    pub async fn backfill_node_embeddings(&self) -> Result<BackfillResult> {
        use crate::ingestion::embedder::truncate_and_validate;

        let embedder = self
            .embedder
            .as_ref()
            .ok_or_else(|| Error::Config("no embedder configured".into()))?;

        let node_dim = self
            .config
            .as_ref()
            .map(|c| c.embedding.table_dims.node)
            .unwrap_or(256);

        // Fetch nodes missing embeddings via SP.
        let rows: Vec<(uuid::Uuid, String, Option<String>)> =
            sqlx::query_as("SELECT * FROM sp_list_nodes_without_embeddings($1)")
                .bind(i32::MAX)
                .fetch_all(self.repo.pool())
                .await?;

        if rows.is_empty() {
            return Ok(BackfillResult {
                total_missing: 0,
                embedded: 0,
                failed: 0,
            });
        }

        let total_missing = rows.len();
        tracing::info!(total_missing, "backfilling node embeddings");

        // Batch embed in chunks of 100.
        let mut embedded = 0u64;
        let mut failed = 0u64;
        for batch in rows.chunks(100) {
            let texts: Vec<String> = batch
                .iter()
                .map(|(_, name, desc)| match desc {
                    Some(d) if !d.is_empty() => format!("{name}: {d}"),
                    _ => name.clone(),
                })
                .collect();

            match embedder.embed(&texts).await {
                Ok(embeddings) => {
                    for ((id, _, _), emb) in batch.iter().zip(embeddings.iter()) {
                        match truncate_and_validate(emb, node_dim, "nodes") {
                            Ok(truncated) => {
                                let nid = crate::types::ids::NodeId::from_uuid(*id);
                                if let Err(e) =
                                    NodeRepo::update_embedding(&*self.repo, nid, &truncated).await
                                {
                                    tracing::warn!(node_id = %id, error = %e, "embed store failed");
                                    failed += 1;
                                } else {
                                    embedded += 1;
                                }
                            }
                            Err(e) => {
                                tracing::warn!(node_id = %id, error = %e, "truncate failed");
                                failed += 1;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "batch embed failed");
                    failed += batch.len() as u64;
                }
            }
        }

        let audit = AuditLog::new(
            AuditAction::AdminAction,
            "admin:backfill_node_embeddings".to_string(),
            serde_json::json!({
                "total_missing": total_missing,
                "embedded": embedded,
                "failed": failed,
            }),
        );
        AuditLogRepo::create(&*self.repo, &audit).await?;

        tracing::info!(embedded, failed, "backfill complete");
        Ok(BackfillResult {
            total_missing: total_missing as u64,
            embedded,
            failed,
        })
    }

    /// Seed epistemic opinions on all nodes and edges from their
    /// extraction records.
    ///
    /// Uses the batch cascade functions to compute Subjective Logic
    /// opinions via cumulative fusion across all active extractions.
    /// Nodes/edges with no extractions get vacuous opinions.
    pub async fn seed_opinions(&self) -> Result<SeedOpinionsResult> {
        use crate::epistemic::cascade::{recalculate_edge_opinions, recalculate_node_opinions};
        use crate::types::ids::{EdgeId, NodeId};

        // Fetch all node IDs.
        let node_uuids: Vec<uuid::Uuid> = sqlx::query_scalar("SELECT id FROM nodes")
            .fetch_all(self.repo.pool())
            .await?;
        let node_ids: Vec<NodeId> = node_uuids.iter().map(|u| NodeId::from_uuid(*u)).collect();

        // Fetch all edge IDs.
        let edge_uuids: Vec<uuid::Uuid> =
            sqlx::query_scalar("SELECT id FROM edges WHERE NOT is_synthetic")
                .fetch_all(self.repo.pool())
                .await?;
        let edge_ids: Vec<EdgeId> = edge_uuids.iter().map(|u| EdgeId::from_uuid(*u)).collect();

        tracing::info!(
            nodes = node_ids.len(),
            edges = edge_ids.len(),
            "seeding opinions"
        );

        // Process nodes in batches of 500.
        let mut node_result = crate::epistemic::cascade::CascadeResult::default();
        for batch in node_ids.chunks(500) {
            let r = recalculate_node_opinions(&*self.repo, batch).await?;
            node_result.merge(&r);
        }

        // Process edges in batches of 500.
        let mut edge_result = crate::epistemic::cascade::CascadeResult::default();
        for batch in edge_ids.chunks(500) {
            let r = recalculate_edge_opinions(&*self.repo, batch).await?;
            edge_result.merge(&r);
        }

        let result = SeedOpinionsResult {
            nodes_seeded: node_result.nodes_recalculated as u64,
            nodes_vacuous: node_result.nodes_vacuated as u64,
            edges_seeded: edge_result.edges_recalculated as u64,
            edges_vacuous: edge_result.edges_vacuated as u64,
        };

        let audit = AuditLog::new(
            AuditAction::AdminAction,
            "admin:seed_opinions".to_string(),
            serde_json::json!({
                "nodes_seeded": result.nodes_seeded,
                "nodes_vacuous": result.nodes_vacuous,
                "edges_seeded": result.edges_seeded,
                "edges_vacuous": result.edges_vacuous,
            }),
        );
        AuditLogRepo::create(&*self.repo, &audit).await?;

        tracing::info!(
            nodes_seeded = result.nodes_seeded,
            nodes_vacuous = result.nodes_vacuous,
            edges_seeded = result.edges_seeded,
            edges_vacuous = result.edges_vacuous,
            "opinion seeding complete"
        );
        Ok(result)
    }

    /// Generate LLM semantic summaries for code-type nodes and
    /// re-embed them using the summary text.
    ///
    /// Finds nodes with code entity types (struct, function, trait,
    /// enum, impl_block, constant, macro, module, class) that don't
    /// already have a `semantic_summary` in their properties. For
    /// each, sends `canonical_name + description` to the LLM for a
    /// plain-English summary, stores it in `properties.semantic_summary`,
    /// and re-generates the embedding from the summary text.
    pub async fn summarize_code_nodes(&self) -> Result<CodeSummaryResult> {
        use crate::ingestion::embedder::truncate_and_validate;

        let chat = self
            .chat_backend
            .as_ref()
            .ok_or_else(|| Error::Config("no chat backend configured".into()))?;
        let embedder = self
            .embedder
            .as_ref()
            .ok_or_else(|| Error::Config("no embedder configured".into()))?;
        let node_dim = self
            .config
            .as_ref()
            .map(|c| c.embedding.table_dims.node)
            .unwrap_or(256);

        let code_types = [
            "struct",
            "function",
            "trait",
            "enum",
            "impl_block",
            "constant",
            "macro",
            "module",
            "class",
        ];

        // Fetch code nodes without semantic summaries.
        type CodeNodeRow = (
            uuid::Uuid,
            String,
            String,
            Option<String>,
            Option<serde_json::Value>,
        );
        let rows: Vec<CodeNodeRow> = sqlx::query_as(
            "SELECT id, canonical_name, node_type, description, properties \
                 FROM nodes \
                 WHERE node_type = ANY($1) \
                   AND (properties IS NULL \
                        OR properties->>'semantic_summary' IS NULL)",
        )
        .bind(&code_types[..])
        .fetch_all(self.repo.pool())
        .await?;

        if rows.is_empty() {
            return Ok(CodeSummaryResult {
                nodes_found: 0,
                summarized: 0,
                failed: 0,
            });
        }

        let nodes_found = rows.len() as u64;
        tracing::info!(nodes_found, "summarizing code nodes");

        let system_prompt = "You are a code documentation assistant. Given a code entity \
            (struct, function, trait, interface, etc.) with its name and description, write a \
            concise 1-3 sentence natural language summary of what it does. Focus on purpose \
            and behavior, not syntax. Use domain terminology that would appear in design docs \
            or specifications. Do not use markdown formatting.";

        let mut summarized = 0u64;
        let mut failed = 0u64;

        for (id, name, node_type, desc, props) in &rows {
            let user_prompt = format!(
                "Entity: {name}\nType: {node_type}\nDescription: {}",
                desc.as_deref().unwrap_or("(none)")
            );

            match chat.chat(system_prompt, &user_prompt, false, 0.3).await {
                Ok(resp) => {
                    let summary = resp.text.trim().to_string();
                    if summary.is_empty() {
                        failed += 1;
                        continue;
                    }

                    // Store semantic_summary in properties.
                    let mut new_props = props.clone().unwrap_or(serde_json::json!({}));
                    new_props["semantic_summary"] = serde_json::json!(&summary);

                    sqlx::query(
                        "UPDATE nodes SET properties = $2 \
                         WHERE id = $1",
                    )
                    .bind(id)
                    .bind(&new_props)
                    .execute(self.repo.pool())
                    .await?;

                    // Re-embed using the summary text.
                    let embed_text = format!("{name}: {summary}");
                    match embedder.embed(&[embed_text]).await {
                        Ok(embeddings) => {
                            if let Some(emb) = embeddings.first() {
                                match truncate_and_validate(emb, node_dim, "nodes") {
                                    Ok(truncated) => {
                                        let nid = crate::types::ids::NodeId::from_uuid(*id);
                                        if let Err(e) =
                                            NodeRepo::update_embedding(&*self.repo, nid, &truncated)
                                                .await
                                        {
                                            tracing::warn!(
                                                node = %name,
                                                error = %e,
                                                "embedding storage after summary failed"
                                            );
                                            failed += 1;
                                            continue;
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            node = %name,
                                            error = %e,
                                            "embedding truncation after summary failed"
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                node = %name,
                                error = %e,
                                "re-embed after summary failed"
                            );
                        }
                    }

                    summarized += 1;
                    if summarized % 50 == 0 {
                        tracing::info!(summarized, "code summary progress");
                    }
                }
                Err(e) => {
                    tracing::warn!(node = %name, error = %e, "LLM summary failed");
                    failed += 1;
                }
            }
        }

        let audit = AuditLog::new(
            AuditAction::AdminAction,
            "admin:summarize_code_nodes".to_string(),
            serde_json::json!({
                "nodes_found": nodes_found,
                "summarized": summarized,
                "failed": failed,
            }),
        );
        AuditLogRepo::create(&*self.repo, &audit).await?;

        tracing::info!(summarized, failed, "code summary complete");
        Ok(CodeSummaryResult {
            nodes_found,
            summarized,
            failed,
        })
    }

    /// Create cross-domain bridge edges between code entities and
    /// prose concept nodes based on embedding similarity.
    ///
    /// Finds code-type nodes with embeddings and compares them against
    /// non-code nodes (concept, theory, method, etc.) using pgvector
    /// cosine distance. Creates `implements` edges for pairs above the
    /// similarity threshold, skipping pairs that already have an edge.
    pub async fn bridge_code_to_concepts(
        &self,
        min_similarity: f64,
        max_edges_per_node: i64,
    ) -> Result<BridgeResult> {
        let code_types: Vec<String> = self
            .config
            .as_ref()
            .map(|c| c.pipeline.code_node_types.clone())
            .unwrap_or_else(crate::config::PipelineConfig::default_code_node_types);

        // Fetch code nodes that have embeddings.
        let code_nodes: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(
            "SELECT id, canonical_name, node_type FROM nodes \
             WHERE node_type = ANY($1) AND embedding IS NOT NULL",
        )
        .bind(&code_types)
        .fetch_all(self.repo.pool())
        .await?;

        if code_nodes.is_empty() {
            return Ok(BridgeResult {
                code_nodes_checked: 0,
                edges_created: 0,
                skipped_existing: 0,
            });
        }

        let code_nodes_checked = code_nodes.len() as u64;
        tracing::info!(code_nodes_checked, "bridging code nodes to concepts");

        let threshold = 1.0 - min_similarity; // cosine distance
        let mut edges_created = 0u64;
        let mut skipped_existing = 0u64;

        for (code_id, code_name, _code_type) in &code_nodes {
            // Find nearest non-code concept nodes by embedding similarity.
            let matches: Vec<(uuid::Uuid, String, f64)> = sqlx::query_as(
                "SELECT n.id, n.canonical_name, \
                        (n.embedding <=> (SELECT embedding FROM nodes WHERE id = $1)) AS dist \
                 FROM nodes n \
                 WHERE n.node_type != ALL($3) \
                   AND n.embedding IS NOT NULL \
                   AND n.id != $1 \
                 ORDER BY dist ASC \
                 LIMIT $2",
            )
            .bind(code_id)
            .bind(max_edges_per_node)
            .bind(&code_types)
            .fetch_all(self.repo.pool())
            .await?;

            for (concept_id, concept_name, dist) in &matches {
                if *dist > threshold {
                    break; // remaining will be worse
                }

                // Check if an edge already exists between these nodes.
                let exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM edges \
                     WHERE source_node_id = $1 AND target_node_id = $2 \
                       AND rel_type = 'implements')",
                )
                .bind(code_id)
                .bind(concept_id)
                .fetch_one(self.repo.pool())
                .await?;

                if exists {
                    skipped_existing += 1;
                    continue;
                }

                let code_nid = crate::types::ids::NodeId::from_uuid(*code_id);
                let concept_nid = crate::types::ids::NodeId::from_uuid(*concept_id);
                let similarity = 1.0 - dist;

                let mut edge =
                    crate::models::edge::Edge::new(code_nid, concept_nid, "implements".to_string());
                edge.confidence = similarity;
                edge.properties = serde_json::json!({
                    "bridge_type": "code_to_concept",
                    "cosine_similarity": similarity,
                });

                use crate::storage::traits::EdgeRepo;
                EdgeRepo::create(&*self.repo, &edge).await?;
                edges_created += 1;

                tracing::debug!(
                    code = %code_name,
                    concept = %concept_name,
                    similarity = similarity,
                    "bridge edge created"
                );
            }
        }

        // Reload graph sidecar so new edges are visible to graph-dimension searches.
        if edges_created > 0 {
            self.graph.reload(self.repo.pool()).await?;
        }

        let audit = AuditLog::new(
            AuditAction::AdminAction,
            "admin:bridge_code_to_concepts".to_string(),
            serde_json::json!({
                "code_nodes_checked": code_nodes_checked,
                "edges_created": edges_created,
                "skipped_existing": skipped_existing,
                "min_similarity": min_similarity,
            }),
        );
        AuditLogRepo::create(&*self.repo, &audit).await?;

        tracing::info!(edges_created, skipped_existing, "bridge complete");
        Ok(BridgeResult {
            code_nodes_checked,
            edges_created,
            skipped_existing,
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
            entity_class: None,
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
            has_valid_from: false,
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

    #[test]
    fn invalidated_edge_stats_serializes() {
        let stats = InvalidatedEdgeStats {
            total_invalidated: 23000,
            total_valid: 113000,
            top_types: vec![
                InvalidatedEdgeType {
                    rel_type: "RELATED_TO".into(),
                    count: 15000,
                },
                InvalidatedEdgeType {
                    rel_type: "co_occurs".into(),
                    count: 5000,
                },
            ],
            top_nodes: vec![InvalidatedEdgeNode {
                node_id: uuid::Uuid::new_v4(),
                canonical_name: "Entity Resolution".into(),
                node_type: "concept".into(),
                invalidated_edge_count: 42,
            }],
        };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("23000"));
        assert!(json.contains("113000"));
        assert!(json.contains("RELATED_TO"));
        assert!(json.contains("co_occurs"));
        assert!(json.contains("Entity Resolution"));
        assert!(json.contains("42"));
    }
}

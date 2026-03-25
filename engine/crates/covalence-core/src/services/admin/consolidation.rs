//! Consolidation triggers and service metrics.

use std::sync::Arc;

use crate::consolidation::batch::BatchJob;
use crate::consolidation::graph_batch::GraphBatchConsolidator;
use crate::consolidation::{BatchConsolidator, BatchStatus};
use crate::error::{Error, Result};
use crate::storage::traits::{AdminRepo, SourceRepo};

use super::AdminService;

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

impl AdminService {
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

        let (chunk_count, article_count, search_trace_count, summary_chunk_count) =
            AdminRepo::metrics_counts(&*self.repo).await?;

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
}

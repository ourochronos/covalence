//! Graph-aware batch consolidator.
//!
//! Groups sources by community, compiles articles per cluster, and
//! flags contentions. Uses an optional LLM extractor for article
//! compilation; falls back to simple concatenation when unavailable.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::config::TableDimensions;
use crate::consolidation::batch::{BatchConsolidator, BatchJob, BatchStatus};
use crate::consolidation::compiler::{ArticleCompiler, CompilationInput, ConcatCompiler};
use crate::consolidation::contention::{Contention, detect_contentions};
use crate::consolidation::topic::{SourceNodes, cluster_sources_by_community};
use crate::error::Result;
use crate::graph::sidecar::SharedGraph;
use crate::ingestion::embedder::Embedder;
use crate::ingestion::embedder::truncate_and_validate;
use crate::models::article::Article;
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{ArticleRepo, ChunkRepo};
use crate::types::ids::NodeId;

/// Batch consolidator backed by the graph sidecar and PG storage.
///
/// For each batch job it:
/// 1. Clusters sources by community using topic detection.
/// 2. Compiles an article for each community cluster.
/// 3. Detects contentions and logs them via tracing.
/// 4. Marks the job as complete.
pub struct GraphBatchConsolidator {
    /// Repository for database operations.
    repo: Arc<PgRepo>,
    /// Shared graph sidecar for community detection.
    graph: SharedGraph,
    /// Article compiler (LLM or fallback concatenation).
    compiler: Arc<dyn ArticleCompiler>,
    /// Optional embedder for article embedding storage.
    embedder: Option<Arc<dyn Embedder>>,
    /// Per-table embedding dimensions for truncation.
    table_dims: TableDimensions,
}

impl GraphBatchConsolidator {
    /// Create a new graph batch consolidator.
    pub fn new(
        repo: Arc<PgRepo>,
        graph: SharedGraph,
        compiler: Option<Arc<dyn ArticleCompiler>>,
        embedder: Option<Arc<dyn Embedder>>,
    ) -> Self {
        let compiler =
            compiler.unwrap_or_else(|| Arc::new(ConcatCompiler) as Arc<dyn ArticleCompiler>);
        Self {
            repo,
            graph,
            compiler,
            embedder,
            table_dims: TableDimensions::default(),
        }
    }

    /// Set per-table embedding dimensions.
    pub fn with_table_dims(mut self, dims: TableDimensions) -> Self {
        self.table_dims = dims;
        self
    }

    /// Build `SourceNodes` entries for the given source IDs by
    /// checking which graph nodes belong to each source's chunks.
    ///
    /// This collects all chunk content for each source and maps
    /// extracted nodes from the graph that correspond to those
    /// sources.
    async fn build_source_nodes(
        &self,
        source_ids: &[crate::types::ids::SourceId],
    ) -> Result<Vec<SourceNodes>> {
        let graph = self.graph.read().await;
        let mut result = Vec::new();

        for &sid in source_ids {
            // Collect node UUIDs that exist in the graph.
            // In a full implementation, we would query extractions
            // to find which nodes came from which source. For now,
            // we collect all graph nodes and let community detection
            // handle the grouping.
            let chunks = ChunkRepo::list_by_source(&*self.repo, sid)
                .await
                .unwrap_or_default();

            // Gather node IDs from the graph that might be related.
            // This is a simplified heuristic: search for nodes whose
            // canonical name appears in any of the source's chunks.
            let mut node_ids = Vec::new();
            let mut seen = HashSet::new();

            for chunk in &chunks {
                let content_lower = chunk.content.to_lowercase();
                for idx in graph.graph().node_indices() {
                    let meta = &graph.graph()[idx];
                    if !seen.contains(&meta.id)
                        && content_lower.contains(&meta.canonical_name.to_lowercase())
                    {
                        seen.insert(meta.id);
                        node_ids.push(meta.id);
                    }
                }
            }

            result.push(SourceNodes {
                source_id: sid,
                node_ids,
            });
        }

        Ok(result)
    }

    /// Compile an article from the chunks belonging to the given
    /// source IDs.
    async fn compile_article(
        &self,
        source_ids: &[crate::types::ids::SourceId],
        community_id: usize,
    ) -> Result<Article> {
        // Collect all chunks from the sources in this cluster
        let mut all_content = Vec::new();
        let mut all_node_ids = Vec::new();
        let mut entity_names = Vec::new();

        for &sid in source_ids {
            let chunks = ChunkRepo::list_by_source(&*self.repo, sid)
                .await
                .unwrap_or_default();

            for chunk in &chunks {
                all_content.push(chunk.content.clone());
            }
        }

        // Collect node IDs and names from the graph for this cluster
        {
            let graph = self.graph.read().await;
            let mut seen = HashSet::new();
            for idx in graph.graph().node_indices() {
                let meta = &graph.graph()[idx];
                let name_lower = meta.canonical_name.to_lowercase();
                for content in &all_content {
                    if content.to_lowercase().contains(&name_lower) && seen.insert(meta.id) {
                        all_node_ids.push(NodeId::from_uuid(meta.id));
                        entity_names.push(meta.canonical_name.clone());
                        break;
                    }
                }
            }
        }

        // Use the compiler to generate the article
        let input = CompilationInput {
            community_id,
            chunks: all_content,
            entity_names,
        };
        let output = self.compiler.compile(&input).await?;

        // Content hash
        let mut hasher = Sha256::new();
        hasher.update(output.body.as_bytes());
        let content_hash = hasher.finalize().to_vec();

        let article = Article::new(output.title, output.body, content_hash, all_node_ids);

        // Embed the article summary if an embedder is available
        if let Some(ref embedder) = self.embedder {
            let texts = vec![output.summary];
            let embeddings = embedder.embed(&texts).await?;
            if let Some(emb) = embeddings.first() {
                let truncated = truncate_and_validate(emb, self.table_dims.article, "articles")?;
                ArticleRepo::update_embedding(&*self.repo, article.id, &truncated).await?;
            }
        }

        Ok(article)
    }

    /// Log any detected contentions.
    fn log_contentions(&self, contentions: &[Contention]) {
        for c in contentions {
            tracing::warn!(
                node_a = %c.node_a,
                node_b = %c.node_b,
                edge_id = %c.edge_id,
                rel_type = %c.rel_type,
                confidence = c.confidence,
                "contention detected during batch consolidation"
            );
        }
    }
}

#[async_trait::async_trait]
impl BatchConsolidator for GraphBatchConsolidator {
    /// Execute a batch consolidation job.
    ///
    /// 1. Groups the batch's sources by community.
    /// 2. Compiles an article for each community cluster.
    /// 3. Detects and logs contentions.
    /// 4. Updates the batch job status to `Complete`.
    async fn run_batch(&self, job: &mut BatchJob) -> Result<()> {
        job.status = BatchStatus::Running;

        let source_nodes = self.build_source_nodes(&job.source_ids).await?;

        // Cluster sources by community
        let clusters = {
            let graph = self.graph.read().await;
            cluster_sources_by_community(graph.graph(), &source_nodes)
        };

        // Detect contentions and log them
        {
            let graph = self.graph.read().await;
            let contentions = detect_contentions(graph.graph());
            if !contentions.is_empty() {
                self.log_contentions(&contentions);
            }
        }

        // Compile an article for each community cluster
        if clusters.is_empty() {
            // If no communities were detected (e.g., empty graph),
            // compile a single article from all sources.
            if !job.source_ids.is_empty() {
                let article = self.compile_article(&job.source_ids, 0).await?;
                ArticleRepo::create(&*self.repo, &article).await?;
            }
        } else {
            for (&community_id, source_ids) in &clusters {
                let article = self.compile_article(source_ids, community_id).await?;
                ArticleRepo::create(&*self.repo, &article).await?;
            }
        }

        job.status = BatchStatus::Complete;
        job.completed_at = Some(Utc::now());

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concat_compiler_produces_article_body() {
        let compiler = ConcatCompiler;
        let input = CompilationInput {
            community_id: 1,
            chunks: vec!["First chunk".to_string(), "Second chunk".to_string()],
            entity_names: vec![],
        };
        let output = compiler.compile(&input).await.unwrap();
        assert!(output.body.contains("First chunk"));
        assert!(output.body.contains("Second chunk"));
        assert!(output.title.contains("Community 1"));
    }
}

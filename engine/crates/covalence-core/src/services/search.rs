//! Search service — multi-dimensional fused search orchestration.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Result;
use crate::graph::SharedGraph;
use crate::ingestion::embedder::Embedder;
use crate::search::dimensions::{
    GlobalDimension, GraphDimension, LexicalDimension, SearchDimension, SearchQuery,
    StructuralDimension, TemporalDimension, VectorDimension,
};
use crate::search::fusion::{self, FusedResult};
use crate::search::strategy::SearchStrategy;
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{ArticleRepo, ChunkRepo, NodeRepo, SourceRepo};
use crate::types::ids::{ArticleId, ChunkId, NodeId};

/// Post-fusion filters for narrowing search results.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchFilters {
    /// Minimum epistemic confidence (projected probability).
    pub min_confidence: Option<f64>,
    /// Restrict to specific node types.
    pub node_types: Option<Vec<String>>,
    /// Restrict to a temporal date range.
    pub date_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
}

/// Service for orchestrating multi-dimensional search and RRF fusion.
pub struct SearchService {
    repo: Arc<PgRepo>,
    embedder: Option<Arc<dyn Embedder>>,
    vector: VectorDimension,
    lexical: LexicalDimension,
    temporal: TemporalDimension,
    graph_dim: GraphDimension,
    structural: StructuralDimension,
    global: GlobalDimension,
}

impl SearchService {
    /// Create a new search service.
    pub fn new(repo: Arc<PgRepo>, graph: SharedGraph) -> Self {
        Self::with_embedder(repo, graph, None)
    }

    /// Create a new search service with an optional embedder for vector search.
    pub fn with_embedder(
        repo: Arc<PgRepo>,
        graph: SharedGraph,
        embedder: Option<Arc<dyn Embedder>>,
    ) -> Self {
        let pool = repo.pool().clone();
        Self {
            repo,
            embedder,
            vector: VectorDimension::new(pool.clone()),
            lexical: LexicalDimension::new(pool.clone()),
            temporal: TemporalDimension::new(pool.clone()),
            graph_dim: GraphDimension::new(Arc::clone(&graph)),
            structural: StructuralDimension::new(graph),
            global: GlobalDimension::new(pool),
        }
    }

    /// Execute a fused search across all dimensions.
    ///
    /// Each dimension independently ranks candidates, then results are
    /// combined via Reciprocal Rank Fusion with strategy-derived weights.
    /// Optional filters narrow results after fusion and enrichment.
    pub async fn search(
        &self,
        query: &str,
        strategy: SearchStrategy,
        limit: usize,
        filters: Option<SearchFilters>,
    ) -> Result<Vec<FusedResult>> {
        let time_range = filters.as_ref().and_then(|f| f.date_range);

        // Embed the query for vector search.
        let query_embedding = if let Some(ref embedder) = self.embedder {
            match embedder.embed(&[query.to_string()]).await {
                Ok(mut vecs) if !vecs.is_empty() => Some(vecs.remove(0)),
                Ok(_) => None,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to embed query, vector search disabled");
                    None
                }
            }
        } else {
            None
        };

        let search_query = SearchQuery {
            text: query.to_string(),
            strategy: strategy.clone(),
            limit,
            time_range,
            embedding: query_embedding,
            ..SearchQuery::default()
        };

        // Run all 6 dimensions concurrently.
        let (vec_r, lex_r, tmp_r, grp_r, str_r, glb_r) = tokio::join!(
            self.vector.search(&search_query),
            self.lexical.search(&search_query),
            self.temporal.search(&search_query),
            self.graph_dim.search(&search_query),
            self.structural.search(&search_query),
            self.global.search(&search_query),
        );

        // Collect successful results, skipping failed dimensions.
        let dimensions: [(&str, std::result::Result<Vec<fusion::SearchResult>, _>, f64); 6] = {
            let w = strategy.weights();
            [
                ("vector", vec_r, w.vector),
                ("lexical", lex_r, w.lexical),
                ("temporal", tmp_r, w.temporal),
                ("graph", grp_r, w.graph),
                ("structural", str_r, w.structural),
                ("global", glb_r, w.global),
            ]
        };

        let mut ranked_lists = Vec::new();
        let mut weights = Vec::new();

        // Collect snippets per entity ID for post-fusion enrichment.
        // Also track result types per ID for type-aware enrichment.
        let mut snippets: HashMap<Uuid, String> = HashMap::new();
        let mut result_types: HashMap<Uuid, String> = HashMap::new();

        for (name, result, weight) in dimensions {
            match result {
                Ok(results) => {
                    for r in &results {
                        if let Some(s) = &r.snippet {
                            snippets.entry(r.id).or_insert_with(|| s.clone());
                        }
                        if let Some(rt) = &r.result_type {
                            result_types.entry(r.id).or_insert_with(|| rt.clone());
                        }
                    }
                    ranked_lists.push(results);
                    weights.push(weight);
                }
                Err(e) => {
                    tracing::warn!(
                        dimension = name,
                        error = %e,
                        "search dimension failed, skipping"
                    );
                }
            }
        }

        let mut fused = fusion::rrf_fuse(&ranked_lists, &weights, fusion::DEFAULT_K);

        // Enrich each result based on its result_type.
        for result in &mut fused {
            result.snippet = snippets.remove(&result.id);
            let rtype = result_types.get(&result.id).map(|s| s.as_str());
            result.result_type = rtype.map(|s| s.to_string());

            match rtype {
                Some("node") => {
                    if let Ok(Some(node)) =
                        NodeRepo::get(&*self.repo, NodeId::from_uuid(result.id)).await
                    {
                        result.name = Some(node.canonical_name);
                        result.entity_type = Some(node.node_type);
                        result.confidence =
                            node.confidence_breakdown.map(|o| o.projected_probability());
                    }
                }
                Some("article") => {
                    if let Ok(Some(article)) =
                        ArticleRepo::get(&*self.repo, ArticleId::from_uuid(result.id)).await
                    {
                        result.name = Some(article.title);
                        result.entity_type = Some("article".to_string());
                        result.confidence = article
                            .confidence_breakdown
                            .map(|o| o.projected_probability());
                        if result.snippet.is_none() {
                            let body = &article.body;
                            result.snippet = Some(if body.len() > 300 {
                                format!("{}...", &body[..300])
                            } else {
                                body.clone()
                            });
                        }
                    }
                }
                _ => {
                    // Chunk or unknown — try chunk lookup for source_uri,
                    // then node lookup for backward compat.
                    if let Ok(Some(chunk)) =
                        ChunkRepo::get(&*self.repo, ChunkId::from_uuid(result.id)).await
                    {
                        if let Ok(Some(source)) =
                            SourceRepo::get(&*self.repo, chunk.source_id).await
                        {
                            result.source_uri = source.uri;
                        }
                    }
                    if let Ok(Some(node)) =
                        NodeRepo::get(&*self.repo, NodeId::from_uuid(result.id)).await
                    {
                        result.name = Some(node.canonical_name);
                        result.entity_type = Some(node.node_type);
                        result.confidence =
                            node.confidence_breakdown.map(|o| o.projected_probability());
                    }
                }
            }
        }

        // Parent-context injection: for paragraph-level chunks,
        // prepend truncated parent (section) content to the snippet.
        for result in &mut fused {
            let is_chunk = result.result_type.as_deref().is_none_or(|rt| rt == "chunk");
            if !is_chunk {
                continue;
            }
            if let Ok(Some(chunk)) =
                ChunkRepo::get(&*self.repo, ChunkId::from_uuid(result.id)).await
            {
                if chunk.level != "paragraph" {
                    continue;
                }
                if let Some(parent_id) = chunk.parent_chunk_id {
                    if let Ok(Some(parent)) = ChunkRepo::get(&*self.repo, parent_id).await {
                        let parent_ctx = if parent.content.len() > 200 {
                            format!("{}...", &parent.content[..200])
                        } else {
                            parent.content.clone()
                        };
                        let prefix = format!("[{}: {}]", parent.level, parent_ctx);
                        result.snippet = Some(match result.snippet.take() {
                            Some(s) => format!("{} {}", prefix, s),
                            None => prefix,
                        });
                    }
                }
            }
        }

        // Apply post-fusion filters.
        if let Some(ref f) = filters {
            if let Some(min_conf) = f.min_confidence {
                fused.retain(|r| r.confidence.is_some_and(|c| c >= min_conf));
            }
            if let Some(ref types) = f.node_types {
                fused.retain(|r| r.entity_type.as_ref().is_some_and(|t| types.contains(t)));
            }
        }

        fused.truncate(limit);
        Ok(fused)
    }
}

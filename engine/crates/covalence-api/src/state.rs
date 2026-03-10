//! Application state shared across request handlers.

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::RwLock;

use covalence_core::config::Config;
use covalence_core::graph::sync::full_reload;
use covalence_core::graph::{GraphSidecar, SharedGraph};
use covalence_core::ingestion::embedder::Embedder;
use covalence_core::ingestion::extractor::Extractor;
use covalence_core::ingestion::resolver::EntityResolver;
use covalence_core::ingestion::{
    ConverterRegistry, GlinerExtractor, LlmExtractor, OpenAiEmbedder, PgResolver,
    ReaderLmConverter, SidecarExtractor, TwoPassExtractor, VoyageConfig, VoyageEmbedder,
};
use covalence_core::search::rerank::{HttpReranker, RerankConfig, Reranker};
use covalence_core::services::{
    AdminService, ArticleService, EdgeService, NodeService, SearchService, SourceService,
};
use covalence_core::storage::postgres::PgRepo;

/// Shared application state for Axum handlers.
#[derive(Clone)]
pub struct AppState {
    /// Application configuration.
    #[allow(dead_code)]
    pub config: Config,
    /// Shared database repository.
    #[allow(dead_code)]
    pub repo: Arc<PgRepo>,
    /// Shared graph sidecar.
    pub graph: SharedGraph,
    /// Source ingestion and management.
    pub source_service: Arc<SourceService>,
    /// Multi-dimensional fused search.
    pub search_service: Arc<SearchService>,
    /// Graph node operations.
    pub node_service: Arc<NodeService>,
    /// Graph edge operations.
    pub edge_service: Arc<EdgeService>,
    /// Compiled article operations.
    #[allow(dead_code)]
    pub article_service: Arc<ArticleService>,
    /// Administrative operations.
    pub admin_service: Arc<AdminService>,
}

impl AppState {
    /// Initialize application state from configuration.
    pub async fn new(config: Config) -> Result<Self> {
        let repo = Arc::new(PgRepo::new(&config.database_url).await?);
        let graph: SharedGraph = Arc::new(RwLock::new(GraphSidecar::new()));

        // Load the graph sidecar from PG on startup so graph and
        // structural search dimensions have data immediately.
        match full_reload(repo.pool(), Arc::clone(&graph)).await {
            Ok(()) => {
                let g = graph.read().await;
                tracing::info!(
                    nodes = g.node_count(),
                    edges = g.edge_count(),
                    "graph sidecar loaded from PG"
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to load graph sidecar on startup");
            }
        }

        // Determine the embedding provider. Voyage is used when
        // explicitly configured or when a Voyage API key is present.
        let use_voyage = config.embed_provider == "voyage" || config.voyage_api_key.is_some();

        #[allow(clippy::type_complexity)]
        let (embedder, reranker): (Option<Arc<dyn Embedder>>, Option<Arc<dyn Reranker>>) =
            if use_voyage {
                let emb = config.voyage_api_key.as_ref().map(|key| {
                    let voyage_cfg = VoyageConfig {
                        api_key: key.clone(),
                        base_url: config
                            .voyage_base_url
                            .clone()
                            .unwrap_or_else(|| "https://api.voyageai.com/v1".to_string()),
                        model: config.embed_model.clone(),
                        dimensions: config.embedding.max_dim(),
                        input_type: "document".to_string(),
                        ..VoyageConfig::default()
                    };
                    Arc::new(VoyageEmbedder::new(voyage_cfg)) as Arc<dyn Embedder>
                });

                // Auto-activate the HTTP reranker with Voyage credentials.
                let rnk = config.voyage_api_key.as_ref().map(|key| {
                    let rerank_cfg = RerankConfig {
                        api_key: key.clone(),
                        base_url: config
                            .voyage_base_url
                            .clone()
                            .unwrap_or_else(|| "https://api.voyageai.com/v1".to_string()),
                        model: "rerank-2.5".to_string(),
                        ..RerankConfig::default()
                    };
                    Arc::new(HttpReranker::new(rerank_cfg)) as Arc<dyn Reranker>
                });

                (emb, rnk)
            } else {
                let emb = config.openai_api_key.as_ref().map(|key| {
                    Arc::new(OpenAiEmbedder::new(
                        &config.embedding,
                        key.clone(),
                        config.openai_base_url.clone(),
                    )) as Arc<dyn Embedder>
                });
                (emb, None)
            };

        let extractor: Option<Arc<dyn Extractor>> = if config.entity_extractor == "sidecar" {
            // Unified sidecar: coref + NER + relationships in one
            // service, with Rust-side windowing for large inputs.
            let base_url = config
                .extract_url
                .clone()
                .unwrap_or_else(|| "http://localhost:8433".to_string());
            tracing::info!(
                url = %base_url,
                "using unified extraction sidecar"
            );
            Some(
                Arc::new(SidecarExtractor::new(base_url, config.gliner_threshold))
                    as Arc<dyn Extractor>,
            )
        } else if config.entity_extractor == "gliner2" {
            let base_url = config
                .extract_url
                .clone()
                .unwrap_or_else(|| "http://localhost:8432".to_string());
            Some(
                Arc::new(GlinerExtractor::new(base_url, config.gliner_threshold))
                    as Arc<dyn Extractor>,
            )
        } else if config.entity_extractor == "two_pass" {
            // Two-pass: GLiNER for entities, LLM for relationships.
            let gliner_url = config
                .extract_url
                .clone()
                .unwrap_or_else(|| "http://localhost:8432".to_string());
            let gliner = Arc::new(GlinerExtractor::new(gliner_url, config.gliner_threshold));

            let chat_key = config
                .chat_api_key
                .as_ref()
                .or(config.openai_api_key.as_ref());
            let chat_base = config.chat_base_url.clone();

            chat_key.map(|key| {
                let llm = Arc::new(LlmExtractor::new(
                    config.chat_model.clone(),
                    key.clone(),
                    chat_base,
                ));
                Arc::new(TwoPassExtractor::new(gliner, llm)) as Arc<dyn Extractor>
            })
        } else if config.chat_model.is_empty() {
            None
        } else {
            // Use dedicated chat API key/URL, falling back to the shared OpenAI config.
            let chat_key = config
                .chat_api_key
                .as_ref()
                .or(config.openai_api_key.as_ref());
            // Chat base URL: use dedicated, else None (LlmExtractor defaults to OpenAI).
            // Do NOT fall back to OPENAI_BASE_URL since it may be a non-chat provider (e.g. Voyage).
            let chat_base = config.chat_base_url.clone();
            chat_key.map(|key| {
                Arc::new(LlmExtractor::new(
                    config.chat_model.clone(),
                    key.clone(),
                    chat_base,
                )) as Arc<dyn Extractor>
            })
        };

        let pg_resolver = Arc::new(match embedder.clone() {
            Some(emb) => PgResolver::with_embedder(
                Arc::clone(&repo),
                config.resolve_trigram_threshold,
                emb,
                config.resolve_vector_threshold,
            )
            .with_node_embed_dim(config.embedding.table_dims.node)
            .with_graph(Arc::clone(&graph)),
            None => PgResolver::with_threshold(Arc::clone(&repo), config.resolve_trigram_threshold),
        });

        let resolver: Option<Arc<dyn EntityResolver>> =
            Some(Arc::clone(&pg_resolver) as Arc<dyn EntityResolver>);

        // Build the converter registry. When a ReaderLM sidecar URL is
        // configured, HTML gets converted to clean Markdown via the MLX
        // model before parsing. The converter falls back to the built-in
        // tag stripper if the sidecar is unavailable.
        let mut converter_registry = ConverterRegistry::new();
        if let Some(ref url) = config.readerlm_url {
            tracing::info!(url = %url, "ReaderLM HTML converter enabled");
            converter_registry.register_front(Box::new(ReaderLmConverter::new(url.clone())));
        }

        let source_service = Arc::new(
            SourceService::with_full_pipeline(
                Arc::clone(&repo),
                embedder.clone(),
                extractor,
                resolver,
                Some(pg_resolver),
            )
            .with_converter_registry(converter_registry)
            .with_table_dims(config.embedding.table_dims.clone())
            .with_chunk_config(config.chunk_size, config.chunk_overlap)
            .with_extract_concurrency(config.extract_concurrency),
        );
        let abstention_config = covalence_core::search::abstention::AbstentionConfig {
            min_relevance_score: config.search.abstention_threshold,
            ..Default::default()
        };
        let search = SearchService::with_config(
            Arc::clone(&repo),
            Arc::clone(&graph),
            embedder.clone(),
            config.embedding.table_dims.clone(),
        )
        .with_abstention_config(abstention_config);
        let search_service = Arc::new(match reranker {
            Some(rnk) => search.with_reranker(rnk),
            None => search,
        });
        let node_service = Arc::new(NodeService::new(Arc::clone(&repo), Arc::clone(&graph)));
        let edge_service = Arc::new(EdgeService::new(Arc::clone(&repo)));
        let article_service = Arc::new(ArticleService::new(Arc::clone(&repo)));
        let admin_service = Arc::new(
            AdminService::new(Arc::clone(&repo), Arc::clone(&graph)).with_embedder(embedder),
        );

        Ok(Self {
            config,
            repo,
            graph,
            source_service,
            search_service,
            node_service,
            edge_service,
            article_service,
            admin_service,
        })
    }
}

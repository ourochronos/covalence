//! Application state shared across request handlers.

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::RwLock;

use covalence_core::config::Config;
use covalence_core::graph::{GraphSidecar, SharedGraph};
use covalence_core::ingestion::embedder::Embedder;
use covalence_core::ingestion::extractor::Extractor;
use covalence_core::ingestion::resolver::EntityResolver;
use covalence_core::ingestion::{LlmExtractor, OpenAiEmbedder, PgResolver};
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

        let embedder: Option<Arc<dyn Embedder>> = config.openai_api_key.as_ref().map(|key| {
            Arc::new(OpenAiEmbedder::new(
                &config.embedding,
                key.clone(),
                config.openai_base_url.clone(),
            )) as Arc<dyn Embedder>
        });

        let extractor: Option<Arc<dyn Extractor>> = if config.chat_model.is_empty() {
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

        let resolver: Option<Arc<dyn EntityResolver>> =
            Some(Arc::new(PgResolver::new(Arc::clone(&repo))));

        let source_service = Arc::new(
            SourceService::with_full_pipeline(
                Arc::clone(&repo),
                embedder.clone(),
                extractor,
                resolver,
            )
            .with_extract_concurrency(config.extract_concurrency),
        );
        let search_service = Arc::new(SearchService::with_embedder(
            Arc::clone(&repo),
            Arc::clone(&graph),
            embedder,
        ));
        let node_service = Arc::new(NodeService::new(Arc::clone(&repo), Arc::clone(&graph)));
        let edge_service = Arc::new(EdgeService::new(Arc::clone(&repo)));
        let article_service = Arc::new(ArticleService::new(Arc::clone(&repo)));
        let admin_service = Arc::new(AdminService::new(Arc::clone(&repo), Arc::clone(&graph)));

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

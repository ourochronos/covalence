//! Application state shared across request handlers.

use std::sync::Arc;

use anyhow::Result;

use covalence_core::config::Config;
use covalence_core::factory::ServiceFactory;
use covalence_core::graph::engine::GraphEngine;
use covalence_core::graph::sync::full_reload;
use covalence_core::graph::{AgeEngine, PetgraphEngine};
use covalence_core::services::{
    AdminService, AnalysisService, AskService, EdgeService, NodeService, RetryQueueService,
    SearchService, SourceService,
};
use covalence_core::storage::postgres::PgRepo;

/// Shared application state for Axum handlers.
#[derive(Clone)]
pub struct AppState {
    /// Application configuration (used by API key middleware).
    pub config: Config,
    /// Shared database repository.
    pub repo: Arc<PgRepo>,
    /// Graph engine trait object for read-path operations.
    pub graph_engine: Arc<dyn GraphEngine>,
    /// Source ingestion and management.
    pub source_service: Arc<SourceService>,
    /// Multi-dimensional fused search.
    pub search_service: Arc<SearchService>,
    /// Graph node operations.
    pub node_service: Arc<NodeService>,
    /// Graph edge operations.
    pub edge_service: Arc<EdgeService>,
    /// Administrative operations.
    pub admin_service: Arc<AdminService>,
    /// Cross-domain analysis.
    pub analysis_service: Arc<AnalysisService>,
    /// LLM-powered knowledge synthesis.
    pub ask_service: Option<Arc<AskService>>,
    /// Persistent retry queue.
    pub queue_service: Arc<RetryQueueService>,
    /// Runtime configuration service.
    pub config_service: Arc<covalence_core::services::ConfigService>,
    /// Ontology service (configurable knowledge schema).
    pub ontology_service: Arc<covalence_core::services::OntologyService>,
    /// Prometheus metrics handle for rendering the `/metrics`
    /// endpoint. `None` if the recorder failed to install.
    pub prometheus_handle: Option<metrics_exporter_prometheus::PrometheusHandle>,
}

impl AppState {
    /// Initialize application state from configuration.
    ///
    /// Delegates shared service construction to [`ServiceFactory`],
    /// then applies API-specific concerns: graph sidecar loading,
    /// graph engine selection (petgraph vs AGE), and wiring the
    /// chosen graph engine into the analysis and admin services.
    pub async fn new(config: Config) -> Result<Self> {
        // Install the Prometheus metrics recorder. This must happen
        // before any metrics are emitted. `.ok()` silently handles
        // the case where a recorder is already installed (e.g. in
        // tests).
        let prometheus_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
            .install_recorder()
            .ok();
        if prometheus_handle.is_some() {
            tracing::info!("prometheus metrics recorder installed");
        }

        let repo = Arc::new(PgRepo::new(&config.database_url).await?);

        // Build all shared services via the factory.
        let factory = ServiceFactory::new(&config, Arc::clone(&repo))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        // Load the graph sidecar from PG on startup so graph and
        // structural search dimensions have data immediately.
        match full_reload(repo.pool(), Arc::clone(&factory.graph)).await {
            Ok(()) => {
                let g = factory.graph.read().await;
                tracing::info!(
                    nodes = g.node_count(),
                    edges = g.edge_count(),
                    "graph sidecar loaded from PG"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "failed to load graph sidecar on startup"
                );
            }
        }

        // Create the graph engine based on configuration.
        // This is API-specific: the worker always uses petgraph.
        let graph_engine: Arc<dyn GraphEngine> = if config.graph_engine == "age" {
            tracing::info!("using Apache AGE graph engine");
            let age = AgeEngine::new(repo.pool().clone(), "covalence_graph".to_string());
            match age.reload(repo.pool()).await {
                Ok(result) => {
                    tracing::info!(
                        nodes = result.node_count,
                        edges = result.edge_count,
                        "AGE graph loaded from PG"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "failed to load AGE graph on startup"
                    );
                }
            }
            Arc::new(age)
        } else {
            tracing::info!("using petgraph in-memory graph engine");
            Arc::new(PetgraphEngine::new(Arc::clone(&factory.graph)))
        };

        // Rebuild analysis and admin services with the real graph
        // engine when AGE is selected (factory defaults to petgraph).
        let analysis_service = if config.graph_engine == "age" {
            Arc::new(
                covalence_core::services::AnalysisService::new(
                    Arc::clone(&repo),
                    Arc::clone(&graph_engine),
                )
                .with_embedder(factory.embedder.clone())
                .with_chat_backend(factory.chat_backend.clone())
                .with_node_embed_dim(config.embedding.table_dims.node),
            )
        } else {
            factory.analysis_service
        };

        let admin_service = if config.graph_engine == "age" {
            Arc::new(
                covalence_core::services::AdminService::new(
                    Arc::clone(&repo),
                    Arc::clone(&graph_engine),
                    Arc::clone(&factory.graph),
                )
                .with_embedder(factory.embedder.clone())
                .with_chat_backend(factory.chat_backend.clone())
                .with_config(config.clone()),
            )
        } else {
            factory.admin_service
        };

        Ok(Self {
            config,
            repo,
            graph_engine,
            source_service: factory.source_service,
            search_service: factory.search_service,
            node_service: factory.node_service,
            edge_service: factory.edge_service,
            admin_service,
            analysis_service,
            ask_service: factory.ask_service,
            queue_service: factory.queue_service,
            config_service: factory.config_service,
            ontology_service: factory.ontology_service,
            prometheus_handle,
        })
    }
}

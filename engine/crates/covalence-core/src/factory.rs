//! Service factory — centralised construction of all shared services.
//!
//! Both the API server (`covalence-api`) and the queue worker
//! (`covalence-worker`) need the same set of services built from
//! the same configuration. This module owns that construction
//! complexity so each binary is a thin wrapper.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::config::Config;
use crate::error::Result;
use crate::extensions::ExtensionLoader;
use crate::graph::engine::GraphEngine;
use crate::graph::{GraphSidecar, PetgraphEngine, SharedGraph};
use crate::ingestion::embedder::Embedder;
use crate::ingestion::extractor::Extractor;
use crate::ingestion::resolver::EntityResolver;
use crate::ingestion::{
    ChainChatBackend, ChatBackend, ChatBackendExtractor, CliChatBackend, ConverterRegistry,
    FastcorefClient, GlinerExtractor, HttpChatBackend, HttpExtractor, LlmExtractor,
    LlmSectionCompiler, LlmStatementExtractor, OpenAiEmbedder, PdfConverter, PgResolver,
    ReaderLmConverter, ServiceExtractor, ServiceRegistry, ServiceTransport, TwoPassExtractor,
    VoyageConfig, VoyageEmbedder, fingerprint_config_from,
};
use crate::search::abstention::AbstentionConfig;
use crate::search::cache::CacheConfig;
use crate::search::rerank::{HttpReranker, RerankConfig, Reranker};
use crate::services::adapter_service::AdapterService;
use crate::services::{
    AdminService, AnalysisService, AskService, ConfigService, EdgeService, HookService,
    NodeService, OntologyService, RetryQueueService, SearchService, SessionService, SourceService,
};
use crate::storage::postgres::PgRepo;

/// Centralised factory that constructs every service from [`Config`]
/// and a [`PgRepo`].
///
/// Both the HTTP API and the queue worker use this factory. Each
/// binary pulls out only the services it needs.
pub struct ServiceFactory {
    /// The shared database repository.
    pub repo: Arc<PgRepo>,
    /// In-memory petgraph sidecar (always built, even when AGE is
    /// the primary graph engine).
    pub graph: SharedGraph,
    /// The embedding provider (Voyage or OpenAI), if configured.
    pub embedder: Option<Arc<dyn Embedder>>,
    /// The reranking provider (Voyage rerank), if configured.
    pub reranker: Option<Arc<dyn Reranker>>,
    /// The chat/LLM backend used by extraction and synthesis.
    pub chat_backend: Option<Arc<dyn ChatBackend>>,
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
    /// Session/conversation management.
    pub session_service: Arc<SessionService>,
    /// Persistent retry queue.
    pub queue_service: Arc<RetryQueueService>,
    /// Runtime configuration service.
    pub config_service: Arc<ConfigService>,
    /// Ontology service (configurable knowledge schema).
    pub ontology_service: Arc<OntologyService>,
    /// Lifecycle hook service for pipeline extensibility.
    pub hook_service: Arc<HookService>,
    /// Registry of external service transports (HTTP and STDIO).
    pub service_registry: Arc<ServiceRegistry>,
    /// Agent memory lifecycle service.
    pub agent_memory_service: Arc<crate::services::AgentMemoryService>,
}

impl ServiceFactory {
    /// Build all services from the given configuration and repo.
    ///
    /// This is an async constructor because several steps require
    /// database queries or service validation.
    pub async fn new(config: &Config, repo: Arc<PgRepo>) -> Result<Self> {
        let graph: SharedGraph = Arc::new(RwLock::new(GraphSidecar::new()));

        // ── Embedder + Reranker ─────────────────────────────────
        let (embedder, reranker) = Self::build_embedder_and_reranker(config);

        // ── Entity extractor ────────────────────────────────────
        let extractor = Self::build_extractor(config);

        // ── Entity resolver (PgResolver) ────────────────────────
        let pg_resolver = Arc::new(match embedder.clone() {
            Some(emb) => PgResolver::with_embedder(
                Arc::clone(&repo),
                config.resolve_trigram_threshold,
                emb,
                config.resolve_vector_threshold,
            )
            .with_node_embed_dim(config.embedding.table_dims.node)
            .with_graph(Arc::clone(&graph))
            .with_tier5(config.pipeline.tier5_enabled),
            None => PgResolver::with_threshold(Arc::clone(&repo), config.resolve_trigram_threshold),
        });
        let resolver: Option<Arc<dyn EntityResolver>> =
            Some(Arc::clone(&pg_resolver) as Arc<dyn EntityResolver>);

        // ── Converter registry ──────────────────────────────────
        let converter_registry = Self::build_converter_registry(config).await?;

        // ── Coref URL (explicit or auto-derived from extractor) ──
        let coref_url = config.coref_url.clone().or_else(|| {
            if config.entity_extractor == "sidecar" {
                config
                    .extract_url
                    .clone()
                    .or_else(|| Some("http://localhost:8433".to_string()))
            } else {
                None
            }
        });

        // ── Source service (progressive builder) ────────────────
        let has_http_extractor = extractor.is_some();
        let mut source_svc = SourceService::with_full_pipeline(
            Arc::clone(&repo),
            embedder.clone(),
            extractor,
            resolver,
            Some(pg_resolver),
        )
        .with_converter_registry(converter_registry)
        .with_table_dims(config.embedding.table_dims.clone())
        .with_chunk_config(config.chunk_size, config.chunk_overlap)
        .with_min_section_size(config.min_section_size)
        .with_extract_concurrency(config.extract_concurrency)
        .with_extract_batch_config(config.min_extract_tokens, config.extract_batch_tokens)
        .with_pipeline_config(config.pipeline.clone())
        .with_fingerprint_config(fingerprint_config_from(
            &config.pipeline,
            config.chunk_size,
            config.chunk_overlap,
            &config.entity_extractor,
            &config.chat_model,
            config.min_extract_tokens,
            config.extract_batch_tokens,
            config.resolve_trigram_threshold,
            config.resolve_vector_threshold,
            &config.embed_model,
            config.readerlm_url.is_some(),
            config.pdf_url.is_some(),
            coref_url.is_some(),
        ));

        // ── Adapter service (config-driven domain classification) ─
        let adapter_service = Arc::new(AdapterService::new(Arc::clone(&repo)));
        source_svc = source_svc.with_adapter_service(Arc::clone(&adapter_service));

        // ── Coref client (validate sidecar) ─────────────────────
        source_svc = Self::wire_coref_client(config, &coref_url, source_svc).await?;

        // ── Chat backend ────────────────────────────────────────
        let chat_backend = Self::build_chat_backend(config);

        // ── Wire statement pipeline ─────────────────────────────
        if config.pipeline.statement_enabled {
            if let Some(ref backend) = chat_backend {
                let stmt_extractor =
                    Arc::new(LlmStatementExtractor::with_backend(Arc::clone(backend)));
                source_svc = source_svc.with_statement_extractor(
                    stmt_extractor as Arc<dyn crate::ingestion::StatementExtractor>,
                );

                let section_compiler =
                    Arc::new(LlmSectionCompiler::with_backend(Arc::clone(backend)));
                source_svc = source_svc
                    .with_section_compiler(
                        Arc::clone(&section_compiler) as Arc<dyn crate::ingestion::SectionCompiler>
                    )
                    .with_source_summary_compiler(
                        section_compiler as Arc<dyn crate::ingestion::SourceSummaryCompiler>,
                    );

                tracing::info!(
                    backend = %config.chat_backend,
                    model = %config.chat_model,
                    window_chars = config.pipeline.statement_window_chars,
                    window_overlap = config.pipeline.statement_window_overlap,
                    "statement-first extraction pipeline enabled \
                     (with clustering + compilation)"
                );
            }
        }

        // ── Wire entity extractor via chat backend (CLI mode) ───
        if config.chat_backend == "cli"
            && !matches!(
                config.entity_extractor.as_str(),
                "sidecar" | "gliner2" | "two_pass"
            )
        {
            if let Some(ref backend) = chat_backend {
                tracing::info!("using chat backend for entity extraction (CLI mode)");
                source_svc = source_svc
                    .with_extractor(Arc::new(ChatBackendExtractor::new(Arc::clone(backend)))
                        as Arc<dyn Extractor>);
            }
        } else if !has_http_extractor {
            if let Some(ref backend) = chat_backend {
                tracing::info!(
                    "no HTTP entity extractor available; \
                     using chat backend for entity extraction"
                );
                source_svc = source_svc
                    .with_extractor(Arc::new(ChatBackendExtractor::new(Arc::clone(backend)))
                        as Arc<dyn Extractor>);
            }
        }

        // Wire chat backend for semantic code summaries
        if let Some(ref backend) = chat_backend {
            source_svc = source_svc.with_chat_backend(Arc::clone(backend));
        }

        // ── Domain extractors from extensions ───────────────────
        // Scan extension manifests for services with `extractor_for`
        // and register them as domain-specific extractors.
        let extensions_dir_path =
            std::env::var("COVALENCE_EXTENSIONS_DIR").unwrap_or_else(|_| "extensions".to_string());
        if std::path::Path::new(&extensions_dir_path).is_dir() {
            if let Ok(entries) = std::fs::read_dir(&extensions_dir_path) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let manifest_path = path.join("extension.yaml");
                    if path.is_dir() && manifest_path.exists() {
                        if let Ok(manifest) = ExtensionLoader::parse_manifest(&manifest_path) {
                            if let Some(ref svc_def) = manifest.service {
                                if let Some(ref domain) = svc_def.extractor_for {
                                    let transport = match svc_def.transport.as_str() {
                                        "stdio" => {
                                            if let Some(ref cmd) = svc_def.command {
                                                ServiceTransport::Stdio {
                                                    command: cmd.clone(),
                                                    args: svc_def.args.clone(),
                                                }
                                            } else {
                                                continue;
                                            }
                                        }
                                        "http" => {
                                            if let Some(ref url) = svc_def.url {
                                                ServiceTransport::Http { url: url.clone() }
                                            } else {
                                                continue;
                                            }
                                        }
                                        _ => continue,
                                    };
                                    let extractor = Arc::new(ServiceExtractor::new(
                                        svc_def.name.clone(),
                                        transport,
                                    ))
                                        as Arc<dyn crate::ingestion::extractor::Extractor>;
                                    tracing::info!(
                                        extension = %manifest.name,
                                        service = %svc_def.name,
                                        domain = %domain,
                                        "registered domain extractor from extension"
                                    );
                                    source_svc =
                                        source_svc.with_domain_extractor(domain.clone(), extractor);
                                }
                            }
                        }
                    }
                }
            }
        }

        // ── Ontology service (polls DB every 60s) ───────────────
        // Created early so source_service can reference it.
        let ontology_service = Arc::new(OntologyService::new(Arc::clone(&repo)));
        if let Err(e) = ontology_service.refresh().await {
            tracing::warn!(
                error = %e,
                "initial ontology load failed (will retry)"
            );
        }

        // ── Hook service (early, so source_svc can reference it) ─
        let hook_service = Arc::new(HookService::new(Arc::clone(&repo)));
        source_svc = source_svc
            .with_hook_service(Arc::clone(&hook_service))
            .with_ontology(Arc::clone(&ontology_service))
            .with_metadata_enforcement(&config.metadata_enforcement);

        let source_service = Arc::new(source_svc);

        // ── Search service ──────────────────────────────────────
        let search_service =
            Self::build_search_service(config, &repo, &graph, embedder.clone(), reranker.clone())
                .await;

        // ── Simple services ─────────────────────────────────────
        let node_service = Arc::new(NodeService::new(Arc::clone(&repo), Arc::clone(&graph)));
        let edge_service = Arc::new(EdgeService::new(Arc::clone(&repo)));
        // ── Extension loader ──────────────────────────────────────
        let extension_loader = ExtensionLoader::new(Arc::clone(&repo));
        let extensions_dir =
            std::env::var("COVALENCE_EXTENSIONS_DIR").unwrap_or_else(|_| "extensions".to_string());
        let mut load_results = Vec::new();
        if std::path::Path::new(&extensions_dir).is_dir() {
            match extension_loader.load_directory(&extensions_dir).await {
                Ok(results) => {
                    if !results.is_empty() {
                        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
                        tracing::info!(
                            extensions = ?names,
                            "loaded extensions"
                        );
                        // Refresh ontology cache after loading
                        // extensions so new types are visible.
                        if let Err(e) = ontology_service.refresh().await {
                            tracing::warn!(
                                error = %e,
                                "ontology refresh after extension \
                                 load failed"
                            );
                        }
                        load_results = results;
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "extension loading failed"
                    );
                }
            }
        }

        ontology_service.spawn_refresh_loop(60);

        // ── Analysis service (needs a graph engine) ─────────────
        // For factory purposes we always use petgraph; the API can
        // override with AGE via `with_graph_engine()` after
        // construction.
        let petgraph_engine: Arc<dyn GraphEngine> =
            Arc::new(PetgraphEngine::new(Arc::clone(&graph)));

        let analysis_service = Arc::new(
            AnalysisService::new(Arc::clone(&repo), Arc::clone(&petgraph_engine))
                .with_embedder(embedder.clone())
                .with_chat_backend(chat_backend.clone())
                .with_node_embed_dim(config.embedding.table_dims.node)
                .with_ontology(Arc::clone(&ontology_service)),
        );

        // ── Session service ──────────────────────────────────────
        let session_service = Arc::new(SessionService::new(Arc::clone(&repo)));

        // ── Agent memory service ─────────────────────────────────
        let agent_memory_service = Arc::new(
            crate::services::AgentMemoryService::new(
                Arc::clone(&repo),
                Arc::clone(&source_service),
                Arc::clone(&search_service),
                Arc::clone(&session_service),
            )
            .with_chat_backend(chat_backend.clone()),
        );

        // ── Ask service ─────────────────────────────────────────
        let ask_service = {
            let ask_model = &config.ask_model;
            let ask_backend: Arc<dyn ChatBackend> = Arc::new(resolve_ask_backend(ask_model));
            tracing::info!(
                model = %ask_model,
                "ask service using dedicated synthesis backend"
            );
            Some(Arc::new(
                AskService::new(Arc::clone(&search_service), ask_backend, Arc::clone(&repo))
                    .with_hooks(Arc::clone(&hook_service))
                    .with_sessions(Arc::clone(&session_service))
                    .with_adapters(Arc::clone(&adapter_service)),
            ))
        };

        // ── Admin service ───────────────────────────────────────
        let admin_service = Arc::new(
            AdminService::new(
                Arc::clone(&repo),
                Arc::clone(&petgraph_engine),
                Arc::clone(&graph),
            )
            .with_embedder(embedder.clone())
            .with_chat_backend(chat_backend.clone())
            .with_config(config.clone()),
        );

        // ── Queue service ───────────────────────────────────────
        let queue_service = Arc::new(RetryQueueService::new(
            Arc::clone(&repo),
            config.queue.clone(),
        ));

        // ── Config service (polls DB every 30s) ─────────────────
        let config_service = Arc::new(ConfigService::new(Arc::clone(&repo)));
        if let Err(e) = config_service.refresh().await {
            tracing::warn!(
                error = %e,
                "initial config load failed (will retry)"
            );
        }

        // Refresh config after extensions may have seeded defaults.
        if !load_results.is_empty() {
            if let Err(e) = config_service.refresh().await {
                tracing::warn!(
                    error = %e,
                    "config refresh after extension load failed"
                );
            }
        }
        config_service.spawn_refresh_loop(30);

        // ── Service registry ──────────────────────────────────────
        let mut service_registry = Self::build_service_registry(config).await;

        // Register extension-declared services into the registry.
        for result in &load_results {
            for svc_def in &result.services {
                let transport = match svc_def.transport.as_str() {
                    "stdio" => {
                        if let Some(ref cmd) = svc_def.command {
                            ServiceTransport::Stdio {
                                command: cmd.clone(),
                                args: svc_def.args.clone(),
                            }
                        } else {
                            tracing::warn!(
                                extension = %result.name,
                                service = %svc_def.name,
                                "stdio service missing command"
                            );
                            continue;
                        }
                    }
                    "http" => {
                        if let Some(ref url) = svc_def.url {
                            ServiceTransport::Http { url: url.clone() }
                        } else {
                            tracing::warn!(
                                extension = %result.name,
                                service = %svc_def.name,
                                "http service missing url"
                            );
                            continue;
                        }
                    }
                    other => {
                        tracing::warn!(
                            extension = %result.name,
                            service = %svc_def.name,
                            transport = %other,
                            "unknown service transport type"
                        );
                        continue;
                    }
                };
                service_registry.register(&svc_def.name, transport);
                tracing::debug!(
                    extension = %result.name,
                    service = %svc_def.name,
                    "registered extension service"
                );
            }
        }

        let service_registry = Arc::new(service_registry);

        // Start periodic health checks for registered services.
        if !service_registry.list().is_empty() {
            service_registry.spawn_health_loop(120);
            tracing::info!("service health loop started (every 120s)");
        }

        Ok(Self {
            repo,
            graph,
            embedder,
            reranker,
            chat_backend,
            source_service,
            search_service,
            node_service,
            edge_service,
            admin_service,
            analysis_service,
            ask_service,
            session_service,
            queue_service,
            config_service,
            ontology_service,
            hook_service,
            service_registry,
            agent_memory_service,
        })
    }

    // ── Private helpers ─────────────────────────────────────────

    /// Build the embedder and reranker from config.
    #[allow(clippy::type_complexity)]
    fn build_embedder_and_reranker(
        config: &Config,
    ) -> (Option<Arc<dyn Embedder>>, Option<Arc<dyn Reranker>>) {
        let use_voyage = config.embed_provider == "voyage" || config.voyage_api_key.is_some();

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
        }
    }

    /// Build the entity extractor from config.
    fn build_extractor(config: &Config) -> Option<Arc<dyn Extractor>> {
        if config.entity_extractor == "sidecar" {
            let base_url = config
                .extract_url
                .clone()
                .unwrap_or_else(|| "http://localhost:8433".to_string());
            tracing::info!(
                url = %base_url,
                "using unified extraction HTTP service"
            );
            Some(Arc::new(HttpExtractor::with_windowing(
                base_url,
                config.gliner_threshold,
                config.pipeline.ner_window_chars,
                config.pipeline.ner_window_overlap,
                config.pipeline.re_window_chars,
                config.pipeline.re_window_overlap,
            )) as Arc<dyn Extractor>)
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

            tracing::info!(
                has_chat_key = chat_key.is_some(),
                entity_extractor = %config.entity_extractor,
                chat_model = %config.chat_model,
                "two_pass extractor init"
            );

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
            let chat_key = config
                .chat_api_key
                .as_ref()
                .or(config.openai_api_key.as_ref());
            let chat_base = config.chat_base_url.clone();
            chat_key.map(|key| {
                Arc::new(LlmExtractor::new(
                    config.chat_model.clone(),
                    key.clone(),
                    chat_base,
                )) as Arc<dyn Extractor>
            })
        }
    }

    /// Build the converter registry (PDF, ReaderLM, etc.).
    async fn build_converter_registry(config: &Config) -> Result<ConverterRegistry> {
        let mut registry = ConverterRegistry::new();

        if let Some(ref url) = config.readerlm_url {
            tracing::info!(url = %url, "ReaderLM HTML converter enabled");
            registry.register_front(Box::new(ReaderLmConverter::new(url.clone())));
        }

        if let Some(ref url) = config.pdf_url {
            let pdf_conv = PdfConverter::new(url.clone());
            match pdf_conv.validate().await {
                Ok(()) => {
                    tracing::info!(
                        url = %url,
                        "PDF converter validated and enabled"
                    );
                    registry.register(Box::new(pdf_conv));
                }
                Err(e) => {
                    return Err(crate::error::Error::Config(format!(
                        "PDF service at {url} is unreachable: {e}. \
                         Either start the service or remove \
                         COVALENCE_PDF_URL."
                    )));
                }
            }
        }

        Ok(registry)
    }

    /// Validate and wire the fastcoref client onto the source service.
    async fn wire_coref_client(
        config: &Config,
        coref_url: &Option<String>,
        mut source_svc: SourceService,
    ) -> Result<SourceService> {
        if let Some(url) = coref_url {
            let coref_client = Arc::new(FastcorefClient::with_windowing(
                url.clone(),
                config.pipeline.coref_window_chars,
                config.pipeline.coref_window_overlap,
            ));
            match coref_client.validate().await {
                Ok(()) => {
                    tracing::info!(
                        url = %url,
                        "fastcoref service validated and enabled"
                    );
                    source_svc = source_svc.with_coref_client(coref_client);
                }
                Err(e) => {
                    if config.coref_url.is_some() {
                        // Explicitly configured — fail fast.
                        return Err(crate::error::Error::Config(format!(
                            "fastcoref service at {url} is unreachable: \
                             {e}. Either start the service or remove \
                             COVALENCE_COREF_URL."
                        )));
                    }
                    // Auto-derived — degrade gracefully.
                    tracing::warn!(
                        url = %url,
                        error = %e,
                        "fastcoref service unavailable — coref disabled \
                         (auto-derived URL, not explicitly configured)"
                    );
                }
            }
        }

        Ok(source_svc)
    }

    /// Build the chat backend (chain or HTTP).
    fn build_chat_backend(config: &Config) -> Option<Arc<dyn ChatBackend>> {
        let chat_model = config
            .pipeline
            .statement_model
            .clone()
            .unwrap_or_else(|| config.chat_model.clone());

        let http_key = config
            .chat_api_key
            .as_ref()
            .or(config.openai_api_key.as_ref())
            .cloned();

        if config.chat_backend == "cli" {
            let mut chain: Vec<(String, Box<dyn ChatBackend>)> = Vec::new();

            // Primary: configured CLI command + model
            chain.push((
                format!("{}({})", config.chat_cli_command, chat_model),
                Box::new(CliChatBackend::new(
                    config.chat_cli_command.clone(),
                    chat_model.clone(),
                )),
            ));

            // Fallback CLIs
            let fallbacks: &[(&str, &str)] = &[
                ("claude", "haiku"),
                ("copilot", "claude-haiku-4.5"),
                ("gemini", "gemini-3-flash-preview"),
            ];
            for &(cmd, model) in fallbacks {
                if cmd != config.chat_cli_command {
                    chain.push((
                        format!("{cmd}({model})"),
                        Box::new(CliChatBackend::new(cmd.to_string(), model.to_string())),
                    ));
                }
            }

            // HTTP fallback (OpenRouter) if API key is available
            if let Some(key) = http_key {
                chain.push((
                    format!("http({})", config.chat_model),
                    Box::new(HttpChatBackend::new(
                        config.chat_model.clone(),
                        key,
                        config.chat_base_url.clone(),
                    )),
                ));
            }

            let labels: Vec<&str> = chain.iter().map(|(l, _)| l.as_str()).collect();
            tracing::info!(
                chain = ?labels,
                "using multi-provider chat backend chain"
            );
            Some(Arc::new(ChainChatBackend::new(chain)) as Arc<dyn ChatBackend>)
        } else {
            http_key.map(|key| {
                Arc::new(HttpChatBackend::new(
                    chat_model,
                    key,
                    config.chat_base_url.clone(),
                )) as Arc<dyn ChatBackend>
            })
        }
    }

    /// Build the search service with all dimensions, ontology
    /// lookups, reranker, and cache.
    async fn build_search_service(
        config: &Config,
        repo: &Arc<PgRepo>,
        graph: &SharedGraph,
        embedder: Option<Arc<dyn Embedder>>,
        reranker: Option<Arc<dyn Reranker>>,
    ) -> Arc<SearchService> {
        let abstention_config = AbstentionConfig {
            min_relevance_score: config.search.abstention_threshold,
            ..Default::default()
        };

        let search = SearchService::with_config(
            Arc::clone(repo),
            Arc::clone(graph),
            embedder,
            config.embedding.table_dims.clone(),
        )
        .with_abstention_config(abstention_config)
        .with_cache(CacheConfig::default());

        // CC fusion toggle
        let use_cc = std::env::var("COVALENCE_CC_FUSION")
            .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
            .unwrap_or(true);
        let search = search.with_cc_fusion(use_cc);
        if !use_cc {
            tracing::info!("using RRF fusion instead of CC (COVALENCE_CC_FUSION=false)");
        }

        // Wire internal domains from ontology for DDSS boost.
        let internal_domains: HashSet<String> =
            sqlx::query_scalar("SELECT id FROM ontology_domains WHERE is_internal = true")
                .fetch_all(repo.pool())
                .await
                .unwrap_or_default()
                .into_iter()
                .collect();
        let search = if !internal_domains.is_empty() {
            search.with_internal_domains(internal_domains)
        } else {
            search
        };

        // Wire view -> edge type mappings from ontology.
        let view_rows: Vec<(String, String)> =
            sqlx::query_as("SELECT view_name, rel_type FROM ontology_view_edges")
                .fetch_all(repo.pool())
                .await
                .unwrap_or_default();
        let search = if !view_rows.is_empty() {
            let mut view_edges: HashMap<String, HashSet<String>> = HashMap::new();
            for (view, rel) in view_rows {
                view_edges.entry(view).or_default().insert(rel);
            }
            search.with_view_edges(view_edges)
        } else {
            search
        };

        Arc::new(match reranker {
            Some(rnk) => search.with_reranker(rnk),
            None => search,
        })
    }

    /// Build the service registry from config-driven STDIO entries
    /// and well-known HTTP services.
    ///
    /// Returns a bare [`ServiceRegistry`] so the caller can add
    /// extension-declared services before wrapping in `Arc`.
    async fn build_service_registry(config: &Config) -> ServiceRegistry {
        let mut registry = ServiceRegistry::new();

        // Register STDIO services from config.
        for sc in &config.external_services {
            registry.register(
                &sc.name,
                ServiceTransport::Stdio {
                    command: sc.command.clone(),
                    args: sc.args.clone(),
                },
            );
        }

        // Register well-known HTTP services.
        if let Some(ref url) = config.pdf_url {
            registry.register("pdf", ServiceTransport::Http { url: url.clone() });
        }
        if let Some(ref url) = config.readerlm_url {
            registry.register("readerlm", ServiceTransport::Http { url: url.clone() });
        }
        if let Some(ref url) = config.coref_url {
            registry.register("coref", ServiceTransport::Http { url: url.clone() });
        }

        // Validate services at startup.
        let failures = registry.validate_all().await;
        for (name, err) in &failures {
            tracing::warn!(
                service = %name,
                error = %err,
                "service validation failed — disabled"
            );
        }

        if !registry.list().is_empty() {
            let names: Vec<&str> = registry.list().iter().map(|(n, _)| *n).collect();
            tracing::info!(
                services = ?names,
                "service registry initialized"
            );
        }

        registry
    }
}

/// Resolve a model name to a [`CliChatBackend`] for the ask service.
///
/// Uses the same mapping as
/// [`crate::services::ask::resolve_model_backend`] so that the
/// default and per-request overrides use the same logic.
pub fn resolve_ask_backend(model: &str) -> CliChatBackend {
    match model {
        "haiku" | "sonnet" | "opus" => CliChatBackend::new("claude".to_string(), model.to_string()),
        "gemini" => CliChatBackend::new("gemini".to_string(), "gemini-2.5-flash".to_string()),
        "copilot" => CliChatBackend::new("copilot".to_string(), "claude-haiku-4.5".to_string()),
        // Default: treat as a claude model name.
        other => CliChatBackend::new("claude".to_string(), other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ConsolidationConfig, EmbeddingConfig, PipelineConfig, RetryQueueConfig, SearchConfig,
        TableDimensions,
    };

    /// Build a minimal [`Config`] suitable for unit tests.
    ///
    /// All API keys are `None` and all URLs point to localhost.
    fn test_config() -> Config {
        Config {
            database_url: "postgres://localhost/test".into(),
            bind_addr: "0.0.0.0:8431".into(),
            api_key: None,
            openai_api_key: None,
            openai_base_url: None,
            voyage_api_key: None,
            voyage_base_url: None,
            graph_engine: "petgraph".into(),
            embed_provider: "openai".into(),
            embed_model: "text-embedding-3-large".into(),
            chat_model: "gpt-4o".into(),
            chat_api_key: None,
            chat_base_url: None,
            chat_backend: "cli".into(),
            chat_cli_command: "gemini".into(),
            chunk_size: 1000,
            chunk_overlap: 200,
            min_section_size: 200,
            embedding: EmbeddingConfig {
                model: "text-embedding-3-large".into(),
                batch_size: 64,
                table_dims: TableDimensions::default(),
            },
            extract_concurrency: 8,
            min_extract_tokens: 30,
            extract_batch_tokens: 2000,
            entity_extractor: "llm".into(),
            extract_url: None,
            gliner_threshold: 0.5,
            consolidation: ConsolidationConfig {
                batch_interval_secs: 300,
                deep_interval_secs: 86_400,
                delta_threshold: 0.1,
            },
            search: SearchConfig {
                rrf_k: 60.0,
                default_limit: 10,
                abstention_threshold: 0.001,
            },
            pipeline: PipelineConfig::default(),
            coref_url: None,
            pdf_url: None,
            readerlm_url: None,
            resolve_trigram_threshold: 0.4,
            resolve_vector_threshold: 0.85,
            queue: RetryQueueConfig::default(),
            ask_model: "sonnet".into(),
            external_services: vec![],
            metadata_enforcement: "warn".into(),
        }
    }

    #[test]
    fn resolve_ask_backend_known_models() {
        let b = resolve_ask_backend("haiku");
        // CliChatBackend doesn't expose fields, but we can verify
        // it's constructed without panicking.
        drop(b);

        let b = resolve_ask_backend("sonnet");
        drop(b);

        let b = resolve_ask_backend("gemini");
        drop(b);

        let b = resolve_ask_backend("copilot");
        drop(b);
    }

    #[test]
    fn resolve_ask_backend_unknown_defaults_to_claude() {
        let b = resolve_ask_backend("custom-model-v3");
        drop(b);
    }

    #[test]
    fn build_embedder_no_key_is_none() {
        let config = test_config();
        let (emb, rnk) = ServiceFactory::build_embedder_and_reranker(&config);
        assert!(emb.is_none());
        assert!(rnk.is_none());
    }

    #[test]
    fn build_chat_backend_http_without_key_is_none() {
        let mut config = test_config();
        config.chat_backend = "http".to_string();
        let backend = ServiceFactory::build_chat_backend(&config);
        assert!(backend.is_none());
    }

    #[test]
    fn build_chat_backend_cli_creates_chain() {
        let config = test_config();
        let backend = ServiceFactory::build_chat_backend(&config);
        assert!(backend.is_some());
    }

    #[test]
    fn build_extractor_empty_model_is_none() {
        let mut config = test_config();
        config.chat_model = String::new();
        config.entity_extractor = "llm".to_string();
        let ext = ServiceFactory::build_extractor(&config);
        assert!(ext.is_none());
    }
}

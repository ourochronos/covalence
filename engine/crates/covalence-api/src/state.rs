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
    ChatBackend, ChatBackendExtractor, CliChatBackend, ConverterRegistry, FallbackChatBackend,
    FastcorefClient, GlinerExtractor, HttpChatBackend, LlmExtractor, LlmSectionCompiler,
    LlmStatementExtractor, OpenAiEmbedder, PdfConverter, PgResolver, ReaderLmConverter,
    SidecarExtractor, TwoPassExtractor, VoyageConfig, VoyageEmbedder, fingerprint_config_from,
};
use covalence_core::search::rerank::{HttpReranker, RerankConfig, Reranker};
use covalence_core::services::{
    AdminService, AnalysisService, ArticleService, EdgeService, NodeService, SearchService,
    SourceService,
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
    /// Cross-domain analysis.
    pub analysis_service: Arc<AnalysisService>,
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
            // Unified sidecar: NER + relationships with Rust-side
            // windowing. Coref is handled separately by
            // FastcorefClient.
            let base_url = config
                .extract_url
                .clone()
                .unwrap_or_else(|| "http://localhost:8433".to_string());
            tracing::info!(
                url = %base_url,
                "using unified extraction sidecar"
            );
            Some(Arc::new(SidecarExtractor::with_windowing(
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
            .with_graph(Arc::clone(&graph))
            .with_tier5(config.pipeline.tier5_enabled),
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
        if let Some(ref url) = config.pdf_url {
            tracing::info!(url = %url, "PDF converter enabled");
            converter_registry.register(Box::new(PdfConverter::new(url.clone())));
        }

        // Determine the Fastcoref sidecar URL for neural coref
        // preprocessing. Explicit COVALENCE_COREF_URL takes priority.
        // When using the unified sidecar extractor, auto-enable
        // coref using the same base URL (coref endpoint lives on
        // the same sidecar).
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

        if let Some(ref url) = coref_url {
            tracing::info!(url = %url, "Fastcoref preprocessing enabled");
            source_svc = source_svc.with_coref_client(Arc::new(FastcorefClient::with_windowing(
                url.clone(),
                config.pipeline.coref_window_chars,
                config.pipeline.coref_window_overlap,
            )));
        }

        // Build the chat backend (shared by statement pipeline + admin endpoints).
        //
        // When "cli" is selected and HTTP credentials exist, a
        // FallbackChatBackend wraps CLI→HTTP so quota exhaustion
        // is handled transparently.
        let chat_backend: Option<Arc<dyn ChatBackend>> = {
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
                let cli = Box::new(CliChatBackend::new(
                    config.chat_cli_command.clone(),
                    chat_model.clone(),
                ));

                if let Some(key) = http_key {
                    tracing::info!(
                        command = %config.chat_cli_command,
                        cli_model = %chat_model,
                        http_model = %config.chat_model,
                        "using CLI chat backend with HTTP fallback"
                    );
                    // HTTP fallback uses config.chat_model (OpenRouter
                    // format, e.g. "google/gemini-2.5-flash") rather
                    // than the CLI-format statement_model.
                    let http = Box::new(HttpChatBackend::new(
                        config.chat_model.clone(),
                        key,
                        config.chat_base_url.clone(),
                    ));
                    Some(Arc::new(FallbackChatBackend::new(cli, http)) as Arc<dyn ChatBackend>)
                } else {
                    tracing::info!(
                        command = %config.chat_cli_command,
                        model = %chat_model,
                        "using CLI chat backend (no HTTP fallback)"
                    );
                    Some(Arc::new(CliChatBackend::new(
                        config.chat_cli_command.clone(),
                        chat_model,
                    )) as Arc<dyn ChatBackend>)
                }
            } else {
                http_key.map(|key| {
                    Arc::new(HttpChatBackend::new(
                        chat_model,
                        key,
                        config.chat_base_url.clone(),
                    )) as Arc<dyn ChatBackend>
                })
            }
        };

        // Wire statement extractor when statement pipeline is enabled.
        if config.pipeline.statement_enabled {
            if let Some(ref backend) = chat_backend {
                let stmt_extractor =
                    Arc::new(LlmStatementExtractor::with_backend(Arc::clone(backend)));
                source_svc = source_svc.with_statement_extractor(
                    stmt_extractor as Arc<dyn covalence_core::ingestion::StatementExtractor>,
                );

                // Wire section compiler + source summary compiler
                // (same LlmSectionCompiler implements both traits).
                let section_compiler =
                    Arc::new(LlmSectionCompiler::with_backend(Arc::clone(backend)));
                source_svc = source_svc
                    .with_section_compiler(Arc::clone(&section_compiler)
                        as Arc<dyn covalence_core::ingestion::SectionCompiler>)
                    .with_source_summary_compiler(
                        section_compiler
                            as Arc<dyn covalence_core::ingestion::SourceSummaryCompiler>,
                    );

                tracing::info!(
                    backend = %config.chat_backend,
                    model = %config.chat_model,
                    window_chars = config.pipeline.statement_window_chars,
                    window_overlap = config.pipeline.statement_window_overlap,
                    "statement-first extraction pipeline enabled (with clustering + compilation)"
                );
            }
        }

        // When a CLI chat backend is available and the entity extractor
        // is the default LLM (HTTP-based), replace it with a
        // ChatBackendExtractor that routes through the CLI — more
        // reliable when HTTP API credits are exhausted. Don't replace
        // sidecar/gliner2/two_pass which are intentionally configured.
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

        let source_service = Arc::new(source_svc);
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
        .with_abstention_config(abstention_config)
        .with_cache(covalence_core::search::cache::CacheConfig::default());
        // CC fusion is the default (outperforms RRF in A/B testing).
        // Set COVALENCE_CC_FUSION=false to revert to RRF.
        let use_cc = std::env::var("COVALENCE_CC_FUSION")
            .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
            .unwrap_or(true);
        let search = search.with_cc_fusion(use_cc);
        if !use_cc {
            tracing::info!("using RRF fusion instead of CC (COVALENCE_CC_FUSION=false)");
        }
        let search_service = Arc::new(match reranker {
            Some(rnk) => search.with_reranker(rnk),
            None => search,
        });
        let node_service = Arc::new(NodeService::new(Arc::clone(&repo), Arc::clone(&graph)));
        let edge_service = Arc::new(EdgeService::new(Arc::clone(&repo)));
        let article_service = Arc::new(ArticleService::new(Arc::clone(&repo)));
        let analysis_service = Arc::new(
            AnalysisService::new(Arc::clone(&repo), Arc::clone(&graph))
                .with_embedder(embedder.clone())
                .with_chat_backend(chat_backend.clone())
                .with_node_embed_dim(config.embedding.table_dims.node),
        );

        let admin_service = Arc::new(
            AdminService::new(Arc::clone(&repo), Arc::clone(&graph))
                .with_embedder(embedder)
                .with_chat_backend(chat_backend)
                .with_config(config.clone()),
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
            analysis_service,
        })
    }
}

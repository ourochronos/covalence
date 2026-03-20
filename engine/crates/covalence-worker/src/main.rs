//! Covalence queue worker binary.
//!
//! Processes ingestion pipeline jobs from the persistent retry queue.
//! Runs as a separate process from the API server, with its own
//! connection pool and concurrency configuration.
//!
//! Job types:
//!   - ProcessSource: chunk + embed + coref + fan-out
//!   - ExtractChunk: per-chunk entity extraction
//!   - SummarizeEntity: semantic code summaries
//!   - ComposeSourceSummary: fan-in file summary
//!   - SynthesizeEdges: co-occurrence edge creation
//!   - ReprocessSource: re-ingest from stored content
//!   - EmbedBatch: batch vector embedding

use std::sync::Arc;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

use tokio::sync::RwLock;

use covalence_core::config::Config;
use covalence_core::graph::{GraphSidecar, PetgraphEngine, SharedGraph};
use covalence_core::ingestion::embedder::Embedder;
use covalence_core::ingestion::extractor::Extractor;
use covalence_core::ingestion::{
    ChainChatBackend, ChatBackend, ChatBackendExtractor, CliChatBackend, ConverterRegistry,
    FastcorefClient, HttpChatBackend, LlmSectionCompiler, LlmStatementExtractor, PdfConverter,
    PgResolver, VoyageConfig, VoyageEmbedder,
};
use covalence_core::services::{AdminService, RetryQueueService, SourceService};
use covalence_core::storage::postgres::PgRepo;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn")),
        )
        .init();

    dotenvy::dotenv().ok();

    let config = Config::from_env()?;

    tracing::info!(
        pool_size = config.queue.reprocess_concurrency,
        "starting covalence-worker"
    );

    // Worker gets its own connection pool, sized for concurrency.
    let pool_size = (config.queue.reprocess_concurrency as u32).max(10);
    let pool = PgPoolOptions::new()
        .max_connections(pool_size + 5) // headroom for queue management
        .connect(&config.database_url)
        .await?;

    let repo = Arc::new(PgRepo::from_pool(pool));

    // Build embedder.
    let embedder: Option<Arc<dyn Embedder>> = config.voyage_api_key.as_ref().map(|key| {
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

    // Build extractor.
    let chat_model = config
        .pipeline
        .statement_model
        .clone()
        .unwrap_or_else(|| config.chat_model.clone());

    let chat_backend: Option<Arc<dyn ChatBackend>> = if config.chat_backend == "cli" {
        let mut chain: Vec<(String, Box<dyn ChatBackend>)> = Vec::new();
        chain.push((
            format!("{}({})", config.chat_cli_command, chat_model),
            Box::new(CliChatBackend::new(
                config.chat_cli_command.clone(),
                chat_model.clone(),
            )),
        ));
        // Add fallback CLIs.
        for (cmd, model) in &[
            ("copilot", "claude-haiku-4.5"),
            ("gemini", "gemini-3-flash-preview"),
        ] {
            if *cmd != config.chat_cli_command {
                chain.push((
                    format!("{cmd}({model})"),
                    Box::new(CliChatBackend::new(cmd.to_string(), model.to_string())),
                ));
            }
        }
        let labels: Vec<&str> = chain.iter().map(|(l, _)| l.as_str()).collect();
        tracing::info!(chain = ?labels, "worker chat backend chain");
        Some(Arc::new(ChainChatBackend::new(chain)) as Arc<dyn ChatBackend>)
    } else {
        config
            .chat_api_key
            .as_ref()
            .or(config.openai_api_key.as_ref())
            .cloned()
            .map(|key| {
                Arc::new(HttpChatBackend::new(
                    chat_model,
                    key,
                    config.chat_base_url.clone(),
                )) as Arc<dyn ChatBackend>
            })
    };

    let extractor: Option<Arc<dyn Extractor>> = chat_backend.as_ref().map(|backend| {
        Arc::new(ChatBackendExtractor::new(Arc::clone(backend))) as Arc<dyn Extractor>
    });

    // Build resolver.
    let pg_resolver = embedder.as_ref().map(|emb| {
        Arc::new(
            PgResolver::with_embedder(
                Arc::clone(&repo),
                config.resolve_trigram_threshold,
                Arc::clone(emb),
                config.resolve_vector_threshold,
            )
            .with_node_embed_dim(config.embedding.table_dims.node)
            .with_tier5(config.pipeline.tier5_enabled),
        )
    });

    // Build converter registry.
    let mut converter_registry = ConverterRegistry::new();
    if let Some(ref url) = config.pdf_url {
        let pdf_conv = PdfConverter::new(url.clone());
        match pdf_conv.validate().await {
            Ok(()) => {
                tracing::info!(url = %url, "PDF converter validated");
                converter_registry.register(Box::new(pdf_conv));
            }
            Err(e) => {
                anyhow::bail!(
                    "PDF sidecar at {url} unreachable: {e}. \
                     Remove COVALENCE_PDF_URL or start the sidecar."
                );
            }
        }
    }

    // Build source service.
    let resolver: Option<Arc<dyn covalence_core::ingestion::resolver::EntityResolver>> =
        pg_resolver
            .as_ref()
            .map(|r| Arc::clone(r) as Arc<dyn covalence_core::ingestion::resolver::EntityResolver>);
    let mut source_svc = SourceService::with_full_pipeline(
        Arc::clone(&repo),
        embedder.clone(),
        extractor,
        resolver,
        pg_resolver,
    )
    .with_table_dims(config.embedding.table_dims.clone())
    .with_chunk_config(config.chunk_size, config.chunk_overlap)
    .with_min_section_size(config.min_section_size)
    .with_extract_concurrency(config.extract_concurrency)
    .with_extract_batch_config(config.min_extract_tokens, config.extract_batch_tokens)
    .with_pipeline_config(config.pipeline.clone());

    source_svc = source_svc.with_converter_registry(converter_registry);

    // Wire coref client.
    if let Some(ref url) = config.coref_url {
        let coref_client = Arc::new(FastcorefClient::with_windowing(
            url.clone(),
            config.pipeline.coref_window_chars,
            config.pipeline.coref_window_overlap,
        ));
        match coref_client.validate().await {
            Ok(()) => {
                tracing::info!(url = %url, "fastcoref validated");
                source_svc = source_svc.with_coref_client(coref_client);
            }
            Err(e) => {
                anyhow::bail!(
                    "fastcoref at {url} unreachable: {e}. \
                     Remove COVALENCE_COREF_URL or start the sidecar."
                );
            }
        }
    }

    // Wire chat backend for summaries.
    if let Some(ref backend) = chat_backend {
        let section_compiler = Arc::new(LlmSectionCompiler::with_backend(Arc::clone(backend)));
        source_svc = source_svc
            .with_section_compiler(Arc::clone(&section_compiler)
                as Arc<dyn covalence_core::ingestion::SectionCompiler>)
            .with_source_summary_compiler(
                section_compiler as Arc<dyn covalence_core::ingestion::SourceSummaryCompiler>,
            )
            .with_chat_backend(Arc::clone(backend));
    }

    // Wire statement extractor.
    if config.pipeline.statement_enabled {
        if let Some(ref backend) = chat_backend {
            let stmt_extractor = Arc::new(LlmStatementExtractor::with_backend(Arc::clone(backend)));
            source_svc = source_svc.with_statement_extractor(
                stmt_extractor as Arc<dyn covalence_core::ingestion::StatementExtractor>,
            );
        }
    }

    let source_svc = Arc::new(source_svc);

    // Build admin service (needed for edge synthesis jobs).
    // The worker uses a minimal petgraph sidecar — graph algorithms
    // are not critical here, but AdminService requires a graph engine.
    let graph: SharedGraph = Arc::new(RwLock::new(GraphSidecar::new()));
    let graph_engine: Arc<dyn covalence_core::graph::engine::GraphEngine> =
        Arc::new(PetgraphEngine::new(Arc::clone(&graph)));
    let admin_svc = Arc::new(
        AdminService::new(Arc::clone(&repo), graph_engine, Arc::clone(&graph))
            .with_embedder(embedder)
            .with_config(config.clone()),
    );

    // Build queue service.
    let queue_svc = RetryQueueService::new(Arc::clone(&repo), config.queue.clone());
    queue_svc.set_source_service(Arc::clone(&source_svc));
    queue_svc.set_admin_service(Arc::clone(&admin_svc));

    tracing::info!(
        concurrency = config.queue.reprocess_concurrency,
        job_timeout_secs = config.queue.job_timeout_secs,
        "worker ready, starting queue polling"
    );

    // Run the queue worker — this blocks forever.
    queue_svc.run_worker().await;

    Ok(())
}

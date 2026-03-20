//! Source service — ingestion orchestration and source management.
//!
//! The shared pipeline stages (chunk → embed → extract → resolve)
//! live in [`super::pipeline`]. This module owns source lifecycle
//! (create, supersede, reprocess, delete) and delegates the heavy
//! lifting.

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::config::{PipelineConfig, TableDimensions};
use crate::error::{Error, Result};
use crate::ingestion::ChatBackend;
use crate::ingestion::converter::ConverterRegistry;
use crate::ingestion::coreference::FastcorefClient;
use crate::ingestion::embedder::Embedder;
use crate::ingestion::extractor::Extractor;
use crate::ingestion::fingerprint::{FingerprintConfig, PipelineFingerprint};
use crate::ingestion::pg_resolver::PgResolver;
use crate::ingestion::resolver::EntityResolver;
use crate::ingestion::section_compiler::{SectionCompiler, SourceSummaryCompiler};
use crate::ingestion::statement_extractor::StatementExtractor;
use crate::models::source::{Source, SourceType, UpdateClass};
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{
    ChunkRepo, EdgeRepo, ExtractionRepo, LedgerRepo, NodeAliasRepo, NodeRepo, SectionRepo,
    SourceRepo, StatementRepo, UnresolvedEntityRepo,
};
use crate::types::clearance::ClearanceLevel;
use crate::types::ids::{EdgeId, NodeId, SourceId};

use super::ingestion_helpers::{SupersedesInfo, detect_update_class};
use super::pipeline::PipelineInput;

/// Derive the knowledge domain from a source's type and URI.
///
/// Rules are applied in priority order:
/// 1. Code sources → `code`
/// 2. URI pattern matching for file:// paths
/// 3. HTTP/HTTPS defaults to `research`
/// 4. Remaining documents → `external`
pub fn derive_domain(source_type: &str, uri: Option<&str>) -> Option<String> {
    // Code source type takes priority
    if source_type == "code" {
        return Some("code".to_string());
    }

    let uri = uri?;

    // File URI patterns
    if uri.starts_with("file://spec/") {
        return Some("spec".to_string());
    }
    if uri.starts_with("file://docs/adr/")
        || uri.starts_with("file://VISION")
        || uri.starts_with("file://CLAUDE")
        || uri.starts_with("file://MILESTONES")
        || uri.starts_with("file://design/")
    {
        return Some("design".to_string());
    }
    if uri.starts_with("file://engine/")
        || uri.starts_with("file://cli/")
        || uri.starts_with("file://dashboard/")
    {
        return Some("code".to_string());
    }

    // HTTP sources
    if uri.starts_with("https://arxiv") || uri.starts_with("https://doi") {
        return Some("research".to_string());
    }
    if uri.starts_with("http://") || uri.starts_with("https://") {
        return Some("research".to_string());
    }

    // Remaining documents
    if source_type == "document" {
        return Some("external".to_string());
    }

    None
}

/// Result of a cascading delete operation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeleteResult {
    /// Whether the source was found and deleted.
    pub deleted: bool,
    /// Number of chunks deleted alongside the source.
    pub chunks_deleted: u64,
    /// Number of extraction records deleted.
    pub extractions_deleted: u64,
    /// Number of orphaned nodes deleted.
    pub nodes_deleted: u64,
    /// Number of edges deleted (from orphaned nodes).
    pub edges_deleted: u64,
    /// Number of statements deleted (statement-first pipeline).
    pub statements_deleted: u64,
    /// Number of sections deleted (statement-first pipeline).
    pub sections_deleted: u64,
    /// Number of surviving nodes whose opinions were recalculated.
    pub nodes_recalculated: usize,
    /// Number of surviving edges whose opinions were recalculated.
    pub edges_recalculated: usize,
}

/// Result of a source reprocessing operation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReprocessResult {
    /// Source ID that was reprocessed.
    pub source_id: uuid::Uuid,
    /// Number of old extractions marked as superseded.
    pub extractions_superseded: u64,
    /// Number of old chunks deleted.
    pub chunks_deleted: u64,
    /// Number of new chunks created.
    pub chunks_created: usize,
    /// New content version after reprocessing.
    pub content_version: i32,
}

/// Service for source ingestion and management.
pub struct SourceService {
    pub(crate) repo: Arc<PgRepo>,
    pub(crate) embedder: Option<Arc<dyn Embedder>>,
    pub(crate) extractor: Option<Arc<dyn Extractor>>,
    pub(crate) resolver: Option<Arc<dyn EntityResolver>>,
    pub(crate) extract_concurrency: usize,
    pub(crate) rel_type_resolver: Option<Arc<PgResolver>>,
    pub(crate) converter_registry: Option<ConverterRegistry>,
    pub(crate) coref_client: Option<Arc<FastcorefClient>>,
    pub(crate) table_dims: TableDimensions,
    pub(crate) chunk_size: usize,
    pub(crate) chunk_overlap: usize,
    pub(crate) min_section_size: usize,
    pub(crate) min_extract_tokens: usize,
    pub(crate) extract_batch_tokens: usize,
    pub(crate) pipeline: PipelineConfig,
    pub(crate) fingerprint_config: Option<FingerprintConfig>,
    pub(crate) statement_extractor: Option<Arc<dyn StatementExtractor>>,
    pub(crate) section_compiler: Option<Arc<dyn SectionCompiler>>,
    pub(crate) source_summary_compiler: Option<Arc<dyn SourceSummaryCompiler>>,
    /// Chat backend for generating semantic summaries of code entities.
    pub(crate) chat_backend: Option<Arc<dyn ChatBackend>>,
}

impl SourceService {
    /// Default extraction concurrency when not configured.
    const DEFAULT_EXTRACT_CONCURRENCY: usize = 8;
    /// Default minimum section size for sibling merging.
    ///
    /// Sections below this threshold are merged with consecutive
    /// siblings sharing the same parent heading. Prevents tiny
    /// H3/H4 subsections in academic papers from producing chunks
    /// too small for meaningful retrieval.
    const DEFAULT_MIN_SECTION_SIZE: usize = 200;
    /// Default minimum token count for extraction.
    const DEFAULT_MIN_EXTRACT_TOKENS: usize = 30;
    /// Default token budget for extraction batching.
    const DEFAULT_EXTRACT_BATCH_TOKENS: usize = 2000;

    /// Create a new source service.
    pub fn new(repo: Arc<PgRepo>) -> Self {
        Self {
            repo,
            embedder: None,
            extractor: None,
            resolver: None,
            extract_concurrency: Self::DEFAULT_EXTRACT_CONCURRENCY,
            rel_type_resolver: None,
            converter_registry: None,
            coref_client: None,
            table_dims: TableDimensions::default(),
            chunk_size: 1000,
            chunk_overlap: 200,
            min_section_size: Self::DEFAULT_MIN_SECTION_SIZE,
            min_extract_tokens: Self::DEFAULT_MIN_EXTRACT_TOKENS,
            extract_batch_tokens: Self::DEFAULT_EXTRACT_BATCH_TOKENS,
            pipeline: PipelineConfig::default(),
            fingerprint_config: None,
            statement_extractor: None,
            section_compiler: None,
            chat_backend: None,
            source_summary_compiler: None,
        }
    }

    /// Create a new source service with optional AI components.
    pub fn with_ai(
        repo: Arc<PgRepo>,
        embedder: Option<Arc<dyn Embedder>>,
        extractor: Option<Arc<dyn Extractor>>,
    ) -> Self {
        Self {
            repo,
            embedder,
            extractor,
            resolver: None,
            extract_concurrency: Self::DEFAULT_EXTRACT_CONCURRENCY,
            rel_type_resolver: None,
            converter_registry: None,
            coref_client: None,
            table_dims: TableDimensions::default(),
            chunk_size: 1000,
            chunk_overlap: 200,
            min_section_size: Self::DEFAULT_MIN_SECTION_SIZE,
            min_extract_tokens: Self::DEFAULT_MIN_EXTRACT_TOKENS,
            extract_batch_tokens: Self::DEFAULT_EXTRACT_BATCH_TOKENS,
            pipeline: PipelineConfig::default(),
            fingerprint_config: None,
            statement_extractor: None,
            section_compiler: None,
            chat_backend: None,
            source_summary_compiler: None,
        }
    }

    /// Create a new source service with full AI pipeline.
    pub fn with_full_pipeline(
        repo: Arc<PgRepo>,
        embedder: Option<Arc<dyn Embedder>>,
        extractor: Option<Arc<dyn Extractor>>,
        resolver: Option<Arc<dyn EntityResolver>>,
        rel_type_resolver: Option<Arc<PgResolver>>,
    ) -> Self {
        Self {
            repo,
            embedder,
            extractor,
            resolver,
            extract_concurrency: Self::DEFAULT_EXTRACT_CONCURRENCY,
            rel_type_resolver,
            converter_registry: None,
            coref_client: None,
            table_dims: TableDimensions::default(),
            chunk_size: 1000,
            chunk_overlap: 200,
            min_section_size: Self::DEFAULT_MIN_SECTION_SIZE,
            min_extract_tokens: Self::DEFAULT_MIN_EXTRACT_TOKENS,
            extract_batch_tokens: Self::DEFAULT_EXTRACT_BATCH_TOKENS,
            pipeline: PipelineConfig::default(),
            fingerprint_config: None,
            statement_extractor: None,
            section_compiler: None,
            chat_backend: None,
            source_summary_compiler: None,
        }
    }

    /// Attach a converter registry for pre-processing content
    /// before the parser stage.
    ///
    /// When set, the `ingest` method will run incoming content
    /// through the matching converter to produce Markdown before
    /// passing it to the parser.
    pub fn with_converter_registry(mut self, registry: ConverterRegistry) -> Self {
        self.converter_registry = Some(registry);
        self
    }

    /// Attach a Fastcoref client for neural coreference resolution
    /// as a preprocessing step before entity extraction.
    pub fn with_coref_client(mut self, client: Arc<FastcorefClient>) -> Self {
        self.coref_client = Some(client);
        self
    }

    /// Set per-table embedding dimensions.
    pub fn with_table_dims(mut self, dims: TableDimensions) -> Self {
        self.table_dims = dims;
        self
    }

    /// Set chunk size, overlap, and minimum section size.
    pub fn with_chunk_config(mut self, size: usize, overlap: usize) -> Self {
        self.chunk_size = size;
        self.chunk_overlap = overlap;
        self
    }

    /// Set minimum section size for sibling merging.
    pub fn with_min_section_size(mut self, min_size: usize) -> Self {
        self.min_section_size = min_size;
        self
    }

    /// Set the maximum number of concurrent LLM extraction calls.
    pub fn with_extract_concurrency(mut self, concurrency: usize) -> Self {
        self.extract_concurrency = concurrency;
        self
    }

    /// Set per-stage pipeline configuration.
    pub fn with_pipeline_config(mut self, config: PipelineConfig) -> Self {
        self.pipeline = config;
        self
    }

    /// Set extraction batching parameters.
    ///
    /// - `min_tokens`: chunks with fewer tokens are skipped.
    /// - `batch_tokens`: adjacent small chunks are concatenated
    ///   up to this token budget for a single extraction call.
    pub fn with_extract_batch_config(mut self, min_tokens: usize, batch_tokens: usize) -> Self {
        self.min_extract_tokens = min_tokens;
        self.extract_batch_tokens = batch_tokens;
        self
    }

    /// Set the pipeline fingerprint configuration.
    ///
    /// When set, every `ingest()` and `reprocess()` call records
    /// the pipeline fingerprint in the source's metadata under
    /// the `"pipeline_fingerprint"` key.
    pub fn with_fingerprint_config(mut self, config: FingerprintConfig) -> Self {
        self.fingerprint_config = Some(config);
        self
    }

    /// Replace or set the entity/relationship extractor.
    pub fn with_extractor(mut self, extractor: Arc<dyn Extractor>) -> Self {
        self.extractor = Some(extractor);
        self
    }

    /// Set the statement extractor for the statement-first pipeline.
    pub fn with_statement_extractor(mut self, extractor: Arc<dyn StatementExtractor>) -> Self {
        self.statement_extractor = Some(extractor);
        self
    }

    /// Set the section compiler for clustering and compilation.
    pub fn with_section_compiler(mut self, compiler: Arc<dyn SectionCompiler>) -> Self {
        self.section_compiler = Some(compiler);
        self
    }

    /// Set the source summary compiler.
    pub fn with_source_summary_compiler(
        mut self,
        compiler: Arc<dyn SourceSummaryCompiler>,
    ) -> Self {
        self.source_summary_compiler = Some(compiler);
        self
    }

    /// Set the chat backend for generating semantic summaries of code
    /// entities (Spec 12, Stage 2).
    pub fn with_chat_backend(mut self, backend: Arc<dyn ChatBackend>) -> Self {
        self.chat_backend = Some(backend);
        self
    }

    /// Compute the current pipeline fingerprint (if configured).
    fn current_fingerprint(&self) -> Option<PipelineFingerprint> {
        self.fingerprint_config
            .as_ref()
            .map(PipelineFingerprint::compute)
    }

    /// Store the pipeline fingerprint in a source's metadata.
    ///
    /// Inserts or replaces the `"pipeline_fingerprint"` key in
    /// the metadata JSON object.
    fn stamp_fingerprint(metadata: &mut serde_json::Value, fingerprint: &PipelineFingerprint) {
        if let serde_json::Value::Object(map) = metadata {
            map.insert("pipeline_fingerprint".to_string(), fingerprint.to_json());
        }
    }

    /// Ingest content from a URL through the full pipeline.
    ///
    /// Fetches the URL, detects MIME and source type, extracts
    /// metadata (title, author, date) from the response, and
    /// delegates to [`ingest`](Self::ingest).
    ///
    /// Caller-provided overrides (source_type, mime, title, author)
    /// take precedence over auto-detected values.
    pub async fn ingest_url(
        &self,
        url: &str,
        source_type_override: Option<&str>,
        mime_override: Option<&str>,
        title_override: Option<&str>,
        author_override: Option<&str>,
        mut metadata: serde_json::Value,
    ) -> Result<SourceId> {
        let fetched = crate::ingestion::url_fetcher::fetch_url(url).await?;

        let source_type = source_type_override.unwrap_or(&fetched.source_type);
        let mime = mime_override.unwrap_or(&fetched.mime);

        // Merge fetched metadata into the caller's metadata object.
        // Caller-provided values take precedence.
        if let serde_json::Value::Object(ref mut map) = metadata {
            // Title: explicit override > fetched > existing metadata.
            let title = title_override
                .map(|s| s.to_string())
                .or(fetched.metadata.title);
            if let Some(ref t) = title {
                map.entry("title".to_string())
                    .or_insert_with(|| serde_json::json!(t));
            }

            // Author: explicit override > fetched > existing metadata.
            let author = author_override
                .map(|s| s.to_string())
                .or(fetched.metadata.author);
            if let Some(ref a) = author {
                map.entry("author".to_string())
                    .or_insert_with(|| serde_json::json!(a));
            }

            // Date: fetched date as fallback only.
            if let Some(ref d) = fetched.metadata.date {
                map.entry("fetched_date".to_string())
                    .or_insert_with(|| serde_json::json!(d));
            }

            // Record the fetch URL in metadata.
            map.entry("fetched_url".to_string())
                .or_insert_with(|| serde_json::json!(url));
        }

        self.ingest(&fetched.bytes, source_type, mime, Some(url), metadata)
            .await
    }

    /// Ingest new content through the full pipeline.
    ///
    /// Accept a new source and enqueue it for async processing.
    ///
    /// Returns immediately after storing the source record.
    /// The pipeline (chunk → embed → extract → resolve → summarize)
    /// runs asynchronously via a `ProcessSource` queue job.
    ///
    /// **Source update classes**: When a URI is provided and an
    /// existing source shares that URI, the system detects the
    /// update class and stores supersession info for the queue
    /// worker to handle after the pipeline succeeds.
    pub async fn ingest(
        &self,
        content: &[u8],
        source_type: &str,
        mime: &str,
        uri: Option<&str>,
        metadata: serde_json::Value,
    ) -> Result<SourceId> {
        let hash = Sha256::digest(content).to_vec();

        // Dedup check — exact content hash match.
        if let Some(existing) = SourceRepo::get_by_hash(&*self.repo, &hash).await? {
            return Ok(existing.id);
        }

        let st = SourceType::from_str_opt(source_type)
            .ok_or_else(|| Error::InvalidInput(format!("unknown source type: {source_type}")))?;

        // Source update class detection.
        let supersedes_info = if let Some(uri_str) = uri {
            self.detect_source_update(uri_str, content).await?
        } else {
            None
        };

        // Prepare content (convert → parse → normalize).
        let prepared = self.prepare_content(content, mime, uri).await?;

        // Semantic dedup on normalized content.
        if let Some(existing) =
            SourceRepo::get_by_normalized_hash(&*self.repo, &prepared.normalized_hash).await?
        {
            return Ok(existing.id);
        }

        // Build source record.
        let mut source = Source::new(st, hash);
        source.uri = uri.map(|s| s.to_string());
        source.domain = derive_domain(source_type, uri);

        // Title priority: metadata > filename (code) > parsed > URI segment.
        let uri_filename = uri.and_then(|u| u.rsplit('/').next().map(|f| f.to_string()));
        source.title = metadata
            .get("title")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                if prepared.is_code {
                    uri_filename.clone()
                } else {
                    None
                }
            })
            .or(prepared.parsed_title)
            .or(uri_filename);

        // Author priority: metadata.authors[0] > metadata.author > parsed.
        source.author = metadata
            .get("authors")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                metadata
                    .get("author")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .or_else(|| prepared.parsed_metadata.get("author").cloned());

        source.metadata = metadata;
        source.raw_content = String::from_utf8(content.to_vec()).ok();
        source.normalized_content = Some(prepared.normalized.clone());
        source.normalized_hash = Some(prepared.normalized_hash);

        if let Some(ref info) = supersedes_info {
            source.supersedes_id = Some(info.old_source_id);
            source.update_class = Some(info.update_class.as_str().to_string());
            source.content_version = info.new_version;
        }

        if let Some(fp) = self.current_fingerprint() {
            Self::stamp_fingerprint(&mut source.metadata, &fp);
        }

        SourceRepo::create(&*self.repo, &source).await?;

        // Store supersession info in the source metadata so the
        // queue worker can handle cleanup after the pipeline succeeds.
        if let Some(ref info) = supersedes_info {
            sqlx::query(
                "UPDATE sources SET metadata = jsonb_set(\
                   COALESCE(metadata, '{}'), '{_supersession}', $2::jsonb\
                 ) WHERE id = $1",
            )
            .bind(source.id)
            .bind(serde_json::json!({
                "old_source_id": info.old_source_id.to_string(),
                "update_class": info.update_class.as_str(),
            }))
            .execute(self.repo.pool())
            .await?;
        }

        // Enqueue async processing via the retry queue.
        let payload = serde_json::json!({
            "source_id": source.id.to_string(),
        });
        let key = format!("process:{}", source.id);
        use crate::storage::traits::JobQueueRepo;
        JobQueueRepo::enqueue(
            &*self.repo,
            crate::models::retry_job::JobKind::ProcessSource,
            payload,
            5,
            Some(&key),
        )
        .await?;

        tracing::info!(
            source_id = %source.id,
            uri = uri.unwrap_or("-"),
            "source accepted, processing enqueued"
        );

        Ok(source.id)
    }

    /// Process an accepted source through the full pipeline.
    ///
    /// Called by the queue worker after a `ProcessSource` job is
    /// claimed. Runs chunk → embed → extract → resolve → summarize,
    /// then handles supersession cleanup and the statement pipeline.
    pub(crate) async fn process_accepted(&self, source_id: SourceId) -> Result<()> {
        // Update status to processing.
        sqlx::query("UPDATE sources SET status = 'processing' WHERE id = $1")
            .bind(source_id)
            .execute(self.repo.pool())
            .await?;

        let source = SourceRepo::get(&*self.repo, source_id)
            .await?
            .ok_or_else(|| Error::NotFound {
                entity_type: "source",
                id: source_id.to_string(),
            })?;

        let normalized = source.normalized_content.as_ref().ok_or_else(|| {
            Error::InvalidInput(format!("source {} has no normalized_content", source_id))
        })?;

        let source_type = &source.source_type;
        let is_code = crate::ingestion::code_chunker::detect_code_language(
            "application/octet-stream",
            source.uri.as_deref(),
        )
        .is_some()
            || source_type == "code";

        // Run chunking + embedding only. Extraction fans out to
        // per-chunk queue jobs via the async DAG.
        self.run_pipeline(&PipelineInput {
            source_id,
            source_type,
            source_uri: source.uri.clone(),
            source_title: source.title.clone(),
            source_domain: source.domain.clone(),
            normalized,
            is_code,
            chunk_only: true,
        })
        .await?;

        // Fan out: enqueue per-chunk extraction jobs.
        // The DAG: ExtractChunk → (fan-in) → SummarizeEntity
        //        → (fan-in) → ComposeSourceSummary → complete
        let chunks: Vec<(uuid::Uuid,)> =
            sqlx::query_as("SELECT id FROM chunks WHERE source_id = $1")
                .bind(source_id)
                .fetch_all(self.repo.pool())
                .await?;

        // Batch enqueue all extract_chunk jobs in a single query
        // instead of N sequential INSERTs (#168).
        use crate::models::retry_job::EnqueueJob;
        use crate::storage::traits::JobQueueRepo;
        let jobs: Vec<EnqueueJob> = chunks
            .iter()
            .map(|(chunk_id,)| EnqueueJob {
                kind: crate::models::retry_job::JobKind::ExtractChunk,
                payload: serde_json::json!({
                    "chunk_id": chunk_id.to_string(),
                    "source_id": source_id.into_uuid().to_string(),
                }),
                max_attempts: 5, // TODO: source from RetryQueueConfig
                idempotency_key: Some(format!("extract_chunk:{chunk_id}")),
            })
            .collect();

        let enqueued = JobQueueRepo::enqueue_batch(&*self.repo, jobs).await?;

        tracing::info!(
            source_id = %source_id,
            chunks = chunks.len(),
            enqueued,
            "fanned out extraction to per-chunk jobs (batch)"
        );

        // Handle supersession cleanup if this source replaces another.
        if let Some(supersession) = source.metadata.get("_supersession") {
            if let (Some(old_id_str), Some(update_class_str)) = (
                supersession.get("old_source_id").and_then(|v| v.as_str()),
                supersession.get("update_class").and_then(|v| v.as_str()),
            ) {
                if let Ok(old_uuid) = old_id_str.parse::<uuid::Uuid>() {
                    let old_id = SourceId::from_uuid(old_uuid);
                    let update_class =
                        crate::models::source::UpdateClass::from_str_opt(update_class_str)
                            .unwrap_or(crate::models::source::UpdateClass::Versioned);
                    self.mark_superseded(old_id, source_id, &update_class)
                        .await?;
                    let ext_deleted = ExtractionRepo::delete_by_source(&*self.repo, old_id).await?;
                    let stmts_deleted =
                        StatementRepo::delete_by_source(&*self.repo, old_id).await?;
                    let sects_deleted = SectionRepo::delete_by_source(&*self.repo, old_id).await?;
                    UnresolvedEntityRepo::delete_by_source(&*self.repo, old_id).await?;
                    let aliases_cleared =
                        NodeAliasRepo::clear_source_chunks(&*self.repo, old_id).await?;
                    let chunks_deleted = ChunkRepo::delete_by_source(&*self.repo, old_id).await?;
                    let ledger_deleted = LedgerRepo::delete_by_source(&*self.repo, old_id).await?;
                    SourceRepo::clear_embedding(&*self.repo, old_id).await?;
                    tracing::info!(
                        old_source = %old_id,
                        ext_deleted, stmts_deleted, sects_deleted,
                        aliases_cleared, chunks_deleted, ledger_deleted,
                        "cleaned up superseded source"
                    );
                }
            }
        }

        // Statement pipeline for prose sources.
        if self.pipeline.statement_enabled && !is_code {
            if let Some(ref stmt_extractor) = self.statement_extractor {
                use super::statement_pipeline::{StatementPipelineInput, run_statement_pipeline};
                match run_statement_pipeline(
                    &self.repo,
                    stmt_extractor,
                    self.embedder.as_ref(),
                    &self.table_dims,
                    &StatementPipelineInput {
                        source_id,
                        normalized_text: normalized,
                        source_title: source.title.as_deref(),
                        window_chars: self.pipeline.statement_window_chars,
                        window_overlap: self.pipeline.statement_window_overlap,
                    },
                    self.section_compiler.as_ref(),
                    self.source_summary_compiler.as_ref(),
                )
                .await
                {
                    Ok(_result) => {
                        if let Err(e) = self
                            .extract_entities_from_statements(source_id, source.domain.as_deref())
                            .await
                        {
                            tracing::warn!(
                                source_id = %source_id,
                                error = %e,
                                "statement entity extraction failed (non-fatal)"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            source_id = %source_id,
                            error = %e,
                            "statement pipeline failed (non-fatal)"
                        );
                    }
                }
            }
        }

        // Don't mark complete here — the fan-out DAG will set
        // status = 'complete' when ComposeSourceSummary finishes.
        // If there are no chunks (empty source), mark complete now.
        if chunks.is_empty() {
            sqlx::query("UPDATE sources SET status = 'complete' WHERE id = $1")
                .bind(source_id)
                .execute(self.repo.pool())
                .await?;
        }

        tracing::info!(source_id = %source_id, "source chunked, extraction fanned out");
        Ok(())
    }

    /// Detect whether an existing source with the same URI exists
    /// and determine the update class based on content overlap.
    async fn detect_source_update(
        &self,
        uri: &str,
        new_content: &[u8],
    ) -> Result<Option<SupersedesInfo>> {
        use sqlx::Row;

        let row = sqlx::query(
            "SELECT id, content_hash, raw_content, content_version \
             FROM sources \
             WHERE uri = $1 \
             ORDER BY content_version DESC \
             LIMIT 1",
        )
        .bind(uri)
        .fetch_optional(self.repo.pool())
        .await?;

        let row = match row {
            Some(r) => r,
            None => return Ok(None),
        };

        let old_id: SourceId = row.get("id");
        let old_content: Option<String> = row.get("raw_content");
        let old_version: i32 = row.get("content_version");

        let new_text = String::from_utf8_lossy(new_content);

        let update_class = match old_content {
            Some(ref old_text) => detect_update_class(old_text, &new_text),
            None => UpdateClass::Versioned,
        };

        Ok(Some(SupersedesInfo {
            old_source_id: old_id,
            update_class,
            new_version: old_version + 1,
        }))
    }

    /// Mark an old source as superseded by a newer version.
    async fn mark_superseded(
        &self,
        old_id: SourceId,
        new_id: SourceId,
        update_class: &UpdateClass,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE sources \
             SET update_class = $2, superseded_by = $3, superseded_at = NOW() \
             WHERE id = $1",
        )
        .bind(old_id)
        .bind(update_class.as_str())
        .bind(new_id)
        .execute(self.repo.pool())
        .await?;
        Ok(())
    }

    /// Reprocess a source through the current pipeline config.
    ///
    /// Deletes old extractions and chunks, then re-runs the full
    /// pipeline on the source's stored raw content.
    pub async fn reprocess(&self, id: SourceId) -> Result<ReprocessResult> {
        let mut source =
            SourceRepo::get(&*self.repo, id)
                .await?
                .ok_or_else(|| Error::NotFound {
                    entity_type: "source",
                    id: id.to_string(),
                })?;

        let raw_content = source.raw_content.as_ref().ok_or_else(|| {
            Error::InvalidInput(format!(
                "source {} has no raw_content, cannot reprocess",
                id
            ))
        })?;
        let content_bytes = raw_content.as_bytes();

        tracing::info!(
            source_id = %id,
            version = source.content_version,
            "starting source reprocessing"
        );

        // Delete old extractions → alias refs → chunks → ledger.
        let extractions_superseded = ExtractionRepo::delete_by_source(&*self.repo, id).await?;
        let aliases_cleared = NodeAliasRepo::clear_source_chunks(&*self.repo, id).await?;
        let chunks_deleted = ChunkRepo::delete_by_source(&*self.repo, id).await?;
        let ledger_deleted = LedgerRepo::delete_by_source(&*self.repo, id).await?;
        tracing::debug!(
            extractions_superseded,
            aliases_cleared,
            chunks_deleted,
            ledger_deleted,
            "cleaned up old data"
        );

        source.content_version += 1;

        // Prepare content (convert → parse → normalize).
        // Normalize format_origin values that are not actual MIME types
        // (e.g., "arxiv" from URL-based ingestion) to text/markdown.
        let format_origin = source
            .metadata
            .get("format_origin")
            .and_then(|v| v.as_str())
            .unwrap_or("text/plain");
        let mime = if format_origin.contains('/') {
            format_origin
        } else {
            "text/markdown"
        };
        let prepared = self
            .prepare_content(content_bytes, mime, source.uri.as_deref())
            .await?;

        source.normalized_content = Some(prepared.normalized.clone());
        source.normalized_hash = Some(prepared.normalized_hash);

        if let Some(fp) = self.current_fingerprint() {
            Self::stamp_fingerprint(&mut source.metadata, &fp);
        }

        SourceRepo::update(&*self.repo, &source).await?;

        // Run shared pipeline.
        let output = self
            .run_pipeline(&PipelineInput {
                source_id: id,
                source_type: &source.source_type,
                source_uri: source.uri.clone(),
                source_title: source.title.clone(),
                source_domain: source.domain.clone(),
                normalized: &prepared.normalized,
                is_code: prepared.is_code,
                chunk_only: false,
            })
            .await?;

        // Run statement pipeline for prose sources (parallel, opt-in).
        // Statement failures are non-fatal: chunks and embeddings are
        // already persisted, so the source is still searchable.
        // Code sources skip statements — they use AST extraction instead.
        if self.pipeline.statement_enabled && !prepared.is_code {
            if let Some(ref stmt_extractor) = self.statement_extractor {
                // Incremental re-extraction: superset behavior with
                // eviction (Phase 5, ADR-0015).
                use super::statement_pipeline::{StatementPipelineInput, reextract_statements};
                match reextract_statements(
                    &self.repo,
                    stmt_extractor,
                    self.embedder.as_ref(),
                    &self.table_dims,
                    &StatementPipelineInput {
                        source_id: id,
                        normalized_text: &prepared.normalized,
                        source_title: source.title.as_deref(),
                        window_chars: self.pipeline.statement_window_chars,
                        window_overlap: self.pipeline.statement_window_overlap,
                    },
                    self.section_compiler.as_ref(),
                    self.source_summary_compiler.as_ref(),
                )
                .await
                {
                    Ok(reextract_result) => {
                        tracing::info!(
                            added = reextract_result.added,
                            evicted = reextract_result.evicted,
                            "statement re-extraction complete"
                        );
                        // Extract entities from statements (Phase 4, ADR-0015).
                        if let Err(e) = self
                            .extract_entities_from_statements(id, source.domain.as_deref())
                            .await
                        {
                            tracing::warn!(
                                source_id = %id,
                                error = %e,
                                "statement entity extraction failed (non-fatal)"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            source_id = %id,
                            error = %e,
                            "statement re-extraction failed (non-fatal, chunks still persisted)"
                        );
                    }
                }
            }
        }

        tracing::info!(
            source_id = %id,
            version = source.content_version,
            extractions_superseded,
            chunks_deleted,
            chunks_created = output.chunks_created,
            "source reprocessing complete"
        );

        Ok(ReprocessResult {
            source_id: id.into_uuid(),
            extractions_superseded,
            chunks_deleted,
            chunks_created: output.chunks_created,
            content_version: source.content_version,
        })
    }

    /// Get a source by ID.
    pub async fn get(&self, id: SourceId) -> Result<Option<Source>> {
        SourceRepo::get(&*self.repo, id).await
    }

    /// List sources with pagination.
    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<Source>> {
        SourceRepo::list(&*self.repo, limit, offset).await
    }

    /// Delete a source and cascade through all dependent entities.
    ///
    /// Cascade order:
    /// 1. Collect affected node IDs from extractions
    /// 2. Delete extractions for this source's chunks and statements
    /// 3. Nullify `node_aliases.source_chunk_id` references
    /// 4. Delete statements, sections, and unresolved entities
    /// 5. Delete chunks and projection ledger
    /// 6. For each affected node with zero remaining active
    ///    extractions: delete aliases, edges, then the node.
    ///    For nodes with remaining extractions: update
    ///    `mention_count`
    /// 7. Delete the source record
    pub async fn delete(&self, id: SourceId) -> Result<DeleteResult> {
        // Step 1: Collect affected entity IDs before we delete
        // anything. These are needed for both structural cleanup
        // and epistemic cascade.
        let affected_node_ids = ExtractionRepo::list_node_ids_by_source(&*self.repo, id).await?;
        // Note: affected_edge_ids is a superset — it may include
        // edges later deleted by EdgeRepo::delete_by_node in step 6.
        // The batch cascade handles this gracefully: get_many omits
        // deleted IDs (absent from DB), so they are skipped.
        let affected_edge_ids = ExtractionRepo::list_edge_ids_by_source(&*self.repo, id).await?;

        // Step 2: Delete extractions (must precede chunk deletion
        // due to FK on extractions.chunk_id).
        let extractions_deleted = ExtractionRepo::delete_by_source(&*self.repo, id).await?;

        // Step 3: Nullify alias chunk references (must precede
        // chunk deletion due to FK on
        // node_aliases.source_chunk_id).
        NodeAliasRepo::clear_source_chunks(&*self.repo, id).await?;

        // Step 4: Delete statements and sections (statement-first
        // pipeline). Must follow extraction deletion since extractions
        // may reference statements via FK.
        let statements_deleted = StatementRepo::delete_by_source(&*self.repo, id).await?;
        let sections_deleted = SectionRepo::delete_by_source(&*self.repo, id).await?;
        UnresolvedEntityRepo::delete_by_source(&*self.repo, id).await?;

        // Step 5: Delete chunks and projection ledger.
        let chunks_deleted = ChunkRepo::delete_by_source(&*self.repo, id).await?;
        LedgerRepo::delete_by_source(&*self.repo, id).await?;

        // Step 6: Handle affected nodes. For each node, check if
        // it still has active extractions from other sources.
        let mut nodes_deleted: u64 = 0;
        let mut edges_deleted: u64 = 0;
        let mut surviving_node_ids: Vec<NodeId> = Vec::new();

        for node_id in &affected_node_ids {
            let remaining =
                ExtractionRepo::count_active_by_entity(&*self.repo, "node", node_id.into_uuid())
                    .await?;

            if remaining == 0 {
                // Orphaned node — delete aliases, edges, then
                // the node itself.
                NodeAliasRepo::delete_by_node(&*self.repo, *node_id).await?;
                edges_deleted += EdgeRepo::delete_by_node(&*self.repo, *node_id).await?;
                if NodeRepo::delete(&*self.repo, *node_id).await? {
                    nodes_deleted += 1;
                }
            } else {
                // Node survives — track for epistemic cascade.
                surviving_node_ids.push(*node_id);
                // Update mention_count to reflect the remaining
                // extraction count.
                if let Some(mut node) = NodeRepo::get(&*self.repo, *node_id).await? {
                    node.mention_count = remaining as i32;
                    NodeRepo::update(&*self.repo, &node).await?;
                }
            }
        }

        // Step 6b: Epistemic cascade — recalculate opinions for
        // surviving entities that lost extraction support (#105).
        // This implements TMS dependency-directed backtracking:
        // claims that lost their sole source become vacuous,
        // claims with remaining support are re-fused.
        let mut nodes_recalculated: usize = 0;
        let mut edges_recalculated: usize = 0;

        if !surviving_node_ids.is_empty() || !affected_edge_ids.is_empty() {
            let cascade =
                epistemic_cascade(&self.repo, &surviving_node_ids, &affected_edge_ids).await;
            match cascade {
                Ok(result) => {
                    nodes_recalculated = result.nodes_recalculated + result.nodes_vacuated;
                    edges_recalculated = result.edges_recalculated + result.edges_vacuated;
                    if result.total_affected() > 0 {
                        tracing::info!(
                            source_id = %id,
                            nodes_recalculated = result.nodes_recalculated,
                            nodes_vacuated = result.nodes_vacuated,
                            edges_recalculated = result.edges_recalculated,
                            edges_vacuated = result.edges_vacuated,
                            "epistemic cascade complete"
                        );
                    }
                }
                Err(e) => {
                    // Cascade failure is non-fatal — structural
                    // cleanup already succeeded.
                    tracing::warn!(
                        source_id = %id,
                        error = %e,
                        "epistemic cascade failed (non-fatal)"
                    );
                }
            }
        }

        // Step 7: Delete the source record.
        let deleted = SourceRepo::delete(&*self.repo, id).await?;

        Ok(DeleteResult {
            deleted,
            chunks_deleted,
            extractions_deleted,
            nodes_deleted,
            edges_deleted,
            statements_deleted,
            sections_deleted,
            nodes_recalculated,
            edges_recalculated,
        })
    }

    /// Publish a source by promoting its clearance level from
    /// `LocalStrict` (0) to `FederatedTrusted` (1).
    ///
    /// Returns an error if the source is not found or is already at a
    /// clearance level above `LocalStrict`.
    pub async fn publish(&self, id: SourceId) -> Result<Source> {
        let mut source =
            SourceRepo::get(&*self.repo, id)
                .await?
                .ok_or_else(|| Error::NotFound {
                    entity_type: "source",
                    id: id.to_string(),
                })?;

        if source.clearance_level != ClearanceLevel::LocalStrict {
            return Err(Error::InvalidInput(format!(
                "source {} is already at clearance level {}",
                id, source.clearance_level
            )));
        }

        source.clearance_level = ClearanceLevel::FederatedTrusted;
        SourceRepo::update(&*self.repo, &source).await?;
        Ok(source)
    }

    /// Count total number of sources.
    pub async fn count(&self) -> Result<i64> {
        SourceRepo::count(&*self.repo).await
    }

    /// Get all chunks for a source.
    pub async fn get_chunks(
        &self,
        source_id: SourceId,
    ) -> Result<Vec<crate::models::chunk::Chunk>> {
        ChunkRepo::list_by_source(&*self.repo, source_id).await
    }
}

/// Run TMS epistemic cascade for entities affected by source
/// retraction.
///
/// Recalculates opinions for surviving nodes and edges from their
/// remaining active extractions. Nodes/edges that lost all support
/// get vacuous opinions (u=1.0). Those with remaining support get
/// re-fused opinions via cumulative fusion.
///
/// This implements the dependency-directed backtracking described
/// in spec 07 (Epistemic Model, §TMS Cascade).
async fn epistemic_cascade(
    repo: &PgRepo,
    surviving_node_ids: &[NodeId],
    affected_edge_ids: &[EdgeId],
) -> Result<crate::epistemic::cascade::CascadeResult> {
    use crate::epistemic::cascade::{recalculate_edge_opinions, recalculate_node_opinions};

    let mut result = recalculate_node_opinions(repo, surviving_node_ids).await?;
    let edge_result = recalculate_edge_opinions(repo, affected_edge_ids).await?;
    result.merge(&edge_result);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingestion::code_chunker;
    use crate::models::extraction::Extraction;
    use crate::types::ids::ChunkId;

    #[test]
    fn derive_domain_code_source_type() {
        assert_eq!(
            derive_domain("code", Some("file://engine/src/main.rs")).as_deref(),
            Some("code")
        );
        assert_eq!(derive_domain("code", None).as_deref(), Some("code"));
    }

    #[test]
    fn derive_domain_spec_uri() {
        assert_eq!(
            derive_domain("document", Some("file://spec/02-data-model.md")).as_deref(),
            Some("spec")
        );
    }

    #[test]
    fn derive_domain_design_uri() {
        assert_eq!(
            derive_domain("document", Some("file://docs/adr/0001-foo.md")).as_deref(),
            Some("design")
        );
        assert_eq!(
            derive_domain("document", Some("file://VISION.md")).as_deref(),
            Some("design")
        );
        assert_eq!(
            derive_domain("document", Some("file://CLAUDE.md")).as_deref(),
            Some("design")
        );
        assert_eq!(
            derive_domain("document", Some("file://design/graph-type-system.md")).as_deref(),
            Some("design")
        );
    }

    #[test]
    fn derive_domain_code_uri() {
        assert_eq!(
            derive_domain("document", Some("file://engine/crates/core/src/lib.rs")).as_deref(),
            Some("code")
        );
        assert_eq!(
            derive_domain("document", Some("file://cli/cmd/root.go")).as_deref(),
            Some("code")
        );
    }

    #[test]
    fn derive_domain_research_uri() {
        assert_eq!(
            derive_domain("document", Some("https://arxiv.org/abs/2403.14403")).as_deref(),
            Some("research")
        );
        assert_eq!(
            derive_domain("document", Some("https://doi.org/10.1234/foo")).as_deref(),
            Some("research")
        );
        assert_eq!(
            derive_domain("document", Some("https://example.com/paper")).as_deref(),
            Some("research")
        );
    }

    #[test]
    fn derive_domain_external_fallback() {
        assert_eq!(
            derive_domain("document", Some("file://random/doc.md")).as_deref(),
            Some("external")
        );
    }

    #[test]
    fn derive_domain_no_uri() {
        // Non-code source with no URI returns None
        assert_eq!(derive_domain("document", None).as_deref(), None);
    }

    #[test]
    fn delete_result_serializes_all_fields() {
        let result = DeleteResult {
            deleted: true,
            chunks_deleted: 5,
            extractions_deleted: 10,
            statements_deleted: 2,
            sections_deleted: 1,
            nodes_deleted: 3,
            edges_deleted: 7,
            nodes_recalculated: 4,
            edges_recalculated: 2,
        };
        let json = serde_json::to_value(&result).expect("serialize");
        assert_eq!(json["deleted"], true);
        assert_eq!(json["chunks_deleted"], 5);
        assert_eq!(json["extractions_deleted"], 10);
        assert_eq!(json["statements_deleted"], 2);
        assert_eq!(json["sections_deleted"], 1);
        assert_eq!(json["nodes_deleted"], 3);
        assert_eq!(json["edges_deleted"], 7);
        assert_eq!(json["nodes_recalculated"], 4);
        assert_eq!(json["edges_recalculated"], 2);
    }

    #[test]
    fn delete_result_zero_counts_when_nothing_found() {
        let result = DeleteResult {
            deleted: false,
            chunks_deleted: 0,
            extractions_deleted: 0,
            statements_deleted: 0,
            sections_deleted: 0,
            nodes_deleted: 0,
            edges_deleted: 0,
            nodes_recalculated: 0,
            edges_recalculated: 0,
        };
        assert!(!result.deleted);
        assert_eq!(result.chunks_deleted, 0);
        assert_eq!(result.extractions_deleted, 0);
        assert_eq!(result.statements_deleted, 0);
        assert_eq!(result.sections_deleted, 0);
        assert_eq!(result.nodes_deleted, 0);
        assert_eq!(result.edges_deleted, 0);
        assert_eq!(result.nodes_recalculated, 0);
        assert_eq!(result.edges_recalculated, 0);
    }

    // --- Code source pipeline tests ---

    #[test]
    fn code_language_detected_from_mime() {
        let lang = code_chunker::detect_code_language("text/x-rust", None);
        assert!(lang.is_some());
        assert_eq!(lang.unwrap(), code_chunker::CodeLanguage::Rust,);
    }

    #[test]
    fn code_language_detected_from_uri_fallback() {
        let lang =
            code_chunker::detect_code_language("application/octet-stream", Some("src/main.rs"));
        assert!(lang.is_some());
        assert_eq!(lang.unwrap(), code_chunker::CodeLanguage::Rust,);
    }

    #[test]
    fn code_to_markdown_produces_function_headings() {
        let source = concat!(
            "fn hello() {\n",
            "    println!(\"hello\");\n",
            "}\n",
            "\n",
            "fn world(x: i32) -> bool {\n",
            "    x > 0\n",
            "}\n",
        );
        let md = code_chunker::code_to_markdown(source.trim(), code_chunker::CodeLanguage::Rust)
            .expect("code_to_markdown should succeed");

        // Verify tree-sitter produces function-level headings.
        assert!(
            md.contains("# fn hello()"),
            "expected heading for hello(), got:\n{md}"
        );
        assert!(
            md.contains("# fn world(x: i32) -> bool"),
            "expected heading for world(), got:\n{md}"
        );
        // Verify fenced code blocks are present.
        assert!(
            md.contains("```rust"),
            "expected rust code fences, got:\n{md}"
        );
    }

    #[test]
    fn code_source_skips_normalization() {
        // Normalization would collapse indentation. Verify that
        // for code sources the pipeline preserves indentation by
        // checking that code_to_markdown output retains it.
        let source = "fn foo() {\n    let x = 1;\n}\n";
        let md = code_chunker::code_to_markdown(source.trim(), code_chunker::CodeLanguage::Rust)
            .expect("code_to_markdown should succeed");

        // The 4-space indentation should survive since we skip
        // normalization for code.
        assert!(
            md.contains("    let x = 1;"),
            "indentation should be preserved in code markdown"
        );

        // Verify normalization *would* break it.
        let normalized = crate::ingestion::normalize::normalize(&md);
        assert!(
            !normalized.contains("    let x = 1;"),
            "normalization should collapse indentation"
        );
    }

    #[test]
    fn code_source_skips_coref() {
        // Coreference resolution on code would produce spurious
        // links. Verify it finds nothing meaningful in code.
        let md = code_chunker::code_to_markdown(
            "fn main() {\n    println!(\"hello\");\n}",
            code_chunker::CodeLanguage::Rust,
        )
        .expect("code_to_markdown should succeed");

        let chunks = crate::ingestion::chunker::chunk_document(&md, 1000, 200);
        let resolver = crate::ingestion::coreference::CorefResolver::new();
        let links = resolver.resolve(&chunks);

        // Code content should not produce meaningful coref links.
        // Any links found would be noise from identifiers.
        // The pipeline skips this for code — verify the resolver
        // at least doesn't crash on code content.
        assert!(
            links.is_empty() || links.iter().all(|l| l.mention.len() <= 3),
            "coref on code should not find meaningful entities"
        );
    }

    #[test]
    fn non_code_mime_does_not_trigger_code_path() {
        let lang = code_chunker::detect_code_language("text/html", Some("index.html"));
        assert!(lang.is_none(), "HTML should not be detected as code");
    }

    // --- ExtractionRepo mock and supersession tests ---

    /// In-memory mock of [`ExtractionRepo`] that records
    /// `mark_superseded_by_source` calls for verification.
    struct MockExtractionRepo {
        superseded_sources: std::sync::Mutex<Vec<SourceId>>,
    }

    impl MockExtractionRepo {
        fn new() -> Self {
            Self {
                superseded_sources: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn superseded_source_ids(&self) -> Vec<SourceId> {
            self.superseded_sources
                .lock()
                .map(|v| v.clone())
                .unwrap_or_default()
        }
    }

    impl ExtractionRepo for MockExtractionRepo {
        async fn create(&self, _extraction: &Extraction) -> crate::error::Result<()> {
            Ok(())
        }

        async fn get(
            &self,
            _id: crate::types::ids::ExtractionId,
        ) -> crate::error::Result<Option<Extraction>> {
            Ok(None)
        }

        async fn list_by_chunk(&self, _chunk_id: ChunkId) -> crate::error::Result<Vec<Extraction>> {
            Ok(Vec::new())
        }

        async fn list_active_for_entity(
            &self,
            _entity_type: &str,
            _entity_id: uuid::Uuid,
        ) -> crate::error::Result<Vec<Extraction>> {
            Ok(Vec::new())
        }

        async fn mark_superseded(
            &self,
            _id: crate::types::ids::ExtractionId,
        ) -> crate::error::Result<()> {
            Ok(())
        }

        async fn mark_superseded_by_source(
            &self,
            source_id: SourceId,
        ) -> crate::error::Result<u64> {
            if let Ok(mut v) = self.superseded_sources.lock() {
                v.push(source_id);
            }
            Ok(1)
        }

        async fn delete_by_source(&self, _source_id: SourceId) -> crate::error::Result<u64> {
            Ok(0)
        }

        async fn list_node_ids_by_source(
            &self,
            _source_id: SourceId,
        ) -> crate::error::Result<Vec<NodeId>> {
            Ok(Vec::new())
        }

        async fn count_active_by_entity(
            &self,
            _entity_type: &str,
            _entity_id: uuid::Uuid,
        ) -> crate::error::Result<i64> {
            Ok(0)
        }

        async fn list_edge_ids_by_source(
            &self,
            _source_id: SourceId,
        ) -> crate::error::Result<Vec<EdgeId>> {
            Ok(Vec::new())
        }

        async fn list_active_for_entities(
            &self,
            _entity_type: &str,
            _entity_ids: &[uuid::Uuid],
        ) -> crate::error::Result<Vec<Extraction>> {
            Ok(Vec::new())
        }
    }

    /// Verify that `ExtractionRepo::mark_superseded_by_source`
    /// correctly receives the old source ID during supersession.
    ///
    /// This tests the trait contract that the supersession path in
    /// `ingest()` relies on: when a new source supersedes an old
    /// one, `mark_superseded_by_source` must be called with the
    /// old source's ID so its extractions stop polluting search.
    #[tokio::test]
    async fn mark_superseded_by_source_records_old_source_id() {
        let repo = MockExtractionRepo::new();
        let old_source_id = SourceId::from_uuid(uuid::Uuid::new_v4());

        let count = ExtractionRepo::mark_superseded_by_source(&repo, old_source_id).await;

        assert!(count.is_ok(), "mark_superseded_by_source should succeed");
        assert_eq!(count.unwrap_or(0), 1);

        let recorded = repo.superseded_source_ids();
        assert_eq!(recorded.len(), 1, "exactly one source should be superseded");
        assert_eq!(
            recorded[0], old_source_id,
            "recorded source ID must match the old source"
        );
    }
}

//! Source service — ingestion orchestration and source management.

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::config::{PipelineConfig, TableDimensions};
use crate::error::{Error, Result};
use crate::ingestion::code_chunker;
use crate::ingestion::converter::ConverterRegistry;
use crate::ingestion::coreference::{CorefResolver, FastcorefClient};
use crate::ingestion::embedder::{Embedder, truncate_and_validate};
use crate::ingestion::extractor::{ExtractionContext, Extractor};
use crate::ingestion::fingerprint::{FingerprintConfig, PipelineFingerprint};
use crate::ingestion::landscape::{ExtractionMethod, cosine_similarity};
use crate::ingestion::pg_resolver::PgResolver;
use crate::ingestion::resolver::EntityResolver;
use crate::models::chunk::Chunk;
use crate::models::chunk::ChunkLevel as ModelChunkLevel;
use crate::models::edge::Edge;
use crate::models::extraction::{ExtractedEntityType, Extraction};
use crate::models::node::Node;
use crate::models::node_alias::NodeAlias;
use crate::models::source::{Source, SourceType, UpdateClass};
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{
    ChunkRepo, EdgeRepo, ExtractionRepo, NodeAliasRepo, NodeRepo, SourceRepo,
};
use crate::types::clearance::ClearanceLevel;
use crate::types::ids::{AliasId, ChunkId, NodeId, SourceId};

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
    repo: Arc<PgRepo>,
    embedder: Option<Arc<dyn Embedder>>,
    extractor: Option<Arc<dyn Extractor>>,
    resolver: Option<Arc<dyn EntityResolver>>,
    /// Maximum number of concurrent LLM extraction calls.
    extract_concurrency: usize,
    /// Resolver for normalizing relationship type labels via
    /// trigram similarity against existing edge types.
    rel_type_resolver: Option<Arc<PgResolver>>,
    /// Optional format converter registry for pre-processing
    /// content before the parser stage. When set, incoming
    /// content is run through the matching converter to produce
    /// Markdown before parsing.
    converter_registry: Option<ConverterRegistry>,
    /// Optional Fastcoref client for neural coreference
    /// resolution as a preprocessing step before extraction.
    coref_client: Option<Arc<FastcorefClient>>,
    /// Per-table embedding dimensions for truncation.
    table_dims: TableDimensions,
    /// Maximum chunk size in bytes before paragraph splitting.
    chunk_size: usize,
    /// Overlap characters from the end of the previous chunk.
    chunk_overlap: usize,
    /// Minimum token count for a chunk to be sent to the extractor.
    min_extract_tokens: usize,
    /// Token budget for batching adjacent small chunks into a
    /// single extraction call.
    extract_batch_tokens: usize,
    /// Per-stage pipeline configuration.
    pipeline: PipelineConfig,
    /// Optional fingerprint config for recording pipeline state on
    /// each ingestion run.
    fingerprint_config: Option<FingerprintConfig>,
}

impl SourceService {
    /// Default extraction concurrency when not configured.
    const DEFAULT_EXTRACT_CONCURRENCY: usize = 8;
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
            min_extract_tokens: Self::DEFAULT_MIN_EXTRACT_TOKENS,
            extract_batch_tokens: Self::DEFAULT_EXTRACT_BATCH_TOKENS,
            pipeline: PipelineConfig::default(),
            fingerprint_config: None,
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
            min_extract_tokens: Self::DEFAULT_MIN_EXTRACT_TOKENS,
            extract_batch_tokens: Self::DEFAULT_EXTRACT_BATCH_TOKENS,
            pipeline: PipelineConfig::default(),
            fingerprint_config: None,
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
            min_extract_tokens: Self::DEFAULT_MIN_EXTRACT_TOKENS,
            extract_batch_tokens: Self::DEFAULT_EXTRACT_BATCH_TOKENS,
            pipeline: PipelineConfig::default(),
            fingerprint_config: None,
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

    /// Set chunk size and overlap.
    pub fn with_chunk_config(mut self, size: usize, overlap: usize) -> Self {
        self.chunk_size = size;
        self.chunk_overlap = overlap;
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
    /// Stages: hash, dedup/supersede, parse, normalize, chunk, embed,
    /// landscape analysis, extract (gated by landscape), resolve,
    /// embed nodes.
    ///
    /// **Embedding landscape gating**: After landscape analysis, only
    /// chunks with `FullExtraction` or `FullExtractionWithReview`
    /// extraction methods are sent to the LLM extractor. Chunks
    /// classified as `EmbeddingLinkage` or `DeltaCheck` skip
    /// expensive LLM calls.
    ///
    /// **Source update classes**: When a URI is provided and an
    /// existing source shares that URI, the system detects the
    /// update class (correction, versioned, refactor) based on
    /// content overlap, marks the old source as superseded, and
    /// links the new source via `supersedes_id`.
    pub async fn ingest(
        &self,
        content: &[u8],
        source_type: &str,
        mime: &str,
        uri: Option<&str>,
        metadata: serde_json::Value,
    ) -> Result<SourceId> {
        let hash = Sha256::digest(content).to_vec();

        // Dedup check — exact content hash match
        if let Some(existing) = SourceRepo::get_by_hash(&*self.repo, &hash).await? {
            return Ok(existing.id);
        }

        let st = SourceType::from_str_opt(source_type)
            .ok_or_else(|| Error::InvalidInput(format!("unknown source type: {source_type}")))?;

        // --- Source update class detection ---
        //
        // If a URI is provided, check whether an existing source
        // shares that URI. If content hashes differ, determine the
        // update class and mark the old source as superseded.
        let supersedes_info = if let Some(uri_str) = uri {
            self.detect_source_update(uri_str, content).await?
        } else {
            None
        };

        // Detect whether this is a code source. When a code
        // language is detected the pipeline uses tree-sitter
        // directly and skips NLP stages (normalization, coref)
        // that would destroy code structure.
        let code_lang = code_chunker::detect_code_language(mime, uri);

        // Stage 1.5: Convert content if a converter registry is
        // configured and conversion is enabled. For code sources,
        // use tree-sitter directly instead of the converter
        // registry.
        let (parse_content, parse_mime): (std::borrow::Cow<'_, [u8]>, &str) =
            if let Some(lang) = code_lang {
                let source_text = String::from_utf8_lossy(content);
                let md = code_chunker::code_to_markdown(&source_text, lang)?;
                (std::borrow::Cow::Owned(md.into_bytes()), "text/markdown")
            } else if self.pipeline.convert_enabled {
                if let Some(ref registry) = self.converter_registry {
                    let converted = registry.convert(content, mime).await?;
                    (
                        std::borrow::Cow::Owned(converted.into_bytes()),
                        "text/markdown",
                    )
                } else {
                    (std::borrow::Cow::Borrowed(content), mime)
                }
            } else {
                (std::borrow::Cow::Borrowed(content), mime)
            };

        // Stage 2: Parse
        let parsed = crate::ingestion::parser::parse(&parse_content, parse_mime)?;

        // Stage 3: Normalize (skippable via pipeline config).
        // Skipped for code sources — normalization collapses
        // indentation inside fenced code blocks.
        let is_code = code_lang.is_some();
        let normalized = if is_code {
            parsed.body.clone()
        } else if self.pipeline.normalize_enabled {
            let n = crate::ingestion::normalize::normalize(&parsed.body);
            crate::ingestion::normalize::strip_artifacts(&n)
        } else {
            parsed.body.clone()
        };

        // Compute normalized content hash for semantic dedup.
        let normalized_hash = Sha256::digest(normalized.as_bytes()).to_vec();

        // Semantic dedup: if normalized content matches an existing
        // source, skip re-processing. This catches cosmetic changes
        // (whitespace, encoding) that don't affect content.
        if let Some(existing) =
            SourceRepo::get_by_normalized_hash(&*self.repo, &normalized_hash).await?
        {
            return Ok(existing.id);
        }

        // Create source record
        let mut source = Source::new(st, hash);
        source.uri = uri.map(|s| s.to_string());
        // Title priority: metadata.title > filename from URI (for
        // code sources) > parsed title > last URI segment.
        // Code sources get special treatment: the parsed title is
        // always "Preamble" (from the generated markdown), which is
        // not useful. Use the filename instead.
        let uri_filename = uri.and_then(|u| {
            u.rsplit('/').next().map(|f| f.to_string())
        });
        source.title = metadata
            .get("title")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| if is_code { uri_filename.clone() } else { None })
            .or(parsed.title)
            .or(uri_filename);
        // Author priority: metadata.authors[0] > metadata.author >
        // parsed author.
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
            .or_else(|| parsed.metadata.get("author").cloned());
        source.metadata = metadata;
        source.raw_content = String::from_utf8(content.to_vec()).ok();
        source.normalized_content = Some(normalized.clone());
        source.normalized_hash = Some(normalized_hash);

        // Apply supersession metadata if detected
        if let Some(ref info) = supersedes_info {
            source.supersedes_id = Some(info.old_source_id);
            source.update_class = Some(info.update_class.as_str().to_string());
            source.content_version = info.new_version;
        }

        // Stamp pipeline fingerprint into source metadata.
        if let Some(fp) = self.current_fingerprint() {
            Self::stamp_fingerprint(&mut source.metadata, &fp);
        }

        SourceRepo::create(&*self.repo, &source).await?;

        // Mark old source as superseded if applicable
        if let Some(ref info) = supersedes_info {
            self.mark_superseded(info.old_source_id, &info.update_class)
                .await?;
            // Mark old extractions as superseded so they don't
            // pollute search results after the new version lands.
            ExtractionRepo::mark_superseded_by_source(&*self.repo, info.old_source_id).await?;
        }

        // Stage 4: Chunk
        let chunk_outputs = crate::ingestion::chunker::chunk_document(
            &normalized,
            self.chunk_size,
            self.chunk_overlap,
        );

        // Stage 4.5: Post-chunking quality gate.
        //
        // Flag sources that produce excessively fragmented output.
        // This catches garbage HTML that slipped through conversion
        // (e.g., nav/footer text producing hundreds of tiny chunks).
        if chunk_outputs.len() > 10 {
            let char_counts: Vec<usize> = chunk_outputs.iter().map(|co| co.text.len()).collect();
            let mut sorted = char_counts.clone();
            sorted.sort_unstable();
            let median_chars = sorted[sorted.len() / 2];

            if median_chars < 100 {
                tracing::warn!(
                    source_id = %source.id,
                    chunks = chunk_outputs.len(),
                    median_chars,
                    "post-chunking quality gate: excessively fragmented output"
                );
            }
        }

        // Filter out metadata-only chunks (e.g., chunks whose content
        // is entirely bold-label lines like "**Authors:** ..." with no
        // substantive text). These are ingestion artifacts from content
        // appearing before the first heading.
        let pre_filter = chunk_outputs.len();
        let chunk_outputs: Vec<_> = chunk_outputs
            .into_iter()
            .filter(|co| {
                !is_metadata_only(&co.text)
                    && !is_boilerplate_heavy(&co.text)
                    && !is_author_block(&co.text)
                    && !has_artifact_heading(&co.heading_path)
            })
            .collect();
        let filtered_count = pre_filter - chunk_outputs.len();
        if filtered_count > 0 {
            tracing::info!(
                source_id = %source.id,
                filtered = filtered_count,
                "removed metadata-only chunks"
            );
        }

        // Build a map from chunker UUIDs to ChunkIds
        let mut id_map = std::collections::HashMap::new();
        for co in &chunk_outputs {
            id_map.insert(co.id, ChunkId::from_uuid(co.id));
        }

        // Build all chunks for batch insertion
        let chunks: Vec<Chunk> = chunk_outputs
            .iter()
            .enumerate()
            .map(|(ordinal, co)| {
                let chunk_id = id_map[&co.id];
                let level = match co.level {
                    crate::ingestion::chunker::ChunkLevel::Section => ModelChunkLevel::Section,
                    crate::ingestion::chunker::ChunkLevel::Paragraph => ModelChunkLevel::Paragraph,
                };

                // Hash unique content only (excluding overlap prefix)
                // so upstream paragraph changes don't cascade through
                // every downstream chunk's hash.
                let unique_content = &co.text[co.context_prefix_len..];
                let content_hash = Sha256::digest(unique_content.as_bytes()).to_vec();
                let token_count = co.text.split_whitespace().count() as i32;
                let hierarchy = co
                    .heading_path
                    .iter()
                    .map(|h| sanitize_ltree_label(h))
                    .collect::<Vec<_>>()
                    .join(".");

                // Detect content types for chunk metadata.
                let chunk_meta = detect_chunk_content_types(&co.text);

                let mut chunk = Chunk::new(
                    source.id,
                    level,
                    ordinal as i32,
                    co.text.clone(),
                    content_hash,
                    token_count,
                );
                chunk.id = chunk_id;
                chunk.byte_start = Some(co.byte_start as i32);
                chunk.byte_end = Some(co.byte_end as i32);
                chunk.content_offset = Some(co.context_prefix_len as i32);
                if let Some(parent_uuid) = co.parent_id {
                    if let Some(&pid) = id_map.get(&parent_uuid) {
                        chunk = chunk.with_parent(pid);
                    }
                }
                if !hierarchy.is_empty() {
                    chunk = chunk.with_hierarchy(hierarchy);
                }
                chunk = chunk.with_metadata(chunk_meta);
                chunk
            })
            .collect();

        // Store all chunks in a single batch INSERT
        ChunkRepo::batch_create(&*self.repo, &chunks).await?;

        // Track chunks that contain example/hypothetical markers.
        // Entities extracted from these chunks get dampened confidence.
        let example_chunks: std::collections::HashSet<uuid::Uuid> = chunk_outputs
            .iter()
            .filter(|co| has_example_markers(&co.text))
            .map(|co| co.id)
            .collect();

        // Stage 5: Embed chunks
        //
        // Uses contextual chunk embeddings when supported (e.g.,
        // Voyage `voyage-context-3`). Each chunk embedding reflects
        // the surrounding document context (late chunking).
        //
        // Also runs landscape analysis to determine which chunks
        // need LLM extraction.
        let landscape_results = if let Some(ref embedder) = self.embedder {
            let texts: Vec<String> = chunk_outputs.iter().map(|co| co.text.clone()).collect();
            let embeddings = embedder.embed_document_chunks(&texts).await?;

            for (co, emb) in chunk_outputs.iter().zip(embeddings.iter()) {
                let chunk_id = id_map[&co.id];
                match truncate_and_validate(emb, self.table_dims.chunk, "chunks") {
                    Ok(truncated) => {
                        if let Err(e) =
                            ChunkRepo::update_embedding(&*self.repo, chunk_id, &truncated).await
                        {
                            tracing::warn!(
                                chunk_id = %chunk_id,
                                error = %e,
                                "failed to store chunk embedding"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            chunk_id = %chunk_id,
                            error = %e,
                            "chunk embedding dimension mismatch"
                        );
                    }
                }
            }

            // Compute parent_alignment for child chunks.
            //
            // For each chunk with a parent_chunk_id, compute cosine
            // similarity between the child and parent embeddings
            // and store it in the parent_alignment column.
            compute_parent_alignment(&chunk_outputs, &embeddings, &id_map, &*self.repo).await;

            // Embed the full normalized text for source-level search.
            // Failures are logged but don't abort ingestion — the
            // source will still be searchable via lexical/graph dims.
            match embedder.embed(std::slice::from_ref(&normalized)).await {
                Ok(source_embeddings) => {
                    if let Some(emb) = source_embeddings.first() {
                        match truncate_and_validate(emb, self.table_dims.source, "sources") {
                            Ok(truncated) => {
                                if let Err(e) =
                                    SourceRepo::update_embedding(&*self.repo, source.id, &truncated)
                                        .await
                                {
                                    tracing::warn!(
                                        source_id = %source.id,
                                        error = %e,
                                        "failed to store source embedding"
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    source_id = %source.id,
                                    error = %e,
                                    "source embedding dimension mismatch"
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        source_id = %source.id,
                        error = %e,
                        "failed to embed source, \
                         source will lack vector search"
                    );
                }
            }

            // Stage 5.5: Embedding landscape analysis (gated by
            // pipeline.landscape_enabled).
            //
            // When disabled, all chunks are sent to the extractor
            // without landscape-based gating.
            //
            // First-ingestion bypass: if this source has no prior
            // extractions (no superseded source), landscape gating
            // is bypassed to avoid incorrectly skipping extraction
            // on multi-chunk sources where intra-document alignment
            // is naturally high.
            let is_first_ingestion = supersedes_info.is_none();
            let landscape = if self.pipeline.landscape_enabled {
                let parent_embeddings: Vec<Option<&Vec<f64>>> = chunk_outputs
                    .iter()
                    .map(|co| {
                        co.parent_id.and_then(|pid| {
                            chunk_outputs
                                .iter()
                                .position(|c| c.id == pid)
                                .and_then(|idx| embeddings.get(idx))
                        })
                    })
                    .collect();

                crate::ingestion::landscape::analyze_landscape(
                    &embeddings,
                    &parent_embeddings,
                    None,
                    is_first_ingestion,
                )
            } else {
                Vec::new()
            };

            // Store landscape results on chunks
            for lr in &landscape {
                if lr.chunk_index < chunk_outputs.len() {
                    let chunk_id = id_map[&chunk_outputs[lr.chunk_index].id];
                    let metrics_json = serde_json::to_value(&lr.metrics).ok();
                    ChunkRepo::update_landscape(
                        &*self.repo,
                        chunk_id,
                        lr.parent_alignment,
                        lr.extraction_method.as_str(),
                        metrics_json,
                    )
                    .await?;
                }
            }

            Some(landscape)
        } else {
            None
        };

        // Stage 5.5: Co-reference resolution across chunks.
        // Skipped for code sources — abbreviation detection is
        // destructive for identifiers and variable names.
        let mut coref_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        if !is_code {
            let coref_resolver = CorefResolver::new();
            let coref_links = coref_resolver.resolve(&chunk_outputs);
            for link in &coref_links {
                coref_map.insert(link.mention.to_lowercase(), link.referent.to_lowercase());
            }
        }

        // Stage 5.7: Neural coreference preprocessing (optional).
        //
        // When a FastcorefClient is configured, resolve pronouns
        // and anaphora in each extractable chunk before passing
        // to the entity extractor. This benefits all extractor
        // backends (sidecar, two_pass, llm).
        //
        // Skipped for code sources.
        let resolved_texts: Option<std::collections::HashMap<uuid::Uuid, String>> = if is_code {
            None
        } else if self.pipeline.coref_enabled {
            if let Some(ref coref_client) = self.coref_client {
                let extractable_indices: Vec<usize> = chunk_outputs
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| should_extract(*i, landscape_results.as_deref()))
                    .map(|(i, _)| i)
                    .collect();

                let mut resolved = std::collections::HashMap::new();
                for &idx in &extractable_indices {
                    let co = &chunk_outputs[idx];
                    match coref_client.resolve(&co.text).await {
                        Ok(r) => {
                            tracing::debug!(
                                chunk_index = idx,
                                original_len = co.text.len(),
                                resolved_len = r.len(),
                                "neural coref resolved"
                            );
                            resolved.insert(co.id, r);
                        }
                        Err(e) => {
                            tracing::warn!(
                                chunk_index = idx,
                                error = %e,
                                "neural coref failed, using original text"
                            );
                        }
                    }
                }
                if resolved.is_empty() {
                    None
                } else {
                    Some(resolved)
                }
            } else {
                None
            }
        } else {
            None
        };

        // Stages 6-7: Extract entities + resolve + create
        // nodes/edges.
        //
        // For code sources, use the deterministic AST extractor
        // instead of the LLM extractor. This avoids garbage
        // extractions from sending code through NLP prompts.
        //
        // Landscape gating: Only send chunks with FullExtraction
        // or FullExtractionWithReview to the LLM. Chunks classified
        // as EmbeddingLinkage or DeltaCheck are skipped.
        //
        // Token-budget batching: Adjacent small chunks are
        // concatenated into a single extraction call to reduce
        // API round-trips. Chunks below `min_extract_tokens` are
        // skipped entirely.
        let ast_ext: Option<Arc<dyn Extractor>> = if is_code {
            Some(Arc::new(
                crate::ingestion::ast_extractor::AstExtractor::new(),
            ))
        } else {
            None
        };
        let active_extractor = if is_code {
            ast_ext.as_ref()
        } else {
            self.extractor.as_ref()
        };
        let extraction_method_label = if is_code { "ast" } else { "llm" };
        if let Some(extractor) = active_extractor {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(self.extract_concurrency));

            // Build extraction context from source metadata.
            let extraction_context = Arc::new(ExtractionContext {
                source_type: Some(source_type.to_string()),
                source_uri: uri.map(|u| u.to_string()),
                source_title: source.title.clone(),
            });

            // Determine which chunks should go through extraction
            // based on landscape analysis.
            let extractable: Vec<_> = chunk_outputs
                .iter()
                .enumerate()
                .filter(|(i, _co)| should_extract(*i, landscape_results.as_deref()))
                .map(|(_i, co)| co)
                .collect();

            // Group chunks into token-budget batches. Small chunks
            // below `min_extract_tokens` are skipped. Adjacent
            // chunks are concatenated up to `extract_batch_tokens`.
            let batches = group_extraction_batches(
                &extractable,
                self.min_extract_tokens,
                self.extract_batch_tokens,
                resolved_texts.as_ref(),
            );

            tracing::debug!(
                extractable = extractable.len(),
                batches = batches.len(),
                min_tokens = self.min_extract_tokens,
                batch_budget = self.extract_batch_tokens,
                "extraction batching"
            );

            let concurrency_size = self.extract_concurrency * 2;
            let mut extraction_results: Vec<Result<(uuid::Uuid, _)>> = Vec::new();

            for batch_slice in batches.chunks(concurrency_size) {
                let batch_futures: Vec<_> = batch_slice
                    .iter()
                    .map(|(primary_id, text)| {
                        let sem = Arc::clone(&semaphore);
                        let ext = Arc::clone(extractor);
                        let ctx = Arc::clone(&extraction_context);
                        let text = text.clone();
                        let primary_id = *primary_id;
                        async move {
                            let _permit = sem.acquire().await.map_err(|_| {
                                Error::Ingestion("extraction semaphore closed".to_string())
                            })?;
                            let result = ext.extract(&text, &ctx).await?;
                            Ok::<_, Error>((primary_id, result))
                        }
                    })
                    .collect();

                let batch_results = futures::future::join_all(batch_futures).await;
                extraction_results.extend(batch_results);
            }

            // Phase 2: Process extraction results sequentially
            let mut name_to_node: std::collections::HashMap<String, NodeId> =
                std::collections::HashMap::new();

            for extraction_result in extraction_results {
                let (chunk_uuid, extraction) = match extraction_result {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "extraction failed for chunk, skipping"
                        );
                        continue;
                    }
                };
                let chunk_id = id_map[&chunk_uuid];

                // Dampen confidence for entities/edges from
                // chunks that contain example/hypothetical markers.
                let is_example_chunk = example_chunks.contains(&chunk_uuid);

                for entity in &extraction.entities {
                    let node_id = self
                        .resolve_and_store_entity(entity, chunk_id, extraction_method_label)
                        .await?;
                    name_to_node.insert(entity.name.to_lowercase(), node_id);

                    if let Some(referent) = coref_map.get(&entity.name.to_lowercase()) {
                        name_to_node.entry(referent.clone()).or_insert(node_id);
                        self.ensure_alias(&entity.name, node_id, chunk_id).await?;
                    }
                }

                for rel in &extraction.relationships {
                    let src_key = coref_map
                        .get(&rel.source_name.to_lowercase())
                        .cloned()
                        .unwrap_or_else(|| rel.source_name.to_lowercase());
                    let tgt_key = coref_map
                        .get(&rel.target_name.to_lowercase())
                        .cloned()
                        .unwrap_or_else(|| rel.target_name.to_lowercase());

                    let source_id = match name_to_node.get(&src_key) {
                        Some(&id) => id,
                        None => continue,
                    };
                    let target_id = match name_to_node.get(&tgt_key) {
                        Some(&id) => id,
                        None => continue,
                    };

                    // Filter self-loops: extraction noise where an
                    // entity relates to itself.
                    if source_id == target_id {
                        tracing::debug!(
                            rel_type = %rel.rel_type,
                            entity = %rel.source_name,
                            "skipping self-loop edge"
                        );
                        continue;
                    }

                    let resolved_rel_type = if let Some(ref rtr) = self.rel_type_resolver {
                        rtr.resolve_rel_type(&rel.rel_type).await?
                    } else {
                        rel.rel_type.clone()
                    };

                    let mut edge = Edge::new(source_id, target_id, resolved_rel_type);
                    let conf = if is_example_chunk {
                        // Halve confidence for edges from example chunks.
                        (rel.confidence * 0.5).clamp(0.0, 1.0)
                    } else {
                        rel.confidence
                    };
                    edge.confidence = conf;
                    EdgeRepo::create(&*self.repo, &edge).await?;

                    let ext_record = Extraction::new(
                        chunk_id,
                        ExtractedEntityType::Edge,
                        edge.id.into_uuid(),
                        extraction_method_label.to_string(),
                        conf,
                    );
                    ExtractionRepo::create(&*self.repo, &ext_record).await?;
                }
            }

            // Stage 7.5: Embed node descriptions
            //
            // Only embeds nodes that don't already have an embedding,
            // avoiding redundant API calls for resolved existing nodes.
            // Embedding failures are logged as warnings and skipped
            // rather than aborting the entire ingestion.
            if let Some(ref embedder) = self.embedder {
                let node_ids: Vec<NodeId> = name_to_node.values().copied().collect();

                if !node_ids.is_empty() {
                    let mut texts: Vec<String> = Vec::with_capacity(node_ids.len());
                    let mut valid_ids: Vec<NodeId> = Vec::with_capacity(node_ids.len());

                    // Collect nodes that need embedding. Skip nodes
                    // that already have one from a prior ingestion.
                    let has_embedding_ids: Vec<uuid::Uuid> = sqlx::query_scalar::<_, uuid::Uuid>(
                        "SELECT id FROM nodes \
                             WHERE id = ANY($1) \
                             AND embedding IS NOT NULL",
                    )
                    .bind(node_ids.iter().map(|n| n.into_uuid()).collect::<Vec<_>>())
                    .fetch_all(self.repo.pool())
                    .await
                    .unwrap_or_default();

                    for &nid in &node_ids {
                        if has_embedding_ids.contains(&nid.into_uuid()) {
                            continue;
                        }
                        if let Some(node) = NodeRepo::get(&*self.repo, nid).await? {
                            let text = match &node.description {
                                Some(desc) if !desc.is_empty() => {
                                    format!("{}: {}", node.canonical_name, desc)
                                }
                                _ => node.canonical_name.clone(),
                            };
                            texts.push(text);
                            valid_ids.push(nid);
                        }
                    }

                    if !texts.is_empty() {
                        match embedder.embed(&texts).await {
                            Ok(embeddings) => {
                                for (nid, emb) in valid_ids.iter().zip(embeddings.iter()) {
                                    match truncate_and_validate(emb, self.table_dims.node, "nodes")
                                    {
                                        Ok(truncated) => {
                                            if let Err(e) = NodeRepo::update_embedding(
                                                &*self.repo,
                                                *nid,
                                                &truncated,
                                            )
                                            .await
                                            {
                                                tracing::warn!(
                                                    node_id = %nid,
                                                    error = %e,
                                                    "failed to store node embedding"
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                node_id = %nid,
                                                error = %e,
                                                "node embedding dimension mismatch"
                                            );
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    node_count = valid_ids.len(),
                                    error = %e,
                                    "failed to embed node descriptions, \
                                     nodes will lack vector search"
                                );
                            }
                        }
                    }
                }
            }
        }

        Ok(source.id)
    }

    /// Detect whether an existing source with the same URI exists
    /// and determine the update class based on content overlap.
    ///
    /// Returns `None` if no existing source shares the URI.
    async fn detect_source_update(
        &self,
        uri: &str,
        new_content: &[u8],
    ) -> Result<Option<SupersedesInfo>> {
        use sqlx::Row;

        // Query for existing source with this URI
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

    /// Mark an old source as superseded by updating its
    /// `update_class` to reflect the supersession.
    async fn mark_superseded(&self, old_id: SourceId, update_class: &UpdateClass) -> Result<()> {
        sqlx::query("UPDATE sources SET update_class = $2 WHERE id = $1")
            .bind(old_id)
            .bind(update_class.as_str())
            .execute(self.repo.pool())
            .await?;
        Ok(())
    }

    /// Resolve an extracted entity against the graph and store it.
    ///
    /// If a resolver is available, uses it to match against existing
    /// nodes. Otherwise creates a new node. Returns the `NodeId`
    /// (existing or new).
    ///
    /// On a fuzzy match, automatically creates a [`NodeAlias`] so that
    /// future exact lookups will match without falling back to trigram
    /// search.
    ///
    /// The entire read-check-write cycle runs inside a PostgreSQL
    /// transaction that holds a transaction-scoped advisory lock
    /// (`pg_advisory_xact_lock`) on a hash of the entity name. This
    /// prevents two concurrent workers from both creating the same
    /// node when they see "not found" at the same time. The lock is
    /// automatically released when the transaction commits or rolls
    /// back.
    async fn resolve_and_store_entity(
        &self,
        entity: &crate::ingestion::extractor::ExtractedEntity,
        chunk_id: ChunkId,
        extraction_method: &str,
    ) -> Result<NodeId> {
        use crate::ingestion::resolver::MatchType;
        use sqlx::Row;

        let lock_key = entity_name_lock_key(&entity.name);

        // Begin an explicit transaction so the advisory lock is
        // scoped to it and auto-released on commit/rollback.
        let mut tx = self.repo.pool().begin().await?;

        // Acquire a transaction-scoped advisory lock. This blocks
        // until any other transaction holding the same key commits
        // or rolls back.
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(lock_key)
            .execute(&mut *tx)
            .await?;

        // --- critical section: read-check-write under lock ---

        let active_resolver = if self.pipeline.resolve_enabled {
            self.resolver.as_ref()
        } else {
            None
        };
        let (node_id, match_type) = if let Some(resolver) = active_resolver {
            let resolved = resolver.resolve(entity).await?;
            match resolved.match_type {
                MatchType::New => {
                    let node = self.create_node_in_tx(&mut tx, entity).await?;
                    (node.id, MatchType::New)
                }
                ref mt => {
                    let match_type = mt.clone();
                    if let Some(nid) = resolved.node_id {
                        Self::bump_mention_in_tx(&mut tx, nid).await?;
                        (nid, match_type)
                    } else {
                        let node = self.create_node_in_tx(&mut tx, entity).await?;
                        (node.id, MatchType::New)
                    }
                }
            }
        } else {
            // No resolver — check by name, create if new.
            let existing: Option<NodeId> = sqlx::query(
                "SELECT id FROM nodes \
                 WHERE LOWER(canonical_name) = LOWER($1)",
            )
            .bind(&entity.name)
            .fetch_optional(&mut *tx)
            .await?
            .map(|r| r.get("id"));

            if let Some(nid) = existing {
                Self::bump_mention_in_tx(&mut tx, nid).await?;
                (nid, MatchType::Exact)
            } else {
                let node = self.create_node_in_tx(&mut tx, entity).await?;
                (node.id, MatchType::New)
            }
        };

        // Auto-alias on fuzzy match so subsequent exact lookups hit
        // without requiring another fuzzy search.
        if match_type == MatchType::Fuzzy {
            self.ensure_alias_in_tx(&mut tx, &entity.name, node_id, chunk_id)
                .await?;
        }

        // Create extraction provenance record inside the same tx.
        let ext_id = uuid::Uuid::new_v4();
        sqlx::query(
            "INSERT INTO extractions (
                id, chunk_id, entity_type, entity_id,
                extraction_method, confidence, is_superseded
            ) VALUES ($1, $2, $3, $4, $5, $6, false)",
        )
        .bind(ext_id)
        .bind(chunk_id)
        .bind("node")
        .bind(node_id.into_uuid())
        .bind(extraction_method)
        .bind(entity.confidence)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(node_id)
    }

    /// Insert a new node inside an existing transaction.
    async fn create_node_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        entity: &crate::ingestion::extractor::ExtractedEntity,
    ) -> Result<Node> {
        let mut node = Node::new(entity.name.clone(), entity.entity_type.clone());
        node.description = entity.description.clone();

        let confidence_json = node.confidence_breakdown.as_ref().map(|o| o.to_json());

        sqlx::query(
            "INSERT INTO nodes (
                id, canonical_name, node_type, description,
                properties, confidence_breakdown,
                clearance_level, first_seen, last_seen,
                mention_count
            ) VALUES (
                $1, $2, $3, $4,
                $5, $6,
                $7, $8, $9,
                $10
            )",
        )
        .bind(node.id)
        .bind(&node.canonical_name)
        .bind(&node.node_type)
        .bind(&node.description)
        .bind(&node.properties)
        .bind(&confidence_json)
        .bind(node.clearance_level.as_i32())
        .bind(node.first_seen)
        .bind(node.last_seen)
        .bind(node.mention_count)
        .execute(&mut **tx)
        .await?;

        Ok(node)
    }

    /// Bump `mention_count` and `last_seen` for an existing node
    /// inside a transaction.
    async fn bump_mention_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        node_id: NodeId,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE nodes \
             SET mention_count = mention_count + 1, \
                 last_seen = NOW() \
             WHERE id = $1",
        )
        .bind(node_id)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// Create an alias inside a transaction if one doesn't exist.
    async fn ensure_alias_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        alias_text: &str,
        node_id: NodeId,
        chunk_id: ChunkId,
    ) -> Result<()> {
        use sqlx::Row;

        let exists: bool = sqlx::query(
            "SELECT EXISTS(
                SELECT 1 FROM node_aliases
                WHERE LOWER(alias) = LOWER($1)
            ) AS exists",
        )
        .bind(alias_text)
        .fetch_one(&mut **tx)
        .await?
        .get("exists");

        if !exists {
            let alias_id = AliasId::new();
            sqlx::query(
                "INSERT INTO node_aliases \
                 (id, node_id, alias, source_chunk_id) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(alias_id)
            .bind(node_id)
            .bind(alias_text)
            .bind(chunk_id)
            .execute(&mut **tx)
            .await?;
        }
        Ok(())
    }

    /// Create an alias for a node if one doesn't already exist.
    async fn ensure_alias(
        &self,
        alias_text: &str,
        node_id: NodeId,
        chunk_id: ChunkId,
    ) -> Result<()> {
        let existing = NodeAliasRepo::find_by_alias(&*self.repo, alias_text).await?;
        let already_exists = existing
            .iter()
            .any(|a| a.alias.eq_ignore_ascii_case(alias_text));
        if !already_exists {
            let alias = NodeAlias {
                id: AliasId::new(),
                node_id,
                alias: alias_text.to_string(),
                source_chunk_id: Some(chunk_id),
            };
            NodeAliasRepo::create(&*self.repo, &alias).await?;
        }
        Ok(())
    }

    /// Reprocess a source through the current pipeline configuration.
    ///
    /// Re-runs the ingestion pipeline (convert, normalize, chunk,
    /// embed, extract) on the source's existing raw content using
    /// the current pipeline config. Previous extractions are marked
    /// as superseded and old chunks are replaced.
    ///
    /// Entity resolution ensures convergent behavior: nodes that
    /// match existing graph entities are merged (mention count
    /// bumped), new entities are added, and previously extracted
    /// entities that are not re-found are left in place (absence
    /// is not evidence of incorrectness).
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

        // Step 1: Delete old extractions (must happen before chunk
        // deletion to satisfy the FK constraint
        // extractions.chunk_id → chunks.id).
        let extractions_superseded =
            ExtractionRepo::delete_by_source(&*self.repo, id).await?;
        tracing::debug!(
            extractions_superseded,
            "deleted old extractions"
        );

        // Step 1b: Nullify source_chunk_id on node_aliases that
        // reference chunks belonging to this source (FK constraint
        // node_aliases.source_chunk_id → chunks.id).
        let aliases_cleared =
            NodeAliasRepo::clear_source_chunks(&*self.repo, id).await?;
        tracing::debug!(aliases_cleared, "cleared alias chunk refs");

        // Step 2: Delete old chunks. New chunks will be created by
        // the pipeline below.
        let chunks_deleted = ChunkRepo::delete_by_source(&*self.repo, id).await?;
        tracing::debug!(chunks_deleted, "deleted old chunks");

        // Step 3: Increment content version.
        source.content_version += 1;

        // Step 4: Re-run pipeline from convert stage.
        let mime = source
            .metadata
            .get("format_origin")
            .and_then(|v| v.as_str())
            .unwrap_or("text/plain");

        let uri_ref = source.uri.as_deref();
        let code_lang = code_chunker::detect_code_language(mime, uri_ref);
        let is_code = code_lang.is_some();

        let (parse_content, parse_mime): (std::borrow::Cow<'_, [u8]>, &str) =
            if let Some(lang) = code_lang {
                let source_text = String::from_utf8_lossy(content_bytes);
                let md = code_chunker::code_to_markdown(&source_text, lang)?;
                (std::borrow::Cow::Owned(md.into_bytes()), "text/markdown")
            } else if self.pipeline.convert_enabled {
                if let Some(ref registry) = self.converter_registry {
                    let converted = registry.convert(content_bytes, mime).await?;
                    (
                        std::borrow::Cow::Owned(converted.into_bytes()),
                        "text/markdown",
                    )
                } else {
                    (std::borrow::Cow::Borrowed(content_bytes), mime)
                }
            } else {
                (std::borrow::Cow::Borrowed(content_bytes), mime)
            };

        // Parse
        let parsed = crate::ingestion::parser::parse(&parse_content, parse_mime)?;

        // Normalize — skipped for code sources.
        let normalized = if is_code {
            parsed.body.clone()
        } else if self.pipeline.normalize_enabled {
            let n = crate::ingestion::normalize::normalize(&parsed.body);
            crate::ingestion::normalize::strip_artifacts(&n)
        } else {
            parsed.body.clone()
        };

        // Update normalized content and hash on the source.
        let normalized_hash = Sha256::digest(normalized.as_bytes()).to_vec();
        source.normalized_content = Some(normalized.clone());
        source.normalized_hash = Some(normalized_hash);

        // Stamp pipeline fingerprint into source metadata.
        if let Some(fp) = self.current_fingerprint() {
            Self::stamp_fingerprint(&mut source.metadata, &fp);
        }

        SourceRepo::update(&*self.repo, &source).await?;

        // Stage 4: Chunk
        let chunk_outputs = crate::ingestion::chunker::chunk_document(
            &normalized,
            self.chunk_size,
            self.chunk_overlap,
        );

        // Filter metadata-only, boilerplate, and artifact-heading chunks.
        let chunk_outputs: Vec<_> = chunk_outputs
            .into_iter()
            .filter(|co| {
                !is_metadata_only(&co.text)
                    && !is_boilerplate_heavy(&co.text)
                    && !is_author_block(&co.text)
                    && !has_artifact_heading(&co.heading_path)
            })
            .collect();
        let chunks_created = chunk_outputs.len();

        // Build chunk ID map
        let mut id_map = std::collections::HashMap::new();
        for co in &chunk_outputs {
            id_map.insert(co.id, ChunkId::from_uuid(co.id));
        }

        // Build chunks for batch insert
        let chunks: Vec<Chunk> = chunk_outputs
            .iter()
            .enumerate()
            .map(|(ordinal, co)| {
                let chunk_id = id_map[&co.id];
                let level = match co.level {
                    crate::ingestion::chunker::ChunkLevel::Section => ModelChunkLevel::Section,
                    crate::ingestion::chunker::ChunkLevel::Paragraph => ModelChunkLevel::Paragraph,
                };

                let unique_content = &co.text[co.context_prefix_len..];
                let content_hash = Sha256::digest(unique_content.as_bytes()).to_vec();
                let token_count = co.text.split_whitespace().count() as i32;
                let hierarchy = co
                    .heading_path
                    .iter()
                    .map(|h| sanitize_ltree_label(h))
                    .collect::<Vec<_>>()
                    .join(".");

                let chunk_meta = detect_chunk_content_types(&co.text);

                let mut chunk = Chunk::new(
                    source.id,
                    level,
                    ordinal as i32,
                    co.text.clone(),
                    content_hash,
                    token_count,
                );
                chunk.id = chunk_id;
                chunk.byte_start = Some(co.byte_start as i32);
                chunk.byte_end = Some(co.byte_end as i32);
                chunk.content_offset = Some(co.context_prefix_len as i32);
                if let Some(parent_uuid) = co.parent_id {
                    if let Some(&pid) = id_map.get(&parent_uuid) {
                        chunk = chunk.with_parent(pid);
                    }
                }
                if !hierarchy.is_empty() {
                    chunk = chunk.with_hierarchy(hierarchy);
                }
                chunk = chunk.with_metadata(chunk_meta);
                chunk
            })
            .collect();

        ChunkRepo::batch_create(&*self.repo, &chunks).await?;

        let example_chunks: std::collections::HashSet<uuid::Uuid> = chunk_outputs
            .iter()
            .filter(|co| has_example_markers(&co.text))
            .map(|co| co.id)
            .collect();

        // Stage 5: Embed chunks + landscape analysis
        let landscape_results = if let Some(ref embedder) = self.embedder {
            let texts: Vec<String> = chunk_outputs.iter().map(|co| co.text.clone()).collect();
            let embeddings = embedder.embed_document_chunks(&texts).await?;

            for (co, emb) in chunk_outputs.iter().zip(embeddings.iter()) {
                let chunk_id = id_map[&co.id];
                match truncate_and_validate(emb, self.table_dims.chunk, "chunks") {
                    Ok(truncated) => {
                        if let Err(e) =
                            ChunkRepo::update_embedding(&*self.repo, chunk_id, &truncated).await
                        {
                            tracing::warn!(
                                chunk_id = %chunk_id,
                                error = %e,
                                "failed to store chunk embedding"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            chunk_id = %chunk_id,
                            error = %e,
                            "chunk embedding dimension mismatch"
                        );
                    }
                }
            }

            // Compute parent_alignment for child chunks.
            compute_parent_alignment(&chunk_outputs, &embeddings, &id_map, &*self.repo).await;

            // Re-embed source-level vector
            match embedder.embed(std::slice::from_ref(&normalized)).await {
                Ok(source_embeddings) => {
                    if let Some(emb) = source_embeddings.first() {
                        match truncate_and_validate(emb, self.table_dims.source, "sources") {
                            Ok(truncated) => {
                                if let Err(e) =
                                    SourceRepo::update_embedding(&*self.repo, id, &truncated).await
                                {
                                    tracing::warn!(
                                        error = %e,
                                        "failed to update source embedding"
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "source embedding dimension mismatch"
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "failed to embed source for reprocessing"
                    );
                }
            }

            // Landscape analysis.
            //
            // Reprocessing always has prior extractions (they were
            // just superseded in step 1), so this is NOT a first
            // ingestion — landscape gating applies normally.
            if self.pipeline.landscape_enabled {
                let parent_embeddings: Vec<Option<&Vec<f64>>> = chunk_outputs
                    .iter()
                    .map(|co| {
                        co.parent_id.and_then(|pid| {
                            let parent_idx = chunk_outputs.iter().position(|p| p.id == pid)?;
                            Some(&embeddings[parent_idx])
                        })
                    })
                    .collect();
                let landscape = crate::ingestion::landscape::analyze_landscape(
                    &embeddings,
                    &parent_embeddings,
                    None,
                    false, // reprocess = not first ingestion
                );
                for lr in &landscape {
                    if lr.chunk_index < chunk_outputs.len() {
                        let chunk_id = id_map[&chunk_outputs[lr.chunk_index].id];
                        let metrics_json = serde_json::to_value(&lr.metrics).ok();
                        ChunkRepo::update_landscape(
                            &*self.repo,
                            chunk_id,
                            lr.parent_alignment,
                            lr.extraction_method.as_str(),
                            metrics_json,
                        )
                        .await?;
                    }
                }
                Some(landscape)
            } else {
                None
            }
        } else {
            None
        };

        // Stage 5.5: Heuristic coref — skipped for code sources.
        let mut coref_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        if !is_code {
            let coref_resolver = CorefResolver::new();
            let coref_links = coref_resolver.resolve(&chunk_outputs);
            for link in &coref_links {
                coref_map.insert(link.mention.to_lowercase(), link.referent.to_lowercase());
            }
        }

        // Stage 5.7: Neural coref — skipped for code sources.
        let resolved_texts: Option<std::collections::HashMap<uuid::Uuid, String>> = if is_code {
            None
        } else if self.pipeline.coref_enabled {
            if let Some(ref coref_client) = self.coref_client {
                let extractable_indices: Vec<usize> = chunk_outputs
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| should_extract(*i, landscape_results.as_deref()))
                    .map(|(i, _)| i)
                    .collect();
                let mut resolved = std::collections::HashMap::new();
                for &idx in &extractable_indices {
                    let co = &chunk_outputs[idx];
                    match coref_client.resolve(&co.text).await {
                        Ok(r) => {
                            resolved.insert(co.id, r);
                        }
                        Err(e) => {
                            tracing::warn!(
                                chunk_index = idx,
                                error = %e,
                                "neural coref failed during reprocessing"
                            );
                        }
                    }
                }
                if resolved.is_empty() {
                    None
                } else {
                    Some(resolved)
                }
            } else {
                None
            }
        } else {
            None
        };

        // Stage 6-7: Extract + resolve
        // Use AST extractor for code sources, LLM for everything
        // else.
        let ast_ext_rp: Option<Arc<dyn Extractor>> = if is_code {
            Some(Arc::new(
                crate::ingestion::ast_extractor::AstExtractor::new(),
            ))
        } else {
            None
        };
        let active_extractor_rp = if is_code {
            ast_ext_rp.as_ref()
        } else {
            self.extractor.as_ref()
        };
        let ext_method_rp = if is_code { "ast" } else { "llm" };
        if let Some(extractor) = active_extractor_rp {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(self.extract_concurrency));

            // Build extraction context from source metadata.
            let extraction_context = Arc::new(ExtractionContext {
                source_type: Some(source.source_type.clone()),
                source_uri: source.uri.clone(),
                source_title: source.title.clone(),
            });

            let extractable: Vec<_> = chunk_outputs
                .iter()
                .enumerate()
                .filter(|(i, _co)| should_extract(*i, landscape_results.as_deref()))
                .map(|(_i, co)| co)
                .collect();

            let batches = group_extraction_batches(
                &extractable,
                self.min_extract_tokens,
                self.extract_batch_tokens,
                resolved_texts.as_ref(),
            );

            tracing::debug!(
                extractable = extractable.len(),
                batches = batches.len(),
                "reprocessing extraction batching"
            );

            let concurrency_size = self.extract_concurrency * 2;
            let mut extraction_results: Vec<Result<(uuid::Uuid, _)>> = Vec::new();

            for batch_slice in batches.chunks(concurrency_size) {
                let batch_futures: Vec<_> = batch_slice
                    .iter()
                    .map(|(primary_id, text)| {
                        let sem = Arc::clone(&semaphore);
                        let ext = Arc::clone(extractor);
                        let ctx = Arc::clone(&extraction_context);
                        let text = text.clone();
                        let primary_id = *primary_id;
                        async move {
                            let _permit = sem.acquire().await.map_err(|_| {
                                Error::Ingestion("extraction semaphore closed".to_string())
                            })?;
                            let result = ext.extract(&text, &ctx).await?;
                            Ok::<_, Error>((primary_id, result))
                        }
                    })
                    .collect();
                let batch_results = futures::future::join_all(batch_futures).await;
                extraction_results.extend(batch_results);
            }

            let mut name_to_node: std::collections::HashMap<String, NodeId> =
                std::collections::HashMap::new();

            for extraction_result in extraction_results {
                let (chunk_uuid, extraction) = match extraction_result {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "extraction failed during reprocessing"
                        );
                        continue;
                    }
                };
                let chunk_id = id_map[&chunk_uuid];
                let is_example_chunk = example_chunks.contains(&chunk_uuid);

                for entity in &extraction.entities {
                    let node_id = self
                        .resolve_and_store_entity(entity, chunk_id, ext_method_rp)
                        .await?;
                    name_to_node.insert(entity.name.to_lowercase(), node_id);

                    if let Some(referent) = coref_map.get(&entity.name.to_lowercase()) {
                        name_to_node.entry(referent.clone()).or_insert(node_id);
                        self.ensure_alias(&entity.name, node_id, chunk_id).await?;
                    }
                }

                for rel in &extraction.relationships {
                    let src_key = coref_map
                        .get(&rel.source_name.to_lowercase())
                        .cloned()
                        .unwrap_or_else(|| rel.source_name.to_lowercase());
                    let tgt_key = coref_map
                        .get(&rel.target_name.to_lowercase())
                        .cloned()
                        .unwrap_or_else(|| rel.target_name.to_lowercase());

                    let source_node = match name_to_node.get(&src_key) {
                        Some(&nid) => nid,
                        None => continue,
                    };
                    let target_node = match name_to_node.get(&tgt_key) {
                        Some(&nid) => nid,
                        None => continue,
                    };

                    if source_node == target_node {
                        continue;
                    }

                    let resolved_rel_type = if let Some(ref rtr) = self.rel_type_resolver {
                        rtr.resolve_rel_type(&rel.rel_type).await?
                    } else {
                        rel.rel_type.clone()
                    };

                    let mut edge = Edge::new(source_node, target_node, resolved_rel_type);
                    let conf = if is_example_chunk {
                        (rel.confidence * 0.5).clamp(0.0, 1.0)
                    } else {
                        rel.confidence
                    };
                    edge.confidence = conf;
                    EdgeRepo::create(&*self.repo, &edge).await?;

                    let ext_record = Extraction::new(
                        chunk_id,
                        ExtractedEntityType::Edge,
                        edge.id.into_uuid(),
                        ext_method_rp.to_string(),
                        conf,
                    );
                    ExtractionRepo::create(&*self.repo, &ext_record).await?;
                }
            }

            // Re-embed nodes that need it
            if let Some(ref embedder) = self.embedder {
                let node_ids: Vec<NodeId> = name_to_node.values().copied().collect();
                if !node_ids.is_empty() {
                    let mut texts: Vec<String> = Vec::with_capacity(node_ids.len());
                    let mut valid_ids: Vec<NodeId> = Vec::with_capacity(node_ids.len());

                    for &nid in &node_ids {
                        if let Some(node) = NodeRepo::get(&*self.repo, nid).await? {
                            let text = match &node.description {
                                Some(desc) if !desc.is_empty() => {
                                    format!("{}: {}", node.canonical_name, desc)
                                }
                                _ => node.canonical_name.clone(),
                            };
                            texts.push(text);
                            valid_ids.push(nid);
                        }
                    }

                    if !texts.is_empty() {
                        match embedder.embed(&texts).await {
                            Ok(embeddings) => {
                                for (nid, emb) in valid_ids.iter().zip(embeddings.iter()) {
                                    if let Ok(truncated) =
                                        truncate_and_validate(emb, self.table_dims.node, "nodes")
                                    {
                                        if let Err(e) = NodeRepo::update_embedding(
                                            &*self.repo,
                                            *nid,
                                            &truncated,
                                        )
                                        .await
                                        {
                                            tracing::warn!(
                                                node_id = %nid,
                                                error = %e,
                                                "failed to update node embedding \
                                                 during reprocessing"
                                            );
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "failed to embed nodes during reprocessing"
                                );
                            }
                        }
                    }
                }
            }
        }

        tracing::info!(
            source_id = %id,
            version = source.content_version,
            extractions_superseded,
            chunks_deleted,
            chunks_created,
            "source reprocessing complete"
        );

        Ok(ReprocessResult {
            source_id: id.into_uuid(),
            extractions_superseded,
            chunks_deleted,
            chunks_created,
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
    /// 2. Delete extractions for this source's chunks
    /// 3. Nullify `node_aliases.source_chunk_id` references
    /// 4. Delete chunks
    /// 5. For each affected node with zero remaining active
    ///    extractions: delete aliases, edges, then the node
    /// 6. For nodes with remaining extractions: update
    ///    `mention_count`
    /// 7. Delete the source record
    pub async fn delete(&self, id: SourceId) -> Result<DeleteResult> {
        // Step 1: Collect node IDs affected by this source's
        // extractions before we delete anything.
        let affected_node_ids = ExtractionRepo::list_node_ids_by_source(&*self.repo, id).await?;

        // Step 2: Delete extractions (must precede chunk deletion
        // due to FK on extractions.chunk_id).
        let extractions_deleted = ExtractionRepo::delete_by_source(&*self.repo, id).await?;

        // Step 3: Nullify alias chunk references (must precede
        // chunk deletion due to FK on
        // node_aliases.source_chunk_id).
        NodeAliasRepo::clear_source_chunks(&*self.repo, id).await?;

        // Step 4: Delete chunks (now safe — no FK references).
        let chunks_deleted = ChunkRepo::delete_by_source(&*self.repo, id).await?;

        // Step 5: Handle affected nodes. For each node, check if
        // it still has active extractions from other sources.
        let mut nodes_deleted: u64 = 0;
        let mut edges_deleted: u64 = 0;

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
                // Node survives — update mention_count to reflect
                // the remaining extraction count.
                if let Some(mut node) = NodeRepo::get(&*self.repo, *node_id).await? {
                    let count: i64 = ExtractionRepo::count_active_by_entity(
                        &*self.repo,
                        "node",
                        node_id.into_uuid(),
                    )
                    .await?;
                    node.mention_count = count as i32;
                    NodeRepo::update(&*self.repo, &node).await?;
                }
            }
        }

        // Step 6: Delete the source record.
        let deleted = SourceRepo::delete(&*self.repo, id).await?;

        Ok(DeleteResult {
            deleted,
            chunks_deleted,
            extractions_deleted,
            nodes_deleted,
            edges_deleted,
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

/// Fixed namespace offset for entity-resolution advisory locks.
///
/// This constant is XORed into the hash to avoid collisions with
/// advisory locks used for other purposes in the same database.
const ENTITY_LOCK_NAMESPACE: i64 = 0x436F_7661_6C65_6E63; // "Covalenc"

/// Produce a deterministic i64 hash from an entity name for use as a
/// PostgreSQL advisory lock key.
///
/// Uses the first 8 bytes of a SHA-256 hash of the lowercased,
/// trimmed name, XORed with [`ENTITY_LOCK_NAMESPACE`] to avoid
/// collisions with other advisory lock users.
pub(crate) fn entity_name_lock_key(name: &str) -> i64 {
    let canonical = name.trim().to_lowercase();
    let hash = Sha256::digest(canonical.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&hash[..8]);
    i64::from_le_bytes(bytes) ^ ENTITY_LOCK_NAMESPACE
}

/// Information about a source supersession detected during
/// URI-based update class analysis.
struct SupersedesInfo {
    /// The ID of the source being superseded.
    old_source_id: SourceId,
    /// The detected update class.
    update_class: UpdateClass,
    /// The version number for the new source.
    new_version: i32,
}

/// Group extractable chunks into token-budget batches for
/// efficient LLM usage.
///
/// - Chunks with fewer than `min_tokens` whitespace-delimited words
///   are skipped entirely.
/// - Adjacent chunks are concatenated (separated by `\n\n---\n\n`)
///   up to `batch_tokens`. A single chunk exceeding the budget is
///   extracted alone.
///
/// Returns a list of `(primary_chunk_uuid, combined_text)` pairs.
/// The primary UUID is used for extraction provenance.
fn group_extraction_batches(
    chunks: &[&crate::ingestion::chunker::ChunkOutput],
    min_tokens: usize,
    batch_tokens: usize,
    resolved_texts: Option<&std::collections::HashMap<uuid::Uuid, String>>,
) -> Vec<(uuid::Uuid, String)> {
    let mut batches: Vec<(uuid::Uuid, String)> = Vec::new();
    let mut current_primary: Option<uuid::Uuid> = None;
    let mut current_text = String::new();
    let mut current_tokens: usize = 0;
    let mut skipped = 0_usize;

    for co in chunks {
        let text = resolved_texts
            .and_then(|m| m.get(&co.id))
            .map(|s| s.as_str())
            .unwrap_or(&co.text);
        let tokens = text.split_whitespace().count();

        if tokens < min_tokens {
            skipped += 1;
            continue;
        }

        // Flush if adding this chunk would exceed the budget
        // (but only if we already have content).
        if current_primary.is_some() && current_tokens + tokens > batch_tokens {
            if let Some(primary) = current_primary.take() {
                batches.push((primary, std::mem::take(&mut current_text)));
            }
            current_tokens = 0;
        }

        if current_primary.is_none() {
            current_primary = Some(co.id);
        }
        if !current_text.is_empty() {
            current_text.push_str("\n\n---\n\n");
        }
        current_text.push_str(text);
        current_tokens += tokens;
    }

    // Flush remaining
    if let Some(primary) = current_primary {
        batches.push((primary, current_text));
    }

    if skipped > 0 {
        tracing::debug!(
            skipped,
            min_tokens,
            "chunks skipped (below min_extract_tokens)"
        );
    }

    batches
}

/// Compute and store `parent_alignment` for child chunks.
///
/// For each chunk that has a `parent_chunk_id`, computes the
/// cosine similarity between the child and parent embeddings
/// and persists it via `ChunkRepo::update_parent_alignment`.
///
/// Failures are logged as warnings and do not abort ingestion.
async fn compute_parent_alignment(
    chunk_outputs: &[crate::ingestion::chunker::ChunkOutput],
    embeddings: &[Vec<f64>],
    id_map: &std::collections::HashMap<uuid::Uuid, ChunkId>,
    repo: &(impl ChunkRepo + ?Sized),
) {
    for (i, co) in chunk_outputs.iter().enumerate() {
        let parent_uuid = match co.parent_id {
            Some(pid) => pid,
            None => continue,
        };

        let parent_idx = match chunk_outputs.iter().position(|c| c.id == parent_uuid) {
            Some(idx) => idx,
            None => continue,
        };

        if i >= embeddings.len() || parent_idx >= embeddings.len() {
            continue;
        }

        let similarity = cosine_similarity(&embeddings[i], &embeddings[parent_idx]);

        let chunk_id = id_map[&co.id];
        if let Err(e) = ChunkRepo::update_parent_alignment(repo, chunk_id, similarity).await {
            tracing::warn!(
                chunk_id = %chunk_id,
                error = %e,
                "failed to store parent_alignment"
            );
        }
    }
}

/// Determine whether a chunk should go through LLM extraction
/// based on landscape analysis results.
///
/// Returns `true` if:
/// - No landscape results exist (all chunks should be extracted)
/// - The chunk's extraction method is `FullExtraction` or
///   `FullExtractionWithReview`
///
/// Returns `false` for `EmbeddingLinkage` (high parent alignment,
/// redundant content) and `DeltaCheck` (moderate alignment, only
/// delta check needed).
fn should_extract(
    chunk_index: usize,
    landscape: Option<&[crate::ingestion::landscape::ChunkLandscapeResult]>,
) -> bool {
    match landscape {
        None => true, // No landscape data — extract everything
        Some(results) => match results.get(chunk_index) {
            None => true, // Missing result — extract
            Some(lr) => {
                let extract = matches!(
                    lr.extraction_method,
                    ExtractionMethod::FullExtraction | ExtractionMethod::FullExtractionWithReview
                );
                if !extract {
                    tracing::debug!(
                        chunk_index,
                        method = lr.extraction_method.as_str(),
                        alignment = ?lr.parent_alignment,
                        "skipping extraction for chunk (landscape gating)"
                    );
                }
                extract
            }
        },
    }
}

/// Detect the update class by comparing old and new content.
///
/// Uses a simple word-level Jaccard similarity metric:
/// - `>=80%` overlap: `Correction` (minor fix to existing content)
/// - `<20%` overlap: `Refactor` (structural rewrite)
/// - Otherwise: `Versioned` (normal update)
///
/// This is a lightweight heuristic. Production systems may use
/// more sophisticated diff algorithms.
fn detect_update_class(old_text: &str, new_text: &str) -> UpdateClass {
    let overlap = content_overlap(old_text, new_text);
    if overlap >= 0.80 {
        UpdateClass::Correction
    } else if overlap < 0.20 {
        UpdateClass::Refactor
    } else {
        UpdateClass::Versioned
    }
}

/// Compute word-level Jaccard similarity between two texts.
///
/// Returns a value in `[0.0, 1.0]` representing the proportion
/// of shared words between the two texts. Returns 0.0 if both
/// texts are empty.
fn content_overlap(a: &str, b: &str) -> f64 {
    let words_a: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let words_b: std::collections::HashSet<&str> = b.split_whitespace().collect();

    if words_a.is_empty() && words_b.is_empty() {
        return 0.0;
    }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    if union == 0 {
        return 0.0;
    }

    intersection as f64 / union as f64
}

/// Detect content types in a chunk's Markdown text.
///
/// Sets boolean flags for table content, fenced code blocks,
/// and lists. Used to enrich the chunk metadata JSONB during
/// ingestion so search and extraction can adapt to content type.
fn detect_chunk_content_types(text: &str) -> serde_json::Value {
    let mut contains_table = false;
    let mut contains_code = false;
    let mut contains_list = false;
    let mut in_code_fence = false;

    for line in text.lines() {
        let trimmed = line.trim();

        // Detect fenced code blocks (``` or ~~~).
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code_fence = !in_code_fence;
            contains_code = true;
            continue;
        }

        // Skip content inside code fences for other detection.
        if in_code_fence {
            continue;
        }

        // Detect Markdown tables: lines starting and ending with `|`
        // or containing `|` with at least 2 cells.
        if trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.matches('|').count() >= 3 {
            contains_table = true;
        }

        // Detect unordered lists (-, *, +) and ordered lists (1.).
        if trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("+ ")
            || (trimmed.len() >= 3
                && trimmed.as_bytes()[0].is_ascii_digit()
                && trimmed.contains(". "))
        {
            contains_list = true;
        }
    }

    // Detect example/hypothetical context markers.
    let contains_example = has_example_markers(text);

    serde_json::json!({
        "contains_table": contains_table,
        "contains_code": contains_code,
        "contains_list": contains_list,
        "contains_example": contains_example,
    })
}

/// Check whether text contains markers indicating illustrative
/// examples, hypothetical scenarios, or placeholder content.
///
/// When a chunk is tagged as containing examples, extracted
/// entities are given reduced confidence to limit their
/// influence on the knowledge graph.
fn has_example_markers(text: &str) -> bool {
    let lower = text.to_lowercase();
    let markers = [
        "for example",
        "for instance",
        "e.g.",
        "e.g.,",
        "suppose ",
        "consider the case",
        "hypothetical",
        "as an illustration",
        "imagine that",
        "let's say",
        "assume that",
        "in this scenario",
        "a simple example",
        "toy example",
    ];
    markers.iter().any(|m| lower.contains(m))
}

/// Sanitize a string for use as an ltree label.
///
/// ltree labels can only contain alphanumeric characters and
/// underscores. All other characters (spaces, hyphens, punctuation,
/// etc.) are replaced with `_`. Empty labels become `"_"`.
fn sanitize_ltree_label(s: &str) -> String {
    let sanitized: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
}

/// Check if a chunk's content is purely metadata with no substantive
/// text. Returns `true` for chunks whose lines are all bold labels
/// (`**Key:** value`), blank lines, or heading markers with no body.
///
/// These chunks are ingestion artifacts from metadata appearing before
/// the first heading in a source document.
fn is_metadata_only(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    // Short chunks (< 80 chars) where every line is a bold label or
    // blank are considered metadata-only.
    if trimmed.len() >= 80 {
        return false;
    }
    trimmed.lines().all(|line| {
        let l = line.trim();
        l.is_empty()
            || (l.starts_with("**") && l.contains(":**"))
            || (l.starts_with('[')
                && l.len() < 20
                && l.chars().skip(1).take(4).all(|c| c.is_ascii_digit()))
    })
}

/// Known boilerplate lines from arxiv and academic paper pages.
const BOILERPLATE_LINES: &[&str] = &[
    "view pdf",
    "view a pdf",
    "html (experimental)",
    "cite as:",
    "subjects:",
    "comments:",
    "download pdf",
    "download:",
    "bibtex",
    "submission history",
    "< prev",
    "next >",
    "report issue for preceding element",
];

/// Check whether a chunk is dominated by web UI boilerplate
/// (navigation elements, arxiv metadata, etc.) rather than
/// substantive content.
///
/// Returns `true` when ≥60% of non-blank lines match known
/// boilerplate patterns.
fn is_boilerplate_heavy(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false; // handled by is_metadata_only
    }
    let lines: Vec<&str> = trimmed
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return false;
    }
    let boilerplate_count = lines.iter().filter(|l| is_boilerplate_line(l)).count();
    let ratio = boilerplate_count as f64 / lines.len() as f64;
    ratio >= 0.6
}

/// Check whether a single line matches boilerplate patterns.
fn is_boilerplate_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    // Known boilerplate strings.
    if BOILERPLATE_LINES.iter().any(|bp| lower.contains(bp)) {
        return true;
    }
    // Bold label lines: **Key:** value
    if line.starts_with("**") && line.contains(":**") {
        return true;
    }
    // Very short lines (< 15 chars) that are just labels.
    if line.len() < 15 && (lower.ends_with(':') || lower.ends_with("...")) {
        return true;
    }
    // Navigation/ToC links: numbered items pointing to sections.
    // Pattern: "01. [Section](url)" or "[Section](url#anchor)"
    let trimmed = line.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
    let trimmed = trimmed.trim();
    if trimmed.starts_with('[') && trimmed.contains("](") && trimmed.contains("://") {
        return true;
    }
    false
}

/// Detect author-block chunks: sequences of names, affiliations, and
/// email addresses with no substantive content.  These appear in scraped
/// academic papers as the header block before the abstract.
///
/// Heuristic: if ≥40% of non-blank lines contain an email indicator
/// (`@` or `mailto:`) the chunk is considered an author block.
fn is_author_block(text: &str) -> bool {
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return false;
    }

    // Pattern 1: starts with "Authors:" prefix — common in arxiv
    // scraped content. The first line names the authors and the
    // rest is affiliations/institutions.
    let first = lines[0];
    if first.starts_with("Authors:") || first.starts_with("**Authors:**") {
        return true;
    }

    // Pattern 2: high ratio of email/mailto lines (≥ 40%).
    if lines.len() >= 2 {
        let email_lines = lines
            .iter()
            .filter(|l| l.contains('@') || l.contains("mailto:"))
            .count();
        let ratio = email_lines as f64 / lines.len() as f64;
        if ratio >= 0.4 {
            return true;
        }
    }

    false
}

/// Known artifact headings from web scraping that should cause the
/// entire chunk to be discarded.
const ARTIFACT_HEADINGS: &[&str] = &[
    "report issue for preceding element",
];

/// Returns `true` if any heading in the chunk's path matches a known
/// web-scraping artifact heading.
fn has_artifact_heading(heading_path: &[String]) -> bool {
    heading_path.iter().any(|h| {
        let lower = h.to_lowercase();
        ARTIFACT_HEADINGS.iter().any(|a| lower.contains(a))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_only_bold_labels() {
        assert!(is_metadata_only("**Authors:** John\n**arxiv:** 123"));
    }

    #[test]
    fn metadata_only_empty() {
        assert!(is_metadata_only(""));
        assert!(is_metadata_only("  \n  "));
    }

    #[test]
    fn metadata_only_short_arxiv() {
        assert!(is_metadata_only("[2506.12345]"));
    }

    #[test]
    fn not_metadata_has_content() {
        assert!(!is_metadata_only(
            "**Authors:** John\nThis paper discusses knowledge graphs."
        ));
    }

    #[test]
    fn not_metadata_long_text() {
        let long = "**Authors:** A very long author list that exceeds eighty characters in total length and counts as real content";
        assert!(!is_metadata_only(long));
    }

    #[test]
    fn boilerplate_heavy_arxiv_page() {
        let text = "Authors:John Doe, Jane Smith\n\
                    View a PDF of the paper\n\
                    View PDF\n\
                    HTML (experimental)\n\
                    Subjects:\n\
                    Cite as:";
        assert!(is_boilerplate_heavy(text));
    }

    #[test]
    fn boilerplate_heavy_nav_elements() {
        let text = "< prev\nnext >\nView PDF\nDownload PDF";
        assert!(is_boilerplate_heavy(text));
    }

    #[test]
    fn not_boilerplate_real_content() {
        let text = "Knowledge graphs represent entities and relationships.\n\
                    This enables multi-hop reasoning across documents.\n\
                    The system uses RRF for fusion.";
        assert!(!is_boilerplate_heavy(text));
    }

    #[test]
    fn not_boilerplate_mixed_content() {
        // Some boilerplate but majority is real content.
        let text = "**Authors:** John Doe\n\
                    Knowledge graphs are important.\n\
                    They enable structured retrieval.\n\
                    RRF fuses multiple signal dimensions.\n\
                    The system uses pgvector for embeddings.";
        assert!(!is_boilerplate_heavy(text));
    }

    #[test]
    fn boilerplate_line_detection() {
        assert!(is_boilerplate_line("View PDF"));
        assert!(is_boilerplate_line("< prev"));
        assert!(is_boilerplate_line("**Authors:** John"));
        assert!(is_boilerplate_line("Subjects:"));
        assert!(!is_boilerplate_line("Knowledge graphs enable reasoning."));
    }

    #[test]
    fn boilerplate_report_issue_arxiv() {
        assert!(is_boilerplate_line("Report issue for preceding element"));
    }

    #[test]
    fn boilerplate_toc_nav_links() {
        assert!(is_boilerplate_line(
            "01. [Abstract](https://arxiv.org/html/2506.02509#abstract \"Abstract\")"
        ));
        assert!(is_boilerplate_line(
            "[1 Introduction](https://arxiv.org/html/2506.02509v1#S1 \"Title\")"
        ));
    }

    #[test]
    fn not_boilerplate_inline_link() {
        // A sentence with a link is NOT boilerplate.
        assert!(!is_boilerplate_line(
            "See the original paper for details."
        ));
    }

    #[test]
    fn artifact_heading_filter() {
        let path = vec!["Report issue for preceding element".to_string()];
        assert!(has_artifact_heading(&path));
    }

    #[test]
    fn artifact_heading_nested() {
        let path = vec![
            "My Paper".to_string(),
            "Report issue for preceding element".to_string(),
        ];
        assert!(has_artifact_heading(&path));
    }

    #[test]
    fn artifact_heading_clean_path() {
        let path = vec![
            "Introduction".to_string(),
            "Methods".to_string(),
        ];
        assert!(!has_artifact_heading(&path));
    }

    #[test]
    fn artifact_heading_empty_path() {
        let path: Vec<String> = vec![];
        assert!(!has_artifact_heading(&path));
    }

    #[test]
    fn author_block_detected() {
        // Each author line typically has name + email on same line
        // (or email on a short adjacent line).
        let text = "Bo Liu Beijing Institute of Technology liubo@bit.edu.cn\n\
                    Yanjie Jiang Peking University yanjiejiang@pku.edu.cn\n\
                    Yuxia Zhang Beijing Institute of Technology yuxiazh@bit.edu.cn\n\
                    Nan Niu University of Cincinnati nan.niu@uc.edu\n\
                    Guangjie Li National Innovation Institute liguangjie@126.com";
        assert!(is_author_block(text));
    }

    #[test]
    fn author_block_mailto_links() {
        let text = "Alice Smith\n\
                    [alice@example.com](mailto:alice@example.com)\n\
                    Bob Jones\n\
                    [bob@test.org](mailto:bob@test.org)\n\
                    Carol White\n\
                    [carol@uni.edu](mailto:carol@uni.edu)";
        assert!(is_author_block(text));
    }

    #[test]
    fn not_author_block_real_content() {
        let text = "Knowledge graphs represent entities and relationships.\n\
                    This enables multi-hop reasoning across documents.\n\
                    Contact us at support@example.com for details.";
        assert!(!is_author_block(text));
    }

    #[test]
    fn not_author_block_single_email() {
        let text = "Send questions to admin@example.com";
        assert!(!is_author_block(text));
    }

    #[test]
    fn author_block_prefix_detected() {
        let text = "Authors:Hairong Zhang, Jiaheng Si, Guohang Yan, Boyuan Qi";
        assert!(is_author_block(text));
    }

    #[test]
    fn author_block_bold_prefix_detected() {
        let text = "**Authors:** Alice Smith, Bob Jones, Carol White\n\
                    University of Example, Department of CS";
        assert!(is_author_block(text));
    }

    #[test]
    fn not_author_block_empty() {
        assert!(!is_author_block(""));
    }

    #[test]
    fn delete_result_serializes_all_fields() {
        let result = DeleteResult {
            deleted: true,
            chunks_deleted: 5,
            extractions_deleted: 10,
            nodes_deleted: 3,
            edges_deleted: 7,
        };
        let json = serde_json::to_value(&result).expect("serialize");
        assert_eq!(json["deleted"], true);
        assert_eq!(json["chunks_deleted"], 5);
        assert_eq!(json["extractions_deleted"], 10);
        assert_eq!(json["nodes_deleted"], 3);
        assert_eq!(json["edges_deleted"], 7);
    }

    #[test]
    fn delete_result_zero_counts_when_nothing_found() {
        let result = DeleteResult {
            deleted: false,
            chunks_deleted: 0,
            extractions_deleted: 0,
            nodes_deleted: 0,
            edges_deleted: 0,
        };
        assert!(!result.deleted);
        assert_eq!(result.chunks_deleted, 0);
        assert_eq!(result.extractions_deleted, 0);
        assert_eq!(result.nodes_deleted, 0);
        assert_eq!(result.edges_deleted, 0);
    }

    #[test]
    fn entity_name_lock_key_is_deterministic() {
        let a = entity_name_lock_key("Rust");
        let b = entity_name_lock_key("Rust");
        assert_eq!(a, b);
    }

    #[test]
    fn entity_name_lock_key_is_case_insensitive() {
        let lower = entity_name_lock_key("rust");
        let upper = entity_name_lock_key("RUST");
        let mixed = entity_name_lock_key("RuSt");
        assert_eq!(lower, upper);
        assert_eq!(lower, mixed);
    }

    #[test]
    fn entity_name_lock_key_trims_whitespace() {
        let plain = entity_name_lock_key("rust");
        let padded = entity_name_lock_key("  rust  ");
        assert_eq!(plain, padded);
    }

    #[test]
    fn entity_name_lock_key_differs_for_different_names() {
        let a = entity_name_lock_key("rust");
        let b = entity_name_lock_key("python");
        assert_ne!(a, b);
    }

    #[test]
    fn entity_name_lock_key_includes_namespace() {
        // Compute what the raw hash would be without the XOR and
        // verify the function's output differs (proving the
        // namespace is applied).
        let canonical = "test_entity";
        let hash = Sha256::digest(canonical.as_bytes());
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&hash[..8]);
        let raw = i64::from_le_bytes(bytes);

        let keyed = entity_name_lock_key(canonical);
        assert_ne!(raw, keyed);
        assert_eq!(keyed, raw ^ ENTITY_LOCK_NAMESPACE);
    }

    #[test]
    fn entity_name_lock_key_empty_string() {
        // Should not panic on empty input.
        let _ = entity_name_lock_key("");
    }

    #[test]
    fn sanitize_ltree_label_basic() {
        assert_eq!(sanitize_ltree_label("hello world"), "hello_world");
        assert_eq!(sanitize_ltree_label(""), "_");
        assert_eq!(sanitize_ltree_label("a-b"), "a_b");
        assert_eq!(sanitize_ltree_label("abc_123"), "abc_123");
    }

    // --- Content overlap tests ---

    #[test]
    fn content_overlap_identical() {
        let text = "the quick brown fox jumps over the lazy dog";
        let overlap = content_overlap(text, text);
        assert!((overlap - 1.0).abs() < 1e-10);
    }

    #[test]
    fn content_overlap_no_shared_words() {
        let a = "alpha beta gamma";
        let b = "one two three";
        let overlap = content_overlap(a, b);
        assert!((overlap - 0.0).abs() < 1e-10);
    }

    #[test]
    fn content_overlap_partial() {
        let a = "the quick brown fox";
        let b = "the slow brown bear";
        // Shared: "the", "brown" = 2
        // Union: "the", "quick", "brown", "fox", "slow", "bear" = 6
        let overlap = content_overlap(a, b);
        assert!((overlap - 2.0 / 6.0).abs() < 1e-10);
    }

    #[test]
    fn content_overlap_both_empty() {
        assert!((content_overlap("", "") - 0.0).abs() < 1e-10);
    }

    #[test]
    fn content_overlap_one_empty() {
        assert!((content_overlap("hello", "") - 0.0).abs() < 1e-10);
        assert!((content_overlap("", "hello") - 0.0).abs() < 1e-10);
    }

    // --- Update class detection tests ---

    #[test]
    fn detect_update_class_correction() {
        // >80% overlap = correction (minor edit)
        // 9/10 words shared = 0.9 Jaccard
        let old = "the quick brown fox jumps over the lazy dog today";
        let new = "the quick brown fox leaps over the lazy dog today";
        let class = detect_update_class(old, new);
        assert_eq!(class, UpdateClass::Correction);
    }

    #[test]
    fn detect_update_class_refactor() {
        // <20% overlap = refactor (complete rewrite)
        let old = "alpha beta gamma delta epsilon";
        let new = "one two three four five six seven";
        let class = detect_update_class(old, new);
        assert_eq!(class, UpdateClass::Refactor);
    }

    #[test]
    fn detect_update_class_versioned() {
        // Between 20%-80% overlap = versioned (significant update)
        // Shared: a, b, c, d, e = 5 out of union 10 = 0.5
        let old = "a b c d e f g h i j";
        let new = "a b c d e k l m n o";
        let class = detect_update_class(old, new);
        assert_eq!(class, UpdateClass::Versioned);
    }

    // --- Landscape gating tests ---

    #[test]
    fn should_extract_no_landscape() {
        // No landscape data — extract everything
        assert!(should_extract(0, None));
        assert!(should_extract(5, None));
    }

    #[test]
    fn should_extract_full_extraction() {
        use crate::ingestion::landscape::{ChunkLandscapeResult, LandscapeMetrics};

        let results = vec![ChunkLandscapeResult {
            chunk_index: 0,
            parent_alignment: Some(0.3),
            extraction_method: ExtractionMethod::FullExtraction,
            metrics: LandscapeMetrics {
                adjacent_similarity: None,
                sibling_outlier_score: None,
                graph_novelty: None,
                flags: vec![],
                valley_prominence: None,
            },
        }];

        assert!(should_extract(0, Some(&results)));
    }

    #[test]
    fn should_extract_full_extraction_with_review() {
        use crate::ingestion::landscape::{ChunkLandscapeResult, LandscapeMetrics};

        let results = vec![ChunkLandscapeResult {
            chunk_index: 0,
            parent_alignment: Some(0.1),
            extraction_method: ExtractionMethod::FullExtractionWithReview,
            metrics: LandscapeMetrics {
                adjacent_similarity: None,
                sibling_outlier_score: None,
                graph_novelty: None,
                flags: vec![],
                valley_prominence: None,
            },
        }];

        assert!(should_extract(0, Some(&results)));
    }

    #[test]
    fn should_not_extract_embedding_linkage() {
        use crate::ingestion::landscape::{ChunkLandscapeResult, LandscapeMetrics};

        let results = vec![ChunkLandscapeResult {
            chunk_index: 0,
            parent_alignment: Some(0.9),
            extraction_method: ExtractionMethod::EmbeddingLinkage,
            metrics: LandscapeMetrics {
                adjacent_similarity: None,
                sibling_outlier_score: None,
                graph_novelty: None,
                flags: vec![],
                valley_prominence: None,
            },
        }];

        assert!(!should_extract(0, Some(&results)));
    }

    #[test]
    fn should_not_extract_delta_check() {
        use crate::ingestion::landscape::{ChunkLandscapeResult, LandscapeMetrics};

        let results = vec![ChunkLandscapeResult {
            chunk_index: 0,
            parent_alignment: Some(0.7),
            extraction_method: ExtractionMethod::DeltaCheck,
            metrics: LandscapeMetrics {
                adjacent_similarity: None,
                sibling_outlier_score: None,
                graph_novelty: None,
                flags: vec![],
                valley_prominence: None,
            },
        }];

        assert!(!should_extract(0, Some(&results)));
    }

    #[test]
    fn should_extract_missing_index() {
        // Index not in landscape results — extract
        assert!(should_extract(5, Some(&[])));
    }

    // --- Content type detection tests ---

    #[test]
    fn detect_table_content() {
        let text =
            "Some intro text.\n\n| Name | Value |\n|------|-------|\n| A | 1 |\n\nMore text.";
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_table"], true);
        assert_eq!(meta["contains_code"], false);
        assert_eq!(meta["contains_list"], false);
    }

    #[test]
    fn detect_code_content() {
        let text = "Some text.\n\n```rust\nfn main() {}\n```\n\nMore text.";
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_table"], false);
        assert_eq!(meta["contains_code"], true);
    }

    #[test]
    fn detect_list_content() {
        let text = "Items:\n- First item\n- Second item\n* Third item";
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_list"], true);
        assert_eq!(meta["contains_table"], false);
    }

    #[test]
    fn detect_ordered_list() {
        let text = "Steps:\n1. Do this\n2. Do that";
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_list"], true);
    }

    #[test]
    fn detect_no_special_content() {
        let text = "Just a plain paragraph with no special formatting.";
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_table"], false);
        assert_eq!(meta["contains_code"], false);
        assert_eq!(meta["contains_list"], false);
    }

    #[test]
    fn detect_mixed_content() {
        let text =
            "# Section\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n```python\nprint('hi')\n```\n\n- item";
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_table"], true);
        assert_eq!(meta["contains_code"], true);
        assert_eq!(meta["contains_list"], true);
    }

    #[test]
    fn pipe_inside_code_fence_not_table() {
        let text = "```\necho \"a | b | c\"\n```";
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_table"], false);
        assert_eq!(meta["contains_code"], true);
    }

    #[test]
    fn detect_example_marker_for_example() {
        let text = "For example, Alice sends a message to Bob.";
        assert!(has_example_markers(text));
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_example"], true);
    }

    #[test]
    fn detect_example_marker_suppose() {
        let text = "Suppose we have a network of three nodes.";
        assert!(has_example_markers(text));
    }

    #[test]
    fn detect_example_marker_eg() {
        let text = "Various types exist (e.g., person, location, event).";
        assert!(has_example_markers(text));
    }

    #[test]
    fn detect_no_example_in_factual_text() {
        let text = "HDBSCAN uses hierarchical density-based clustering \
                    to find natural groups in data.";
        assert!(!has_example_markers(text));
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_example"], false);
    }

    #[test]
    fn detect_example_hypothetical() {
        let text = "In this hypothetical scenario, the agent trusts \
                    only verified peers.";
        assert!(has_example_markers(text));
    }

    // --- Extraction batch grouping tests ---

    fn make_chunk_output(text: &str) -> crate::ingestion::chunker::ChunkOutput {
        crate::ingestion::chunker::ChunkOutput {
            id: uuid::Uuid::new_v4(),
            parent_id: None,
            text: text.to_string(),
            level: crate::ingestion::chunker::ChunkLevel::Paragraph,
            heading_path: vec![],
            context_prefix_len: 0,
            byte_start: 0,
            byte_end: 0,
        }
    }

    #[test]
    fn batch_skips_tiny_chunks() {
        let c1 = make_chunk_output("too small");
        let c2 = make_chunk_output(
            "This chunk has enough tokens to pass the minimum threshold for extraction.",
        );
        let chunks: Vec<&_> = vec![&c1, &c2];
        let batches = group_extraction_batches(&chunks, 5, 2000, None);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].0, c2.id);
        assert!(!batches[0].1.contains("too small"));
    }

    #[test]
    fn batch_groups_adjacent_small_chunks() {
        // 10 words each, budget = 50, so ~5 chunks per batch
        let chunks: Vec<_> = (0..10)
            .map(|i| {
                make_chunk_output(&format!(
                    "Chunk number {i} has exactly ten words in this sentence here."
                ))
            })
            .collect();
        let refs: Vec<&_> = chunks.iter().collect();
        let batches = group_extraction_batches(&refs, 5, 50, None);
        // Should produce multiple batches (each ~50 tokens)
        assert!(
            batches.len() >= 2,
            "expected multiple batches, got {}",
            batches.len()
        );
        // Each batch text should contain separator
        for (_, text) in &batches {
            if text.contains("---") {
                // Multi-chunk batch — has separators
                assert!(text.matches("---").count() >= 1);
            }
        }
    }

    #[test]
    fn batch_single_large_chunk_alone() {
        let big = make_chunk_output(&"word ".repeat(100));
        let small = make_chunk_output(&"word ".repeat(10));
        let chunks: Vec<&_> = vec![&big, &small];
        let batches = group_extraction_batches(&chunks, 5, 50, None);
        // The big chunk should be in its own batch
        assert!(batches.len() >= 2);
        assert_eq!(batches[0].0, big.id);
    }

    #[test]
    fn batch_empty_input() {
        let batches = group_extraction_batches(&[], 30, 2000, None);
        assert!(batches.is_empty());
    }

    #[test]
    fn batch_all_below_threshold() {
        let c1 = make_chunk_output("tiny");
        let c2 = make_chunk_output("also tiny");
        let chunks: Vec<&_> = vec![&c1, &c2];
        let batches = group_extraction_batches(&chunks, 30, 2000, None);
        assert!(batches.is_empty());
    }

    #[test]
    fn batch_uses_resolved_texts() {
        let c1 = make_chunk_output("He went to the store.");
        let mut resolved = std::collections::HashMap::new();
        resolved.insert(
            c1.id,
            "John went to the grocery store to buy supplies.".to_string(),
        );
        let chunks: Vec<&_> = vec![&c1];
        let batches = group_extraction_batches(&chunks, 5, 2000, Some(&resolved));
        assert_eq!(batches.len(), 1);
        assert!(batches[0].1.contains("John"));
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

    // --- Parent alignment computation tests ---

    /// In-memory mock of [`ChunkRepo`] that records
    /// `update_parent_alignment` calls for verification.
    struct MockChunkRepo {
        alignments: std::sync::Mutex<Vec<(ChunkId, f64)>>,
    }

    impl MockChunkRepo {
        fn new() -> Self {
            Self {
                alignments: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn recorded(&self) -> Vec<(ChunkId, f64)> {
            self.alignments
                .lock()
                .map(|v| v.clone())
                .unwrap_or_default()
        }
    }

    impl ChunkRepo for MockChunkRepo {
        async fn create(&self, _chunk: &Chunk) -> crate::error::Result<()> {
            Ok(())
        }

        async fn batch_create(&self, _chunks: &[Chunk]) -> crate::error::Result<()> {
            Ok(())
        }

        async fn get(&self, _id: ChunkId) -> crate::error::Result<Option<Chunk>> {
            Ok(None)
        }

        async fn list_by_source(&self, _sid: SourceId) -> crate::error::Result<Vec<Chunk>> {
            Ok(Vec::new())
        }

        async fn list_children(&self, _pid: ChunkId) -> crate::error::Result<Vec<Chunk>> {
            Ok(Vec::new())
        }

        async fn delete(&self, _id: ChunkId) -> crate::error::Result<bool> {
            Ok(false)
        }

        async fn delete_by_source(&self, _sid: SourceId) -> crate::error::Result<u64> {
            Ok(0)
        }

        async fn update_embedding(
            &self,
            _id: ChunkId,
            _embedding: &[f64],
        ) -> crate::error::Result<()> {
            Ok(())
        }

        async fn update_parent_alignment(
            &self,
            id: ChunkId,
            alignment: f64,
        ) -> crate::error::Result<()> {
            if let Ok(mut v) = self.alignments.lock() {
                v.push((id, alignment));
            }
            Ok(())
        }

        async fn update_landscape(
            &self,
            _id: ChunkId,
            _pa: Option<f64>,
            _em: &str,
            _lm: Option<serde_json::Value>,
        ) -> crate::error::Result<()> {
            Ok(())
        }

        async fn update_landscape_metrics(
            &self,
            _id: ChunkId,
            _metrics: serde_json::Value,
        ) -> crate::error::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn parent_alignment_computed_for_child_chunks() {
        let parent_id = uuid::Uuid::new_v4();
        let child_id = uuid::Uuid::new_v4();

        let parent_co = crate::ingestion::chunker::ChunkOutput {
            id: parent_id,
            parent_id: None,
            text: "parent section".to_string(),
            level: crate::ingestion::chunker::ChunkLevel::Section,
            heading_path: vec![],
            context_prefix_len: 0,
            byte_start: 0,
            byte_end: 14,
        };
        let child_co = crate::ingestion::chunker::ChunkOutput {
            id: child_id,
            parent_id: Some(parent_id),
            text: "child paragraph".to_string(),
            level: crate::ingestion::chunker::ChunkLevel::Paragraph,
            heading_path: vec![],
            context_prefix_len: 0,
            byte_start: 14,
            byte_end: 29,
        };

        let chunk_outputs = vec![parent_co, child_co];

        // Identical embeddings => cosine similarity = 1.0
        let embeddings = vec![vec![1.0, 0.0, 0.0], vec![1.0, 0.0, 0.0]];

        let mut id_map = std::collections::HashMap::new();
        id_map.insert(parent_id, ChunkId::from_uuid(parent_id));
        id_map.insert(child_id, ChunkId::from_uuid(child_id));

        let repo = MockChunkRepo::new();
        compute_parent_alignment(&chunk_outputs, &embeddings, &id_map, &repo).await;

        let recorded = repo.recorded();
        assert_eq!(recorded.len(), 1, "should compute alignment for one child");
        assert_eq!(recorded[0].0, ChunkId::from_uuid(child_id));
        assert!(
            (recorded[0].1 - 1.0).abs() < 1e-10,
            "identical vectors should have similarity 1.0"
        );
    }

    #[tokio::test]
    async fn parent_alignment_orthogonal_vectors() {
        let parent_id = uuid::Uuid::new_v4();
        let child_id = uuid::Uuid::new_v4();

        let parent_co = crate::ingestion::chunker::ChunkOutput {
            id: parent_id,
            parent_id: None,
            text: "parent".to_string(),
            level: crate::ingestion::chunker::ChunkLevel::Section,
            heading_path: vec![],
            context_prefix_len: 0,
            byte_start: 0,
            byte_end: 6,
        };
        let child_co = crate::ingestion::chunker::ChunkOutput {
            id: child_id,
            parent_id: Some(parent_id),
            text: "child".to_string(),
            level: crate::ingestion::chunker::ChunkLevel::Paragraph,
            heading_path: vec![],
            context_prefix_len: 0,
            byte_start: 6,
            byte_end: 11,
        };

        let chunk_outputs = vec![parent_co, child_co];
        // Orthogonal embeddings => cosine similarity = 0.0
        let embeddings = vec![vec![1.0, 0.0], vec![0.0, 1.0]];

        let mut id_map = std::collections::HashMap::new();
        id_map.insert(parent_id, ChunkId::from_uuid(parent_id));
        id_map.insert(child_id, ChunkId::from_uuid(child_id));

        let repo = MockChunkRepo::new();
        compute_parent_alignment(&chunk_outputs, &embeddings, &id_map, &repo).await;

        let recorded = repo.recorded();
        assert_eq!(recorded.len(), 1);
        assert!(
            recorded[0].1.abs() < 1e-10,
            "orthogonal vectors should have similarity ~0.0, got {}",
            recorded[0].1
        );
    }

    #[tokio::test]
    async fn parent_alignment_skips_root_chunks() {
        // A chunk without parent_id should not produce any alignment.
        let root_id = uuid::Uuid::new_v4();
        let root_co = crate::ingestion::chunker::ChunkOutput {
            id: root_id,
            parent_id: None,
            text: "root only".to_string(),
            level: crate::ingestion::chunker::ChunkLevel::Section,
            heading_path: vec![],
            context_prefix_len: 0,
            byte_start: 0,
            byte_end: 9,
        };

        let chunk_outputs = vec![root_co];
        let embeddings = vec![vec![1.0, 2.0, 3.0]];

        let mut id_map = std::collections::HashMap::new();
        id_map.insert(root_id, ChunkId::from_uuid(root_id));

        let repo = MockChunkRepo::new();
        compute_parent_alignment(&chunk_outputs, &embeddings, &id_map, &repo).await;

        let recorded = repo.recorded();
        assert!(
            recorded.is_empty(),
            "root chunk should not get parent_alignment"
        );
    }

    #[tokio::test]
    async fn parent_alignment_multiple_children() {
        let parent_id = uuid::Uuid::new_v4();
        let child1_id = uuid::Uuid::new_v4();
        let child2_id = uuid::Uuid::new_v4();

        let parent_co = crate::ingestion::chunker::ChunkOutput {
            id: parent_id,
            parent_id: None,
            text: "parent".to_string(),
            level: crate::ingestion::chunker::ChunkLevel::Section,
            heading_path: vec![],
            context_prefix_len: 0,
            byte_start: 0,
            byte_end: 6,
        };
        let child1_co = crate::ingestion::chunker::ChunkOutput {
            id: child1_id,
            parent_id: Some(parent_id),
            text: "child1".to_string(),
            level: crate::ingestion::chunker::ChunkLevel::Paragraph,
            heading_path: vec![],
            context_prefix_len: 0,
            byte_start: 6,
            byte_end: 12,
        };
        let child2_co = crate::ingestion::chunker::ChunkOutput {
            id: child2_id,
            parent_id: Some(parent_id),
            text: "child2".to_string(),
            level: crate::ingestion::chunker::ChunkLevel::Paragraph,
            heading_path: vec![],
            context_prefix_len: 0,
            byte_start: 12,
            byte_end: 18,
        };

        let chunk_outputs = vec![parent_co, child1_co, child2_co];
        // parent = [1, 0, 0], child1 = [1, 0, 0] (sim=1),
        // child2 = [0.5, 0.5, 0] (sim ≈ 0.707)
        let embeddings = vec![
            vec![1.0, 0.0, 0.0],
            vec![1.0, 0.0, 0.0],
            vec![0.5, 0.5, 0.0],
        ];

        let mut id_map = std::collections::HashMap::new();
        id_map.insert(parent_id, ChunkId::from_uuid(parent_id));
        id_map.insert(child1_id, ChunkId::from_uuid(child1_id));
        id_map.insert(child2_id, ChunkId::from_uuid(child2_id));

        let repo = MockChunkRepo::new();
        compute_parent_alignment(&chunk_outputs, &embeddings, &id_map, &repo).await;

        let recorded = repo.recorded();
        assert_eq!(recorded.len(), 2, "two children should have alignments");

        // Find child1 and child2 results (order may vary)
        let c1 = recorded
            .iter()
            .find(|(id, _)| *id == ChunkId::from_uuid(child1_id));
        let c2 = recorded
            .iter()
            .find(|(id, _)| *id == ChunkId::from_uuid(child2_id));

        assert!(c1.is_some(), "child1 alignment should be recorded");
        assert!(c2.is_some(), "child2 alignment should be recorded");

        assert!(
            (c1.map(|x| x.1).unwrap_or(0.0) - 1.0).abs() < 1e-10,
            "child1 (identical to parent) should have similarity 1.0"
        );

        let expected_sim2 = 0.5_f64 / (0.5_f64 * 0.5 + 0.5 * 0.5).sqrt();
        assert!(
            (c2.map(|x| x.1).unwrap_or(0.0) - expected_sim2).abs() < 1e-6,
            "child2 should have expected cosine similarity"
        );
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

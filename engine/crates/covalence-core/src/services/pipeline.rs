//! Shared ingestion pipeline — stages common to `ingest()` and
//! `reprocess()`.
//!
//! Extracted from `source.rs` to eliminate duplication. Both callers
//! prepare a [`PipelineInput`] and delegate to
//! [`SourceService::run_pipeline`] for the chunk-through-embed stages.

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::ingestion::code_chunker;
use crate::ingestion::coreference::CorefResolver;
use crate::ingestion::embedder::truncate_and_validate;
use crate::ingestion::extractor::{ExtractionContext, Extractor};
use crate::models::chunk::Chunk;
use crate::models::chunk::ChunkLevel as ModelChunkLevel;
use crate::models::edge::Edge;
use crate::models::extraction::{ExtractedEntityType, Extraction};
use crate::models::node::Node;
use crate::models::node_alias::NodeAlias;
use crate::storage::traits::{
    ChunkRepo, EdgeRepo, ExtractionRepo, NodeAliasRepo, NodeRepo, SourceRepo,
};
use crate::types::ids::{AliasId, ChunkId, NodeId, SourceId};

use super::chunk_quality::{
    has_artifact_heading, is_author_block, is_bibliography_entry, is_boilerplate_heavy,
    is_metadata_only, is_reference_section, is_title_only,
};
use super::ingestion_helpers::{
    detect_chunk_content_types, entity_name_lock_key, group_extraction_batches,
    has_example_markers, sanitize_ltree_label,
};
use super::noise_filter::is_noise_entity;
use super::source::SourceService;

/// Result of the content preparation stage (convert → parse →
/// normalize).
pub(crate) struct PreparedContent {
    /// Normalized text ready for chunking.
    pub normalized: String,
    /// SHA-256 hash of the normalized content.
    pub normalized_hash: Vec<u8>,
    /// Whether this is a code source.
    pub is_code: bool,
    /// Title extracted during parsing (if any).
    pub parsed_title: Option<String>,
    /// Metadata extracted during parsing.
    pub parsed_metadata: std::collections::HashMap<String, String>,
}

/// Input to the shared ingestion pipeline.
pub(crate) struct PipelineInput<'a> {
    /// The source ID (already created or loaded).
    pub source_id: SourceId,
    /// Source type string (e.g., "document", "code").
    pub source_type: &'a str,
    /// Optional URI for context in extraction.
    pub source_uri: Option<String>,
    /// Optional title for extraction context.
    pub source_title: Option<String>,
    /// Normalized text to chunk and process.
    pub normalized: &'a str,
    /// Whether this is a code source (skips coref).
    pub is_code: bool,
}

/// Output from the shared ingestion pipeline.
pub(crate) struct PipelineOutput {
    /// Number of chunks created.
    pub chunks_created: usize,
}

/// Provenance source for entity extraction records.
pub(crate) enum ExtractionProvenance {
    /// Entity was extracted from a chunk.
    Chunk(ChunkId),
    /// Entity was extracted from a statement.
    Statement(crate::types::ids::StatementId),
}

impl SourceService {
    /// Shared convert → parse → normalize stages.
    ///
    /// Returns the normalized text, its hash, and whether this is
    /// a code source.
    pub(crate) async fn prepare_content(
        &self,
        content: &[u8],
        mime: &str,
        uri: Option<&str>,
    ) -> Result<PreparedContent> {
        // Content-sniff: override MIME if raw content is clearly HTML
        // but was declared as something else (e.g., text/plain).
        let mime = if !mime.contains("html") && sniff_html(content) {
            tracing::debug!(
                declared_mime = mime,
                "content-sniffed as HTML, overriding MIME"
            );
            "text/html"
        } else {
            mime
        };

        let code_lang = code_chunker::detect_code_language(mime, uri);
        let is_code = code_lang.is_some();

        // Stage 1.5: Convert
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

        // Stage 3: Normalize via composable pass chain.
        //
        // The profile registry selects the right normalization chain
        // based on source type + URI (e.g., arXiv gets MathJax
        // stripping, code gets minimal normalization).
        let normalized = if !self.pipeline.normalize_enabled {
            parsed.body.clone()
        } else {
            let source_type = if is_code {
                crate::models::source::SourceType::Code
            } else {
                crate::models::source::SourceType::Document
            };
            let registry = crate::ingestion::source_profile::ProfileRegistry::new();
            let profile = registry.match_profile(&source_type, uri);
            tracing::debug!(
                profile = profile.name,
                uri = uri.unwrap_or("-"),
                "selected normalization profile"
            );
            profile.normalize_chain().run(&parsed.body)
        };

        let normalized_hash = Sha256::digest(normalized.as_bytes()).to_vec();

        Ok(PreparedContent {
            normalized,
            normalized_hash,
            is_code,
            parsed_title: parsed.title,
            parsed_metadata: parsed.metadata,
        })
    }

    /// Check for conflicting edges and invalidate any that contradict
    /// the new edge after it has been created.
    ///
    /// Queries existing active edges sharing the same `(source_node,
    /// rel_type)` and runs temporal conflict detection. Edges with
    /// different targets and overlapping temporal ranges are
    /// invalidated — superseded by the new edge.
    ///
    /// Must be called **after** `EdgeRepo::create()` so the new edge
    /// exists for the `invalidated_by` foreign key.
    async fn check_and_invalidate_conflicts(&self, edge: &Edge) -> Result<usize> {
        use crate::epistemic::invalidation::{ExistingEdgeRecord, detect_conflicts};

        let existing =
            EdgeRepo::find_by_source_and_rel_type(&*self.repo, edge.source_node_id, &edge.rel_type)
                .await?;

        if existing.is_empty() {
            return Ok(0);
        }

        // Exclude the new edge itself from the candidates.
        let records: Vec<ExistingEdgeRecord> = existing
            .iter()
            .filter(|e| e.id != edge.id)
            .map(|e| {
                (
                    e.id,
                    e.target_node_id,
                    e.valid_from,
                    e.valid_until,
                    e.invalid_at,
                )
            })
            .collect();

        let check = detect_conflicts(
            edge.id,
            edge.target_node_id,
            &edge.rel_type,
            edge.valid_from,
            &records,
        );

        let mut invalidated = 0;
        for conflict in &check.conflicts {
            tracing::info!(
                existing_edge = %conflict.existing_edge_id,
                new_edge = %conflict.new_edge_id,
                conflict_type = ?conflict.conflict_type,
                rel_type = %edge.rel_type,
                "invalidating conflicting edge"
            );
            EdgeRepo::invalidate(&*self.repo, conflict.existing_edge_id, edge.id).await?;
            invalidated += 1;
        }

        Ok(invalidated)
    }

    /// Run the shared ingestion pipeline from chunking through node
    /// embedding.
    ///
    /// Both `ingest()` and `reprocess()` delegate here after
    /// handling their caller-specific setup (source creation,
    /// dedup, supersession, or cleanup).
    pub(crate) async fn run_pipeline(&self, input: &PipelineInput<'_>) -> Result<PipelineOutput> {
        // --- Stage 4: Chunk (with small-section merging) ---
        let chunk_outputs = crate::ingestion::chunker::chunk_document_with_merge(
            input.normalized,
            self.chunk_size,
            self.chunk_overlap,
            self.min_section_size,
        );

        // Fragmentation warning (quality signal).
        if chunk_outputs.len() > 10 {
            let mut sizes: Vec<usize> = chunk_outputs.iter().map(|co| co.text.len()).collect();
            sizes.sort_unstable();
            let median = sizes[sizes.len() / 2];
            if median < 100 {
                tracing::warn!(
                    source_id = %input.source_id,
                    chunks = chunk_outputs.len(),
                    median_chars = median,
                    "excessively fragmented output"
                );
            }
        }

        // Quality filter.
        let pre_filter = chunk_outputs.len();
        let chunk_outputs: Vec<_> = chunk_outputs
            .into_iter()
            .filter(|co| {
                !is_metadata_only(&co.text)
                    && !is_title_only(&co.text)
                    && !is_boilerplate_heavy(&co.text)
                    && !is_author_block(&co.text)
                    && !has_artifact_heading(&co.heading_path)
                    && !is_bibliography_entry(&co.text)
                    && !is_reference_section(&co.text)
            })
            .collect();
        let filtered_count = pre_filter - chunk_outputs.len();
        if filtered_count > 0 {
            tracing::info!(
                source_id = %input.source_id,
                filtered = filtered_count,
                "removed low-quality chunks"
            );
        }

        let chunks_created = chunk_outputs.len();

        // Build chunk ID map.
        let mut id_map = std::collections::HashMap::new();
        for co in &chunk_outputs {
            id_map.insert(co.id, ChunkId::from_uuid(co.id));
        }

        // Build Chunk models for batch insert.
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
                    input.source_id,
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

        // Track example/hypothetical chunks for confidence dampening.
        let example_chunks: std::collections::HashSet<uuid::Uuid> = chunk_outputs
            .iter()
            .filter(|co| has_example_markers(&co.text))
            .map(|co| co.id)
            .collect();

        // --- Stage 5: Embed chunks ---
        if let Some(ref embedder) = self.embedder {
            let texts: Vec<String> = chunk_outputs.iter().map(|co| co.text.clone()).collect();
            let embeddings = embedder.embed_document_chunks(&texts).await?;

            // Store chunk embeddings.
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

            // Source-level embedding.
            let norm_owned = input.normalized.to_string();
            match embedder.embed(std::slice::from_ref(&norm_owned)).await {
                Ok(source_embeddings) => {
                    if let Some(emb) = source_embeddings.first() {
                        match truncate_and_validate(emb, self.table_dims.source, "sources") {
                            Ok(truncated) => {
                                if let Err(e) = SourceRepo::update_embedding(
                                    &*self.repo,
                                    input.source_id,
                                    &truncated,
                                )
                                .await
                                {
                                    tracing::warn!(
                                        source_id = %input.source_id,
                                        error = %e,
                                        "failed to store source embedding"
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    source_id = %input.source_id,
                                    error = %e,
                                    "source embedding dimension mismatch"
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        source_id = %input.source_id,
                        error = %e,
                        "failed to embed source"
                    );
                }
            }
        };

        // --- Stage 5.5: Heuristic coreference ---
        // Skipped for code sources.
        let mut coref_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        if !input.is_code {
            let coref_resolver = CorefResolver::new();
            let coref_links = coref_resolver.resolve(&chunk_outputs);
            for link in &coref_links {
                coref_map.insert(link.mention.to_lowercase(), link.referent.to_lowercase());
            }
        }

        // --- Stage 5.7: Neural coreference ---
        // Skipped for code sources. When the fastcoref sidecar is
        // available, each chunk's text is resolved (pronouns →
        // antecedents) and byte offset mutations are recorded in the
        // offset projection ledger for reverse-projecting entity
        // spans back to canonical source positions.
        let resolved_texts: Option<std::collections::HashMap<uuid::Uuid, String>> = if input.is_code
        {
            None
        } else if self.pipeline.coref_enabled {
            if let Some(ref coref_client) = self.coref_client {
                let extractable_indices: Vec<usize> = (0..chunk_outputs.len()).collect();

                let mut resolved = std::collections::HashMap::new();
                let mut all_ledger_entries: Vec<crate::models::projection::LedgerEntry> =
                    Vec::new();

                for &idx in &extractable_indices {
                    let co = &chunk_outputs[idx];
                    match coref_client.resolve(&co.text).await {
                        Ok(result) => {
                            tracing::debug!(
                                chunk_index = idx,
                                original_len = co.text.len(),
                                resolved_len = result.resolved.len(),
                                mutations = result.mutations.len(),
                                "neural coref resolved"
                            );
                            // Record mutations as ledger entries for offset projection.
                            // Mutation offsets from the coref client are
                            // chunk-relative. Shift by the chunk's
                            // byte_start to make them source-absolute.
                            // Skip mutations entirely within the overlap
                            // prefix (context_prefix_len) to avoid
                            // double-counting — those were already
                            // recorded for the previous chunk.
                            let base = co.byte_start;
                            let prefix = co.context_prefix_len;
                            for m in &result.mutations {
                                if m.canonical_start < prefix {
                                    continue;
                                }
                                all_ledger_entries.push(
                                    crate::models::projection::LedgerEntry::new(
                                        input.source_id,
                                        (base + m.canonical_start, base + m.canonical_end),
                                        m.canonical_token.clone(),
                                        (base + m.mutated_start, base + m.mutated_end),
                                        m.mutated_token.clone(),
                                    ),
                                );
                            }
                            resolved.insert(co.id, result.resolved);
                        }
                        Err(e) => {
                            tracing::warn!(
                                chunk_index = idx,
                                error = %e,
                                "neural coref failed, using original"
                            );
                        }
                    }
                }

                // Store ledger entries in the database for later
                // reverse projection of entity byte spans.
                if !all_ledger_entries.is_empty() {
                    use crate::storage::traits::LedgerRepo;
                    let entry_count = all_ledger_entries.len();
                    match LedgerRepo::create_batch(self.repo.as_ref(), &all_ledger_entries).await {
                        Ok(()) => {
                            tracing::info!(
                                source_id = %input.source_id,
                                entries = entry_count,
                                "stored offset projection ledger"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                source_id = %input.source_id,
                                error = %e,
                                "failed to store projection ledger"
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

        // --- Stages 6-7: Extract + resolve ---
        let ast_ext: Option<Arc<dyn Extractor>> = if input.is_code {
            Some(Arc::new(
                crate::ingestion::ast_extractor::AstExtractor::new(),
            ))
        } else {
            None
        };
        let active_extractor = if input.is_code {
            ast_ext.as_ref()
        } else {
            self.extractor.as_ref()
        };
        let extraction_method = if input.is_code { "ast" } else { "llm" };

        if let Some(extractor) = active_extractor {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(self.extract_concurrency));

            let extraction_context = Arc::new(ExtractionContext {
                source_type: Some(input.source_type.to_string()),
                source_uri: input.source_uri.clone(),
                source_title: input.source_title.clone(),
            });

            let extractable: Vec<_> = chunk_outputs.iter().collect();

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

            // Phase 2: Process extraction results sequentially.
            let mut name_to_node: std::collections::HashMap<String, NodeId> =
                std::collections::HashMap::new();

            for extraction_result in extraction_results {
                let (chunk_uuid, extraction) = match extraction_result {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "extraction failed, skipping chunk"
                        );
                        continue;
                    }
                };
                let chunk_id = id_map[&chunk_uuid];
                let is_example_chunk = example_chunks.contains(&chunk_uuid);

                for entity in &extraction.entities {
                    if is_noise_entity(&entity.name, &entity.entity_type) {
                        tracing::debug!(
                            name = entity.name.as_str(),
                            "filtered noise entity during extraction"
                        );
                        continue;
                    }
                    let node_id = self
                        .resolve_and_store_entity(
                            entity,
                            ExtractionProvenance::Chunk(chunk_id),
                            extraction_method,
                            input.source_id,
                        )
                        .await?;
                    let Some(node_id) = node_id else {
                        continue; // Deferred to Tier 5
                    };
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
                        Some(&id) => id,
                        None => continue,
                    };
                    let target_node = match name_to_node.get(&tgt_key) {
                        Some(&id) => id,
                        None => continue,
                    };

                    if source_node == target_node {
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

                    let mut edge = Edge::new(source_node, target_node, resolved_rel_type);
                    let conf = if is_example_chunk {
                        (rel.confidence * 0.5).clamp(0.0, 1.0)
                    } else {
                        rel.confidence
                    };
                    edge.confidence = conf;
                    EdgeRepo::create(&*self.repo, &edge).await?;
                    self.check_and_invalidate_conflicts(&edge).await?;

                    let ext_record = Extraction::new(
                        chunk_id,
                        ExtractedEntityType::Edge,
                        edge.id.into_uuid(),
                        extraction_method.to_string(),
                        conf,
                    );
                    ExtractionRepo::create(&*self.repo, &ext_record).await?;
                }
            }

            // --- Stage 7.5: Embed node descriptions ---
            // Only embeds nodes that don't already have an embedding.
            if let Some(ref embedder) = self.embedder {
                let node_ids: Vec<NodeId> = name_to_node.values().copied().collect();

                if !node_ids.is_empty() {
                    let mut texts: Vec<String> = Vec::with_capacity(node_ids.len());
                    let mut valid_ids: Vec<NodeId> = Vec::with_capacity(node_ids.len());

                    // Skip nodes that already have embeddings.
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
                                                "node embedding dim mismatch"
                                            );
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    node_count = valid_ids.len(),
                                    error = %e,
                                    "failed to embed node descriptions"
                                );
                            }
                        }
                    }
                }
            }
        }

        Ok(PipelineOutput { chunks_created })
    }

    /// Resolve an extracted entity against the graph and store it.
    ///
    /// If a resolver is available, uses it to match against existing
    /// nodes. Otherwise creates a new node. Returns the `NodeId`.
    ///
    /// Uses a transaction-scoped advisory lock to prevent concurrent
    /// workers from duplicating nodes.
    pub(crate) async fn resolve_and_store_entity(
        &self,
        entity: &crate::ingestion::extractor::ExtractedEntity,
        provenance: ExtractionProvenance,
        extraction_method: &str,
        source_id: SourceId,
    ) -> Result<Option<NodeId>> {
        use crate::ingestion::resolver::MatchType;
        use sqlx::Row;

        let lock_key = entity_name_lock_key(&entity.name);

        let mut tx = self.repo.pool().begin().await?;

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
                MatchType::Deferred => {
                    // Route to unresolved_entities pool for Tier 5 HDBSCAN.
                    let mut unresolved = crate::models::unresolved_entity::UnresolvedEntity::new(
                        source_id,
                        entity.name.clone(),
                        entity.entity_type.clone(),
                        entity.confidence,
                    );
                    unresolved.description = entity.description.clone();
                    match &provenance {
                        ExtractionProvenance::Chunk(cid) => {
                            unresolved.chunk_id = Some(cid.into_uuid());
                        }
                        ExtractionProvenance::Statement(sid) => {
                            unresolved.statement_id = Some(sid.into_uuid());
                        }
                    }
                    use crate::storage::traits::UnresolvedEntityRepo;
                    UnresolvedEntityRepo::create(self.repo.as_ref(), &unresolved).await?;
                    tx.commit().await?;
                    tracing::debug!(
                        entity_name = %entity.name,
                        "deferred entity to Tier 5 pool"
                    );
                    return Ok(None);
                }
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

        // Only create aliases from vector matches (high confidence).
        // Fuzzy matches are too unreliable to persist as aliases —
        // short names like "EC" would get aliased to "Error Correction"
        // and then pollute future resolution (#122).
        if let (MatchType::Vector, ExtractionProvenance::Chunk(chunk_id)) =
            (&match_type, &provenance)
        {
            self.ensure_alias_in_tx(&mut tx, &entity.name, node_id, *chunk_id)
                .await?;
        }

        // For code entities resolved to existing nodes: compare
        // ast_hash. If the hash changed, clear semantic_summary so
        // the node gets re-summarized. Always update ast_hash.
        if !matches!(match_type, MatchType::New) {
            if let Some(new_hash) = entity
                .metadata
                .as_ref()
                .and_then(|m| m.get("ast_hash"))
                .and_then(|v| v.as_str())
            {
                let row: Option<(Option<serde_json::Value>,)> =
                    sqlx::query_as("SELECT properties FROM nodes WHERE id = $1")
                        .bind(node_id)
                        .fetch_optional(&mut *tx)
                        .await?;
                let old_hash = row
                    .as_ref()
                    .and_then(|(p,)| p.as_ref())
                    .and_then(|p| p.get("ast_hash"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if old_hash != new_hash {
                    // Hash changed → clear semantic_summary, update
                    // ast_hash and description.
                    let mut props = row
                        .and_then(|(p,)| p)
                        .unwrap_or_else(|| serde_json::json!({}));
                    props["ast_hash"] = serde_json::json!(new_hash);
                    props.as_object_mut().map(|o| o.remove("semantic_summary"));
                    sqlx::query(
                        "UPDATE nodes SET properties = $2, \
                             description = COALESCE($3, description) \
                         WHERE id = $1",
                    )
                    .bind(node_id)
                    .bind(&props)
                    .bind(&entity.description)
                    .execute(&mut *tx)
                    .await?;
                    tracing::debug!(
                        node = %entity.name,
                        "ast_hash changed, cleared semantic_summary"
                    );
                } else {
                    // Hash unchanged — just update ast_hash (idempotent).
                    sqlx::query(
                        "UPDATE nodes \
                         SET properties = jsonb_set(\
                             COALESCE(properties, '{}'), \
                             '{ast_hash}', $2::jsonb) \
                         WHERE id = $1",
                    )
                    .bind(node_id)
                    .bind(serde_json::json!(new_hash))
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }

        let ext_id = uuid::Uuid::new_v4();
        match provenance {
            ExtractionProvenance::Chunk(chunk_id) => {
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
            }
            ExtractionProvenance::Statement(stmt_id) => {
                sqlx::query(
                    "INSERT INTO extractions (
                        id, statement_id, entity_type, entity_id,
                        extraction_method, confidence, is_superseded
                    ) VALUES ($1, $2, $3, $4, $5, $6, false)",
                )
                .bind(ext_id)
                .bind(stmt_id)
                .bind("node")
                .bind(node_id.into_uuid())
                .bind(extraction_method)
                .bind(entity.confidence)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;

        Ok(Some(node_id))
    }

    /// Insert a new node inside an existing transaction.
    async fn create_node_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        entity: &crate::ingestion::extractor::ExtractedEntity,
    ) -> Result<Node> {
        let mut node = Node::new(entity.name.clone(), entity.entity_type.clone());
        node.description = entity.description.clone();

        // Merge entity metadata (e.g. ast_hash) into node properties.
        if let Some(ref meta) = entity.metadata {
            if let Some(obj) = meta.as_object() {
                if let Some(p) = node.properties.as_object_mut() {
                    for (k, v) in obj {
                        p.insert(k.clone(), v.clone());
                    }
                }
            }
        }

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

    /// Bump `mention_count` and `last_seen` for an existing node.
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

    /// Minimum alias length to prevent short-name pollution (#122).
    ///
    /// 2-letter aliases like "EC", "EL", "ER" match too many unrelated
    /// contexts. Require at least 3 characters.
    const MIN_ALIAS_LEN: usize = 3;

    /// Create an alias inside a transaction if one doesn't exist.
    ///
    /// Validates that the alias is long enough and doesn't already
    /// point to a different node (conflict avoidance).
    async fn ensure_alias_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        alias_text: &str,
        node_id: NodeId,
        chunk_id: ChunkId,
    ) -> Result<()> {
        use sqlx::Row;

        // Reject short aliases that cause noise (#122).
        if alias_text.trim().len() < Self::MIN_ALIAS_LEN {
            tracing::debug!(
                alias = alias_text,
                "skipping alias creation: too short (< {} chars)",
                Self::MIN_ALIAS_LEN,
            );
            return Ok(());
        }

        // Check if alias already exists (for any node).
        let existing: Option<uuid::Uuid> = sqlx::query(
            "SELECT node_id FROM node_aliases \
             WHERE LOWER(alias) = LOWER($1) \
             LIMIT 1",
        )
        .bind(alias_text)
        .fetch_optional(&mut **tx)
        .await?
        .map(|r| r.get("node_id"));

        match existing {
            Some(existing_node_id) if existing_node_id == node_id.into_uuid() => {
                // Already exists for this node — nothing to do.
            }
            Some(existing_node_id) => {
                // Alias already points to a different node — skip to
                // avoid conflicting aliases (#122).
                tracing::debug!(
                    alias = alias_text,
                    target_node = %node_id.into_uuid(),
                    existing_node = %existing_node_id,
                    "skipping alias creation: already points to different node",
                );
            }
            None => {
                // New alias — create it.
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
        }
        Ok(())
    }

    /// Create an alias for a node if one doesn't already exist.
    ///
    /// Same validation as `ensure_alias_in_tx`: minimum length and
    /// conflict avoidance.
    pub(crate) async fn ensure_alias(
        &self,
        alias_text: &str,
        node_id: NodeId,
        chunk_id: ChunkId,
    ) -> Result<()> {
        // Reject short aliases (#122).
        if alias_text.trim().len() < Self::MIN_ALIAS_LEN {
            return Ok(());
        }

        let existing = NodeAliasRepo::find_by_alias(&*self.repo, alias_text).await?;
        // Skip if alias already exists (for any node) — prevents
        // conflicting aliases pointing to different entities.
        if !existing.is_empty() {
            return Ok(());
        }
        let alias = NodeAlias {
            id: AliasId::new(),
            node_id,
            alias: alias_text.to_string(),
            source_chunk_id: Some(chunk_id),
        };
        NodeAliasRepo::create(&*self.repo, &alias).await?;
        Ok(())
    }

    /// Run entity extraction on stored statements.
    ///
    /// Reuses the existing `Extractor` trait — statements are
    /// self-contained text, so they work as extraction input without
    /// windowing or batching. Creates extraction records with
    /// `statement_id` provenance.
    pub(crate) async fn extract_entities_from_statements(
        &self,
        source_id: SourceId,
    ) -> Result<usize> {
        let extractor = match self.extractor.as_ref() {
            Some(e) => e,
            None => return Ok(0),
        };

        let statements =
            crate::storage::traits::StatementRepo::list_by_source(&*self.repo, source_id).await?;
        if statements.is_empty() {
            return Ok(0);
        }

        let extraction_context = Arc::new(ExtractionContext {
            source_type: None,
            source_uri: None,
            source_title: None,
        });

        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.extract_concurrency));
        let mut entity_count = 0usize;
        let mut name_to_node: std::collections::HashMap<String, NodeId> =
            std::collections::HashMap::new();

        // Extract from each statement (they're already self-contained).
        let batch_futures: Vec<_> = statements
            .iter()
            .filter(|s| !s.is_evicted)
            .map(|stmt| {
                let sem = Arc::clone(&semaphore);
                let ext = Arc::clone(extractor);
                let ctx = Arc::clone(&extraction_context);
                let content = stmt.content.clone();
                let stmt_id = stmt.id;
                async move {
                    let _permit = sem
                        .acquire()
                        .await
                        .map_err(|_| Error::Ingestion("extraction semaphore closed".to_string()))?;
                    let result = ext.extract(&content, &ctx).await?;
                    Ok::<_, Error>((stmt_id, result))
                }
            })
            .collect();

        let results = futures::future::join_all(batch_futures).await;

        for result in results {
            let (stmt_id, extraction) = match result {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "statement entity extraction failed, skipping"
                    );
                    continue;
                }
            };

            for entity in &extraction.entities {
                if is_noise_entity(&entity.name, &entity.entity_type) {
                    continue;
                }
                let node_id = self
                    .resolve_and_store_entity(
                        entity,
                        ExtractionProvenance::Statement(stmt_id),
                        "llm_statement",
                        source_id,
                    )
                    .await?;
                let Some(node_id) = node_id else {
                    continue; // Deferred to Tier 5
                };
                name_to_node.insert(entity.name.to_lowercase(), node_id);
                entity_count += 1;
            }

            for rel in &extraction.relationships {
                let src_key = rel.source_name.to_lowercase();
                let tgt_key = rel.target_name.to_lowercase();

                let source_node = match name_to_node.get(&src_key) {
                    Some(&id) => id,
                    None => continue,
                };
                let target_node = match name_to_node.get(&tgt_key) {
                    Some(&id) => id,
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
                edge.confidence = rel.confidence;
                EdgeRepo::create(&*self.repo, &edge).await?;
                self.check_and_invalidate_conflicts(&edge).await?;

                let ext_record = Extraction::from_statement(
                    stmt_id,
                    ExtractedEntityType::Edge,
                    edge.id.into_uuid(),
                    "llm_statement".to_string(),
                    rel.confidence,
                );
                ExtractionRepo::create(&*self.repo, &ext_record).await?;
            }
        }

        // Embed new node descriptions.
        if !name_to_node.is_empty() {
            if let Some(ref embedder) = self.embedder {
                let node_ids: Vec<NodeId> = name_to_node.values().copied().collect();
                let has_embedding_ids: Vec<uuid::Uuid> = sqlx::query_scalar::<_, uuid::Uuid>(
                    "SELECT id FROM nodes \
                     WHERE id = ANY($1) AND embedding IS NOT NULL",
                )
                .bind(node_ids.iter().map(|n| n.into_uuid()).collect::<Vec<_>>())
                .fetch_all(self.repo.pool())
                .await
                .unwrap_or_default();

                let mut texts = Vec::new();
                let mut valid_ids = Vec::new();
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
                    if let Ok(embeddings) = embedder.embed(&texts).await {
                        for (nid, emb) in valid_ids.iter().zip(embeddings.iter()) {
                            if let Ok(truncated) =
                                truncate_and_validate(emb, self.table_dims.node, "nodes")
                            {
                                let _ =
                                    NodeRepo::update_embedding(&*self.repo, *nid, &truncated).await;
                            }
                        }
                    }
                }
            }
        }

        tracing::info!(
            source_id = %source_id,
            entities = entity_count,
            "entity extraction from statements complete"
        );

        Ok(entity_count)
    }
}

/// Check if raw content looks like HTML by inspecting leading bytes.
///
/// Skips whitespace/BOM then checks for `<!DOCTYPE` or `<html`.
fn sniff_html(content: &[u8]) -> bool {
    // Skip BOM + whitespace.
    let trimmed = content
        .iter()
        .position(|&b| !b.is_ascii_whitespace() && b != 0xEF && b != 0xBB && b != 0xBF)
        .map(|pos| &content[pos..])
        .unwrap_or(content);

    let prefix: Vec<u8> = trimmed
        .iter()
        .take(15)
        .map(|b| b.to_ascii_lowercase())
        .collect();
    prefix.starts_with(b"<!doctype") || prefix.starts_with(b"<html")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_html_detects_doctype() {
        assert!(sniff_html(b"<!DOCTYPE html>\n<html>"));
        assert!(sniff_html(b"<!doctype html>"));
    }

    #[test]
    fn sniff_html_detects_html_tag() {
        assert!(sniff_html(b"<html lang=\"en\">"));
        assert!(sniff_html(b"  <html>"));
    }

    #[test]
    fn sniff_html_rejects_markdown() {
        assert!(!sniff_html(b"# Heading\n\nSome text"));
        assert!(!sniff_html(b"Hello world"));
    }

    #[test]
    fn sniff_html_skips_bom() {
        assert!(sniff_html(b"\xEF\xBB\xBF<!DOCTYPE html>"));
    }

    #[test]
    fn sniff_html_empty_content() {
        assert!(!sniff_html(b""));
        assert!(!sniff_html(b"   "));
    }
}

//! Shared ingestion pipeline — stages common to `ingest()` and
//! `reprocess()`.
//!
//! Extracted from `source.rs` to eliminate duplication. Both callers
//! prepare a [`PipelineInput`] and delegate to
//! [`SourceService::run_pipeline`] for the chunk-through-embed stages.
//!
//! Submodules:
//!   - [`types`] — pipeline data types
//!   - [`content`] — content preparation + conflict detection
//!   - [`entity_resolution`] — resolve entities, create nodes, manage aliases
//!   - [`statement_extraction`] — entity extraction from stored statements

mod content;
mod entity_resolution;
mod statement_extraction;
mod types;

pub(crate) use types::{ExtractionProvenance, PipelineInput, PipelineOutput};

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::ingestion::coreference::CorefResolver;
use crate::ingestion::embedder::truncate_and_validate;
use crate::ingestion::extractor::{ExtractionContext, Extractor};
use crate::models::chunk::Chunk;
use crate::models::chunk::ChunkLevel as ModelChunkLevel;
use crate::models::edge::Edge;
use crate::models::extraction::{ExtractedEntityType, Extraction};
use crate::storage::traits::{ChunkRepo, EdgeRepo, ExtractionRepo, NodeRepo, SourceRepo};
use crate::types::ids::{ChunkId, NodeId};

use super::chunk_quality::{
    has_artifact_heading, is_author_block, is_bibliography_entry, is_boilerplate_heavy,
    is_metadata_only, is_reference_section, is_title_only,
};
use super::ingestion_helpers::{
    detect_chunk_content_types, group_extraction_batches, has_example_markers,
    sanitize_ltree_label,
};
use super::noise_filter::is_noise_entity;
use super::source::SourceService;

impl SourceService {
    /// Run the shared ingestion pipeline from chunking through node
    /// embedding.
    ///
    /// Both `ingest()` and `reprocess()` delegate here after
    /// handling their caller-specific setup (source creation,
    /// dedup, supersession, or cleanup).
    pub(crate) async fn run_pipeline(&self, input: &PipelineInput<'_>) -> Result<PipelineOutput> {
        // Resolve source profile for per-type chunk parameters.
        let source_type_enum = crate::models::source::SourceType::from_str_opt(input.source_type)
            .unwrap_or(crate::models::source::SourceType::Document);
        let registry = crate::ingestion::source_profile::ProfileRegistry::new();
        let profile = registry.match_profile(&source_type_enum, input.source_uri.as_deref());
        tracing::debug!(
            profile = profile.name,
            chunk_size = profile.chunk_size,
            chunk_overlap = profile.chunk_overlap,
            "resolved source profile for chunking"
        );

        // --- Stage 4: Chunk (with small-section merging) ---
        let chunk_outputs = crate::ingestion::chunker::chunk_document_with_merge(
            input.normalized,
            profile.chunk_size,
            profile.chunk_overlap,
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
                            input.source_domain.as_deref(),
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

            // --- Stage 7.25: Semantic summaries for code entities ---
            // (Spec 12, Stage 2) For code entities, generate a natural
            // language summary of what the code does and why, using the
            // chat backend. This places code entities in the same vector
            // space as prose concepts. Only runs for code sources.
            if input.is_code {
                if let Some(ref chat) = self.chat_backend {
                    let code_node_ids: Vec<NodeId> = name_to_node.values().copied().collect();
                    let mut summarized = 0usize;
                    for &nid in &code_node_ids {
                        let node = match NodeRepo::get(&*self.repo, nid).await? {
                            Some(n) => n,
                            None => continue,
                        };
                        // Skip nodes that already have a semantic summary
                        if node
                            .properties
                            .get("semantic_summary")
                            .and_then(|v| v.as_str())
                            .is_some_and(|s| !s.is_empty())
                        {
                            continue;
                        }
                        // Get source code from the chunk that best matches
                        // this entity. Prefer chunks whose content contains
                        // the entity name (exact match) over just highest
                        // confidence — a function "search" should get the
                        // chunk containing "fn search", not an unrelated
                        // chunk from the same source.
                        // Find the chunk that contains the *definition* of
                        // this entity, not just a mention. For functions,
                        // look for "fn name"; for structs, "struct name";
                        // etc. This prevents getting a chunk that merely
                        // references the entity name.
                        let def_pattern = match node.node_type.as_str() {
                            "function" => format!("fn {}", node.canonical_name),
                            "struct" => format!("struct {}", node.canonical_name),
                            "enum" => format!("enum {}", node.canonical_name),
                            "trait" => format!("trait {}", node.canonical_name),
                            "impl_block" => format!("impl {}", node.canonical_name),
                            "module" => format!("mod {}", node.canonical_name),
                            "constant" => format!("const {}", node.canonical_name),
                            "macro" => format!("macro_rules! {}", node.canonical_name),
                            _ => node.canonical_name.clone(),
                        };

                        // First try: extraction-linked chunks with
                        // definition pattern match.
                        let mut chunk_content: Option<String> = sqlx::query_scalar(
                            "SELECT c.content FROM extractions ex \
                             JOIN chunks c ON c.id = ex.chunk_id \
                             WHERE ex.entity_id = $1 AND ex.entity_type = 'node' \
                               AND ex.chunk_id IS NOT NULL \
                               AND c.content LIKE '%' || $2 || '%' \
                             ORDER BY ex.confidence DESC \
                             LIMIT 1",
                        )
                        .bind(nid)
                        .bind(&def_pattern)
                        .fetch_optional(self.repo.pool())
                        .await
                        .ok()
                        .flatten();

                        // Fallback: search ALL chunks from the same source
                        // for the definition pattern. Handles cases where
                        // the entity was resolved to an existing node but
                        // the extraction points to a different chunk.
                        if chunk_content.is_none() {
                            chunk_content = sqlx::query_scalar(
                                "SELECT c.content FROM chunks c \
                                 WHERE c.source_id = $1 \
                                   AND c.content LIKE '%' || $2 || '%' \
                                 ORDER BY LENGTH(c.content) ASC \
                                 LIMIT 1",
                            )
                            .bind(input.source_id)
                            .bind(&def_pattern)
                            .fetch_optional(self.repo.pool())
                            .await
                            .ok()
                            .flatten();
                        }

                        let raw = chunk_content
                            .as_deref()
                            .or(node.description.as_deref())
                            .unwrap_or(&node.canonical_name);

                        // Skip very short code (bare signatures without
                        // bodies aren't worth summarizing).
                        if raw.len() < 50 {
                            continue;
                        }

                        let file_path = node
                            .properties
                            .get("file_path")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");

                        let prompt = super::prompts::build_summary_prompt(
                            &node.canonical_name,
                            &node.node_type,
                            file_path,
                            raw,
                        );

                        match chat.chat("", &prompt, false, 0.2).await {
                            Ok(resp) if !resp.text.trim().is_empty() => {
                                let summary = resp.text.trim().to_string();
                                // Store summary in properties and update
                                // description for embedding.
                                sqlx::query(
                                    "UPDATE nodes SET \
                                       properties = jsonb_set(\
                                         COALESCE(properties, '{}'), \
                                         '{semantic_summary}', \
                                         $2::jsonb\
                                       ), \
                                       description = $3, \
                                       embedding = NULL \
                                     WHERE id = $1",
                                )
                                .bind(nid)
                                .bind(serde_json::json!(summary))
                                .bind(&summary)
                                .execute(self.repo.pool())
                                .await?;
                                summarized += 1;
                            }
                            Ok(_) => {
                                tracing::debug!(
                                    node = %node.canonical_name,
                                    "LLM returned empty summary, skipping"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    node = %node.canonical_name,
                                    error = %e,
                                    "semantic summary generation failed"
                                );
                                // Non-fatal: continue with syntactic description
                            }
                        }
                    }
                    if summarized > 0 {
                        tracing::info!(
                            summarized,
                            total = code_node_ids.len(),
                            "generated semantic summaries for code entities"
                        );
                    }
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

        // --- Stage 7.75: Compose code source summary ---
        // For code sources, compose a file-level summary from the
        // individual entity summaries. Mirrors the prose pipeline's
        // section→source summary composition (statement_pipeline.rs).
        if input.is_code {
            if let Some(ref summary_compiler) = self.source_summary_compiler {
                // Collect entity summaries grouped by type
                let summaries: Vec<(String, String, String)> = sqlx::query_as(
                    "SELECT canonical_name, node_type, \
                            COALESCE(properties->>'semantic_summary', \
                                     description, canonical_name) \
                     FROM nodes n \
                     JOIN extractions ex ON ex.entity_id = n.id \
                       AND ex.entity_type = 'node' \
                     JOIN chunks c ON c.id = ex.chunk_id \
                     WHERE c.source_id = $1 \
                       AND n.entity_class = 'code' \
                     GROUP BY n.id, canonical_name, node_type, \
                              properties, description \
                     ORDER BY n.node_type, n.canonical_name",
                )
                .bind(input.source_id)
                .fetch_all(self.repo.pool())
                .await
                .unwrap_or_default();

                if !summaries.is_empty() {
                    use crate::ingestion::section_compiler::{
                        SectionSummaryEntry, SourceSummaryInput,
                    };

                    let section_entries: Vec<SectionSummaryEntry> = summaries
                        .iter()
                        .map(|(name, ntype, summary)| SectionSummaryEntry {
                            title: format!("{ntype}: {name}"),
                            summary: summary.clone(),
                        })
                        .collect();

                    let file_name = input
                        .source_uri
                        .as_deref()
                        .and_then(|u| u.rsplit('/').next())
                        .unwrap_or("unknown");

                    match summary_compiler
                        .compile_source_summary(&SourceSummaryInput {
                            section_summaries: section_entries,
                            source_title: Some(file_name.to_string()),
                        })
                        .await
                    {
                        Ok(compilation) if !compilation.text.trim().is_empty() => {
                            let summary = compilation.text.trim();
                            SourceRepo::update_summary(&*self.repo, input.source_id, summary)
                                .await?;

                            // Re-embed from the composed summary.
                            if let Some(ref embedder) = self.embedder {
                                if let Ok(vecs) = embedder.embed(&[summary.to_string()]).await {
                                    if let Some(emb) = vecs.first() {
                                        if let Ok(t) =
                                            crate::ingestion::embedder::truncate_and_validate(
                                                emb,
                                                self.table_dims.source,
                                                "sources",
                                            )
                                        {
                                            let _ = SourceRepo::update_embedding(
                                                &*self.repo,
                                                input.source_id,
                                                &t,
                                            )
                                            .await;
                                        }
                                    }
                                }
                            }
                            tracing::info!(
                                source_id = %input.source_id,
                                entities = summaries.len(),
                                "composed code source summary from entity summaries"
                            );
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(
                                source_id = %input.source_id,
                                error = %e,
                                "code source summary compilation failed (non-fatal)"
                            );
                        }
                    }
                }
            }
        }

        Ok(PipelineOutput { chunks_created })
    }
}

//! Source ingestion — accepting and processing new sources.

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::models::source::{Source, SourceType, UpdateClass};
use crate::storage::traits::SourceRepo;
use crate::types::ids::SourceId;

use super::SourceService;
use crate::services::ingestion_helpers::{SupersedesInfo, detect_update_class};
use crate::services::pipeline::PipelineInput;

impl SourceService {
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
        // Reject scratch-path URIs. Smoke tests against the live engine
        // have a habit of leaking files like `/tmp/copilot-smoke.md` into
        // the persistent store, where they pollute search and citation
        // results forever. If callers genuinely want to ingest a temp
        // file, they should copy it to a stable path first.
        if let Some(u) = uri
            && is_scratch_uri(u)
        {
            return Err(Error::InvalidInput(format!(
                "refusing to ingest source with scratch-path URI: {u} \
                 — copy the file to a stable location first"
            )));
        }

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

        // Multi-domain classification.
        let domains = self
            .derive_domains_via_adapter(source_type, uri, Some(mime))
            .await;
        source.domains = domains;

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

        // Validate source metadata against extension-declared schemas.
        if let Some(ref ontology) = self.ontology_service {
            let cache = ontology.get().await;
            crate::extensions::metadata::validate_source_metadata(
                &source.domains,
                &metadata,
                &cache.source_metadata_schemas,
                self.metadata_enforcement,
            )?;
        }

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
            use crate::storage::traits::PipelineRepo;
            PipelineRepo::update_source_supersession_metadata(
                &*self.repo,
                source.id,
                &serde_json::json!({
                    "old_source_id": info.old_source_id.to_string(),
                    "update_class": info.update_class.as_str(),
                }),
            )
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
        // Update status to processing via SP.
        use crate::storage::traits::PipelineRepo;
        PipelineRepo::update_source_status(&*self.repo, source_id, "processing").await?;

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

        // Fire pre_ingest hooks (can skip extraction or override domain).
        let mut effective_domain = source.domains.first().cloned();
        let mut skip_extraction = false;
        if let Some(ref hook_svc) = self.hook_service {
            // Send a content preview (first 500 chars) so hooks can
            // make classification decisions without the full payload.
            let preview = normalized.chars().take(500).collect::<String>();
            let preview_ref = if preview.is_empty() {
                None
            } else {
                Some(preview.as_str())
            };
            match hook_svc
                .fire_pre_ingest(
                    &source_id,
                    source_type,
                    effective_domain.as_deref(),
                    preview_ref,
                )
                .await
            {
                Ok(resp) => {
                    if resp.skip_extraction == Some(true) {
                        skip_extraction = true;
                        tracing::info!(
                            source_id = %source_id,
                            "pre_ingest hook requested skip_extraction"
                        );
                    }
                    if let Some(domain) = resp.override_domain {
                        tracing::info!(
                            source_id = %source_id,
                            old_domain = ?effective_domain,
                            new_domain = %domain,
                            "pre_ingest hook overrode domain"
                        );
                        effective_domain = Some(domain);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        source_id = %source_id,
                        error = %e,
                        "pre_ingest hook failed (continuing)"
                    );
                }
            }
        }

        // Run chunking + embedding only. Extraction fans out to
        // per-chunk queue jobs via the async DAG.
        self.run_pipeline(&PipelineInput {
            source_id,
            source_type,
            source_uri: source.uri.clone(),
            source_title: source.title.clone(),
            source_domain: effective_domain.clone(),
            normalized,
            is_code,
            chunk_only: true,
        })
        .await?;

        // Fan out: enqueue per-chunk extraction jobs.
        // The DAG: ExtractChunk → (fan-in) → SummarizeEntity
        //        → (fan-in) → ComposeSourceSummary → complete
        let chunks: Vec<(uuid::Uuid,)> =
            PipelineRepo::list_chunk_ids_by_source(&*self.repo, source_id).await?;

        let _enqueued = if skip_extraction {
            tracing::info!(
                source_id = %source_id,
                chunks = chunks.len(),
                "skipping extraction fan-out (pre_ingest hook)"
            );
            0
        } else {
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
                    max_attempts: 5,
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
            enqueued
        };

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

                    // Cascade cleanup via SP: cancels pending jobs,
                    // deletes extractions, statements, sections,
                    // unresolved entities, clears alias refs, deletes
                    // chunks, ledger, and clears source embedding.
                    let cascade: (i64, i64, i64, i64, i64, i64, i64) =
                        PipelineRepo::delete_source_cascade(&*self.repo, old_id).await?;
                    tracing::info!(
                        old_source = %old_id,
                        ext_deleted = cascade.0,
                        stmts_deleted = cascade.1,
                        sects_deleted = cascade.2,
                        aliases_cleared = cascade.3,
                        chunks_deleted = cascade.4,
                        ledger_deleted = cascade.5,
                        jobs_cancelled = cascade.6,
                        "cleaned up superseded source via SP"
                    );
                }
            }
        }

        // Statement pipeline for prose sources.
        if self.pipeline.statement_enabled && !is_code {
            if let Some(ref stmt_extractor) = self.statement_extractor {
                use crate::services::statement_pipeline::{
                    StatementPipelineInput, run_statement_pipeline,
                };
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
                            .extract_entities_from_statements(
                                source_id,
                                source.domains.first().map(|s| s.as_str()),
                            )
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
        // If there are no chunks (empty source) or extraction was
        // skipped by a hook, mark complete now.
        if chunks.is_empty() || skip_extraction {
            PipelineRepo::update_source_status(&*self.repo, source_id, "complete").await?;
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
        use crate::storage::traits::PipelineRepo;

        let row = PipelineRepo::find_source_by_uri(&*self.repo, uri).await?;

        let (old_id, old_content, old_version) = match row {
            Some(r) => r,
            None => return Ok(None),
        };

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
        use crate::storage::traits::PipelineRepo;
        PipelineRepo::mark_source_superseded(&*self.repo, old_id, new_id, update_class.as_str())
            .await?;
        Ok(())
    }
}

/// Returns true if the URI points at a scratch / temp location that should
/// not be persisted into the knowledge base. Currently catches `/tmp/` and
/// `file:///tmp/` prefixes.
fn is_scratch_uri(uri: &str) -> bool {
    const PREFIXES: &[&str] = &["/tmp/", "file:///tmp/"];
    PREFIXES.iter().any(|p| uri.starts_with(p))
}

#[cfg(test)]
mod scratch_uri_tests {
    use super::is_scratch_uri;

    #[test]
    fn rejects_bare_tmp() {
        assert!(is_scratch_uri("/tmp/copilot-smoke.md"));
    }

    #[test]
    fn rejects_file_scheme_tmp() {
        assert!(is_scratch_uri("file:///tmp/copilot-smoke.md"));
    }

    #[test]
    fn allows_normal_paths() {
        assert!(!is_scratch_uri("file:///home/user/notes.md"));
        assert!(!is_scratch_uri("https://example.com/paper.pdf"));
        assert!(!is_scratch_uri("file://engine/crates/foo.rs"));
    }

    #[test]
    fn allows_tmp_substring_in_other_paths() {
        // Don't false-positive on legitimate paths that happen to contain "tmp".
        assert!(!is_scratch_uri("file:///home/attempts/log.md"));
        assert!(!is_scratch_uri("file:///opt/temp_keeper.md"));
    }
}

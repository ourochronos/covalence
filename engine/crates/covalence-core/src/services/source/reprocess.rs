//! Source reprocessing — re-running the pipeline on existing sources.

use crate::error::{Error, Result};
use crate::services::pipeline::PipelineInput;
use crate::storage::traits::{ChunkRepo, ExtractionRepo, LedgerRepo, NodeAliasRepo, SourceRepo};
use crate::types::ids::SourceId;

use super::SourceService;

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

impl SourceService {
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
                use crate::services::statement_pipeline::{
                    StatementPipelineInput, reextract_statements,
                };
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
}

//! Statement pipeline — windowed statement extraction, dedup,
//! clustering, section compilation, and storage.
//!
//! Runs alongside the existing chunk pipeline when
//! `PipelineConfig.statement_enabled` is true. Extracts atomic
//! statements from normalized source text via windowed LLM calls,
//! deduplicates across windows by content hash, clusters by
//! embedding similarity, compiles sections and source summary,
//! embeds everything, and stores it all.

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::config::TableDimensions;
use crate::error::Result;
use crate::ingestion::embedder::Embedder;
use crate::ingestion::section_compiler::{
    SectionCompilationInput, SectionCompiler, SectionSummaryEntry, SourceSummaryCompiler,
    SourceSummaryInput,
};
use crate::ingestion::statement_cluster::{ClusterConfig, cluster_statements};
use crate::ingestion::statement_extractor::StatementExtractor;
use crate::models::section::Section;
use crate::models::statement::Statement;
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{SectionRepo, SourceRepo, StatementRepo};
use crate::types::ids::SourceId;

/// Input for the statement pipeline.
pub struct StatementPipelineInput<'a> {
    /// Source ID.
    pub source_id: SourceId,
    /// Normalized source text (statements are extracted from this).
    pub normalized_text: &'a str,
    /// Source title (passed to LLM for context).
    pub source_title: Option<&'a str>,
    /// Window size in characters for extraction.
    pub window_chars: usize,
    /// Overlap between windows in characters.
    pub window_overlap: usize,
}

/// Result of the statement pipeline.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StatementPipelineResult {
    /// Number of statements extracted.
    pub statements_created: usize,
    /// Number of duplicate statements removed.
    pub duplicates_removed: usize,
    /// Number of sections created.
    pub sections_created: usize,
    /// Whether a source summary was generated.
    pub source_summary_generated: bool,
}

/// Run the full statement extraction pipeline.
///
/// 1. Window over normalized source text
/// 2. Extract atomic statements from each window via LLM
/// 3. Adjust byte offsets to source-level positions
/// 4. Deduplicate across windows by content hash
/// 5. Embed all statements
/// 6. Store statements in database
/// 7. Cluster statements by embedding similarity (HAC)
/// 8. Compile each cluster into a section (title + summary) via LLM
/// 9. Embed sections
/// 10. Store sections, assign statements to sections
/// 11. Compile source summary from section summaries
/// 12. Store source summary
pub async fn run_statement_pipeline(
    repo: &Arc<PgRepo>,
    statement_extractor: &Arc<dyn StatementExtractor>,
    embedder: Option<&Arc<dyn Embedder>>,
    table_dims: &TableDimensions,
    input: &StatementPipelineInput<'_>,
    section_compiler: Option<&Arc<dyn SectionCompiler>>,
    source_summary_compiler: Option<&Arc<dyn SourceSummaryCompiler>>,
) -> Result<StatementPipelineResult> {
    let text = input.normalized_text;
    if text.trim().is_empty() {
        return Ok(StatementPipelineResult {
            statements_created: 0,
            duplicates_removed: 0,
            sections_created: 0,
            source_summary_generated: false,
        });
    }

    // Stage 1: Window and extract.
    let windows = compute_windows(text, input.window_chars, input.window_overlap);
    tracing::info!(
        source_id = %input.source_id,
        windows = windows.len(),
        text_len = text.len(),
        "starting statement extraction"
    );

    let mut all_raw_statements = Vec::new();
    for (window_start, window_end) in &windows {
        let window_text = &text[*window_start..*window_end];
        let result = statement_extractor
            .extract(window_text, input.source_title)
            .await?;

        for stmt in result.statements {
            // Adjust byte offsets to source-level positions.
            let adjusted_start = window_start + stmt.byte_start.min(window_text.len());
            let adjusted_end = window_start + stmt.byte_end.min(window_text.len());

            all_raw_statements.push((stmt, adjusted_start, adjusted_end));
        }
    }

    // Stage 2: Deduplicate by content hash.
    let mut seen_hashes = std::collections::HashSet::new();
    let mut unique_statements = Vec::new();
    let mut duplicates_removed = 0;

    for (stmt, start, end) in all_raw_statements {
        let hash = compute_content_hash(&stmt.content);
        if seen_hashes.insert(hash.clone()) {
            unique_statements.push((stmt, start, end, hash));
        } else {
            duplicates_removed += 1;
            tracing::debug!(
                content_preview = %stmt.content.chars().take(80).collect::<String>(),
                "deduplicating overlapping statement"
            );
        }
    }

    tracing::info!(
        unique = unique_statements.len(),
        duplicates = duplicates_removed,
        "statement dedup complete"
    );

    // Stage 3: Build Statement models.
    let mut statements: Vec<Statement> = unique_statements
        .into_iter()
        .enumerate()
        .map(|(ordinal, (raw, start, end, hash))| {
            let mut stmt = Statement::new(
                input.source_id,
                raw.content,
                hash,
                start as i32,
                end as i32,
                ordinal as i32,
            )
            .with_confidence(raw.confidence);

            if let Some(path) = raw.heading_path {
                stmt = stmt.with_heading_path(path);
            }
            stmt.paragraph_index = raw.paragraph_index;

            stmt
        })
        .collect();

    // Stage 4: Embed statements.
    if let Some(embedder) = embedder {
        let texts: Vec<String> = statements.iter().map(|s| s.content.clone()).collect();
        if !texts.is_empty() {
            let embeddings: Vec<Vec<f64>> = embedder.embed(&texts).await?;
            let dim = table_dims.statement;
            for (stmt, embedding) in statements.iter_mut().zip(embeddings.into_iter()) {
                let truncated = crate::ingestion::embedder::truncate_and_validate(
                    &embedding,
                    dim,
                    "statement",
                )?;
                // Store as f32 for the model field (PG storage uses
                // update_embedding with f64 conversion).
                stmt.embedding = Some(truncated.iter().map(|v| *v as f32).collect());
            }
        }
    }

    // Stage 5: Batch insert statements.
    StatementRepo::batch_create(&**repo, &statements).await?;

    // Stage 5.1: Update embeddings (halfvec requires separate path).
    for stmt in &statements {
        if let Some(ref emb) = stmt.embedding {
            let emb_f64: Vec<f64> = emb.iter().map(|v| *v as f64).collect();
            StatementRepo::update_embedding(&**repo, stmt.id, &emb_f64).await?;
        }
    }

    let created = statements.len();
    tracing::info!(
        source_id = %input.source_id,
        statements = created,
        "statement extraction and storage complete"
    );

    // Stage 6: Cluster statements by embedding similarity.
    let mut sections_created = 0;
    let mut source_summary_generated = false;

    if let Some(compiler) = section_compiler {
        if created >= 2 {
            let embeddings_ref: Vec<Option<&[f32]>> =
                statements.iter().map(|s| s.embedding.as_deref()).collect();

            let cluster_config = ClusterConfig::default();
            let assignments = cluster_statements(&embeddings_ref, &cluster_config);

            let num_clusters = assignments.iter().copied().max().map_or(0, |m| m + 1);
            tracing::info!(
                clusters = num_clusters,
                statements = created,
                "statement clustering complete"
            );

            // Group statements by cluster.
            let mut cluster_groups: Vec<Vec<usize>> = vec![Vec::new(); num_clusters];
            for (stmt_idx, &cluster_id) in assignments.iter().enumerate() {
                cluster_groups[cluster_id].push(stmt_idx);
            }

            let mut sections: Vec<Section> = Vec::new();
            let mut section_summaries_for_source: Vec<SectionSummaryEntry> = Vec::new();

            for (ordinal, group) in cluster_groups.iter().enumerate() {
                if group.is_empty() {
                    continue;
                }

                let stmt_contents: Vec<String> = group
                    .iter()
                    .map(|&idx| statements[idx].content.clone())
                    .collect();

                // Build a context window from the source text around the
                // first and last statement byte positions.
                let context_window = build_context_window(text, &statements, group);

                let compilation_input = SectionCompilationInput {
                    statements: stmt_contents,
                    source_title: input.source_title.map(|s| s.to_string()),
                    context_window,
                };

                let output = compiler.compile_section(&compilation_input).await?;

                let stmt_ids = group.iter().map(|&idx| statements[idx].id).collect();

                let section_hash = compute_content_hash(&output.summary);
                let section = Section::new(
                    input.source_id,
                    output.title.clone(),
                    output.summary.clone(),
                    section_hash,
                    stmt_ids,
                    ordinal as i32,
                );

                // Track for source-level summary.
                section_summaries_for_source.push(SectionSummaryEntry {
                    title: output.title,
                    summary: output.summary,
                });

                sections.push(section);
            }

            // Stage 7: Embed sections.
            if let Some(embedder) = embedder {
                let section_texts: Vec<String> = sections
                    .iter()
                    .map(|s| format!("{}\n\n{}", s.title, s.summary))
                    .collect();
                if !section_texts.is_empty() {
                    let embeddings: Vec<Vec<f64>> = embedder.embed(&section_texts).await?;
                    let dim = table_dims.section;
                    for (section, embedding) in sections.iter_mut().zip(embeddings.into_iter()) {
                        let truncated = crate::ingestion::embedder::truncate_and_validate(
                            &embedding, dim, "section",
                        )?;
                        section.embedding = Some(truncated.iter().map(|v| *v as f32).collect());
                    }
                }
            }

            // Stage 8: Store sections and assign statements.
            for section in &sections {
                SectionRepo::create(&**repo, section).await?;

                // Update section embedding (halfvec path).
                if let Some(ref emb) = section.embedding {
                    let emb_f64: Vec<f64> = emb.iter().map(|v| *v as f64).collect();
                    SectionRepo::update_embedding(&**repo, section.id, &emb_f64).await?;
                }

                // Assign each member statement to this section.
                for &stmt_id in &section.statement_ids {
                    StatementRepo::assign_section(&**repo, stmt_id, section.id).await?;
                }
            }

            sections_created = sections.len();
            tracing::info!(
                source_id = %input.source_id,
                sections = sections_created,
                "section compilation and storage complete"
            );

            // Stage 9: Compile source summary from section summaries.
            if let Some(summary_compiler) = source_summary_compiler {
                if !section_summaries_for_source.is_empty() {
                    let summary_input = SourceSummaryInput {
                        section_summaries: section_summaries_for_source,
                        source_title: input.source_title.map(|s| s.to_string()),
                    };

                    let source_summary = summary_compiler
                        .compile_source_summary(&summary_input)
                        .await?;

                    if !source_summary.is_empty() {
                        SourceRepo::update_summary(&**repo, input.source_id, &source_summary)
                            .await?;

                        // Embed the source summary and store on the
                        // source's embedding field.
                        if let Some(embedder) = embedder {
                            let embs = embedder
                                .embed(std::slice::from_ref(&source_summary))
                                .await?;
                            if let Some(emb) = embs.into_iter().next() {
                                let dim = table_dims.source;
                                let truncated = crate::ingestion::embedder::truncate_and_validate(
                                    &emb, dim, "source",
                                )?;
                                SourceRepo::update_embedding(&**repo, input.source_id, &truncated)
                                    .await?;
                            }
                        }

                        source_summary_generated = true;
                        tracing::info!(
                            source_id = %input.source_id,
                            summary_len = source_summary.len(),
                            "source summary generated and stored"
                        );
                    }
                }
            }
        } // if created >= 2
    } // if let Some(compiler)

    Ok(StatementPipelineResult {
        statements_created: created,
        duplicates_removed,
        sections_created,
        source_summary_generated,
    })
}

/// Build a context window from the source text around the statement
/// byte positions in a cluster. Takes the min byte_start and max
/// byte_end with 500-char padding on each side.
fn build_context_window(text: &str, statements: &[Statement], indices: &[usize]) -> Option<String> {
    if indices.is_empty() {
        return None;
    }

    let min_start = indices
        .iter()
        .map(|&i| statements[i].byte_start as usize)
        .min()
        .unwrap_or(0);
    let max_end = indices
        .iter()
        .map(|&i| statements[i].byte_end as usize)
        .max()
        .unwrap_or(0);

    let ctx_start = min_start.saturating_sub(500);
    let ctx_end = (max_end + 500).min(text.len());

    // Align to char boundaries.
    let mut start = ctx_start;
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = ctx_end;
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }

    if start >= end || end > text.len() {
        return None;
    }

    Some(text[start..end].to_string())
}

/// Re-extract statements from a source, producing a superset.
///
/// Instead of delete + recreate, this:
/// 1. Extracts new statement set from source text
/// 2. Matches against existing statements (content hash, then
///    embedding cosine > 0.92)
/// 3. New statements → added
/// 4. Missing from re-extraction → checked against source text at
///    byte_start:byte_end. If source text still supports the claim,
///    keep it. Otherwise mark `is_evicted = true`.
///
/// Returns the number of new statements added and statements evicted.
pub async fn reextract_statements(
    repo: &Arc<PgRepo>,
    statement_extractor: &Arc<dyn StatementExtractor>,
    embedder: Option<&Arc<dyn Embedder>>,
    table_dims: &TableDimensions,
    input: &StatementPipelineInput<'_>,
    section_compiler: Option<&Arc<dyn SectionCompiler>>,
    source_summary_compiler: Option<&Arc<dyn SourceSummaryCompiler>>,
) -> Result<ReextractionResult> {
    let text = input.normalized_text;
    if text.trim().is_empty() {
        return Ok(ReextractionResult::default());
    }

    // Load existing statements.
    let existing = StatementRepo::list_by_source(&**repo, input.source_id).await?;
    let existing_hashes: std::collections::HashSet<Vec<u8>> = existing
        .iter()
        .filter(|s| !s.is_evicted)
        .map(|s| s.content_hash.clone())
        .collect();

    // Stage 1: Extract new statements.
    let windows = compute_windows(text, input.window_chars, input.window_overlap);
    let mut all_raw_statements = Vec::new();
    for (window_start, window_end) in &windows {
        let window_text = &text[*window_start..*window_end];
        let result = statement_extractor
            .extract(window_text, input.source_title)
            .await?;
        for stmt in result.statements {
            let adjusted_start = window_start + stmt.byte_start.min(window_text.len());
            let adjusted_end = window_start + stmt.byte_end.min(window_text.len());
            all_raw_statements.push((stmt, adjusted_start, adjusted_end));
        }
    }

    // Stage 2: Dedup and match against existing.
    let mut seen_hashes = std::collections::HashSet::new();
    let mut new_statements = Vec::new();
    let mut new_hashes = std::collections::HashSet::new();

    for (stmt, start, end) in all_raw_statements {
        let hash = compute_content_hash(&stmt.content);
        if !seen_hashes.insert(hash.clone()) {
            continue; // Duplicate within this extraction run.
        }
        new_hashes.insert(hash.clone());
        if existing_hashes.contains(&hash) {
            continue; // Already exists — exact match.
        }

        // Check for semantic match via embedding cosine > 0.92.
        let is_semantic_match = if let Some(embedder) = embedder {
            let embs = embedder
                .embed(std::slice::from_ref(&stmt.content))
                .await
                .unwrap_or_default();
            if let Some(new_emb) = embs.first() {
                existing.iter().filter(|s| !s.is_evicted).any(|s| {
                    s.embedding.as_ref().is_some_and(|existing_emb| {
                        let existing_f64: Vec<f64> =
                            existing_emb.iter().map(|v| *v as f64).collect();
                        cosine_similarity(new_emb, &existing_f64) > 0.92
                    })
                })
            } else {
                false
            }
        } else {
            false
        };

        if !is_semantic_match {
            new_statements.push((stmt, start, end, hash));
        }
    }

    // Stage 3: Evict missing statements.
    // A statement is "missing" if its content hash wasn't in the
    // new extraction and the source text at its byte range no longer
    // supports the content.
    let mut evicted = 0usize;
    for old_stmt in &existing {
        if old_stmt.is_evicted {
            continue;
        }
        if new_hashes.contains(&old_stmt.content_hash) {
            continue; // Still present.
        }
        // Check source text support.
        let start = old_stmt.byte_start as usize;
        let end = old_stmt.byte_end as usize;
        let supported = if start < text.len() && end <= text.len() && start < end {
            let source_window = &text[start..end];
            // Simple heuristic: if >30% of the statement words appear
            // in the source text window, consider it still supported.
            let stmt_words: Vec<&str> = old_stmt
                .content
                .split_whitespace()
                .filter(|w| w.len() > 3)
                .collect();
            if stmt_words.is_empty() {
                true
            } else {
                let matches = stmt_words
                    .iter()
                    .filter(|w| source_window.contains(*w))
                    .count();
                matches as f64 / stmt_words.len() as f64 > 0.3
            }
        } else {
            false
        };

        if !supported {
            StatementRepo::mark_evicted(&**repo, old_stmt.id).await?;
            evicted += 1;
        }
    }

    let added = new_statements.len();

    tracing::info!(
        source_id = %input.source_id,
        existing = existing.len(),
        new_added = added,
        evicted,
        "statement re-extraction complete"
    );

    // Stage 4: Build, embed, and store new statements.
    if !new_statements.is_empty() {
        let base_ordinal = existing.iter().map(|s| s.ordinal).max().unwrap_or(-1) + 1;

        let mut statements: Vec<Statement> = new_statements
            .into_iter()
            .enumerate()
            .map(|(i, (raw, start, end, hash))| {
                let mut stmt = Statement::new(
                    input.source_id,
                    raw.content,
                    hash,
                    start as i32,
                    end as i32,
                    base_ordinal + i as i32,
                )
                .with_confidence(raw.confidence);
                if let Some(path) = raw.heading_path {
                    stmt = stmt.with_heading_path(path);
                }
                stmt.paragraph_index = raw.paragraph_index;
                stmt
            })
            .collect();

        // Embed new statements.
        if let Some(embedder) = embedder {
            let texts: Vec<String> = statements.iter().map(|s| s.content.clone()).collect();
            if !texts.is_empty() {
                let embeddings = embedder.embed(&texts).await?;
                let dim = table_dims.statement;
                for (stmt, embedding) in statements.iter_mut().zip(embeddings.into_iter()) {
                    let truncated = crate::ingestion::embedder::truncate_and_validate(
                        &embedding,
                        dim,
                        "statement",
                    )?;
                    stmt.embedding = Some(truncated.iter().map(|v| *v as f32).collect());
                }
            }
        }

        // Store new statements.
        StatementRepo::batch_create(&**repo, &statements).await?;
        for stmt in &statements {
            if let Some(ref emb) = stmt.embedding {
                let emb_f64: Vec<f64> = emb.iter().map(|v| *v as f64).collect();
                StatementRepo::update_embedding(&**repo, stmt.id, &emb_f64).await?;
            }
        }
    }

    // Stage 5: Re-cluster and re-compile sections from all
    // non-evicted statements.
    if section_compiler.is_some() {
        // Delete old sections first.
        SectionRepo::delete_by_source(&**repo, input.source_id).await?;
        // Clear section assignments.
        for stmt in &existing {
            if !stmt.is_evicted && stmt.section_id.is_some() {
                StatementRepo::assign_section(
                    &**repo,
                    stmt.id,
                    crate::types::ids::SectionId::from_uuid(uuid::Uuid::nil()),
                )
                .await
                .ok(); // Best effort.
            }
        }

        // Reload all non-evicted statements for clustering.
        let all_statements = StatementRepo::list_by_source(&**repo, input.source_id).await?;
        let active: Vec<&Statement> = all_statements.iter().filter(|s| !s.is_evicted).collect();

        if active.len() >= 2 {
            if let Some(compiler) = section_compiler {
                let embeddings_ref: Vec<Option<&[f32]>> =
                    active.iter().map(|s| s.embedding.as_deref()).collect();
                let cluster_config = crate::ingestion::statement_cluster::ClusterConfig::default();
                let assignments = crate::ingestion::statement_cluster::cluster_statements(
                    &embeddings_ref,
                    &cluster_config,
                );
                let num_clusters = assignments.iter().copied().max().map_or(0, |m| m + 1);

                let mut cluster_groups: Vec<Vec<usize>> = vec![Vec::new(); num_clusters];
                for (idx, &cluster_id) in assignments.iter().enumerate() {
                    cluster_groups[cluster_id].push(idx);
                }

                let mut sections = Vec::new();
                let mut section_summaries = Vec::new();

                for (ordinal, group) in cluster_groups.iter().enumerate() {
                    if group.is_empty() {
                        continue;
                    }
                    let stmt_contents: Vec<String> =
                        group.iter().map(|&i| active[i].content.clone()).collect();
                    let compilation_input = SectionCompilationInput {
                        statements: stmt_contents,
                        source_title: input.source_title.map(|s| s.to_string()),
                        context_window: None,
                    };
                    let output = compiler.compile_section(&compilation_input).await?;
                    let stmt_ids = group.iter().map(|&i| active[i].id).collect();
                    let hash = compute_content_hash(&output.summary);
                    let section = Section::new(
                        input.source_id,
                        output.title.clone(),
                        output.summary.clone(),
                        hash,
                        stmt_ids,
                        ordinal as i32,
                    );
                    section_summaries.push(SectionSummaryEntry {
                        title: output.title,
                        summary: output.summary,
                    });
                    sections.push(section);
                }

                // Embed and store sections.
                if let Some(embedder) = embedder {
                    let section_texts: Vec<String> = sections
                        .iter()
                        .map(|s| format!("{}\n\n{}", s.title, s.summary))
                        .collect();
                    if !section_texts.is_empty() {
                        let embeddings = embedder.embed(&section_texts).await?;
                        let dim = table_dims.section;
                        for (section, emb) in sections.iter_mut().zip(embeddings.into_iter()) {
                            let truncated = crate::ingestion::embedder::truncate_and_validate(
                                &emb, dim, "section",
                            )?;
                            section.embedding = Some(truncated.iter().map(|v| *v as f32).collect());
                        }
                    }
                }

                for section in &sections {
                    SectionRepo::create(&**repo, section).await?;
                    if let Some(ref emb) = section.embedding {
                        let emb_f64: Vec<f64> = emb.iter().map(|v| *v as f64).collect();
                        SectionRepo::update_embedding(&**repo, section.id, &emb_f64).await?;
                    }
                    for &stmt_id in &section.statement_ids {
                        StatementRepo::assign_section(&**repo, stmt_id, section.id).await?;
                    }
                }

                // Source summary.
                if let Some(summary_compiler) = source_summary_compiler {
                    if !section_summaries.is_empty() {
                        let summary_input = SourceSummaryInput {
                            section_summaries,
                            source_title: input.source_title.map(|s| s.to_string()),
                        };
                        let summary = summary_compiler
                            .compile_source_summary(&summary_input)
                            .await?;
                        if !summary.is_empty() {
                            SourceRepo::update_summary(&**repo, input.source_id, &summary).await?;
                        }
                    }
                }
            }
        }
    }

    Ok(ReextractionResult { added, evicted })
}

/// Result of statement re-extraction.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ReextractionResult {
    /// Number of new statements added.
    pub added: usize,
    /// Number of statements evicted.
    pub evicted: usize,
}

/// Cosine similarity between two vectors.
fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let norm_b = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

/// Compute window boundaries for sliding-window extraction.
///
/// Returns `Vec<(start, end)>` byte positions. Windows are
/// character-aligned (never split a multi-byte character).
fn compute_windows(text: &str, window_chars: usize, overlap: usize) -> Vec<(usize, usize)> {
    if text.is_empty() || window_chars == 0 {
        return Vec::new();
    }

    let mut windows = Vec::new();
    let mut start = 0;
    let text_len = text.len();

    while start < text_len {
        // Find the end position (char-aligned).
        let mut end = text_len.min(start + window_chars);
        // Adjust to char boundary.
        while end < text_len && !text.is_char_boundary(end) {
            end += 1;
        }
        windows.push((start, end));

        if end >= text_len {
            break;
        }

        // Advance by (window - overlap), char-aligned.
        let advance = window_chars.saturating_sub(overlap);
        if advance == 0 {
            break; // Prevent infinite loop.
        }
        let mut next_start = start + advance;
        while next_start < text_len && !text.is_char_boundary(next_start) {
            next_start += 1;
        }
        if next_start <= start {
            break; // No progress.
        }
        start = next_start;
    }

    windows
}

/// Compute SHA-256 hash of statement content for dedup.
fn compute_content_hash(content: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hasher.finalize().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_windows_single() {
        let text = "Hello, world!";
        let windows = compute_windows(text, 100, 10);
        assert_eq!(windows, vec![(0, 13)]);
    }

    #[test]
    fn compute_windows_multiple() {
        let text = "a".repeat(100);
        let windows = compute_windows(&text, 40, 10);
        // Window 0: 0..40
        // Window 1: 30..70
        // Window 2: 60..100
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0], (0, 40));
        assert_eq!(windows[1], (30, 70));
        assert_eq!(windows[2], (60, 100));
    }

    #[test]
    fn compute_windows_empty() {
        assert!(compute_windows("", 40, 10).is_empty());
    }

    #[test]
    fn compute_windows_exact_fit() {
        let text = "a".repeat(40);
        let windows = compute_windows(&text, 40, 10);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0], (0, 40));
    }

    #[test]
    fn compute_windows_no_overlap() {
        let text = "a".repeat(80);
        let windows = compute_windows(&text, 40, 0);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0], (0, 40));
        assert_eq!(windows[1], (40, 80));
    }

    #[test]
    fn content_hash_deterministic() {
        let h1 = compute_content_hash("test");
        let h2 = compute_content_hash("test");
        assert_eq!(h1, h2);

        let h3 = compute_content_hash("different");
        assert_ne!(h1, h3);
    }

    #[test]
    fn content_hash_length() {
        let h = compute_content_hash("anything");
        assert_eq!(h.len(), 32); // SHA-256 = 32 bytes
    }

    #[test]
    fn build_context_window_basic() {
        let text = "a".repeat(2000);
        let source_id = crate::types::ids::SourceId::new();
        let stmts = vec![
            Statement::new(source_id, "s".into(), vec![], 100, 200, 0),
            Statement::new(source_id, "s".into(), vec![], 300, 400, 1),
        ];
        let ctx = build_context_window(&text, &stmts, &[0, 1]);
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        // Should span from ~0 (100-500 clamped) to ~900 (400+500).
        assert!(ctx.len() >= 900);
    }

    #[test]
    fn build_context_window_empty_indices() {
        let text = "hello";
        let ctx = build_context_window(text, &[], &[]);
        assert!(ctx.is_none());
    }
}

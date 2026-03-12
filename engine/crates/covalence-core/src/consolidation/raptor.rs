//! RAPTOR-style recursive summarization.
//!
//! Builds a tree of abstract summaries over chunks by:
//! 1. Grouping paragraph chunks by section parent
//! 2. Summarizing each group via LLM
//! 3. Embedding the summaries for vector search
//! 4. Recursing on summaries for higher-level abstractions
//!
//! Summary chunks participate in regular vector search, enabling
//! multi-resolution retrieval. See issue #74.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::TableDimensions;
use crate::error::{Error, Result};
use crate::ingestion::embedder::{Embedder, truncate_and_validate};
use crate::models::chunk::Chunk;
use crate::storage::postgres::PgRepo;
use crate::storage::traits::ChunkRepo;
use crate::types::clearance::ClearanceLevel;
use crate::types::ids::{ChunkId, SourceId};

const SUMMARY_SYSTEM_PROMPT: &str = r#"You are a summarization assistant. Given a collection of text passages from the same section of a document, produce a concise summary that captures the key concepts, findings, and relationships.

Rules:
- Write 2-4 sentences that capture the essential meaning.
- Preserve technical terminology and named entities.
- Focus on what the text says, not how it says it.
- Do not add information not present in the passages.
- Do not include meta-commentary like "This section discusses..."
- Return only the summary text, no formatting or prefixes."#;

const SOURCE_SUMMARY_PROMPT: &str = r#"You are a summarization assistant. Given summaries of different sections from the same document, produce a high-level summary that captures the document's overall themes, key findings, and contributions.

Rules:
- Write 3-5 sentences covering the main ideas across all sections.
- Preserve technical terminology and named entities.
- Capture cross-section relationships and themes.
- Do not add information not present in the summaries.
- Return only the summary text, no formatting or prefixes."#;

/// Configuration for RAPTOR summarization.
#[derive(Debug, Clone)]
pub struct RaptorConfig {
    /// Maximum recursion depth (default: 2).
    /// Level 1 = section summaries, Level 2 = source summary.
    pub max_levels: usize,

    /// Minimum number of child chunks required to generate a summary
    /// (default: 2). Sections with fewer children are skipped.
    pub min_children: usize,

    /// Maximum total characters of child text to include in the
    /// summarization prompt (default: 12000).
    pub summary_context_chars: usize,
}

impl Default for RaptorConfig {
    fn default() -> Self {
        Self {
            max_levels: 2,
            min_children: 2,
            summary_context_chars: 12_000,
        }
    }
}

/// Report from a RAPTOR summarization run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RaptorReport {
    /// Number of sources processed.
    pub sources_processed: usize,
    /// Number of sources skipped (already have summaries or too few chunks).
    pub sources_skipped: usize,
    /// Total summary chunks created across all sources and levels.
    pub summaries_created: usize,
    /// Number of LLM calls made.
    pub llm_calls: usize,
    /// Number of embedding calls made.
    pub embed_calls: usize,
    /// Summaries created per level (e.g. {"summary_l1": 15, "summary_l2": 3}).
    pub per_level: HashMap<String, usize>,
    /// Errors encountered (source_id → error message).
    pub errors: Vec<String>,
}

/// Orchestrates RAPTOR summarization across sources.
pub struct RaptorConsolidator {
    repo: Arc<PgRepo>,
    embedder: Arc<dyn Embedder>,
    chat_base_url: String,
    chat_api_key: String,
    chat_model: String,
    table_dims: TableDimensions,
    config: RaptorConfig,
}

impl RaptorConsolidator {
    /// Create a new RAPTOR consolidator.
    pub fn new(
        repo: Arc<PgRepo>,
        embedder: Arc<dyn Embedder>,
        chat_base_url: String,
        chat_api_key: String,
        chat_model: String,
    ) -> Self {
        Self {
            repo,
            embedder,
            chat_base_url,
            chat_api_key,
            chat_model,
            table_dims: TableDimensions::default(),
            config: RaptorConfig::default(),
        }
    }

    /// Set per-table embedding dimensions.
    pub fn with_table_dims(mut self, dims: TableDimensions) -> Self {
        self.table_dims = dims;
        self
    }

    /// Set RAPTOR configuration.
    pub fn with_config(mut self, config: RaptorConfig) -> Self {
        self.config = config;
        self
    }

    /// Summarize all sources that don't yet have summary chunks.
    pub async fn run_all_sources(&self) -> Result<RaptorReport> {
        let sources =
            crate::storage::traits::SourceRepo::list(&*self.repo, 1000, 0).await?;
        let mut report = RaptorReport::default();

        for source in &sources {
            match self.summarize_source(source.id).await {
                Ok(source_report) => {
                    if source_report.summaries_created > 0 {
                        report.sources_processed += 1;
                    } else {
                        report.sources_skipped += 1;
                    }
                    report.summaries_created += source_report.summaries_created;
                    report.llm_calls += source_report.llm_calls;
                    report.embed_calls += source_report.embed_calls;
                    for (level, count) in &source_report.per_level {
                        *report.per_level.entry(level.clone()).or_default() += count;
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        source_id = %source.id.into_uuid(),
                        error = %e,
                        "RAPTOR summarization failed for source"
                    );
                    report.errors.push(format!(
                        "{}: {}",
                        source.id.into_uuid(),
                        e
                    ));
                    report.sources_skipped += 1;
                }
            }
        }

        tracing::info!(
            processed = report.sources_processed,
            skipped = report.sources_skipped,
            summaries = report.summaries_created,
            errors = report.errors.len(),
            "RAPTOR summarization complete"
        );

        Ok(report)
    }

    /// Summarize a single source. Returns a per-source report.
    ///
    /// Idempotent: deletes existing summary chunks before regenerating.
    pub async fn summarize_source(&self, source_id: SourceId) -> Result<RaptorReport> {
        let mut report = RaptorReport::default();

        // Load all chunks for this source.
        let all_chunks = ChunkRepo::list_by_source(&*self.repo, source_id).await?;
        if all_chunks.is_empty() {
            return Ok(report);
        }

        // Delete any existing RAPTOR summary chunks (idempotent).
        let existing_summaries: Vec<ChunkId> = all_chunks
            .iter()
            .filter(|c| c.level.starts_with("summary_l"))
            .map(|c| c.id)
            .collect();
        for id in &existing_summaries {
            ChunkRepo::delete(&*self.repo, *id).await?;
        }

        // Separate non-summary chunks for processing.
        let base_chunks: Vec<&Chunk> = all_chunks
            .iter()
            .filter(|c| !c.level.starts_with("summary_l"))
            .collect();

        if base_chunks.len() < self.config.min_children {
            return Ok(report);
        }

        // --- Level 1: Summarize paragraphs grouped by section parent ---
        let l1_summaries =
            self.build_level_summaries(&base_chunks, source_id, 1, &mut report).await?;

        if l1_summaries.is_empty() {
            return Ok(report);
        }

        // --- Level 2: Summarize L1 summaries into a source summary ---
        if self.config.max_levels >= 2 && l1_summaries.len() >= self.config.min_children {
            let l1_refs: Vec<&Chunk> = l1_summaries.iter().collect();
            self.build_level_summaries(&l1_refs, source_id, 2, &mut report).await?;
        }

        Ok(report)
    }

    /// Build summary chunks for one level.
    ///
    /// Groups chunks by parent, summarizes each group, embeds, and stores.
    /// Returns the newly created summary chunks.
    async fn build_level_summaries(
        &self,
        chunks: &[&Chunk],
        source_id: SourceId,
        level: usize,
        report: &mut RaptorReport,
    ) -> Result<Vec<Chunk>> {
        let level_name = format!("summary_l{level}");
        let system_prompt = if level == 1 {
            SUMMARY_SYSTEM_PROMPT
        } else {
            SOURCE_SUMMARY_PROMPT
        };

        // Group chunks by section hierarchy.
        //
        // For L1: group paragraphs by their section path (structural_hierarchy
        //         or parent_chunk_id). The hierarchy is an ltree path like
        //         "Doc.Section.Subsection" — we group by the full path since
        //         paragraphs sharing a section share the same hierarchy.
        // For L2: all L1 summaries group together.
        let mut groups: HashMap<String, Vec<&Chunk>> = HashMap::new();
        for chunk in chunks {
            let key = if level == 1 {
                if !chunk.structural_hierarchy.is_empty() {
                    chunk.structural_hierarchy.clone()
                } else {
                    "_root".to_string()
                }
            } else {
                "_all".to_string()
            };
            groups.entry(key).or_default().push(chunk);
        }

        let mut created = Vec::new();

        for (hierarchy_key, group) in &groups {
            if group.len() < self.config.min_children {
                continue;
            }

            // Build the text to summarize (concatenate children, truncated).
            let mut combined = String::new();
            for (i, chunk) in group.iter().enumerate() {
                if combined.len() >= self.config.summary_context_chars {
                    break;
                }
                if i > 0 {
                    combined.push_str("\n\n---\n\n");
                }
                combined.push_str(&chunk.content);
            }

            // Call LLM for summary.
            tracing::debug!(
                source_id = %source_id.into_uuid(),
                level = level,
                members = group.len(),
                text_chars = combined.len(),
                "calling LLM for RAPTOR summary"
            );
            let summary_text = match self.llm_summarize(system_prompt, &combined).await {
                Ok(text) => text,
                Err(e) => {
                    tracing::warn!(
                        source_id = %source_id.into_uuid(),
                        level = level,
                        error = %e,
                        "LLM summarization failed, skipping group"
                    );
                    report.errors.push(format!("llm: {e}"));
                    continue;
                }
            };
            report.llm_calls += 1;

            if summary_text.trim().is_empty() {
                continue;
            }

            // Build the hierarchy path from the group key.
            // structural_hierarchy is ltree (dot-separated, alphanumeric + underscores).
            let hierarchy = if hierarchy_key.starts_with('_') {
                format!("summary_l{level}")
            } else {
                format!("{hierarchy_key}.summary_l{level}")
            };

            // Use the first child's parent_chunk_id as our parent if available.
            let parent_id = group.first().and_then(|c| c.parent_chunk_id);

            // Collect member chunk IDs for metadata.
            let member_ids: Vec<String> = group
                .iter()
                .map(|c| c.id.into_uuid().to_string())
                .collect();

            // Create the summary chunk.
            let content_hash = Sha256::digest(summary_text.as_bytes()).to_vec();
            let token_count = (summary_text.len() / 4) as i32; // rough estimate
            let mut chunk = Chunk {
                id: ChunkId::new(),
                source_id,
                parent_chunk_id: parent_id,
                level: level_name.clone(),
                ordinal: created.len() as i32,
                content: summary_text.clone(),
                content_hash,
                contextual_prefix: None,
                token_count,
                structural_hierarchy: hierarchy,
                clearance_level: ClearanceLevel::default(),
                parent_alignment: None,
                extraction_method: Some("raptor_summary".to_string()),
                landscape_metrics: None,
                metadata: serde_json::json!({
                    "raptor": {
                        "level": level,
                        "member_count": group.len(),
                        "member_chunk_ids": member_ids,
                        "model": self.chat_model,
                    }
                }),
                byte_start: None,
                byte_end: None,
                content_offset: None,
                created_at: Utc::now(),
            };

            // Store the chunk first (embedding UPDATE needs the row to exist).
            ChunkRepo::create(&*self.repo, &chunk).await?;

            // Embed the summary.
            match self.embedder.embed(&[summary_text]).await {
                Ok(embeddings) => {
                    if let Some(emb) = embeddings.first() {
                        let truncated =
                            truncate_and_validate(emb, self.table_dims.chunk, "chunks")?;
                        ChunkRepo::update_embedding(&*self.repo, chunk.id, &truncated).await?;
                        report.embed_calls += 1;
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        chunk_id = %chunk.id.into_uuid(),
                        error = %e,
                        "failed to embed summary chunk"
                    );
                    report.errors.push(format!("embed: {e}"));
                }
            }

            // Clear parent_alignment since it's not meaningful for summaries.
            chunk.parent_alignment = None;

            *report.per_level.entry(level_name.clone()).or_default() += 1;
            report.summaries_created += 1;
            created.push(chunk);

            tracing::debug!(
                source_id = %source_id.into_uuid(),
                level = level,
                members = group.len(),
                "created RAPTOR summary chunk"
            );
        }

        Ok(created)
    }

    /// Call an OpenAI-compatible chat completions endpoint for summarization.
    async fn llm_summarize(&self, system_prompt: &str, text: &str) -> Result<String> {
        let body = ChatRequest {
            model: &self.chat_model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: system_prompt,
                },
                ChatMessage {
                    role: "user",
                    content: text,
                },
            ],
            temperature: 0.1,
        };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| Error::Ingestion(format!("failed to build HTTP client: {e}")))?;
        let resp = client
            .post(format!("{}/chat/completions", self.chat_base_url))
            .bearer_auth(&self.chat_api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Ingestion(format!("RAPTOR summarization request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(Error::Ingestion(format!(
                "RAPTOR summarization API returned {status}: {body_text}"
            )));
        }

        let chat_resp: ChatResponse = resp
            .json()
            .await
            .map_err(|e| Error::Ingestion(format!("failed to parse summary response: {e}")))?;

        let content = chat_resp
            .choices
            .first()
            .and_then(|c| c.message.content.as_deref())
            .unwrap_or("")
            .to_string();

        Ok(content)
    }
}

// --- Minimal chat completions types (same pattern as llm_extractor) ---

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f64,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raptor_config_defaults() {
        let config = RaptorConfig::default();
        assert_eq!(config.max_levels, 2);
        assert_eq!(config.min_children, 2);
        assert_eq!(config.summary_context_chars, 12_000);
    }

    #[test]
    fn raptor_report_defaults() {
        let report = RaptorReport::default();
        assert_eq!(report.sources_processed, 0);
        assert_eq!(report.summaries_created, 0);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn raptor_report_serialization() {
        let mut report = RaptorReport {
            sources_processed: 5,
            sources_skipped: 2,
            summaries_created: 15,
            llm_calls: 15,
            embed_calls: 15,
            per_level: HashMap::new(),
            errors: vec![],
        };
        report.per_level.insert("summary_l1".into(), 12);
        report.per_level.insert("summary_l2".into(), 3);

        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["sources_processed"], 5);
        assert_eq!(json["summaries_created"], 15);
        assert_eq!(json["per_level"]["summary_l1"], 12);
    }

    #[test]
    fn summary_system_prompt_is_non_empty() {
        assert!(!SUMMARY_SYSTEM_PROMPT.is_empty());
        assert!(!SOURCE_SUMMARY_PROMPT.is_empty());
    }

    #[test]
    fn chat_request_serialization() {
        let body = ChatRequest {
            model: "gemini-2.5-flash",
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: "Summarize.",
                },
                ChatMessage {
                    role: "user",
                    content: "Some text to summarize.",
                },
            ],
            temperature: 0.1,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["model"], "gemini-2.5-flash");
        assert_eq!(json["messages"].as_array().unwrap().len(), 2);
        assert_eq!(json["temperature"], 0.1);
    }

    #[test]
    fn chat_response_deserialization() {
        let json = serde_json::json!({
            "id": "chatcmpl-raptor",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "This is a summary of the passages."
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 100, "completion_tokens": 20}
        });
        let resp: ChatResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.choices.len(), 1);
        assert_eq!(
            resp.choices[0].message.content.as_deref(),
            Some("This is a summary of the passages.")
        );
    }

    #[test]
    fn summary_level_names() {
        assert_eq!(format!("summary_l{}", 1), "summary_l1");
        assert_eq!(format!("summary_l{}", 2), "summary_l2");
    }

    #[test]
    fn content_hash_deterministic() {
        let text = "This is a summary.";
        let h1 = Sha256::digest(text.as_bytes()).to_vec();
        let h2 = Sha256::digest(text.as_bytes()).to_vec();
        assert_eq!(h1, h2);
    }
}

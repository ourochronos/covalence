//! Ask service — LLM-powered knowledge synthesis over the graph.
//!
//! Takes a natural language question, searches across all dimensions
//! to gather relevant context, enriches it with provenance and
//! confidence metadata, sends it to an LLM for grounded synthesis,
//! and returns a structured answer with citations.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::ingestion::ChatBackend;
use crate::ingestion::chat_backend::CliChatBackend;
use crate::search::fusion::FusedResult;
use crate::search::strategy::SearchStrategy;
use crate::services::SearchService;
use crate::storage::postgres::PgRepo;

/// Options controlling ask behavior.
#[derive(Debug, Clone)]
pub struct AskOptions {
    /// Maximum search results to include as context (default 15).
    pub max_context: usize,
    /// Search strategy. Default: auto.
    pub strategy: Option<String>,
    /// Override the LLM model for this request.
    /// Format: "haiku", "sonnet", "opus", "gemini", "copilot".
    /// If None, uses the default configured backend.
    pub model: Option<String>,
}

impl Default for AskOptions {
    fn default() -> Self {
        Self {
            max_context: 15,
            strategy: None,
            model: None,
        }
    }
}

/// A citation from the knowledge graph backing the answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    /// Source name or URI.
    pub source: String,
    /// Relevant snippet from the source.
    pub snippet: String,
    /// Result type (chunk, statement, section, node, etc.).
    pub result_type: String,
    /// Confidence score.
    pub confidence: f64,
}

/// Structured response from the ask endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskResponse {
    /// The synthesized answer.
    pub answer: String,
    /// Citations from the knowledge graph.
    pub citations: Vec<Citation>,
    /// Number of search results used as context.
    pub context_used: usize,
}

/// Service for answering questions via LLM synthesis over graph search.
pub struct AskService {
    search: Arc<SearchService>,
    chat: Arc<dyn ChatBackend>,
    /// Database repo — reserved for future provenance enrichment
    /// (e.g. looking up source metadata for node-type results).
    #[allow(dead_code)]
    repo: Arc<PgRepo>,
}

impl AskService {
    /// Create a new ask service.
    pub fn new(search: Arc<SearchService>, chat: Arc<dyn ChatBackend>, repo: Arc<PgRepo>) -> Self {
        Self { search, chat, repo }
    }

    /// Answer a question by searching, enriching, and synthesizing.
    pub async fn ask(&self, question: &str, options: AskOptions) -> Result<AskResponse> {
        // 1. Search for context.
        let strategy = parse_strategy(options.strategy.as_deref());
        let results = self
            .search
            .search(question, strategy, options.max_context, None)
            .await?;

        if results.is_empty() {
            return Ok(AskResponse {
                answer: "No relevant information found in the knowledge \
                         graph to answer this question."
                    .to_string(),
                citations: Vec::new(),
                context_used: 0,
            });
        }

        // 2. Enrich with provenance and build context blocks.
        let context_blocks = self.build_context_blocks(&results).await;
        let context_used = context_blocks.len();

        // 3. Build the grounded prompt.
        let system_prompt = build_system_prompt();
        let user_prompt = build_user_prompt(question, &context_blocks);

        // 4. Call LLM (use per-request model override if specified).
        let backend: Arc<dyn ChatBackend> = if let Some(ref model) = options.model {
            Arc::new(resolve_model_backend(model))
        } else {
            Arc::clone(&self.chat)
        };
        let answer = backend
            .chat(&system_prompt, &user_prompt, false, 0.3)
            .await?;

        // 5. Build citations from the search results.
        let citations = build_citations(&results, &context_blocks);

        Ok(AskResponse {
            answer,
            citations,
            context_used,
        })
    }

    /// Build numbered context blocks from search results, enriching
    /// each with source provenance metadata.
    async fn build_context_blocks(&self, results: &[FusedResult]) -> Vec<ContextBlock> {
        let mut blocks = Vec::with_capacity(results.len());
        for (i, result) in results.iter().enumerate() {
            let source_info = self.lookup_source_info(result).await.unwrap_or_default();

            let result_type = result.result_type.as_deref().unwrap_or("unknown");
            let confidence = result.confidence.unwrap_or(0.0);
            let snippet = result
                .content
                .as_deref()
                .or(result.snippet.as_deref())
                .unwrap_or("")
                .to_string();

            blocks.push(ContextBlock {
                number: i + 1,
                result_type: result_type.to_string(),
                confidence,
                source_title: source_info.title,
                source_uri: source_info.uri,
                snippet,
                fused_score: result.fused_score,
            });
        }
        blocks
    }

    /// Look up source title and URI for a search result.
    async fn lookup_source_info(&self, result: &FusedResult) -> Option<SourceInfo> {
        // Prefer pre-enriched source metadata from the result.
        let title = result.source_title.clone();
        let uri = result.source_uri.clone();
        if title.is_some() || uri.is_some() {
            return Some(SourceInfo {
                title: title.unwrap_or_default(),
                uri: uri.unwrap_or_default(),
            });
        }

        // Fall back to DB lookup for node-type results that lack
        // source metadata (nodes aren't directly tied to a single
        // source). Use the name as a fallback.
        if result.result_type.as_deref().is_some_and(|rt| rt == "node") {
            return Some(SourceInfo {
                title: result
                    .name
                    .clone()
                    .unwrap_or_else(|| "knowledge graph node".to_string()),
                uri: String::new(),
            });
        }

        // Attempt a source lookup via the source_id embedded in the
        // result's source_uri field (if it looks like a UUID). This
        // handles edge cases where enrichment didn't populate the
        // source_title.
        None
    }
}

/// Internal representation of a context fragment.
#[derive(Debug, Clone)]
struct ContextBlock {
    number: usize,
    result_type: String,
    confidence: f64,
    source_title: String,
    source_uri: String,
    snippet: String,
    /// Retained for potential ranking/filtering use.
    #[allow(dead_code)]
    fused_score: f64,
}

/// Source provenance metadata.
#[derive(Debug, Default)]
struct SourceInfo {
    title: String,
    uri: String,
}

/// Parse a strategy string to a `SearchStrategy`.
/// Resolve a model name to a CLI chat backend.
fn resolve_model_backend(model: &str) -> CliChatBackend {
    match model {
        "haiku" | "sonnet" | "opus" => CliChatBackend::new("claude".to_string(), model.to_string()),
        "gemini" => CliChatBackend::new("gemini".to_string(), "gemini-2.5-flash".to_string()),
        "copilot" => CliChatBackend::new("copilot".to_string(), "claude-haiku-4.5".to_string()),
        // Default: treat as a claude model name.
        other => CliChatBackend::new("claude".to_string(), other.to_string()),
    }
}

fn parse_strategy(s: Option<&str>) -> SearchStrategy {
    match s {
        Some("balanced") => SearchStrategy::Balanced,
        Some("precise") => SearchStrategy::Precise,
        Some("exploratory") => SearchStrategy::Exploratory,
        Some("recent") => SearchStrategy::Recent,
        Some("graph_first") => SearchStrategy::GraphFirst,
        Some("global") => SearchStrategy::Global,
        _ => SearchStrategy::Auto,
    }
}

/// Build the system prompt for grounded synthesis.
fn build_system_prompt() -> String {
    "You are a knowledge synthesis engine for the Covalence \
     project \u{2014} a hybrid GraphRAG knowledge engine. You answer \
     questions by synthesizing information from retrieved knowledge \
     fragments.\n\n\
     Rules:\n\
     - Base your answer ONLY on the provided context fragments. Do \
       not fabricate information.\n\
     - Cite sources using [1], [2], etc. matching the fragment \
       numbers.\n\
     - If the context is insufficient to fully answer the question, \
       say what you can answer and what's missing.\n\
     - Distinguish between: code behavior (from code sources), \
       design intent (from specs/ADRs), and research findings (from \
       papers).\n\
     - Be specific and technical. Include exact names, numbers, and \
       terminology from the sources.\n\
     - Keep the answer focused and concise. Don't pad with generic \
       statements."
        .to_string()
}

/// Build the user prompt including the question and context blocks.
fn build_user_prompt(question: &str, blocks: &[ContextBlock]) -> String {
    let mut prompt = format!("Question: {question}\n\nRetrieved context:\n");

    for block in blocks {
        let source_label = if block.source_title.is_empty() {
            block.source_uri.clone()
        } else if block.source_uri.is_empty() {
            block.source_title.clone()
        } else {
            format!("\"{}\" {}", block.source_title, block.source_uri)
        };

        prompt.push_str(&format!(
            "\n[{}] ({}, confidence: {:.2}, source: {})\n{}\n",
            block.number,
            block.result_type,
            block.confidence,
            source_label,
            truncate_context(&block.snippet, 2000),
        ));
    }

    prompt.push_str("\nProvide a comprehensive answer based on the context above.");
    prompt
}

/// Build citations from search results and context blocks.
fn build_citations(results: &[FusedResult], blocks: &[ContextBlock]) -> Vec<Citation> {
    results
        .iter()
        .zip(blocks.iter())
        .map(|(result, block)| {
            let source = if !block.source_title.is_empty() {
                block.source_title.clone()
            } else if !block.source_uri.is_empty() {
                block.source_uri.clone()
            } else {
                result.name.clone().unwrap_or_else(|| "unknown".to_string())
            };

            Citation {
                source,
                snippet: truncate_context(&block.snippet, 500),
                result_type: block.result_type.clone(),
                confidence: block.confidence,
            }
        })
        .collect()
}

/// Truncate context text to a maximum character length, appending
/// an ellipsis if truncation occurred.
fn truncate_context(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        return s.to_string();
    }
    // Find a safe truncation point (avoid splitting multi-byte chars).
    let mut end = max_chars;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_options_default() {
        let opts = AskOptions::default();
        assert_eq!(opts.max_context, 15);
        assert!(opts.strategy.is_none());
    }

    #[test]
    fn parse_strategy_auto() {
        assert!(matches!(parse_strategy(None), SearchStrategy::Auto));
        assert!(matches!(parse_strategy(Some("auto")), SearchStrategy::Auto));
    }

    #[test]
    fn parse_strategy_precise() {
        assert!(matches!(
            parse_strategy(Some("precise")),
            SearchStrategy::Precise
        ));
    }

    #[test]
    fn parse_strategy_balanced() {
        assert!(matches!(
            parse_strategy(Some("balanced")),
            SearchStrategy::Balanced
        ));
    }

    #[test]
    fn parse_strategy_exploratory() {
        assert!(matches!(
            parse_strategy(Some("exploratory")),
            SearchStrategy::Exploratory
        ));
    }

    #[test]
    fn parse_strategy_recent() {
        assert!(matches!(
            parse_strategy(Some("recent")),
            SearchStrategy::Recent
        ));
    }

    #[test]
    fn parse_strategy_graph_first() {
        assert!(matches!(
            parse_strategy(Some("graph_first")),
            SearchStrategy::GraphFirst
        ));
    }

    #[test]
    fn parse_strategy_global() {
        assert!(matches!(
            parse_strategy(Some("global")),
            SearchStrategy::Global
        ));
    }

    #[test]
    fn parse_strategy_unknown() {
        assert!(matches!(
            parse_strategy(Some("bogus")),
            SearchStrategy::Auto
        ));
    }

    #[test]
    fn truncate_context_short() {
        let s = "hello world";
        assert_eq!(truncate_context(s, 100), "hello world");
    }

    #[test]
    fn truncate_context_long() {
        let s = "a".repeat(200);
        let result = truncate_context(&s, 50);
        assert_eq!(result.len(), 53); // 50 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_context_multibyte() {
        // Multi-byte UTF-8: each char is 4 bytes.
        let s = "\u{1F600}".repeat(10); // 10 emoji
        let result = truncate_context(&s, 5);
        // Should truncate at a valid boundary.
        assert!(result.ends_with("..."));
    }

    #[test]
    fn build_system_prompt_contains_rules() {
        let prompt = build_system_prompt();
        assert!(prompt.contains("knowledge synthesis engine"));
        assert!(prompt.contains("[1]"));
        assert!(prompt.contains("fabricate"));
    }

    #[test]
    fn build_user_prompt_formats_correctly() {
        let blocks = vec![
            ContextBlock {
                number: 1,
                result_type: "chunk".to_string(),
                confidence: 0.92,
                source_title: "HippoRAG Paper".to_string(),
                source_uri: "file://spec/05-ingestion.md".to_string(),
                snippet: "Entity resolution uses 5 tiers.".to_string(),
                fused_score: 0.85,
            },
            ContextBlock {
                number: 2,
                result_type: "node".to_string(),
                confidence: 0.0,
                source_title: "Entity Resolution".to_string(),
                source_uri: String::new(),
                snippet: "HDBSCAN clustering approach.".to_string(),
                fused_score: 0.6,
            },
        ];
        let prompt = build_user_prompt("How does entity resolution work?", &blocks);
        assert!(prompt.contains("Question: How does entity resolution work?"));
        assert!(prompt.contains("[1] (chunk, confidence: 0.92"));
        assert!(prompt.contains("HippoRAG Paper"));
        assert!(prompt.contains("[2] (node, confidence: 0.00"));
        assert!(prompt.contains("Entity Resolution"));
        assert!(prompt.contains("Provide a comprehensive answer"));
    }

    #[test]
    fn build_citations_maps_results() {
        let results = vec![FusedResult {
            id: uuid::Uuid::nil(),
            fused_score: 0.9,
            confidence: Some(0.85),
            entity_type: None,
            name: Some("Test Node".to_string()),
            snippet: Some("test snippet".to_string()),
            content: Some("test content".to_string()),
            source_uri: None,
            source_title: None,
            source_type: None,
            result_type: Some("chunk".to_string()),
            created_at: None,
            dimension_scores: Default::default(),
            dimension_ranks: Default::default(),
            graph_context: None,
        }];
        let blocks = vec![ContextBlock {
            number: 1,
            result_type: "chunk".to_string(),
            confidence: 0.85,
            source_title: "My Source".to_string(),
            source_uri: "file://test.md".to_string(),
            snippet: "test content".to_string(),
            fused_score: 0.9,
        }];
        let citations = build_citations(&results, &blocks);
        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].source, "My Source");
        assert_eq!(citations[0].result_type, "chunk");
        assert!((citations[0].confidence - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn citation_falls_back_to_name() {
        let results = vec![FusedResult {
            id: uuid::Uuid::nil(),
            fused_score: 0.5,
            confidence: None,
            entity_type: None,
            name: Some("Fallback Name".to_string()),
            snippet: None,
            content: None,
            source_uri: None,
            source_title: None,
            source_type: None,
            result_type: Some("node".to_string()),
            created_at: None,
            dimension_scores: Default::default(),
            dimension_ranks: Default::default(),
            graph_context: None,
        }];
        let blocks = vec![ContextBlock {
            number: 1,
            result_type: "node".to_string(),
            confidence: 0.0,
            source_title: String::new(),
            source_uri: String::new(),
            snippet: String::new(),
            fused_score: 0.5,
        }];
        let citations = build_citations(&results, &blocks);
        assert_eq!(citations[0].source, "Fallback Name");
    }
}

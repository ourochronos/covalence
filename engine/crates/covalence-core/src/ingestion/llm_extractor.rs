//! LLM-driven entity and relationship extractor.
//!
//! Posts text to an OpenAI-compatible `/chat/completions` endpoint with
//! a system prompt that requests structured JSON extraction.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::ingestion::extractor::{
    ExtractedEntity, ExtractedRelationship, ExtractionContext, ExtractionResult, Extractor,
};
use crate::ingestion::utils::sanitize_latex_in_json;

const SYSTEM_PROMPT: &str = r#"You are an entity and relationship extractor. Given a text passage, extract all notable entities and relationships.

Return a JSON object with this exact schema:
{
  "entities": [
    {
      "name": "entity name as it appears in text",
      "entity_type": "person|organization|location|concept|technology|algorithm|framework|dataset|metric|model|event|role",
      "description": "brief description or null",
      "confidence": 0.0-1.0
    }
  ],
  "relationships": [
    {
      "source_name": "source entity name",
      "target_name": "target entity name",
      "rel_type": "relationship type (e.g. works_at, is_part_of, created, located_in)",
      "description": "brief description or null",
      "confidence": 0.0-1.0
    }
  ]
}

Rules:
- Only extract entities and relationships clearly supported by the text.
- Use consistent entity names (match the text exactly).
- Confidence should reflect how clearly the text supports the extraction.
- Do NOT extract entities from illustrative examples, hypothetical scenarios, or placeholder text (e.g. "suppose Alice sends a message to Bob", "for example, John works at Google"). These are not real facts.
- Do NOT extract bibliographic references, citations, or items from reference/bibliography sections. Specifically:
  - Do NOT extract paper titles that appear in citation contexts (e.g. "Retrieval-augmented generation for knowledge-intensive NLP tasks").
  - Do NOT extract author names that appear only in citation contexts (e.g. "Lewis et al. (2020)", "Smith and Jones").
  - Do NOT extract journal or conference names from citations (e.g. "Proceedings of NeurIPS", "arXiv preprint").
  - Do NOT extract dataset names that are only mentioned in passing citations without substantive discussion.
  - If an entity is BOTH cited AND substantively discussed in the text (i.e., the text explains what it is, how it works, or why it matters), then extract it. A bare mention like "as shown in [Smith 2020]" is NOT substantive discussion.
- Return valid JSON only, no markdown fences or extra text."#;

/// System prompt for relationship-only extraction (used in two-pass mode).
///
/// Receives pre-identified entities from the first pass (e.g., GLiNER)
/// and focuses the LLM on finding relationships between them only.
const SYSTEM_PROMPT_RELATIONSHIPS: &str = r#"You are a relationship extractor. The following entities have been identified in the text below.

Given the text, extract only the relationships between these entities. Do not add new entities.

Return a JSON object with this exact schema:
{
  "relationships": [
    {
      "source_name": "source entity name (must match an entity above)",
      "target_name": "target entity name (must match an entity above)",
      "rel_type": "relationship type (e.g. works_at, is_part_of, created, located_in)",
      "description": "brief description or null",
      "confidence": 0.0-1.0
    }
  ]
}

Rules:
- Only extract relationships clearly supported by the text.
- source_name and target_name MUST match entity names from the provided list.
- Confidence should reflect how clearly the text supports the relationship.
- Return valid JSON only, no markdown fences or extra text."#;

/// An extractor that calls an OpenAI-compatible chat completions endpoint.
pub struct LlmExtractor {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl LlmExtractor {
    /// Create a new LLM extractor.
    ///
    /// `base_url` defaults to `https://api.openai.com/v1` when `None`.
    pub fn new(model: String, api_key: String, base_url: Option<String>) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        Self {
            client: reqwest::Client::new(),
            base_url: base.trim_end_matches('/').to_string(),
            api_key,
            model,
        }
    }
}

impl LlmExtractor {
    /// Extract only relationships given pre-identified entities.
    ///
    /// Used by `TwoPassExtractor` after GLiNER provides the entity
    /// list. The LLM receives the entity names and focuses on
    /// finding relationships between them — cheaper, faster, and
    /// less prone to hallucination than full extraction.
    pub async fn extract_relationships(
        &self,
        text: &str,
        entities: &[ExtractedEntity],
    ) -> Result<Vec<ExtractedRelationship>> {
        if text.trim().is_empty() || entities.is_empty() {
            return Ok(Vec::new());
        }

        // Build the entity list for the prompt.
        let entity_list: String = entities
            .iter()
            .map(|e| format!("- {} ({})", e.name, e.entity_type))
            .collect::<Vec<_>>()
            .join("\n");

        let user_content = format!("Entities:\n{entity_list}\n\nText:\n{text}");

        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: SYSTEM_PROMPT_RELATIONSHIPS,
                },
                ChatMessage {
                    role: "user",
                    content: &user_content,
                },
            ],
            response_format: ResponseFormat {
                r#type: "json_object",
            },
            temperature: 0.0,
        };

        let resp = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                Error::Ingestion(format!("relationship extraction request failed: {e}"))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(Error::Ingestion(format!(
                "relationship extraction API returned {status}: {body_text}"
            )));
        }

        let chat_resp: ChatResponse = resp
            .json()
            .await
            .map_err(|e| Error::Ingestion(format!("failed to parse relationship response: {e}")))?;

        let content = chat_resp
            .choices
            .first()
            .and_then(|c| c.message.content.as_deref())
            .unwrap_or("{}");

        let raw: RawRelationshipResult = match serde_json::from_str(content) {
            Ok(r) => r,
            Err(e) => {
                let preview: String = content.chars().take(500).collect();
                tracing::warn!(
                    error = %e,
                    raw_output = %preview,
                    "relationship extraction JSON parse failed — returning empty"
                );
                return Ok(Vec::new());
            }
        };

        // Build a set of known entity names for validation.
        let known_names: std::collections::HashSet<&str> =
            entities.iter().map(|e| e.name.as_str()).collect();

        let raw_count = raw.relationships.len();
        let relationships: Vec<ExtractedRelationship> = raw
            .relationships
            .into_iter()
            .filter(|r| {
                let valid = known_names.contains(r.source_name.as_str())
                    && known_names.contains(r.target_name.as_str());
                if !valid {
                    tracing::debug!(
                        source = %r.source_name,
                        target = %r.target_name,
                        "dropping relationship: entity name not in known set"
                    );
                }
                valid
            })
            .map(|r| ExtractedRelationship {
                source_name: r.source_name,
                target_name: r.target_name,
                rel_type: r.rel_type,
                description: r.description,
                confidence: r.confidence.clamp(0.0, 1.0),
            })
            .collect();

        tracing::debug!(
            raw = raw_count,
            kept = relationships.len(),
            "relationship extraction parsed"
        );

        Ok(relationships)
    }
}

/// Relationship-only extraction result (no entities).
#[derive(Deserialize)]
struct RawRelationshipResult {
    #[serde(default)]
    relationships: Vec<RawRelationship>,
}

/// Chat message in the request.
#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

/// Response format specification.
#[derive(Serialize)]
struct ResponseFormat<'a> {
    r#type: &'a str,
}

/// Chat completions request body.
#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    response_format: ResponseFormat<'a>,
    temperature: f64,
}

/// A single choice in the response.
#[derive(Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

/// The message content in a response choice.
#[derive(Deserialize)]
struct ChatResponseMessage {
    content: Option<String>,
}

/// Top-level chat completions response.
#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

/// Raw extraction JSON that maps to our domain types.
#[derive(Deserialize)]
struct RawExtractionResult {
    #[serde(default)]
    entities: Vec<RawEntity>,
    #[serde(default)]
    relationships: Vec<RawRelationship>,
}

#[derive(Deserialize)]
struct RawEntity {
    name: String,
    entity_type: String,
    description: Option<String>,
    #[serde(default = "default_confidence")]
    confidence: f64,
}

#[derive(Deserialize)]
struct RawRelationship {
    source_name: String,
    target_name: String,
    rel_type: String,
    description: Option<String>,
    #[serde(default = "default_confidence")]
    confidence: f64,
}

fn default_confidence() -> f64 {
    0.5
}

/// Common placeholder names used in examples and documentation.
/// These are almost never real entities worth extracting.
const PLACEHOLDER_NAMES: &[&str] = &[
    "alice", "bob", "charlie", "dave", "eve", "frank", "grace", "john", "jane", "jane doe",
    "john doe", "user", "user a", "user b", "player 1", "player 2", "person a", "person b",
    "agent a", "agent b", "node a", "node b", "foo", "bar", "baz",
];

/// Returns true if the entity name is a noisy hub candidate that
/// should be filtered out (bare years, pure numbers, placeholder
/// names from examples, citation patterns, etc.).
fn is_noisy_entity(name: &str) -> bool {
    let trimmed = name.trim();
    // Reject bare numbers (e.g. "2024", "42")
    if trimmed.parse::<i64>().is_ok() {
        return true;
    }
    // Reject bare year ranges like "2023-2024"
    if trimmed.len() <= 9
        && trimmed
            .split('-')
            .all(|part| part.trim().parse::<i64>().is_ok())
    {
        return true;
    }
    // Reject common placeholder names from examples.
    let lower = trimmed.to_lowercase();
    if PLACEHOLDER_NAMES.contains(&lower.as_str()) {
        return true;
    }
    // Reject citation patterns: "Author et al. (YYYY)" or
    // "Author and Author (YYYY)".
    if is_citation_pattern(trimmed) {
        return true;
    }
    // Reject arXiv identifiers (e.g. "arXiv:2501.00309",
    // "arXiv preprint arXiv:2404.16130v1").
    if lower.starts_with("arxiv:") || lower.starts_with("arxiv preprint") {
        return true;
    }
    // Reject DOI references.
    if lower.starts_with("doi:") || lower.starts_with("https://doi.org/") {
        return true;
    }
    false
}

/// Returns true if the name looks like an academic citation:
/// - "Author et al. (YYYY)"
/// - "Author and Author (YYYY)"
/// - "Author et al., YYYY"
/// - "Author (YYYY)"
fn is_citation_pattern(name: &str) -> bool {
    // Must end with a year in parens or after a comma.
    let has_paren_year = name
        .rfind('(')
        .and_then(|i| {
            let after = &name[i + 1..];
            let year_part = after.trim_end_matches(')').trim();
            if year_part.len() == 4 && year_part.parse::<u16>().is_ok() {
                Some(())
            } else {
                None
            }
        })
        .is_some();

    let has_comma_year = name
        .rfind(',')
        .and_then(|i| {
            let after = name[i + 1..].trim();
            if after.len() == 4 && after.parse::<u16>().is_ok() {
                Some(())
            } else {
                None
            }
        })
        .is_some();

    if !has_paren_year && !has_comma_year {
        return false;
    }

    // Check for "et al." or "and" — common citation connectors.
    let lower = name.to_lowercase();
    if lower.contains("et al") {
        return true;
    }

    // "Author and Author (YYYY)" — at least one " and " before the year.
    if lower.contains(" and ") && has_paren_year {
        return true;
    }

    // Single author with year: "Author (YYYY)" — name part is short
    // (no spaces or just one space for "First Last").
    if has_paren_year {
        if let Some(paren_pos) = name.rfind('(') {
            let author_part = name[..paren_pos].trim();
            let word_count = author_part.split_whitespace().count();
            // Single author citations: 1-3 words before "(YYYY)".
            if (1..=3).contains(&word_count) {
                return true;
            }
        }
    }

    false
}

#[async_trait::async_trait]
impl Extractor for LlmExtractor {
    async fn extract(&self, text: &str, context: &ExtractionContext) -> Result<ExtractionResult> {
        if text.trim().is_empty() {
            return Ok(ExtractionResult::default());
        }

        // Build user message with optional source metadata.
        let mut user_msg = String::new();
        if let Some(ref st) = context.source_type {
            user_msg.push_str(&format!("Source type: {st}\n"));
        }
        if let Some(ref uri) = context.source_uri {
            user_msg.push_str(&format!("Source URI: {uri}\n"));
        }
        if let Some(ref title) = context.source_title {
            user_msg.push_str(&format!("Source title: {title}\n"));
        }
        if !user_msg.is_empty() {
            user_msg.push_str("\n---\n\n");
        }
        user_msg.push_str(text);

        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: SYSTEM_PROMPT,
                },
                ChatMessage {
                    role: "user",
                    content: &user_msg,
                },
            ],
            response_format: ResponseFormat {
                r#type: "json_object",
            },
            temperature: 0.0,
        };

        let resp = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Ingestion(format!("extraction request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::Ingestion(format!(
                "extraction API returned {status}: {text}"
            )));
        }

        let chat_resp: ChatResponse = resp
            .json()
            .await
            .map_err(|e| Error::Ingestion(format!("failed to parse chat response: {e}")))?;

        let content = chat_resp
            .choices
            .first()
            .and_then(|c| c.message.content.as_deref())
            .unwrap_or("{}");

        parse_extraction_json(content)
    }
}

/// Parse the LLM's JSON output into an ExtractionResult.
///
/// Handles malformed responses gracefully by returning an empty result
/// with a warning log so failures are visible to operators.
fn parse_extraction_json(json_str: &str) -> Result<ExtractionResult> {
    let cleaned = sanitize_latex_in_json(json_str);
    let raw: RawExtractionResult = match serde_json::from_str(&cleaned) {
        Ok(r) => r,
        Err(e) => {
            // Truncate raw output for logging to avoid flooding.
            let preview: String = json_str.chars().take(500).collect();
            tracing::warn!(
                error = %e,
                raw_output = %preview,
                "extraction JSON parse failed — returning empty result"
            );
            return Ok(ExtractionResult::default());
        }
    };

    let raw_entity_count = raw.entities.len();
    let raw_rel_count = raw.relationships.len();

    let entities: Vec<ExtractedEntity> = raw
        .entities
        .into_iter()
        .filter(|e| !is_noisy_entity(&e.name))
        .map(|e| ExtractedEntity {
            name: e.name,
            entity_type: e.entity_type,
            description: e.description,
            confidence: e.confidence.clamp(0.0, 1.0),
            metadata: None,
        })
        .collect();

    let filtered_count = raw_entity_count - entities.len();

    let relationships: Vec<ExtractedRelationship> = raw
        .relationships
        .into_iter()
        .map(|r| ExtractedRelationship {
            source_name: r.source_name,
            target_name: r.target_name,
            rel_type: r.rel_type,
            description: r.description,
            confidence: r.confidence.clamp(0.0, 1.0),
        })
        .collect();

    tracing::debug!(
        entities = entities.len(),
        relationships = relationships.len(),
        filtered_noisy = filtered_count,
        raw_entities = raw_entity_count,
        raw_relationships = raw_rel_count,
        "extraction JSON parsed"
    );

    Ok(ExtractionResult {
        entities,
        relationships,
    })
}

// ── ChatBackend-based extractor ─────────────────────────────────

/// An entity/relationship extractor that delegates to a [`ChatBackend`]
/// instead of making its own HTTP calls.
///
/// This is the preferred extractor when the pipeline uses a CLI-based
/// chat backend (e.g. `copilot`, `gemini`) — it reuses the same backend,
/// prompts, and JSON parsing as [`LlmExtractor`] but routes through
/// the shared chat backend.
pub struct ChatBackendExtractor {
    backend: std::sync::Arc<dyn crate::ingestion::chat_backend::ChatBackend>,
}

impl ChatBackendExtractor {
    /// Create a new extractor wrapping the given chat backend.
    pub fn new(backend: std::sync::Arc<dyn crate::ingestion::chat_backend::ChatBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait::async_trait]
impl Extractor for ChatBackendExtractor {
    async fn extract(&self, text: &str, context: &ExtractionContext) -> Result<ExtractionResult> {
        if text.trim().is_empty() {
            return Ok(ExtractionResult::default());
        }

        // Build user message with optional source metadata (same as LlmExtractor).
        let mut user_msg = String::new();
        if let Some(ref st) = context.source_type {
            user_msg.push_str(&format!("Source type: {st}\n"));
        }
        if let Some(ref uri) = context.source_uri {
            user_msg.push_str(&format!("Source URI: {uri}\n"));
        }
        if let Some(ref title) = context.source_title {
            user_msg.push_str(&format!("Source title: {title}\n"));
        }
        if !user_msg.is_empty() {
            user_msg.push_str("\n---\n\n");
        }
        user_msg.push_str(text);

        let content = self
            .backend
            .chat(SYSTEM_PROMPT, &user_msg, true, 0.0)
            .await?;

        // Strip markdown fences (CLI backends often wrap JSON in ```).
        let cleaned = crate::ingestion::utils::strip_markdown_fences(&content);
        parse_extraction_json(&cleaned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_extraction() {
        let json = serde_json::json!({
            "entities": [
                {
                    "name": "Marie Curie",
                    "entity_type": "person",
                    "description": "A physicist",
                    "confidence": 0.95
                },
                {
                    "name": "Acme Corp",
                    "entity_type": "organization",
                    "description": null,
                    "confidence": 0.8
                }
            ],
            "relationships": [
                {
                    "source_name": "Marie Curie",
                    "target_name": "Acme Corp",
                    "rel_type": "works_at",
                    "description": "Marie Curie works at Acme Corp",
                    "confidence": 0.9
                }
            ]
        });

        let result = parse_extraction_json(&json.to_string()).unwrap();
        assert_eq!(result.entities.len(), 2);
        assert_eq!(result.entities[0].name, "Marie Curie");
        assert_eq!(result.entities[0].entity_type, "person");
        assert_eq!(result.entities[0].confidence, 0.95);
        assert_eq!(result.entities[1].name, "Acme Corp");
        assert_eq!(result.relationships.len(), 1);
        assert_eq!(result.relationships[0].rel_type, "works_at");
    }

    #[test]
    fn parse_malformed_json_returns_empty() {
        let result = parse_extraction_json("this is not json").unwrap();
        assert!(result.entities.is_empty());
        assert!(result.relationships.is_empty());
    }

    #[test]
    fn parse_partial_json_uses_defaults() {
        let json = r#"{"entities": [{"name": "Einstein", "entity_type": "person"}]}"#;
        let result = parse_extraction_json(json).unwrap();
        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].confidence, 0.5);
        assert!(result.relationships.is_empty());
    }

    #[test]
    fn parse_empty_object() {
        let result = parse_extraction_json("{}").unwrap();
        assert!(result.entities.is_empty());
        assert!(result.relationships.is_empty());
    }

    #[test]
    fn confidence_clamping() {
        let json = r#"{
            "entities": [
                {"name": "X", "entity_type": "thing", "confidence": 1.5},
                {"name": "Y", "entity_type": "thing", "confidence": -0.2}
            ]
        }"#;
        let result = parse_extraction_json(json).unwrap();
        assert_eq!(result.entities[0].confidence, 1.0);
        assert_eq!(result.entities[1].confidence, 0.0);
    }

    #[test]
    fn chat_request_serialization() {
        let body = ChatRequest {
            model: "gpt-4o",
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: "You are helpful.",
                },
                ChatMessage {
                    role: "user",
                    content: "Extract entities.",
                },
            ],
            response_format: ResponseFormat {
                r#type: "json_object",
            },
            temperature: 0.0,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["model"], "gpt-4o");
        assert_eq!(json["messages"].as_array().unwrap().len(), 2);
        assert_eq!(json["response_format"]["type"], "json_object");
        assert_eq!(json["temperature"], 0.0);
    }

    #[test]
    fn chat_response_deserialization() {
        let json = serde_json::json!({
            "id": "chatcmpl-abc",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "{\"entities\": [], \"relationships\": []}"
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        });
        let resp: ChatResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.choices.len(), 1);
        assert!(resp.choices[0].message.content.is_some());
    }

    #[tokio::test]
    async fn extract_empty_text() {
        let extractor = LlmExtractor::new("gpt-4o".to_string(), "sk-test".to_string(), None);
        let ctx = ExtractionContext::default();
        let result = extractor.extract("   ", &ctx).await.unwrap();
        assert!(result.entities.is_empty());
        assert!(result.relationships.is_empty());
    }

    #[test]
    fn noisy_entity_filter() {
        // Bare numbers and year ranges.
        assert!(is_noisy_entity("2024"));
        assert!(is_noisy_entity("2023"));
        assert!(is_noisy_entity("42"));
        assert!(is_noisy_entity("2023-2024"));
        // Placeholder names from examples.
        assert!(is_noisy_entity("Alice"));
        assert!(is_noisy_entity("Bob"));
        assert!(is_noisy_entity("john"));
        assert!(is_noisy_entity("Jane Doe"));
        assert!(is_noisy_entity("foo"));
        // Real entities should pass through.
        assert!(!is_noisy_entity("GraphRAG"));
        assert!(!is_noisy_entity("ISO-8601"));
        assert!(!is_noisy_entity("GPT-4o"));
        assert!(!is_noisy_entity("Anthropic"));
        assert!(!is_noisy_entity("HDBSCAN"));
    }

    #[test]
    fn citation_patterns_filtered() {
        // "et al." citations.
        assert!(is_noisy_entity("Lewis et al. (2020)"));
        assert!(is_noisy_entity("Trivedi et al. (2022)"));
        assert!(is_noisy_entity("Kočiskỳ et al., 2018"));
        // Author and Author citations.
        assert!(is_noisy_entity("Smith and Jones (2021)"));
        assert!(is_noisy_entity("Wang and Li (2023)"));
        // Single author citations.
        assert!(is_noisy_entity("Jøsang (2016)"));
        assert!(is_noisy_entity("Dasigi (2021)"));
        // arXiv identifiers.
        assert!(is_noisy_entity("arXiv:2501.00309"));
        assert!(is_noisy_entity("arXiv preprint arXiv:2404.16130v1"));
        // DOI references.
        assert!(is_noisy_entity("doi:10.1007/978-3-319-42337-1"));
        assert!(is_noisy_entity("https://doi.org/10.1145/1234567"));
    }

    #[test]
    fn real_entities_not_filtered_as_citations() {
        // Real entity names that contain years but aren't citations.
        assert!(!is_noisy_entity("GPT-4o"));
        assert!(!is_noisy_entity("Subjective Logic"));
        assert!(!is_noisy_entity("Reciprocal Rank Fusion"));
        assert!(!is_noisy_entity("Louvain community detection"));
        assert!(!is_noisy_entity("PostgreSQL 17"));
        // Multi-word entity that happens to have "and" but no year.
        assert!(!is_noisy_entity("Retrieval and Generation"));
        // Named entity with parenthetical that isn't a year.
        assert!(!is_noisy_entity("BERT (base)"));
        assert!(!is_noisy_entity("GraphRAG (Microsoft)"));
    }

    #[test]
    fn noisy_entities_filtered_from_extraction() {
        let json = r#"{
            "entities": [
                {"name": "GraphRAG", "entity_type": "concept", "confidence": 0.9},
                {"name": "2024", "entity_type": "event", "confidence": 0.7},
                {"name": "Alice", "entity_type": "person", "confidence": 0.8},
                {"name": "Bob", "entity_type": "person", "confidence": 0.7}
            ]
        }"#;
        let result = parse_extraction_json(json).unwrap();
        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].name, "GraphRAG");
    }

    #[test]
    fn user_message_with_full_context() {
        let context = ExtractionContext {
            source_type: Some("web_page".to_string()),
            source_uri: Some("https://example.com/page".to_string()),
            source_title: Some("Example Page".to_string()),
        };
        let text = "Alice works at Acme Corp.";

        let mut user_msg = String::new();
        if let Some(ref st) = context.source_type {
            user_msg.push_str(&format!("Source type: {st}\n"));
        }
        if let Some(ref uri) = context.source_uri {
            user_msg.push_str(&format!("Source URI: {uri}\n"));
        }
        if let Some(ref title) = context.source_title {
            user_msg.push_str(&format!("Source title: {title}\n"));
        }
        if !user_msg.is_empty() {
            user_msg.push_str("\n---\n\n");
        }
        user_msg.push_str(text);

        assert!(user_msg.starts_with("Source type: web_page\n"));
        assert!(user_msg.contains("Source URI: https://example.com/page\n"));
        assert!(user_msg.contains("Source title: Example Page\n"));
        assert!(user_msg.contains("\n---\n\n"));
        assert!(user_msg.ends_with(text));
    }

    #[test]
    fn user_message_with_no_context() {
        let context = ExtractionContext::default();
        let text = "Alice works at Acme Corp.";

        let mut user_msg = String::new();
        if let Some(ref st) = context.source_type {
            user_msg.push_str(&format!("Source type: {st}\n"));
        }
        if let Some(ref uri) = context.source_uri {
            user_msg.push_str(&format!("Source URI: {uri}\n"));
        }
        if let Some(ref title) = context.source_title {
            user_msg.push_str(&format!("Source title: {title}\n"));
        }
        if !user_msg.is_empty() {
            user_msg.push_str("\n---\n\n");
        }
        user_msg.push_str(text);

        // No context means no prefix — just the raw text.
        assert_eq!(user_msg, text);
    }

    #[test]
    fn user_message_with_partial_context() {
        let context = ExtractionContext {
            source_type: Some("document".to_string()),
            source_uri: None,
            source_title: Some("Research Paper".to_string()),
        };
        let text = "Quantum computing overview.";

        let mut user_msg = String::new();
        if let Some(ref st) = context.source_type {
            user_msg.push_str(&format!("Source type: {st}\n"));
        }
        if let Some(ref uri) = context.source_uri {
            user_msg.push_str(&format!("Source URI: {uri}\n"));
        }
        if let Some(ref title) = context.source_title {
            user_msg.push_str(&format!("Source title: {title}\n"));
        }
        if !user_msg.is_empty() {
            user_msg.push_str("\n---\n\n");
        }
        user_msg.push_str(text);

        assert!(user_msg.starts_with("Source type: document\n"));
        assert!(!user_msg.contains("Source URI:"));
        assert!(user_msg.contains("Source title: Research Paper\n"));
        assert!(user_msg.ends_with(text));
    }
}

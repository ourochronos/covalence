//! LLM-driven entity and relationship extractor.
//!
//! Posts text to an OpenAI-compatible `/chat/completions` endpoint with
//! a system prompt that requests structured JSON extraction.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::ingestion::extractor::{
    ExtractedEntity, ExtractedRelationship, ExtractionResult, Extractor,
};

const SYSTEM_PROMPT: &str = r#"You are an entity and relationship extractor. Given a text passage, extract all notable entities and relationships.

Return a JSON object with this exact schema:
{
  "entities": [
    {
      "name": "entity name as it appears in text",
      "entity_type": "person|organization|location|concept|event|technology|other",
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

/// Returns true if the entity name is a noisy hub candidate that
/// should be filtered out (bare years, pure numbers, etc.).
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
    false
}

#[async_trait::async_trait]
impl Extractor for LlmExtractor {
    async fn extract(&self, text: &str) -> Result<ExtractionResult> {
        if text.trim().is_empty() {
            return Ok(ExtractionResult::default());
        }

        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: SYSTEM_PROMPT,
                },
                ChatMessage {
                    role: "user",
                    content: text,
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
/// Handles malformed responses gracefully by returning an empty result.
fn parse_extraction_json(json_str: &str) -> Result<ExtractionResult> {
    let raw: RawExtractionResult = serde_json::from_str(json_str).unwrap_or(RawExtractionResult {
        entities: Vec::new(),
        relationships: Vec::new(),
    });

    let entities = raw
        .entities
        .into_iter()
        .filter(|e| !is_noisy_entity(&e.name))
        .map(|e| ExtractedEntity {
            name: e.name,
            entity_type: e.entity_type,
            description: e.description,
            confidence: e.confidence.clamp(0.0, 1.0),
        })
        .collect();

    let relationships = raw
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

    Ok(ExtractionResult {
        entities,
        relationships,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_extraction() {
        let json = serde_json::json!({
            "entities": [
                {
                    "name": "Alice",
                    "entity_type": "person",
                    "description": "A software engineer",
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
                    "source_name": "Alice",
                    "target_name": "Acme Corp",
                    "rel_type": "works_at",
                    "description": "Alice works at Acme Corp",
                    "confidence": 0.9
                }
            ]
        });

        let result = parse_extraction_json(&json.to_string()).unwrap();
        assert_eq!(result.entities.len(), 2);
        assert_eq!(result.entities[0].name, "Alice");
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
        let json = r#"{"entities": [{"name": "Bob", "entity_type": "person"}]}"#;
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
        let result = extractor.extract("   ").await.unwrap();
        assert!(result.entities.is_empty());
        assert!(result.relationships.is_empty());
    }

    #[test]
    fn noisy_entity_filter() {
        assert!(is_noisy_entity("2024"));
        assert!(is_noisy_entity("2023"));
        assert!(is_noisy_entity("42"));
        assert!(is_noisy_entity("2023-2024"));
        assert!(!is_noisy_entity("GraphRAG"));
        assert!(!is_noisy_entity("Alice"));
        assert!(!is_noisy_entity("ISO-8601"));
        assert!(!is_noisy_entity("GPT-4o"));
    }

    #[test]
    fn noisy_year_entities_filtered_from_extraction() {
        let json = r#"{
            "entities": [
                {"name": "GraphRAG", "entity_type": "concept", "confidence": 0.9},
                {"name": "2024", "entity_type": "event", "confidence": 0.7},
                {"name": "2023", "entity_type": "event", "confidence": 0.6}
            ]
        }"#;
        let result = parse_extraction_json(json).unwrap();
        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].name, "GraphRAG");
    }
}

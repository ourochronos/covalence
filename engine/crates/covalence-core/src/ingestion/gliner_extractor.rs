//! GLiNER2 HTTP sidecar entity and relationship extractor.
//!
//! Posts text to a GLiNER2 HTTP sidecar for fast, local NER-based
//! entity extraction without requiring an LLM.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::ingestion::extractor::{
    ExtractedEntity, ExtractedRelationship, ExtractionContext, ExtractionResult, Extractor,
};

/// Default entity types to request from the GLiNER2 sidecar.
const DEFAULT_ENTITY_TYPES: &[&str] = &[
    "person",
    "organization",
    "location",
    "concept",
    "event",
    "technology",
];

/// Request body sent to the GLiNER2 sidecar.
#[derive(Debug, Serialize)]
struct GlinerRequest<'a> {
    /// The text to extract entities from.
    text: &'a str,
    /// Entity type labels for the model to detect.
    entity_types: &'a [&'a str],
    /// Minimum confidence threshold for returned entities.
    threshold: f32,
}

/// A single entity span returned by the GLiNER2 sidecar.
#[derive(Debug, Deserialize)]
struct GlinerEntity {
    /// The entity text as it appears in the input.
    text: String,
    /// The predicted entity type label.
    label: String,
    /// The model's confidence score.
    score: f64,
}

/// Top-level response from the GLiNER2 sidecar.
#[derive(Debug, Deserialize)]
struct GlinerResponse {
    /// Extracted entity spans.
    #[serde(default)]
    entities: Vec<GlinerEntity>,
    /// Extracted relationships (if the sidecar supports them).
    #[serde(default)]
    relationships: Vec<GlinerRelationship>,
}

/// A relationship returned by the GLiNER2 sidecar (optional).
#[derive(Debug, Deserialize)]
struct GlinerRelationship {
    /// Name of the source entity.
    source: String,
    /// Name of the target entity.
    target: String,
    /// Relationship type label.
    label: String,
    /// Confidence score.
    score: f64,
}

/// An extractor that calls a GLiNER2 HTTP sidecar for entity extraction.
///
/// GLiNER2 is a fast, local NER model that detects entities from a
/// configurable set of type labels. Unlike the LLM extractor, it does
/// not require an API key and runs as a local sidecar process.
pub struct GlinerExtractor {
    /// HTTP client for sidecar requests.
    client: reqwest::Client,
    /// Base URL of the GLiNER2 sidecar (e.g. `http://localhost:8432`).
    base_url: String,
    /// Minimum confidence threshold for returned entities.
    threshold: f32,
}

impl GlinerExtractor {
    /// Create a new GLiNER2 extractor.
    ///
    /// # Arguments
    ///
    /// * `base_url` — Base URL of the GLiNER2 HTTP sidecar
    ///   (e.g. `http://localhost:8432`).
    /// * `threshold` — Minimum confidence threshold in \[0.0, 1.0\].
    ///   Entities below this score are filtered out by the sidecar.
    pub fn new(base_url: String, threshold: f32) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            threshold,
        }
    }
}

#[async_trait::async_trait]
impl Extractor for GlinerExtractor {
    async fn extract(&self, text: &str, _context: &ExtractionContext) -> Result<ExtractionResult> {
        if text.trim().is_empty() {
            return Ok(ExtractionResult::default());
        }

        let body = GlinerRequest {
            text,
            entity_types: DEFAULT_ENTITY_TYPES,
            threshold: self.threshold,
        };

        let resp = self
            .client
            .post(format!("{}/extract", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Ingestion(format!("GLiNER2 sidecar request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(Error::Ingestion(format!(
                "GLiNER2 sidecar returned {status}: {body_text}"
            )));
        }

        let gliner_resp: GlinerResponse = resp
            .json()
            .await
            .map_err(|e| Error::Ingestion(format!("failed to parse GLiNER2 response: {e}")))?;

        let entities = gliner_resp
            .entities
            .into_iter()
            .map(|e| ExtractedEntity {
                name: e.text,
                entity_type: e.label,
                description: None,
                confidence: e.score.clamp(0.0, 1.0),
                metadata: None,
            })
            .collect();

        let relationships = gliner_resp
            .relationships
            .into_iter()
            .map(|r| ExtractedRelationship {
                source_name: r.source,
                target_name: r.target,
                rel_type: r.label,
                description: None,
                confidence: r.score.clamp(0.0, 1.0),
            })
            .collect();

        Ok(ExtractionResult {
            entities,
            relationships,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructor_trims_trailing_slash() {
        let ext = GlinerExtractor::new("http://localhost:8432/".to_string(), 0.5);
        assert_eq!(ext.base_url, "http://localhost:8432");
    }

    #[test]
    fn constructor_preserves_clean_url() {
        let ext = GlinerExtractor::new("http://localhost:8432".to_string(), 0.5);
        assert_eq!(ext.base_url, "http://localhost:8432");
    }

    #[test]
    fn constructor_stores_threshold() {
        let ext = GlinerExtractor::new("http://localhost:8432".to_string(), 0.7);
        assert!((ext.threshold - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn gliner_extractor_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GlinerExtractor>();
    }

    #[test]
    fn request_serialization() {
        let req = GlinerRequest {
            text: "Alice works at Acme Corp.",
            entity_types: DEFAULT_ENTITY_TYPES,
            threshold: 0.5,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["text"], "Alice works at Acme Corp.");
        assert_eq!(json["threshold"], 0.5);
        assert!(json["entity_types"].is_array());
        let types = json["entity_types"].as_array().unwrap();
        assert_eq!(types.len(), DEFAULT_ENTITY_TYPES.len());
        assert_eq!(types[0], "person");
    }

    #[test]
    fn response_deserialization_entities_only() {
        let json = serde_json::json!({
            "entities": [
                {"text": "Alice", "label": "person", "score": 0.95},
                {"text": "Acme Corp", "label": "organization", "score": 0.88}
            ]
        });
        let resp: GlinerResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.entities.len(), 2);
        assert_eq!(resp.entities[0].text, "Alice");
        assert_eq!(resp.entities[0].label, "person");
        assert!((resp.entities[0].score - 0.95).abs() < f64::EPSILON);
        assert!(resp.relationships.is_empty());
    }

    #[test]
    fn response_deserialization_with_relationships() {
        let json = serde_json::json!({
            "entities": [
                {"text": "Alice", "label": "person", "score": 0.95}
            ],
            "relationships": [
                {
                    "source": "Alice",
                    "target": "Acme Corp",
                    "label": "works_at",
                    "score": 0.8
                }
            ]
        });
        let resp: GlinerResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.entities.len(), 1);
        assert_eq!(resp.relationships.len(), 1);
        assert_eq!(resp.relationships[0].source, "Alice");
        assert_eq!(resp.relationships[0].label, "works_at");
    }

    #[test]
    fn empty_response_deserialization() {
        let json = serde_json::json!({});
        let resp: GlinerResponse = serde_json::from_value(json).unwrap();
        assert!(resp.entities.is_empty());
        assert!(resp.relationships.is_empty());
    }

    #[tokio::test]
    async fn extract_empty_text_returns_default() {
        let ext = GlinerExtractor::new("http://localhost:9999".to_string(), 0.5);
        let ctx = ExtractionContext::default();
        let result = ext.extract("   ", &ctx).await.unwrap();
        assert!(result.entities.is_empty());
        assert!(result.relationships.is_empty());
    }

    #[test]
    fn confidence_clamping_on_conversion() {
        let entities = vec![
            GlinerEntity {
                text: "X".to_string(),
                label: "thing".to_string(),
                score: 1.5,
            },
            GlinerEntity {
                text: "Y".to_string(),
                label: "thing".to_string(),
                score: -0.2,
            },
        ];

        let converted: Vec<ExtractedEntity> = entities
            .into_iter()
            .map(|e| ExtractedEntity {
                name: e.text,
                entity_type: e.label,
                description: None,
                confidence: e.score.clamp(0.0, 1.0),
                metadata: None,
            })
            .collect();

        assert!((converted[0].confidence - 1.0).abs() < f64::EPSILON);
        assert!((converted[1].confidence - 0.0).abs() < f64::EPSILON);
    }
}

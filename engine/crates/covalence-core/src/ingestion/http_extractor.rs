//! Unified extraction HTTP service client with windowed processing.
//!
//! Calls two service endpoints in sequence:
//! 1. `/ner` — named entity recognition (GLiNER2, ~1200 char windows)
//! 2. `/relationships` — relationship extraction (NuExtract, ~15K char limit)
//!
//! Coreference resolution (Fastcoref) is handled by the separate
//! [`FastcorefClient`](crate::ingestion::coreference::FastcorefClient)
//! preprocessing stage, which runs before extraction so that all
//! extractor backends benefit from neural coref.
//!
//! Rust owns the windowing: the service stays dumb and stateless.
//! Each model has a configurable `max_input_chars` limit. Input
//! exceeding the limit is split into overlapping windows at sentence
//! boundaries, and results are merged/deduplicated.

use std::collections::HashSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::ingestion::coreference::split_text_windows;
use crate::ingestion::extractor::{
    ExtractedEntity, ExtractedRelationship, ExtractionContext, ExtractionResult, Extractor,
};

/// Maximum input characters for the NER model (GLiNER2, 384 tokens).
const NER_MAX_CHARS: usize = 1200;
/// Overlap between NER windows (in characters).
const NER_OVERLAP_CHARS: usize = 200;
/// Maximum input characters for the relationship model.
const LARGE_MODEL_MAX_CHARS: usize = 15_000;
/// HTTP request timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Default entity type labels for NER extraction.
const DEFAULT_LABELS: &[&str] = &[
    "person",
    "organization",
    "location",
    "concept",
    "event",
    "technology",
    "algorithm",
];

// ---------------------------------------------------------------------------
// Service request/response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct NerRequest<'a> {
    text: &'a str,
    labels: Vec<&'a str>,
    threshold: f32,
}

#[derive(Debug, Deserialize)]
struct NerEntity {
    text: String,
    label: String,
    score: f64,
}

#[derive(Debug, Deserialize)]
struct NerResponse {
    entities: Vec<NerEntity>,
}

#[derive(Debug, Serialize)]
struct RelRequest<'a> {
    text: &'a str,
    entities: Vec<RelEntityHint>,
}

#[derive(Debug, Serialize)]
struct RelEntityHint {
    text: String,
    label: String,
}

#[derive(Debug, Deserialize)]
struct RelRelationship {
    #[serde(default)]
    source_entity: String,
    #[serde(default)]
    target_entity: String,
    #[serde(default)]
    relationship_type: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    score: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct RelResponse {
    #[serde(default)]
    relationships: Vec<RelRelationship>,
}

// ---------------------------------------------------------------------------
// HttpExtractor
// ---------------------------------------------------------------------------

/// Two-stage extraction client that calls the extraction HTTP service
/// with windowed processing for models with limited context windows.
///
/// Pipeline:
/// 1. NER in overlapping ~1200-char windows, deduplicated
/// 2. Relationship extraction with entity hints
///
/// Coreference resolution is handled by the separate
/// [`FastcorefClient`](crate::ingestion::coreference::FastcorefClient)
/// preprocessing stage before this extractor is called.
pub struct HttpExtractor {
    /// HTTP client with timeout.
    client: reqwest::Client,
    /// Base URL of the extraction service.
    base_url: String,
    /// NER confidence threshold.
    threshold: f32,
    /// NER window size in characters.
    ner_max_chars: usize,
    /// NER window overlap in characters.
    ner_overlap_chars: usize,
    /// Relationship extraction window size in characters.
    re_max_chars: usize,
    /// Relationship extraction window overlap in characters.
    re_overlap_chars: usize,
}

impl HttpExtractor {
    /// Create a new HTTP extractor with default windowing.
    pub fn new(base_url: String, threshold: f32) -> Self {
        Self::with_windowing(
            base_url,
            threshold,
            NER_MAX_CHARS,
            NER_OVERLAP_CHARS,
            LARGE_MODEL_MAX_CHARS,
            500,
        )
    }

    /// Create a new HTTP extractor with custom windowing parameters.
    pub fn with_windowing(
        base_url: String,
        threshold: f32,
        ner_max_chars: usize,
        ner_overlap_chars: usize,
        re_max_chars: usize,
        re_overlap_chars: usize,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            threshold,
            ner_max_chars,
            ner_overlap_chars,
            re_max_chars,
            re_overlap_chars,
        }
    }

    /// Stage 1: NER in overlapping windows, deduplicated.
    async fn extract_entities(&self, text: &str) -> Result<Vec<ExtractedEntity>> {
        let windows = split_text_windows(text, self.ner_max_chars, self.ner_overlap_chars);

        tracing::debug!(
            windows = windows.len(),
            text_len = text.len(),
            "NER windowing"
        );

        let mut all_entities: Vec<ExtractedEntity> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for window in &windows {
            match self.ner_single(window).await {
                Ok(entities) => {
                    for entity in entities {
                        let key = entity.name.to_lowercase();
                        if seen.insert(key) {
                            all_entities.push(entity);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "NER window failed, skipping"
                    );
                }
            }
        }

        tracing::debug!(entities = all_entities.len(), "NER extraction complete");
        Ok(all_entities)
    }

    /// Call `/ner` for a single window.
    async fn ner_single(&self, text: &str) -> Result<Vec<ExtractedEntity>> {
        let body = NerRequest {
            text,
            labels: DEFAULT_LABELS.to_vec(),
            threshold: self.threshold,
        };
        let resp = self
            .client
            .post(format!("{}/ner", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Ingestion(format!("NER request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(Error::Ingestion(format!(
                "NER returned {status}: {body_text}"
            )));
        }

        let parsed: NerResponse = resp
            .json()
            .await
            .map_err(|e| Error::Ingestion(format!("failed to parse NER response: {e}")))?;

        Ok(parsed
            .entities
            .into_iter()
            .map(|e| ExtractedEntity {
                name: e.text,
                entity_type: e.label,
                description: None,
                confidence: e.score.clamp(0.0, 1.0),
                metadata: None,
            })
            .collect())
    }

    /// Stage 2: Relationship extraction with entity hints.
    async fn extract_relationships(
        &self,
        text: &str,
        entities: &[ExtractedEntity],
    ) -> Result<Vec<ExtractedRelationship>> {
        let entity_hints: Vec<RelEntityHint> = entities
            .iter()
            .map(|e| RelEntityHint {
                text: e.name.clone(),
                label: e.entity_type.clone(),
            })
            .collect();

        let windows = split_text_windows(text, self.re_max_chars, self.re_overlap_chars);

        let mut all_rels: Vec<ExtractedRelationship> = Vec::new();
        let mut seen: HashSet<(String, String, String)> = HashSet::new();

        for window in &windows {
            match self.rel_single(window, &entity_hints).await {
                Ok(rels) => {
                    for rel in rels {
                        let key = (
                            rel.source_name.to_lowercase(),
                            rel.target_name.to_lowercase(),
                            rel.rel_type.to_lowercase(),
                        );
                        if seen.insert(key) {
                            all_rels.push(rel);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "relationship window failed, skipping"
                    );
                }
            }
        }

        tracing::debug!(
            relationships = all_rels.len(),
            "relationship extraction complete"
        );
        Ok(all_rels)
    }

    /// Call `/relationships` for a single window.
    async fn rel_single(
        &self,
        text: &str,
        entities: &[RelEntityHint],
    ) -> Result<Vec<ExtractedRelationship>> {
        let body = RelRequest {
            text,
            entities: entities.to_vec(),
        };
        let resp = self
            .client
            .post(format!("{}/relationships", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Ingestion(format!("relationships request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(Error::Ingestion(format!(
                "relationships returned {status}: {body_text}"
            )));
        }

        let parsed: RelResponse = resp.json().await.map_err(|e| {
            Error::Ingestion(format!("failed to parse relationships response: {e}"))
        })?;

        Ok(parsed
            .relationships
            .into_iter()
            .filter(|r| !r.source_entity.is_empty() && !r.target_entity.is_empty())
            .map(|r| ExtractedRelationship {
                source_name: r.source_entity,
                target_name: r.target_entity,
                rel_type: r.relationship_type,
                description: if r.description.is_empty() {
                    None
                } else {
                    Some(r.description)
                },
                confidence: r.score.unwrap_or(0.7).clamp(0.0, 1.0),
            })
            .collect())
    }
}

// We need Clone for RelEntityHint since we reuse across windows.
impl Clone for RelEntityHint {
    fn clone(&self) -> Self {
        Self {
            text: self.text.clone(),
            label: self.label.clone(),
        }
    }
}

#[async_trait::async_trait]
impl Extractor for HttpExtractor {
    async fn extract(&self, text: &str, _context: &ExtractionContext) -> Result<ExtractionResult> {
        if text.trim().is_empty() {
            return Ok(ExtractionResult::default());
        }

        // Stage 1: NER on text (coref preprocessing is handled
        // upstream by the pipeline's FastcorefClient).
        let entities = match self.extract_entities(text).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "NER extraction failed, returning empty"
                );
                return Ok(ExtractionResult::default());
            }
        };

        // Stage 2: Relationship extraction.
        let relationships = match self.extract_relationships(text, &entities).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "relationship extraction failed, returning entities only"
                );
                Vec::new()
            }
        };

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
    fn entity_dedup_by_lowercase() {
        let mut seen: HashSet<String> = HashSet::new();
        let names = ["Alice", "alice", "ALICE", "Bob"];
        let mut unique = Vec::new();
        for name in names {
            if seen.insert(name.to_lowercase()) {
                unique.push(name);
            }
        }
        assert_eq!(unique, vec!["Alice", "Bob"]);
    }

    #[test]
    fn relationship_dedup_by_tuple() {
        let mut seen: HashSet<(String, String, String)> = HashSet::new();
        let tuples = [
            ("Alice", "Bob", "knows"),
            ("alice", "bob", "knows"),
            ("Alice", "Carol", "knows"),
        ];
        let mut unique = Vec::new();
        for (s, t, r) in tuples {
            let key = (s.to_lowercase(), t.to_lowercase(), r.to_lowercase());
            if seen.insert(key) {
                unique.push((s, t, r));
            }
        }
        assert_eq!(unique.len(), 2);
    }

    #[test]
    fn http_extractor_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<HttpExtractor>();
    }

    #[tokio::test]
    async fn extract_empty_text_returns_default() {
        let ext = HttpExtractor::new("http://localhost:9999".to_string(), 0.4);
        let ctx = ExtractionContext::default();
        let result = ext.extract("   ", &ctx).await.unwrap();
        assert!(result.entities.is_empty());
        assert!(result.relationships.is_empty());
    }

    #[test]
    fn ner_response_deserialization() {
        let json = serde_json::json!({
            "entities": [
                {"text": "Alice", "label": "person", "score": 0.95},
                {"text": "Acme", "label": "organization", "score": 0.88}
            ]
        });
        let resp: NerResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.entities.len(), 2);
        assert_eq!(resp.entities[0].text, "Alice");
    }

    #[test]
    fn rel_response_deserialization() {
        let json = serde_json::json!({
            "relationships": [{
                "source_entity": "Alice",
                "target_entity": "Acme Corp",
                "relationship_type": "works_at",
                "description": "employed by",
                "score": 0.8
            }]
        });
        let resp: RelResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.relationships.len(), 1);
        assert_eq!(resp.relationships[0].source_entity, "Alice");
    }

    #[test]
    fn rel_response_empty_entities_filtered() {
        let json = serde_json::json!({
            "relationships": [
                {
                    "source_entity": "Alice",
                    "target_entity": "Acme",
                    "relationship_type": "works_at",
                    "description": ""
                },
                {
                    "source_entity": "",
                    "target_entity": "Acme",
                    "relationship_type": "works_at",
                    "description": ""
                }
            ]
        });
        let resp: RelResponse = serde_json::from_value(json).unwrap();
        let filtered: Vec<_> = resp
            .relationships
            .into_iter()
            .filter(|r| !r.source_entity.is_empty() && !r.target_entity.is_empty())
            .collect();
        assert_eq!(filtered.len(), 1);
    }
}

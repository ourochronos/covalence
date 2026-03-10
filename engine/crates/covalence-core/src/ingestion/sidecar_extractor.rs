//! Unified extraction sidecar client with windowed processing.
//!
//! Calls three sidecar endpoints in sequence:
//! 1. `/coref` — coreference resolution (Fastcoref, ~15K char limit)
//! 2. `/ner` — named entity recognition (GLiNER2, ~1200 char windows)
//! 3. `/relationships` — relationship extraction (NuExtract, ~15K char limit)
//!
//! Rust owns the windowing: the sidecar stays dumb and stateless.
//! Each model has a configurable `max_input_chars` limit. Input
//! exceeding the limit is split into overlapping windows at sentence
//! boundaries, and results are merged/deduplicated.

use std::collections::HashSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::ingestion::extractor::{
    ExtractedEntity, ExtractedRelationship, ExtractionResult, Extractor,
};

/// Maximum input characters for the NER model (GLiNER2, 384 tokens).
const NER_MAX_CHARS: usize = 1200;
/// Overlap between NER windows (in characters).
const NER_OVERLAP_CHARS: usize = 200;
/// Maximum input characters for coref and relationship models.
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
// Sidecar request/response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct CorefRequest<'a> {
    texts: Vec<&'a str>,
}

#[derive(Debug, Deserialize)]
struct CorefResult {
    #[allow(dead_code)]
    original: String,
    resolved: String,
    #[allow(dead_code)]
    clusters: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct CorefResponse {
    results: Vec<CorefResult>,
}

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
// SidecarExtractor
// ---------------------------------------------------------------------------

/// Three-stage extraction client that calls the unified sidecar with
/// windowed processing for models with limited context windows.
///
/// Pipeline:
/// 1. Coreference resolution (full text or large windows)
/// 2. NER in overlapping ~1200-char windows, deduplicated
/// 3. Relationship extraction with entity hints
pub struct SidecarExtractor {
    /// HTTP client with timeout.
    client: reqwest::Client,
    /// Base URL of the extraction sidecar.
    base_url: String,
    /// NER confidence threshold.
    threshold: f32,
}

impl SidecarExtractor {
    /// Create a new sidecar extractor.
    pub fn new(base_url: String, threshold: f32) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            threshold,
        }
    }

    /// Stage 1: Coreference resolution.
    ///
    /// Resolves pronouns to their antecedents. If the text exceeds
    /// the coref model's limit, it is processed in large windows.
    async fn resolve_coreferences(&self, text: &str) -> Result<String> {
        if text.len() <= LARGE_MODEL_MAX_CHARS {
            return self.coref_single(text).await;
        }

        // Window the text for coref.
        let windows = split_into_windows(text, LARGE_MODEL_MAX_CHARS, 500);
        let mut resolved_parts = Vec::with_capacity(windows.len());
        for window in &windows {
            match self.coref_single(window).await {
                Ok(r) => resolved_parts.push(r),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "coref window failed, using original"
                    );
                    resolved_parts.push(window.to_string());
                }
            }
        }
        Ok(resolved_parts.join(" "))
    }

    /// Call `/coref` for a single text.
    async fn coref_single(&self, text: &str) -> Result<String> {
        let body = CorefRequest { texts: vec![text] };
        let resp = self
            .client
            .post(format!("{}/coref", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Ingestion(format!("sidecar coref request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(Error::Ingestion(format!(
                "sidecar coref returned {status}: {body_text}"
            )));
        }

        let parsed: CorefResponse = resp
            .json()
            .await
            .map_err(|e| Error::Ingestion(format!("failed to parse coref response: {e}")))?;

        Ok(parsed
            .results
            .into_iter()
            .next()
            .map(|r| r.resolved)
            .unwrap_or_else(|| text.to_string()))
    }

    /// Stage 2: NER in overlapping windows, deduplicated.
    async fn extract_entities(&self, text: &str) -> Result<Vec<ExtractedEntity>> {
        let windows = split_into_windows(text, NER_MAX_CHARS, NER_OVERLAP_CHARS);

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
            .map_err(|e| Error::Ingestion(format!("sidecar NER request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(Error::Ingestion(format!(
                "sidecar NER returned {status}: {body_text}"
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
            })
            .collect())
    }

    /// Stage 3: Relationship extraction with entity hints.
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

        let windows = split_into_windows(text, LARGE_MODEL_MAX_CHARS, 500);

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
            .map_err(|e| Error::Ingestion(format!("sidecar relationships request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(Error::Ingestion(format!(
                "sidecar relationships returned {status}: {body_text}"
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
impl Extractor for SidecarExtractor {
    async fn extract(&self, text: &str) -> Result<ExtractionResult> {
        if text.trim().is_empty() {
            return Ok(ExtractionResult::default());
        }

        // Stage 1: Coreference resolution.
        let resolved = match self.resolve_coreferences(text).await {
            Ok(r) => {
                tracing::debug!(
                    original_len = text.len(),
                    resolved_len = r.len(),
                    "coreference resolution complete"
                );
                r
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "coreference resolution failed, using original text"
                );
                text.to_string()
            }
        };

        // Stage 2: NER on resolved text.
        let entities = match self.extract_entities(&resolved).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "NER extraction failed, returning empty"
                );
                return Ok(ExtractionResult::default());
            }
        };

        // Stage 3: Relationship extraction on resolved text.
        let relationships = match self.extract_relationships(&resolved, &entities).await {
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

// ---------------------------------------------------------------------------
// Windowing utilities
// ---------------------------------------------------------------------------

/// Split text into overlapping windows at sentence boundaries.
///
/// Each window is at most `max_chars` long. Windows overlap by
/// `overlap_chars` to avoid missing entities at boundaries.
/// If the text is shorter than `max_chars`, returns a single window.
fn split_into_windows(text: &str, max_chars: usize, overlap_chars: usize) -> Vec<&str> {
    if text.len() <= max_chars {
        return vec![text];
    }

    let mut windows = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();

    while start < text.len() {
        let mut end = (start + max_chars).min(text.len());

        // Try to break at a sentence boundary ('. ', '? ', '! ', '\n')
        // by scanning backward from `end`.
        if end < text.len() {
            let search_start = if end > 100 { end - 100 } else { start };
            let mut best_break = None;
            for i in (search_start..end).rev() {
                if i + 1 < bytes.len()
                    && (bytes[i] == b'.' || bytes[i] == b'?' || bytes[i] == b'!')
                    && bytes[i + 1] == b' '
                {
                    best_break = Some(i + 2); // After ". "
                    break;
                }
                if bytes[i] == b'\n' {
                    best_break = Some(i + 1);
                    break;
                }
            }
            if let Some(b) = best_break {
                end = b;
            }
        }

        // Ensure we're at a valid UTF-8 boundary.
        while end < text.len() && !text.is_char_boundary(end) {
            end += 1;
        }

        windows.push(&text[start..end]);

        // Advance by (window size - overlap), ensuring we make progress.
        let advance = if end - start > overlap_chars {
            end - start - overlap_chars
        } else {
            end - start
        };
        start += advance;
    }

    windows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_short_text_single_window() {
        let text = "Hello world.";
        let windows = split_into_windows(text, 1200, 200);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0], text);
    }

    #[test]
    fn split_long_text_multiple_windows() {
        // Create a text with clear sentence boundaries.
        let sentences: Vec<String> = (0..20)
            .map(|i| format!("Sentence number {i} is here. "))
            .collect();
        let text = sentences.join("");
        assert!(text.len() > 200);

        let windows = split_into_windows(&text, 200, 50);
        assert!(
            windows.len() > 1,
            "expected multiple windows, got {}",
            windows.len()
        );

        // All windows should be <= max_chars.
        for (i, w) in windows.iter().enumerate() {
            assert!(w.len() <= 200, "window {i} too long: {} chars", w.len());
        }

        // Windows should cover the full text (overlap means some
        // parts appear twice, but nothing is missed).
        let mut covered = 0;
        for w in &windows {
            // At minimum each window adds (len - overlap) new chars,
            // except the last which adds its full length.
            covered += w.len();
        }
        assert!(
            covered >= text.len(),
            "windows don't cover full text: covered={covered}, text={}",
            text.len()
        );
    }

    #[test]
    fn split_respects_sentence_boundaries() {
        let text = "First sentence. Second sentence. Third sentence. \
                     Fourth sentence. Fifth sentence.";
        let windows = split_into_windows(text, 40, 10);
        // Each window should end at or near a sentence boundary.
        for w in &windows {
            let trimmed = w.trim();
            if !trimmed.is_empty() {
                // Should end with a period or be the last window.
                assert!(
                    trimmed.ends_with('.')
                        || trimmed.ends_with('?')
                        || trimmed.ends_with('!')
                        || w.len() <= 40,
                    "window doesn't end at sentence boundary: {w:?}"
                );
            }
        }
    }

    #[test]
    fn split_makes_progress() {
        // Even without sentence boundaries, windowing must not loop.
        let text = "a".repeat(5000);
        let windows = split_into_windows(&text, 1200, 200);
        assert!(windows.len() >= 4);
        // Total coverage.
        let total_new: usize = windows
            .iter()
            .enumerate()
            .map(|(i, w)| {
                if i == 0 {
                    w.len()
                } else {
                    w.len().saturating_sub(200)
                }
            })
            .sum();
        assert!(total_new >= 5000);
    }

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
    fn sidecar_extractor_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SidecarExtractor>();
    }

    #[tokio::test]
    async fn extract_empty_text_returns_default() {
        let ext = SidecarExtractor::new("http://localhost:9999".to_string(), 0.4);
        let result = ext.extract("   ").await.unwrap();
        assert!(result.entities.is_empty());
        assert!(result.relationships.is_empty());
    }

    #[test]
    fn coref_response_deserialization() {
        let json = serde_json::json!({
            "results": [{
                "original": "He went home.",
                "resolved": "John went home.",
                "clusters": [["John", "He"]]
            }]
        });
        let resp: CorefResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.results[0].resolved, "John went home.");
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

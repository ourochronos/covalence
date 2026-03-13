//! Co-reference resolution within a single document.
//!
//! Two co-reference resolvers:
//!
//! 1. **Heuristic** ([`CorefResolver`]) — in-process abbreviation
//!    detection (e.g., "Natural Language Processing" → "NLP"). Runs
//!    across all chunks with zero external dependencies.
//!
//! 2. **Neural** ([`FastcorefClient`]) — HTTP client for the
//!    Fastcoref sidecar (`/coref` endpoint). Resolves pronouns and
//!    other anaphora using a neural model. Runs as an independent
//!    preprocessing stage before extraction so that **all** extractor
//!    backends benefit from neural coref.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::ingestion::chunker::ChunkOutput;

/// A link from a mention (abbreviation or short form) to the full
/// referent entity name within a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorefLink {
    /// The mention text (e.g., "NLP", "ML").
    pub mention: String,
    /// The full referent entity name (e.g., "Natural Language Processing").
    pub referent: String,
    /// Index of the chunk where the mention was detected.
    pub chunk_index: usize,
}

/// Heuristic co-reference resolver for abbreviation detection.
///
/// Scans chunks for multi-word entity names and tracks their
/// abbreviations (formed from the first letter of each word).
/// When the abbreviation appears in a later (or same) chunk, a
/// [`CorefLink`] is emitted.
pub struct CorefResolver;

impl CorefResolver {
    /// Create a new co-reference resolver.
    pub fn new() -> Self {
        Self
    }

    /// Resolve co-references across the given chunks.
    ///
    /// Returns a list of [`CorefLink`]s mapping abbreviation mentions
    /// to their full referent names.
    pub fn resolve(&self, chunks: &[ChunkOutput]) -> Vec<CorefLink> {
        // Phase 1: Collect multi-word terms and their abbreviations
        // from all chunks.
        //
        // We look for sequences of capitalized words (2+ words) and
        // compute the uppercase-initials abbreviation.
        let mut abbrev_to_full: HashMap<String, String> = HashMap::new();

        for chunk in chunks {
            let terms = extract_multiword_terms(&chunk.text);
            for term in terms {
                let abbrev = compute_abbreviation(&term);
                if abbrev.len() >= 2 {
                    abbrev_to_full.entry(abbrev).or_insert_with(|| term);
                }
            }
        }

        // Phase 2: Scan chunks for abbreviation occurrences and emit
        // CorefLinks.
        let mut links = Vec::new();
        for (chunk_index, chunk) in chunks.iter().enumerate() {
            for (abbrev, full_name) in &abbrev_to_full {
                if contains_word(&chunk.text, abbrev) {
                    links.push(CorefLink {
                        mention: abbrev.clone(),
                        referent: full_name.clone(),
                        chunk_index,
                    });
                }
            }
        }

        links
    }
}

impl Default for CorefResolver {
    fn default() -> Self {
        Self::new()
    }
}

/// Common words that start sentences with uppercase but are not part
/// of entity names.
const STOP_WORDS: &[&str] = &[
    "The", "This", "That", "These", "Those", "It", "Its", "They", "Their", "There", "Here",
    "Where", "When", "What", "Which", "Who", "How", "But", "And", "For", "Nor", "Yet", "So", "Or",
    "If", "Then", "Than", "Each", "Every", "Some", "Any", "All", "Most", "Many", "Much", "Such",
    "Our", "Your", "His", "Her", "We", "He", "She", "An", "As", "At", "Be", "By", "Do", "In", "Is",
    "No", "Of", "On", "To", "Up", "Was", "Are", "Has", "Had", "Not", "Did", "Can", "May", "Now",
    "Use", "With", "From", "Into", "Also", "Been", "Have", "Will", "Would", "Could", "Should",
    "About", "After", "Before", "Between", "During", "Under", "Over",
];

/// Extract multi-word capitalized terms from text.
///
/// Finds sequences of 2 or more words where each word starts with an
/// uppercase letter and is not a common stop word. This is a simple
/// heuristic for detecting entity names like "Natural Language
/// Processing" or "Machine Learning".
fn extract_multiword_terms(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current_words: Vec<&str> = Vec::new();

    for word in text.split_whitespace() {
        // Strip trailing punctuation for matching purposes
        let clean = word.trim_matches(|c: char| c.is_ascii_punctuation());
        if clean.is_empty() {
            flush_term(&mut current_words, &mut terms);
            continue;
        }

        // Safe: empty `clean` is handled by the `continue` above.
        let Some(first_char) = clean.chars().next() else {
            continue;
        };
        let is_stop = STOP_WORDS.contains(&clean);
        if first_char.is_uppercase() && clean.len() > 1 && !is_stop {
            current_words.push(clean);
        } else {
            flush_term(&mut current_words, &mut terms);
        }
    }
    flush_term(&mut current_words, &mut terms);

    terms
}

/// Flush accumulated capitalized words into a term if there are 2+
/// words.
fn flush_term(current_words: &mut Vec<&str>, terms: &mut Vec<String>) {
    if current_words.len() >= 2 {
        terms.push(current_words.join(" "));
    }
    current_words.clear();
}

/// Compute the abbreviation of a multi-word term by taking the first
/// letter of each word (uppercase).
///
/// Example: "Natural Language Processing" -> "NLP"
fn compute_abbreviation(term: &str) -> String {
    term.split_whitespace()
        .filter_map(|w| w.chars().next())
        .map(|c| c.to_uppercase().next().unwrap_or(c))
        .collect()
}

/// Check whether `text` contains `word` as a standalone word (not as
/// a substring of a larger word).
fn contains_word(text: &str, word: &str) -> bool {
    for candidate in text.split_whitespace() {
        let clean = candidate.trim_matches(|c: char| c.is_ascii_punctuation());
        if clean == word {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Neural coreference resolution via Fastcoref sidecar
// ---------------------------------------------------------------------------

/// Result of neural coreference resolution containing the resolved
/// text and byte offset mutations for the projection ledger.
pub struct CorefResult {
    /// Text with pronouns replaced by antecedents.
    pub resolved: String,
    /// Byte offset mutations recording each replacement. Empty if
    /// no pronouns were resolved.
    pub mutations: Vec<CorefMutation>,
}

/// Maximum input characters per coref window.
const COREF_MAX_CHARS: usize = 15_000;
/// Overlap between coref windows.
const COREF_OVERLAP_CHARS: usize = 500;
/// HTTP request timeout for coref calls.
const COREF_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Serialize)]
struct CorefRequest<'a> {
    texts: Vec<&'a str>,
}

#[derive(Debug, Deserialize)]
struct CorefResultData {
    #[allow(dead_code)]
    original: String,
    resolved: String,
    #[allow(dead_code)]
    clusters: Vec<Vec<String>>,
    /// Byte offset mutations from coreference replacement.
    /// Each mutation records the canonical (original) and mutated
    /// (resolved) byte spans plus the token strings. Used to build
    /// the offset projection ledger.
    #[serde(default)]
    mutations: Vec<CorefMutation>,
}

/// A single coreference replacement with byte offsets in both the
/// canonical (original) and mutated (resolved) texts.
#[derive(Debug, Clone, Deserialize)]
pub struct CorefMutation {
    /// Start byte offset in the original text.
    pub canonical_start: usize,
    /// End byte offset in the original text.
    pub canonical_end: usize,
    /// Start byte offset in the resolved (mutated) text.
    pub mutated_start: usize,
    /// End byte offset in the resolved (mutated) text.
    pub mutated_end: usize,
    /// The original mention token (e.g. "He").
    pub canonical_token: String,
    /// The replacement token (e.g. "Einstein").
    pub mutated_token: String,
}

#[derive(Debug, Deserialize)]
struct CorefResponse {
    results: Vec<CorefResultData>,
}

/// HTTP client for the Fastcoref neural coreference resolution sidecar.
///
/// Calls `POST /coref` to resolve pronouns and other anaphoric
/// references. Texts exceeding the model's context limit are
/// processed in overlapping windows and reassembled.
///
/// This is an independent preprocessing stage that runs **before**
/// entity extraction, so all extractor backends (sidecar, two_pass,
/// llm) benefit from neural coref.
pub struct FastcorefClient {
    /// HTTP client with timeout.
    client: reqwest::Client,
    /// Base URL of the sidecar (e.g., `http://localhost:8433`).
    base_url: String,
    /// Maximum input characters per coref window.
    max_chars: usize,
    /// Overlap between coref windows.
    overlap_chars: usize,
}

impl FastcorefClient {
    /// Create a new Fastcoref client with default windowing.
    pub fn new(base_url: String) -> Self {
        Self::with_windowing(base_url, COREF_MAX_CHARS, COREF_OVERLAP_CHARS)
    }

    /// Create a new Fastcoref client with custom windowing parameters.
    pub fn with_windowing(base_url: String, max_chars: usize, overlap_chars: usize) -> Self {
        let client = reqwest::Client::builder()
            .timeout(COREF_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            max_chars,
            overlap_chars,
        }
    }

    /// Resolve coreferences in the given text.
    ///
    /// Splits text into overlapping windows if it exceeds the
    /// model's context limit. Returns the resolved text with
    /// pronouns replaced by their antecedents, plus byte offset
    /// mutations for the offset projection ledger.
    pub async fn resolve(&self, text: &str) -> Result<CorefResult> {
        if text.trim().is_empty() {
            return Ok(CorefResult {
                resolved: text.to_string(),
                mutations: Vec::new(),
            });
        }

        if text.len() <= self.max_chars {
            return self.resolve_single(text).await;
        }

        let windows = split_text_windows(text, self.max_chars, self.overlap_chars);
        let mut resolved_parts = Vec::with_capacity(windows.len());
        let mut all_mutations = Vec::new();
        let mut mutated_byte_offset: usize = 0;
        let mut prev_canonical_end: usize = 0;

        for (win_idx, window) in windows.iter().enumerate() {
            // Compute the canonical start position of this window.
            // Safe: windows are subslices of the original text.
            let canonical_offset = window.as_ptr() as usize - text.as_ptr() as usize;

            // How many bytes at the start of this window overlap with the
            // previous window.
            let overlap_size = if win_idx > 0 {
                prev_canonical_end.saturating_sub(canonical_offset)
            } else {
                0
            };

            match self.resolve_single(window).await {
                Ok(result) => {
                    // Compute how many resolved-text bytes correspond to
                    // the overlap prefix so we can strip them.
                    let resolved_overlap =
                        compute_resolved_overlap(overlap_size, &result.mutations);

                    // Shift non-overlap mutations to global offsets.
                    for mut m in result.mutations {
                        // Skip mutations in the overlap prefix — already
                        // captured by the previous window.
                        if m.canonical_start < overlap_size {
                            continue;
                        }
                        // Canonical offsets: relative to original text.
                        m.canonical_start += canonical_offset;
                        m.canonical_end += canonical_offset;
                        // Mutated offsets: relative to the joined resolved
                        // text, accounting for overlap stripping.
                        m.mutated_start =
                            m.mutated_start.saturating_sub(resolved_overlap) + mutated_byte_offset;
                        m.mutated_end =
                            m.mutated_end.saturating_sub(resolved_overlap) + mutated_byte_offset;
                        all_mutations.push(m);
                    }

                    // Strip the overlap prefix from the resolved text.
                    let mut skip = resolved_overlap;
                    while skip < result.resolved.len() && !result.resolved.is_char_boundary(skip) {
                        skip += 1;
                    }
                    let part = if skip > 0 && skip < result.resolved.len() {
                        result.resolved[skip..].to_string()
                    } else if skip >= result.resolved.len() {
                        String::new()
                    } else {
                        result.resolved
                    };
                    mutated_byte_offset += part.len();
                    resolved_parts.push(part);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "coref window failed, using original");
                    let skip = overlap_size.min(window.len());
                    let part = &window[skip..];
                    mutated_byte_offset += part.len();
                    resolved_parts.push(part.to_string());
                }
            }
            prev_canonical_end = canonical_offset + window.len();
        }

        Ok(CorefResult {
            resolved: resolved_parts.concat(),
            mutations: all_mutations,
        })
    }

    /// Call `/coref` for a single text.
    async fn resolve_single(&self, text: &str) -> Result<CorefResult> {
        let body = CorefRequest { texts: vec![text] };
        let resp = self
            .client
            .post(format!("{}/coref", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Ingestion(format!("fastcoref request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(Error::Ingestion(format!(
                "fastcoref returned {status}: {body_text}"
            )));
        }

        let parsed: CorefResponse = resp
            .json()
            .await
            .map_err(|e| Error::Ingestion(format!("failed to parse coref response: {e}")))?;

        match parsed.results.into_iter().next() {
            Some(r) => Ok(CorefResult {
                resolved: r.resolved,
                mutations: r.mutations,
            }),
            None => Ok(CorefResult {
                resolved: text.to_string(),
                mutations: Vec::new(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Windowing utilities (shared with sidecar_extractor)
// ---------------------------------------------------------------------------

/// Compute the byte length in the resolved text that corresponds to
/// `overlap_size` bytes in the canonical text, accounting for mutations
/// that expand or contract text within the overlap region.
fn compute_resolved_overlap(overlap_size: usize, mutations: &[CorefMutation]) -> usize {
    if overlap_size == 0 {
        return 0;
    }
    let mut delta: isize = 0;
    for m in mutations {
        if m.canonical_start >= overlap_size {
            break;
        }
        if m.canonical_end <= overlap_size {
            // Mutation fully inside the overlap — count all of its delta.
            delta += m.mutated_token.len() as isize - m.canonical_token.len() as isize;
        }
        // Straddling mutations (canonical_start < overlap_size but
        // canonical_end > overlap_size) are ambiguous: the replacement
        // spans the boundary and was already captured by the previous
        // window. Skip their delta entirely — the non-overlap portion
        // will be re-resolved in this window.
    }
    (overlap_size as isize + delta).max(0) as usize
}

/// Split text into overlapping windows at sentence boundaries.
///
/// Each window is at most `max_chars` long. Windows overlap by
/// `overlap_chars` to avoid missing entities at boundaries.
/// If the text is shorter than `max_chars`, returns a single window.
pub(crate) fn split_text_windows(text: &str, max_chars: usize, overlap_chars: usize) -> Vec<&str> {
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
        // Ensure start is at a valid UTF-8 boundary.
        while start < text.len() && !text.is_char_boundary(start) {
            start += 1;
        }
    }

    windows
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn make_chunk(text: &str) -> ChunkOutput {
        ChunkOutput {
            id: Uuid::new_v4(),
            parent_id: None,
            text: text.to_string(),
            level: crate::ingestion::chunker::ChunkLevel::Paragraph,
            heading_path: vec![],
            context_prefix_len: 0,
            byte_start: 0,
            byte_end: 0,
        }
    }

    #[test]
    fn abbreviation_detection_nlp() {
        let chunks = vec![
            make_chunk("Natural Language Processing is a field of AI."),
            make_chunk("NLP has many applications in search."),
        ];
        let resolver = CorefResolver::new();
        let links = resolver.resolve(&chunks);

        let nlp_link = links.iter().find(|l| l.mention == "NLP");
        assert!(
            nlp_link.is_some(),
            "Expected a CorefLink for NLP, got: {links:?}"
        );
        let link = nlp_link.unwrap();
        assert_eq!(link.referent, "Natural Language Processing");
        assert_eq!(link.chunk_index, 1);
    }

    #[test]
    fn abbreviation_detection_ml() {
        let chunks = vec![
            make_chunk("Machine Learning powers modern AI systems."),
            make_chunk("ML models require large datasets."),
        ];
        let resolver = CorefResolver::new();
        let links = resolver.resolve(&chunks);

        let ml_link = links.iter().find(|l| l.mention == "ML");
        assert!(
            ml_link.is_some(),
            "Expected a CorefLink for ML, got: {links:?}"
        );
        assert_eq!(ml_link.unwrap().referent, "Machine Learning");
    }

    #[test]
    fn no_false_positives_for_single_words() {
        let chunks = vec![make_chunk("Alice went to the store.")];
        let resolver = CorefResolver::new();
        let links = resolver.resolve(&chunks);
        assert!(
            links.is_empty(),
            "Single words should not produce links: {links:?}"
        );
    }

    #[test]
    fn abbreviation_not_emitted_without_occurrence() {
        let chunks = vec![make_chunk("Natural Language Processing is great.")];
        let resolver = CorefResolver::new();
        let links = resolver.resolve(&chunks);
        // "NLP" never appears in the text, so no CorefLink
        let nlp_link = links.iter().find(|l| l.mention == "NLP");
        assert!(
            nlp_link.is_none(),
            "Should not emit link when abbreviation is absent"
        );
    }

    #[test]
    fn multiple_abbreviations_in_same_document() {
        let chunks = vec![
            make_chunk(
                "Natural Language Processing and Machine Learning \
                 are subfields of Artificial Intelligence.",
            ),
            make_chunk("NLP and ML are widely used. AI is everywhere."),
        ];
        let resolver = CorefResolver::new();
        let links = resolver.resolve(&chunks);

        let mentions: Vec<&str> = links.iter().map(|l| l.mention.as_str()).collect();
        assert!(mentions.contains(&"NLP"), "missing NLP: {links:?}");
        assert!(mentions.contains(&"ML"), "missing ML: {links:?}");
    }

    #[test]
    fn compute_abbreviation_basic() {
        assert_eq!(compute_abbreviation("Natural Language Processing"), "NLP");
        assert_eq!(compute_abbreviation("Machine Learning"), "ML");
        assert_eq!(compute_abbreviation("Artificial Intelligence"), "AI");
    }

    #[test]
    fn extract_multiword_terms_finds_capitalized_phrases() {
        let terms =
            extract_multiword_terms("The Natural Language Processing field is growing fast.");
        assert!(
            terms.contains(&"Natural Language Processing".to_string()),
            "Expected 'Natural Language Processing', got: {terms:?}"
        );
    }

    #[test]
    fn contains_word_boundary_check() {
        assert!(contains_word("NLP is great", "NLP"));
        assert!(contains_word("I love NLP.", "NLP"));
        assert!(!contains_word("NLPX is not NLP", "NLPX is"));
        // Standalone word match
        assert!(contains_word("use NLP today", "NLP"));
    }

    // --- split_text_windows tests ---

    #[test]
    fn split_short_text_single_window() {
        let text = "Hello world.";
        let windows = split_text_windows(text, 1200, 200);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0], text);
    }

    #[test]
    fn split_long_text_multiple_windows() {
        let sentences: Vec<String> = (0..20)
            .map(|i| format!("Sentence number {i} is here. "))
            .collect();
        let text = sentences.join("");
        assert!(text.len() > 200);

        let windows = split_text_windows(&text, 200, 50);
        assert!(
            windows.len() > 1,
            "expected multiple windows, got {}",
            windows.len()
        );

        for (i, w) in windows.iter().enumerate() {
            assert!(w.len() <= 200, "window {i} too long: {} chars", w.len());
        }
    }

    #[test]
    fn split_respects_sentence_boundaries() {
        let text = "First sentence. Second sentence. Third sentence. \
                     Fourth sentence. Fifth sentence.";
        let windows = split_text_windows(text, 40, 10);
        for w in &windows {
            let trimmed = w.trim();
            if !trimmed.is_empty() {
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
        let text = "a".repeat(5000);
        let windows = split_text_windows(&text, 1200, 200);
        assert!(windows.len() >= 4);
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
    fn split_multibyte_utf8_does_not_panic() {
        // Text with 3-byte UTF-8 characters (→) that could cause
        // boundary issues when overlap subtraction lands mid-char.
        let text = "O(d) where d=D→s for a fixed k. ".repeat(100);
        let windows = split_text_windows(&text, 200, 50);
        assert!(!windows.is_empty());
        for w in &windows {
            assert!(w.len() <= 250, "window too large: {}", w.len());
        }
    }

    // --- FastcorefClient tests ---

    #[test]
    fn coref_response_deserialization_without_mutations() {
        // Backward compatible: no mutations field.
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
        assert!(resp.results[0].mutations.is_empty());
    }

    #[test]
    fn coref_response_deserialization_with_mutations() {
        let json = serde_json::json!({
            "results": [{
                "original": "Einstein published his theory. He won.",
                "resolved": "Einstein published Einstein's theory. Einstein won.",
                "clusters": [["Einstein", "his", "He"]],
                "mutations": [
                    {
                        "canonical_start": 20,
                        "canonical_end": 23,
                        "mutated_start": 20,
                        "mutated_end": 30,
                        "canonical_token": "his",
                        "mutated_token": "Einstein's"
                    },
                    {
                        "canonical_start": 32,
                        "canonical_end": 34,
                        "mutated_start": 39,
                        "mutated_end": 47,
                        "canonical_token": "He",
                        "mutated_token": "Einstein"
                    }
                ]
            }]
        });
        let resp: CorefResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.results[0].mutations.len(), 2);
        assert_eq!(resp.results[0].mutations[0].canonical_token, "his");
        assert_eq!(resp.results[0].mutations[0].mutated_token, "Einstein's");
        assert_eq!(resp.results[0].mutations[1].canonical_start, 32);
    }

    #[tokio::test]
    async fn fastcoref_empty_text_noop() {
        let client = FastcorefClient::new("http://localhost:9999".to_string());
        let result = client.resolve("   ").await.unwrap();
        assert_eq!(result.resolved, "   ");
        assert!(result.mutations.is_empty());
    }

    #[test]
    fn fastcoref_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FastcorefClient>();
    }

    // --- compute_resolved_overlap tests ---

    #[test]
    fn resolved_overlap_zero() {
        assert_eq!(compute_resolved_overlap(0, &[]), 0);
    }

    #[test]
    fn resolved_overlap_no_mutations() {
        assert_eq!(compute_resolved_overlap(100, &[]), 100);
    }

    #[test]
    fn resolved_overlap_with_expansion() {
        // "He" (2 bytes) → "Einstein" (8 bytes) within the overlap.
        let mutations = vec![CorefMutation {
            canonical_start: 10,
            canonical_end: 12,
            mutated_start: 10,
            mutated_end: 18,
            canonical_token: "He".to_string(),
            mutated_token: "Einstein".to_string(),
        }];
        // overlap=50, mutation adds 6 bytes → 56
        assert_eq!(compute_resolved_overlap(50, &mutations), 56);
    }

    #[test]
    fn resolved_overlap_mutation_outside() {
        // Mutation at byte 60 is outside the 50-byte overlap.
        let mutations = vec![CorefMutation {
            canonical_start: 60,
            canonical_end: 62,
            mutated_start: 60,
            mutated_end: 68,
            canonical_token: "He".to_string(),
            mutated_token: "Einstein".to_string(),
        }];
        assert_eq!(compute_resolved_overlap(50, &mutations), 50);
    }

    #[test]
    fn resolved_overlap_with_contraction() {
        // "Einstein" (8 bytes) → "He" (2 bytes) — contracts by 6.
        let mutations = vec![CorefMutation {
            canonical_start: 5,
            canonical_end: 13,
            mutated_start: 5,
            mutated_end: 7,
            canonical_token: "Einstein".to_string(),
            mutated_token: "He".to_string(),
        }];
        // overlap=50, mutation removes 6 bytes → 44
        assert_eq!(compute_resolved_overlap(50, &mutations), 44);
    }

    #[test]
    fn resolved_overlap_multiple_mutations() {
        let mutations = vec![
            CorefMutation {
                canonical_start: 5,
                canonical_end: 7,
                mutated_start: 5,
                mutated_end: 13,
                canonical_token: "He".to_string(),
                mutated_token: "Einstein".to_string(),
            },
            CorefMutation {
                canonical_start: 20,
                canonical_end: 23,
                mutated_start: 26,
                mutated_end: 36,
                canonical_token: "his".to_string(),
                mutated_token: "Einstein's".to_string(),
            },
        ];
        // overlap=30: mutation 1 adds 6, mutation 2 adds 7 → 30 + 13 = 43
        assert_eq!(compute_resolved_overlap(30, &mutations), 43);
    }

    #[test]
    fn resolved_overlap_straddling_mutation() {
        // Mutation straddles the overlap boundary: canonical_start inside
        // the overlap but canonical_end outside. Its delta must NOT be
        // counted because the previous window already captured it and
        // including it would inflate resolved_overlap, causing underflow
        // on mutated_start subtraction.
        let mutations = vec![CorefMutation {
            canonical_start: 45,
            canonical_end: 55,
            mutated_start: 45,
            mutated_end: 60,
            canonical_token: "0123456789".to_string(), // 10 bytes
            mutated_token: "0123456789abcde".to_string(), // 15 bytes
        }];
        // overlap=50: straddling mutation is skipped → still 50
        assert_eq!(compute_resolved_overlap(50, &mutations), 50);
    }
}

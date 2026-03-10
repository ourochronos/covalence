//! Co-reference resolution within a single document.
//!
//! Provides a heuristic-based resolver that detects abbreviations
//! (e.g., "Natural Language Processing" -> "NLP") and maps them back
//! to the full entity name. This is a v1 starting point that can be
//! replaced with LLM-based co-reference resolution later.

use std::collections::HashMap;

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

        let first_char = clean.chars().next().unwrap();
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
}

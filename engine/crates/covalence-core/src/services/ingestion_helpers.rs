//! Pure helper functions used during source ingestion.
//!
//! Extracted from `source.rs` for modularity. These are all
//! non-`SourceService` functions that handle content analysis,
//! update detection, extraction batching, and entity locking.

use sha2::{Digest, Sha256};

use crate::models::source::UpdateClass;
use crate::types::ids::SourceId;

/// Fixed namespace offset for entity-resolution advisory locks.
///
/// This constant is XORed into the hash to avoid collisions with
/// advisory locks used for other purposes in the same database.
pub(crate) const ENTITY_LOCK_NAMESPACE: i64 = 0x436F_7661_6C65_6E63; // "Covalenc"

/// Produce a deterministic i64 hash from an entity name for use as a
/// PostgreSQL advisory lock key.
///
/// Uses the first 8 bytes of a SHA-256 hash of the lowercased,
/// trimmed name, XORed with [`ENTITY_LOCK_NAMESPACE`] to avoid
/// collisions with other advisory lock users.
pub(crate) fn entity_name_lock_key(name: &str) -> i64 {
    let canonical = name.trim().to_lowercase();
    let hash = Sha256::digest(canonical.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&hash[..8]);
    i64::from_le_bytes(bytes) ^ ENTITY_LOCK_NAMESPACE
}

/// Information about a source supersession detected during
/// URI-based update class analysis.
pub(crate) struct SupersedesInfo {
    /// The ID of the source being superseded.
    pub old_source_id: SourceId,
    /// The detected update class.
    pub update_class: UpdateClass,
    /// The version number for the new source.
    pub new_version: i32,
}

/// Group extractable chunks into token-budget batches for
/// efficient LLM usage.
///
/// - Chunks with fewer than `min_tokens` whitespace-delimited words
///   are skipped entirely.
/// - Adjacent chunks are concatenated (separated by `\n\n---\n\n`)
///   up to `batch_tokens`. A single chunk exceeding the budget is
///   extracted alone.
///
/// Returns a list of `(primary_chunk_uuid, combined_text)` pairs.
/// The primary UUID is used for extraction provenance.
pub(crate) fn group_extraction_batches(
    chunks: &[&crate::ingestion::chunker::ChunkOutput],
    min_tokens: usize,
    batch_tokens: usize,
    resolved_texts: Option<&std::collections::HashMap<uuid::Uuid, String>>,
) -> Vec<(uuid::Uuid, String)> {
    let mut batches: Vec<(uuid::Uuid, String)> = Vec::new();
    let mut current_primary: Option<uuid::Uuid> = None;
    let mut current_text = String::new();
    let mut current_tokens: usize = 0;
    let mut skipped = 0_usize;

    for co in chunks {
        let text = resolved_texts
            .and_then(|m| m.get(&co.id))
            .map(|s| s.as_str())
            .unwrap_or(&co.text);
        let tokens = text.split_whitespace().count();

        if tokens < min_tokens {
            skipped += 1;
            continue;
        }

        // Flush if adding this chunk would exceed the budget
        // (but only if we already have content).
        if current_primary.is_some() && current_tokens + tokens > batch_tokens {
            if let Some(primary) = current_primary.take() {
                batches.push((primary, std::mem::take(&mut current_text)));
            }
            current_tokens = 0;
        }

        if current_primary.is_none() {
            current_primary = Some(co.id);
        }
        if !current_text.is_empty() {
            current_text.push_str("\n\n---\n\n");
        }
        current_text.push_str(text);
        current_tokens += tokens;
    }

    // Flush remaining
    if let Some(primary) = current_primary {
        batches.push((primary, current_text));
    }

    if skipped > 0 {
        tracing::debug!(
            skipped,
            min_tokens,
            "chunks skipped (below min_extract_tokens)"
        );
    }

    batches
}

/// Detect the update class by comparing old and new content.
///
/// Uses a simple word-level Jaccard similarity metric:
/// - `>=80%` overlap: `Correction` (minor fix to existing content)
/// - `<20%` overlap: `Refactor` (structural rewrite)
/// - Otherwise: `Versioned` (normal update)
///
/// This is a lightweight heuristic. Production systems may use
/// more sophisticated diff algorithms.
pub(crate) fn detect_update_class(old_text: &str, new_text: &str) -> UpdateClass {
    let overlap = content_overlap(old_text, new_text);
    if overlap >= 0.80 {
        UpdateClass::Correction
    } else if overlap < 0.20 {
        UpdateClass::Refactor
    } else {
        UpdateClass::Versioned
    }
}

/// Compute word-level Jaccard similarity between two texts.
///
/// Returns a value in `[0.0, 1.0]` representing the proportion
/// of shared words between the two texts. Returns 0.0 if both
/// texts are empty.
pub(crate) fn content_overlap(a: &str, b: &str) -> f64 {
    let words_a: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let words_b: std::collections::HashSet<&str> = b.split_whitespace().collect();

    if words_a.is_empty() && words_b.is_empty() {
        return 0.0;
    }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    if union == 0 {
        return 0.0;
    }

    intersection as f64 / union as f64
}

/// Detect content types in a chunk's Markdown text.
///
/// Sets boolean flags for table content, fenced code blocks,
/// and lists. Used to enrich the chunk metadata JSONB during
/// ingestion so search and extraction can adapt to content type.
pub(crate) fn detect_chunk_content_types(text: &str) -> serde_json::Value {
    let mut contains_table = false;
    let mut contains_code = false;
    let mut contains_list = false;
    let mut in_code_fence = false;

    for line in text.lines() {
        let trimmed = line.trim();

        // Detect fenced code blocks (``` or ~~~).
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code_fence = !in_code_fence;
            contains_code = true;
            continue;
        }

        // Skip content inside code fences for other detection.
        if in_code_fence {
            continue;
        }

        // Detect Markdown tables: lines starting and ending with `|`
        // or containing `|` with at least 2 cells.
        if trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.matches('|').count() >= 3 {
            contains_table = true;
        }

        // Detect unordered lists (-, *, +) and ordered lists (1.).
        if trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("+ ")
            || (trimmed.len() >= 3
                && trimmed.as_bytes()[0].is_ascii_digit()
                && trimmed.contains(". "))
        {
            contains_list = true;
        }
    }

    // Detect example/hypothetical context markers.
    let contains_example = has_example_markers(text);

    serde_json::json!({
        "contains_table": contains_table,
        "contains_code": contains_code,
        "contains_list": contains_list,
        "contains_example": contains_example,
    })
}

/// Check whether text contains markers indicating illustrative
/// examples, hypothetical scenarios, or placeholder content.
///
/// When a chunk is tagged as containing examples, extracted
/// entities are given reduced confidence to limit their
/// influence on the knowledge graph.
pub(crate) fn has_example_markers(text: &str) -> bool {
    let lower = text.to_lowercase();
    let markers = [
        "for example",
        "for instance",
        "e.g.",
        "e.g.,",
        "suppose ",
        "consider the case",
        "hypothetical",
        "as an illustration",
        "imagine that",
        "let's say",
        "assume that",
        "in this scenario",
        "a simple example",
        "toy example",
    ];
    markers.iter().any(|m| lower.contains(m))
}

/// Sanitize a string for use as an ltree label.
///
/// ltree labels can only contain alphanumeric characters and
/// underscores. All other characters (spaces, hyphens, punctuation,
/// etc.) are replaced with `_`. Empty labels become `"_"`.
pub(crate) fn sanitize_ltree_label(s: &str) -> String {
    let sanitized: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::chunk::Chunk;

    // --- Entity name lock key tests ---

    #[test]
    fn entity_name_lock_key_is_deterministic() {
        let a = entity_name_lock_key("Rust");
        let b = entity_name_lock_key("Rust");
        assert_eq!(a, b);
    }

    #[test]
    fn entity_name_lock_key_is_case_insensitive() {
        let lower = entity_name_lock_key("rust");
        let upper = entity_name_lock_key("RUST");
        let mixed = entity_name_lock_key("RuSt");
        assert_eq!(lower, upper);
        assert_eq!(lower, mixed);
    }

    #[test]
    fn entity_name_lock_key_trims_whitespace() {
        let plain = entity_name_lock_key("rust");
        let padded = entity_name_lock_key("  rust  ");
        assert_eq!(plain, padded);
    }

    #[test]
    fn entity_name_lock_key_differs_for_different_names() {
        let a = entity_name_lock_key("rust");
        let b = entity_name_lock_key("python");
        assert_ne!(a, b);
    }

    #[test]
    fn entity_name_lock_key_includes_namespace() {
        // Compute what the raw hash would be without the XOR and
        // verify the function's output differs (proving the
        // namespace is applied).
        let canonical = "test_entity";
        let hash = Sha256::digest(canonical.as_bytes());
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&hash[..8]);
        let raw = i64::from_le_bytes(bytes);

        let keyed = entity_name_lock_key(canonical);
        assert_ne!(raw, keyed);
        assert_eq!(keyed, raw ^ ENTITY_LOCK_NAMESPACE);
    }

    #[test]
    fn entity_name_lock_key_empty_string() {
        // Should not panic on empty input.
        let _ = entity_name_lock_key("");
    }

    // --- Sanitize ltree label tests ---

    #[test]
    fn sanitize_ltree_label_basic() {
        assert_eq!(sanitize_ltree_label("hello world"), "hello_world");
        assert_eq!(sanitize_ltree_label(""), "_");
        assert_eq!(sanitize_ltree_label("a-b"), "a_b");
        assert_eq!(sanitize_ltree_label("abc_123"), "abc_123");
    }

    // --- Content overlap tests ---

    #[test]
    fn content_overlap_identical() {
        let text = "the quick brown fox jumps over the lazy dog";
        let overlap = content_overlap(text, text);
        assert!((overlap - 1.0).abs() < 1e-10);
    }

    #[test]
    fn content_overlap_no_shared_words() {
        let a = "alpha beta gamma";
        let b = "one two three";
        let overlap = content_overlap(a, b);
        assert!((overlap - 0.0).abs() < 1e-10);
    }

    #[test]
    fn content_overlap_partial() {
        let a = "the quick brown fox";
        let b = "the slow brown bear";
        // Shared: "the", "brown" = 2
        // Union: "the", "quick", "brown", "fox", "slow", "bear" = 6
        let overlap = content_overlap(a, b);
        assert!((overlap - 2.0 / 6.0).abs() < 1e-10);
    }

    #[test]
    fn content_overlap_both_empty() {
        assert!((content_overlap("", "") - 0.0).abs() < 1e-10);
    }

    #[test]
    fn content_overlap_one_empty() {
        assert!((content_overlap("hello", "") - 0.0).abs() < 1e-10);
        assert!((content_overlap("", "hello") - 0.0).abs() < 1e-10);
    }

    // --- Update class detection tests ---

    #[test]
    fn detect_update_class_correction() {
        // >80% overlap = correction (minor edit)
        // 9/10 words shared = 0.9 Jaccard
        let old = "the quick brown fox jumps over the lazy dog today";
        let new = "the quick brown fox leaps over the lazy dog today";
        let class = detect_update_class(old, new);
        assert_eq!(class, UpdateClass::Correction);
    }

    #[test]
    fn detect_update_class_refactor() {
        // <20% overlap = refactor (complete rewrite)
        let old = "alpha beta gamma delta epsilon";
        let new = "one two three four five six seven";
        let class = detect_update_class(old, new);
        assert_eq!(class, UpdateClass::Refactor);
    }

    #[test]
    fn detect_update_class_versioned() {
        // Between 20%-80% overlap = versioned (significant update)
        // Shared: a, b, c, d, e = 5 out of union 10 = 0.5
        let old = "a b c d e f g h i j";
        let new = "a b c d e k l m n o";
        let class = detect_update_class(old, new);
        assert_eq!(class, UpdateClass::Versioned);
    }

    // --- Content type detection tests ---

    #[test]
    fn detect_table_content() {
        let text =
            "Some intro text.\n\n| Name | Value |\n|------|-------|\n| A | 1 |\n\nMore text.";
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_table"], true);
        assert_eq!(meta["contains_code"], false);
        assert_eq!(meta["contains_list"], false);
    }

    #[test]
    fn detect_code_content() {
        let text = "Some text.\n\n```rust\nfn main() {}\n```\n\nMore text.";
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_table"], false);
        assert_eq!(meta["contains_code"], true);
    }

    #[test]
    fn detect_list_content() {
        let text = "Items:\n- First item\n- Second item\n* Third item";
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_list"], true);
        assert_eq!(meta["contains_table"], false);
    }

    #[test]
    fn detect_ordered_list() {
        let text = "Steps:\n1. Do this\n2. Do that";
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_list"], true);
    }

    #[test]
    fn detect_no_special_content() {
        let text = "Just a plain paragraph with no special formatting.";
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_table"], false);
        assert_eq!(meta["contains_code"], false);
        assert_eq!(meta["contains_list"], false);
    }

    #[test]
    fn detect_mixed_content() {
        let text =
            "# Section\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n```python\nprint('hi')\n```\n\n- item";
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_table"], true);
        assert_eq!(meta["contains_code"], true);
        assert_eq!(meta["contains_list"], true);
    }

    #[test]
    fn pipe_inside_code_fence_not_table() {
        let text = "```\necho \"a | b | c\"\n```";
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_table"], false);
        assert_eq!(meta["contains_code"], true);
    }

    #[test]
    fn detect_example_marker_for_example() {
        let text = "For example, Alice sends a message to Bob.";
        assert!(has_example_markers(text));
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_example"], true);
    }

    #[test]
    fn detect_example_marker_suppose() {
        let text = "Suppose we have a network of three nodes.";
        assert!(has_example_markers(text));
    }

    #[test]
    fn detect_example_marker_eg() {
        let text = "Various types exist (e.g., person, location, event).";
        assert!(has_example_markers(text));
    }

    #[test]
    fn detect_no_example_in_factual_text() {
        let text = "HDBSCAN uses hierarchical density-based clustering \
                    to find natural groups in data.";
        assert!(!has_example_markers(text));
        let meta = detect_chunk_content_types(text);
        assert_eq!(meta["contains_example"], false);
    }

    #[test]
    fn detect_example_hypothetical() {
        let text = "In this hypothetical scenario, the agent trusts \
                    only verified peers.";
        assert!(has_example_markers(text));
    }

    // --- Extraction batch grouping tests ---

    fn make_chunk_output(text: &str) -> crate::ingestion::chunker::ChunkOutput {
        crate::ingestion::chunker::ChunkOutput {
            id: uuid::Uuid::new_v4(),
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
    fn batch_skips_tiny_chunks() {
        let c1 = make_chunk_output("too small");
        let c2 = make_chunk_output(
            "This chunk has enough tokens to pass the minimum threshold for extraction.",
        );
        let chunks: Vec<&_> = vec![&c1, &c2];
        let batches = group_extraction_batches(&chunks, 5, 2000, None);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].0, c2.id);
        assert!(!batches[0].1.contains("too small"));
    }

    #[test]
    fn batch_groups_adjacent_small_chunks() {
        // 10 words each, budget = 50, so ~5 chunks per batch
        let chunks: Vec<_> = (0..10)
            .map(|i| {
                make_chunk_output(&format!(
                    "Chunk number {i} has exactly ten words in this sentence here."
                ))
            })
            .collect();
        let refs: Vec<&_> = chunks.iter().collect();
        let batches = group_extraction_batches(&refs, 5, 50, None);
        // Should produce multiple batches (each ~50 tokens)
        assert!(
            batches.len() >= 2,
            "expected multiple batches, got {}",
            batches.len()
        );
        // Each batch text should contain separator
        for (_, text) in &batches {
            if text.contains("---") {
                // Multi-chunk batch — has separators
                assert!(text.matches("---").count() >= 1);
            }
        }
    }

    #[test]
    fn batch_single_large_chunk_alone() {
        let big = make_chunk_output(&"word ".repeat(100));
        let small = make_chunk_output(&"word ".repeat(10));
        let chunks: Vec<&_> = vec![&big, &small];
        let batches = group_extraction_batches(&chunks, 5, 50, None);
        // The big chunk should be in its own batch
        assert!(batches.len() >= 2);
        assert_eq!(batches[0].0, big.id);
    }

    #[test]
    fn batch_empty_input() {
        let batches = group_extraction_batches(&[], 30, 2000, None);
        assert!(batches.is_empty());
    }

    #[test]
    fn batch_all_below_threshold() {
        let c1 = make_chunk_output("tiny");
        let c2 = make_chunk_output("also tiny");
        let chunks: Vec<&_> = vec![&c1, &c2];
        let batches = group_extraction_batches(&chunks, 30, 2000, None);
        assert!(batches.is_empty());
    }

    #[test]
    fn batch_uses_resolved_texts() {
        let c1 = make_chunk_output("He went to the store.");
        let mut resolved = std::collections::HashMap::new();
        resolved.insert(
            c1.id,
            "John went to the grocery store to buy supplies.".to_string(),
        );
        let chunks: Vec<&_> = vec![&c1];
        let batches = group_extraction_batches(&chunks, 5, 2000, Some(&resolved));
        assert_eq!(batches.len(), 1);
        assert!(batches[0].1.contains("John"));
    }
}

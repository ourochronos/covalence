//! Pure text-processing helpers for the search service.
//!
//! These functions handle chunk name derivation, snippet extraction,
//! heading qualification, and other text transformations used during
//! search result enrichment. They have no dependency on
//! [`super::search::SearchService`] and are fully unit-testable.

use crate::search::strategy::SearchStrategy;

/// Demotion factor applied to bare entity nodes in content-focused
/// search strategies. Entity nodes (e.g., "GraphRAG" with no content)
/// rank high on lexical + graph + structural dimensions but provide
/// no useful content to the user. This factor pushes them below
/// chunks and articles without removing them entirely.
pub(super) const ENTITY_DEMOTION_FACTOR: f64 = 0.3;

/// Maximum length for derived chunk names.
pub(super) const MAX_CHUNK_NAME_LEN: usize = 80;

/// Generic section headings that benefit from source-title
/// qualification (e.g., "Overview" → "Paper Title: Overview").
const GENERIC_HEADINGS: &[&str] = &[
    "overview",
    "introduction",
    "abstract",
    "summary",
    "background",
    "conclusion",
    "conclusions",
    "discussion",
    "methods",
    "methodology",
    "results",
    "implementation",
    "architecture",
    "design",
    "analysis",
    "evaluation",
    "related work",
    "future work",
    "appendix",
    "references",
    "acknowledgments",
    "prerequisites",
    "setup",
    "configuration",
    "usage",
    "examples",
    "getting started",
    "installation",
    "motivation",
];

/// Truncate a string to at most `max_bytes` bytes, snapping backward
/// to a valid UTF-8 character boundary. Appends `"..."` if truncated.
///
/// This prevents panics from slicing multi-byte characters (emoji,
/// CJK, accented characters) at arbitrary byte positions.
pub(super) fn truncate_with_ellipsis(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes.saturating_sub(3);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

/// Extract a keyword-in-context (KWIC) snippet from content.
///
/// Finds the best window of text surrounding query terms. Falls back
/// to `truncate_with_ellipsis` if no query terms appear in the content.
pub(super) fn kwic_snippet(content: &str, query: &str, window_bytes: usize) -> String {
    let content_lower = content.to_lowercase();
    let terms: Vec<&str> = query.split_whitespace().filter(|t| t.len() >= 3).collect();

    if terms.is_empty() {
        return truncate_with_ellipsis(content, window_bytes);
    }

    // Find the position of the best term match (longest term first
    // for more specific hits).
    let mut best_pos = None;
    let mut best_len = 0;
    for term in &terms {
        let term_lower = term.to_lowercase();
        if let Some(pos) = content_lower.find(&term_lower) {
            if term_lower.len() > best_len {
                best_pos = Some(pos);
                best_len = term_lower.len();
            }
        }
    }

    let Some(match_pos) = best_pos else {
        return truncate_with_ellipsis(content, window_bytes);
    };

    // Center the window around the match.
    let half = window_bytes / 2;
    let mut start = match_pos.saturating_sub(half);
    // Walk to a char boundary.
    while start > 0 && !content.is_char_boundary(start) {
        start -= 1;
    }
    // Snap forward to a word boundary (avoid starting mid-word).
    if start > 0 {
        if let Some(space) = content[start..].find(|c: char| c.is_whitespace()) {
            let candidate = start + space + 1;
            // Only snap forward if we don't skip too far (< 30 chars).
            if candidate < content.len() && candidate - start < 30 {
                start = candidate;
            }
        }
    }

    let mut end = (start + window_bytes).min(content.len());
    while end < content.len() && !content.is_char_boundary(end) {
        end += 1;
    }
    // Snap backward to a word boundary (avoid ending mid-word).
    if end < content.len() {
        if let Some(space) = content[..end].rfind(|c: char| c.is_whitespace()) {
            // Only snap back if we don't lose too much (< 30 chars).
            if end - space < 30 {
                end = space;
            }
        }
    }

    let slice = &content[start..end];
    let prefix = if start > 0 { "..." } else { "" };
    let suffix = if end < content.len() { "..." } else { "" };
    format!("{prefix}{slice}{suffix}")
}

/// Derive a readable name from chunk content, optionally qualified
/// by a source title when the heading is generic.
///
/// Strategy:
/// 1. Skip leading lines that are metadata (bold labels, links, refs).
/// 2. If a Markdown heading (`# ...`) is found, use it (qualify with
///    source title if the heading is generic like "Overview").
/// 3. Otherwise, take the first meaningful sentence, truncated to
///    [`MAX_CHUNK_NAME_LEN`].
#[cfg(test)]
fn derive_chunk_name(content: &str) -> String {
    derive_chunk_name_qualified(content, None)
}

/// Derive a chunk name, qualifying generic headings with the
/// source title when available.
pub(super) fn derive_chunk_name_qualified(content: &str, source_title: Option<&str>) -> String {
    let trimmed = content.trim();

    // Try to find the first meaningful line: skip bold labels
    // (`**Key:** ...`), link-only lines (`[ref](url)`), and very
    // short lines (< 10 chars) that are usually metadata.
    let meaningful = trimmed.lines().find(|line| {
        let l = line.trim();
        if l.is_empty() {
            return false;
        }
        // Skip bold-label lines: **Label:** value
        if l.starts_with("**") && l.contains(":**") {
            return false;
        }
        // Skip bare markdown links: [text](url)
        if l.starts_with('[') && l.contains("](") && l.len() < 100 {
            return false;
        }
        // Skip arxiv-style references: [2506.12345]
        if l.starts_with('[')
            && l.len() < 20
            && l.chars().skip(1).take(4).all(|c| c.is_ascii_digit())
        {
            return false;
        }
        // Skip bare numbered list items: "3.", "4.", "1)"
        if l.len() < 5
            && l.chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == ')')
        {
            return false;
        }
        // Skip code fence lines: ```rust, ```python, ```
        if l.starts_with("```") {
            return false;
        }
        true
    });

    // If no meaningful line was found (all lines were metadata),
    // use the first line as a last resort, stripping bold markers.
    let meaningful = match meaningful {
        Some(line) => line.trim(),
        None => {
            let first = trimmed.lines().next().unwrap_or(trimmed).trim();
            // Strip bold labels for display: "**Authors:** X" → "Authors: X"
            let cleaned = strip_inline_markdown(first);
            let cleaned = cleaned.trim();
            if cleaned.is_empty() {
                return String::new();
            }
            // Truncate and return directly — skip heading logic.
            return truncate_with_ellipsis(cleaned, MAX_CHUNK_NAME_LEN);
        }
    };

    if meaningful.is_empty() {
        return String::new();
    }

    // Check for Markdown heading.
    if let Some(heading) = meaningful.strip_prefix('#') {
        let heading = heading.trim_start_matches('#').trim();
        let heading = if let Some(end) = heading.find('\n') {
            heading[..end].trim()
        } else {
            heading
        };
        return qualify_heading(heading, source_title);
    }

    // Strip inline markdown formatting: **bold**, *italic*, `code`.
    let clean = strip_inline_markdown(meaningful);
    let clean = clean.trim();

    // First sentence: up to the first sentence-ending punctuation
    // or newline. Periods must be followed by a space (or EOF) to
    // count — bare periods in numbers like "2504.09823" are not
    // sentence boundaries.
    let sentence_end = clean
        .char_indices()
        .find_map(|(i, c)| {
            if c == '\n' || c == '!' || c == '?' {
                return Some(i + 1);
            }
            if c == '.' {
                let after = clean.as_bytes().get(i + 1).copied();
                // Period counts as sentence-end only when followed
                // by whitespace, closing paren/bracket, or EOF.
                if after.is_none()
                    || after == Some(b' ')
                    || after == Some(b'\n')
                    || after == Some(b')')
                    || after == Some(b']')
                {
                    // Skip numbered list prefixes like "8. " or
                    // "10. " — the text before the period is all
                    // digits. These are ordinals, not sentences.
                    let prefix = &clean[..i];
                    if !prefix.is_empty() && prefix.bytes().all(|b| b.is_ascii_digit()) {
                        return None;
                    }
                    return Some(i + 1);
                }
            }
            None
        })
        .unwrap_or(clean.len());

    if sentence_end <= MAX_CHUNK_NAME_LEN {
        let name = clean[..sentence_end].trim();
        if name.len() < clean.len() && !name.ends_with('.') {
            format!("{name}...")
        } else {
            name.to_string()
        }
    } else {
        truncate_with_ellipsis(clean, MAX_CHUNK_NAME_LEN)
    }
}

/// Qualify a heading with the source title when the heading is
/// generic (e.g., "Overview" → "Paper Title: Overview").
fn qualify_heading(heading: &str, source_title: Option<&str>) -> String {
    // Strip leading section numbers like "2 ", "1.2 ", "3.1.4. "
    // so "2 Background" matches the generic heading "background".
    let bare = strip_section_number(heading);
    let is_generic = GENERIC_HEADINGS
        .iter()
        .any(|g| bare.eq_ignore_ascii_case(g));

    if is_generic {
        if let Some(title) = source_title {
            let title = title.trim();
            if !title.is_empty() {
                // Truncate source title to leave room for heading.
                let max_title = MAX_CHUNK_NAME_LEN
                    .saturating_sub(heading.len())
                    .saturating_sub(2); // ": "
                let t = truncate_with_ellipsis(title, max_title);
                return format!("{t}: {heading}");
            }
        }
    }

    truncate_with_ellipsis(heading, MAX_CHUNK_NAME_LEN)
}

/// Strip leading section numbers like "2 ", "1.2 ", "3.1.4. ".
///
/// Handles patterns like "2 Background", "1.2 Methods",
/// "3.1.4. Results", and "A.1 Appendix". Returns the heading
/// text after the number prefix, or the original string if no
/// prefix is found.
fn strip_section_number(heading: &str) -> &str {
    let s = heading.trim();
    // Find where the numeric prefix ends. Allow digits, dots,
    // and a single trailing dot (e.g., "2.", "3.1.4.").
    let prefix_end = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(0);
    if prefix_end == 0 {
        return s;
    }
    // The character after the prefix must be whitespace.
    let rest = &s[prefix_end..];
    let trimmed = rest.trim_start();
    if trimmed.is_empty() || rest.len() == trimmed.len() {
        // No space after prefix — not a section number.
        return s;
    }
    trimmed
}

/// Strip basic inline markdown: `**bold**`, `*italic*`, `` `code` ``.
pub(super) fn strip_inline_markdown(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '*' {
            // Skip **
            i += 2;
        } else if chars[i] == '*' || chars[i] == '`' {
            // Skip single * or `
            i += 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Map a search strategy enum to its string label.
pub(super) fn strategy_name(strategy: &SearchStrategy) -> &'static str {
    match strategy {
        SearchStrategy::Auto => "auto",
        SearchStrategy::Balanced => "balanced",
        SearchStrategy::Precise => "precise",
        SearchStrategy::Exploratory => "exploratory",
        SearchStrategy::Recent => "recent",
        SearchStrategy::GraphFirst => "graph_first",
        SearchStrategy::Global => "global",
        SearchStrategy::Custom(_) => "custom",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- truncate_with_ellipsis tests ---

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate_with_ellipsis("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_length_unchanged() {
        assert_eq!(truncate_with_ellipsis("hello", 5), "hello");
    }

    #[test]
    fn truncate_ascii_adds_ellipsis() {
        let result = truncate_with_ellipsis("hello world", 8);
        assert_eq!(result, "hello...");
        assert!(result.len() <= 8);
    }

    #[test]
    fn truncate_empty_string() {
        assert_eq!(truncate_with_ellipsis("", 10), "");
    }

    #[test]
    fn truncate_emoji_does_not_panic() {
        // '🔥' is 4 bytes. Cutting at byte 5 would land inside the
        // second emoji — must snap back to the boundary.
        let input = "🔥🔥🔥"; // 12 bytes
        let result = truncate_with_ellipsis(input, 8);
        // Can fit one emoji (4 bytes) + "..." (3 bytes) = 7 bytes
        assert_eq!(result, "🔥...");
        assert!(result.len() <= 8);
    }

    #[test]
    fn truncate_cjk_does_not_panic() {
        // CJK characters are 3 bytes each.
        let input = "漢字漢字漢字"; // 18 bytes
        let result = truncate_with_ellipsis(input, 10);
        // max_bytes=10, subtract 3 for "..." = 7, snap back from 7
        // to char boundary at 6 (2 CJK chars), result = "漢字..."
        assert_eq!(result, "漢字...");
        assert!(result.len() <= 10);
    }

    #[test]
    fn truncate_accented_chars_does_not_panic() {
        // 'é' as composed form is 2 bytes in UTF-8.
        let input = "résumé here";
        let result = truncate_with_ellipsis(input, 8);
        // Must not panic and must be valid UTF-8.
        assert!(result.len() <= 8);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_max_bytes_less_than_three() {
        // Edge case: max_bytes < 3 means no room for ellipsis text.
        let result = truncate_with_ellipsis("hello", 2);
        // saturating_sub(3) = 0, so result is "..."
        assert_eq!(result, "...");
    }

    #[test]
    fn derive_chunk_name_heading() {
        let content = "# Introduction\nThis is the body.";
        assert_eq!(derive_chunk_name(content), "Introduction");
    }

    #[test]
    fn derive_chunk_name_multi_hash_heading() {
        let content = "### Configuration Options\nSome config details.";
        assert_eq!(derive_chunk_name(content), "Configuration Options");
    }

    #[test]
    fn derive_chunk_name_first_sentence() {
        let content = "The ingestion pipeline processes documents. It has 9 stages.";
        assert_eq!(
            derive_chunk_name(content),
            "The ingestion pipeline processes documents."
        );
    }

    #[test]
    fn derive_chunk_name_no_period() {
        let content = "Some short text without punctuation";
        assert_eq!(
            derive_chunk_name(content),
            "Some short text without punctuation"
        );
    }

    #[test]
    fn derive_chunk_name_decimal_not_sentence_end() {
        // Periods in version numbers (2504.09823) should not be
        // treated as sentence boundaries.
        let content = "Version 2504.09823 introduces new features.";
        assert_eq!(
            derive_chunk_name(content),
            "Version 2504.09823 introduces new features."
        );
    }

    #[test]
    fn derive_chunk_name_code_fence_skipped() {
        // Code fence language tags should not become the chunk name.
        let content = "```rust\nimpl SearchService {\n    pub fn new() -> Self {}\n}\n```";
        let name = derive_chunk_name(content);
        assert!(!name.starts_with("rust"));
        assert!(name.contains("SearchService"));
    }

    #[test]
    fn derive_chunk_name_numbered_list_item() {
        // "8. Apply topological confidence..." should not produce "8."
        let content = "8. Apply topological confidence as a multiplier";
        assert_eq!(
            derive_chunk_name(content),
            "8. Apply topological confidence as a multiplier"
        );
        // Also test multi-digit: "10. Return top-N results"
        let content2 = "10. Return top-N results with provenance.";
        assert_eq!(
            derive_chunk_name(content2),
            "10. Return top-N results with provenance."
        );
    }

    #[test]
    fn derive_chunk_name_long_truncates() {
        let long = "a".repeat(200);
        let name = derive_chunk_name(&long);
        assert!(name.len() <= MAX_CHUNK_NAME_LEN);
        assert!(name.ends_with("..."));
    }

    #[test]
    fn derive_chunk_name_empty() {
        assert_eq!(derive_chunk_name(""), "");
    }

    #[test]
    fn derive_chunk_name_newline_before_period() {
        // Per-line processing: "First line" is the whole first line.
        let content = "First line\nSecond line.";
        assert_eq!(derive_chunk_name(content), "First line");
    }

    #[test]
    fn derive_chunk_name_skips_bold_label() {
        let content = "**Status:** Draft\nThe real description starts here.";
        assert_eq!(
            derive_chunk_name(content),
            "The real description starts here."
        );
    }

    #[test]
    fn derive_chunk_name_skips_arxiv_ref() {
        let content = "[2506.12345]\nAbstract of the paper.";
        assert_eq!(derive_chunk_name(content), "Abstract of the paper.");
    }

    #[test]
    fn derive_chunk_name_strips_bold() {
        let content = "The **Subjective Logic** framework defines opinions.";
        assert_eq!(
            derive_chunk_name(content),
            "The Subjective Logic framework defines opinions."
        );
    }

    #[test]
    fn derive_chunk_name_strips_inline_code() {
        let content = "Use the `derive_chunk_name` function.";
        assert_eq!(
            derive_chunk_name(content),
            "Use the derive_chunk_name function."
        );
    }

    #[test]
    fn derive_chunk_name_skips_link_line() {
        let content = "[Paper](https://arxiv.org)\nKnowledge graphs are important.";
        assert_eq!(
            derive_chunk_name(content),
            "Knowledge graphs are important."
        );
    }

    #[test]
    fn derive_chunk_name_all_metadata_lines() {
        // When all lines are metadata, fall back to the first line
        // with bold markers stripped.
        let content = "**Authors:** (2506.00049)\n**arxiv:** 2506.00049";
        assert_eq!(derive_chunk_name(content), "Authors: (2506.00049)");
    }

    #[test]
    fn derive_chunk_name_skips_bare_numbered_item() {
        let content = "3.\nThe actual content starts here.";
        assert_eq!(
            derive_chunk_name(content),
            "The actual content starts here."
        );
    }

    #[test]
    fn strip_inline_markdown_bold() {
        assert_eq!(strip_inline_markdown("**bold** text"), "bold text");
    }

    #[test]
    fn strip_inline_markdown_mixed() {
        assert_eq!(
            strip_inline_markdown("**bold** and *italic* and `code`"),
            "bold and italic and code"
        );
    }

    // --- qualify_heading tests ---

    #[test]
    fn qualify_generic_heading_with_source() {
        assert_eq!(
            qualify_heading("Overview", Some("Epistemic Model Spec")),
            "Epistemic Model Spec: Overview"
        );
    }

    #[test]
    fn qualify_generic_heading_case_insensitive() {
        assert_eq!(
            qualify_heading("INTRODUCTION", Some("Paper Title")),
            "Paper Title: INTRODUCTION"
        );
    }

    #[test]
    fn qualify_non_generic_heading_unchanged() {
        assert_eq!(
            qualify_heading("Reciprocal Rank Fusion", Some("Paper Title")),
            "Reciprocal Rank Fusion"
        );
    }

    #[test]
    fn qualify_generic_heading_no_source() {
        assert_eq!(qualify_heading("Overview", None), "Overview");
    }

    #[test]
    fn qualify_generic_heading_empty_source() {
        assert_eq!(qualify_heading("Overview", Some("")), "Overview");
    }

    // --- strip_section_number tests ---

    #[test]
    fn strip_simple_section_number() {
        assert_eq!(strip_section_number("2 Background"), "Background");
    }

    #[test]
    fn strip_dotted_section_number() {
        assert_eq!(strip_section_number("1.2 Methods"), "Methods");
    }

    #[test]
    fn strip_deep_section_number() {
        assert_eq!(strip_section_number("3.1.4. Results"), "Results");
    }

    #[test]
    fn strip_no_section_number() {
        assert_eq!(strip_section_number("Background"), "Background");
    }

    #[test]
    fn strip_number_no_space() {
        // "2Background" is not a section number.
        assert_eq!(strip_section_number("2Background"), "2Background");
    }

    #[test]
    fn strip_just_number() {
        // Bare "2" has no text after it.
        assert_eq!(strip_section_number("2"), "2");
    }

    // --- numbered heading qualification ---

    #[test]
    fn qualify_numbered_generic_heading() {
        assert_eq!(
            qualify_heading("2 Background", Some("My Paper")),
            "My Paper: 2 Background"
        );
    }

    #[test]
    fn qualify_dotted_numbered_heading() {
        assert_eq!(
            qualify_heading("1.2 Introduction", Some("Survey")),
            "Survey: 1.2 Introduction"
        );
    }

    #[test]
    fn numbered_non_generic_not_qualified() {
        assert_eq!(
            qualify_heading("3 Reciprocal Rank Fusion", Some("Paper")),
            "3 Reciprocal Rank Fusion"
        );
    }

    #[test]
    fn qualified_name_via_derive() {
        let content = "## Overview\nThe system is designed for...";
        assert_eq!(
            derive_chunk_name_qualified(content, Some("Search Engine")),
            "Search Engine: Overview"
        );
    }

    #[test]
    fn qualified_name_specific_heading_not_qualified() {
        let content = "## Reciprocal Rank Fusion\nRRF merges ranked lists.";
        assert_eq!(
            derive_chunk_name_qualified(content, Some("Search Engine")),
            "Reciprocal Rank Fusion"
        );
    }

    #[test]
    fn qualified_name_no_source_falls_back() {
        let content = "## Overview\nThe system is designed for...";
        assert_eq!(derive_chunk_name_qualified(content, None), "Overview");
    }

    // --- KWIC snippet tests ---

    #[test]
    fn kwic_finds_query_term() {
        let content = "The preamble discusses many topics. \
            GraphRAG is a powerful paradigm for knowledge retrieval. \
            It combines graph structure with RAG approaches.";
        let snippet = kwic_snippet(content, "GraphRAG knowledge", 80);
        assert!(snippet.contains("GraphRAG"));
    }

    #[test]
    fn kwic_falls_back_to_truncate() {
        let content = "Some content that has no matching terms at all.";
        let snippet = kwic_snippet(content, "xyzzy foobar", 20);
        // Should fall back to first 20 bytes.
        assert!(snippet.len() <= 23); // 20 + "..."
    }

    #[test]
    fn kwic_short_terms_ignored() {
        // Terms < 3 chars should be skipped.
        let content = "A is B or C and D.";
        let snippet = kwic_snippet(content, "A B C", 10);
        // All terms are < 3 chars, falls back to truncate.
        assert_eq!(snippet, kwic_snippet(content, "", 10));
    }

    #[test]
    fn kwic_centers_window() {
        let content = "aaa bbb ccc ddd eee fff ggg hhh iii jjj kkk lll mmm";
        let snippet = kwic_snippet(content, "ggg", 20);
        assert!(snippet.contains("ggg"));
        // Should have ... prefix since ggg is in the middle.
        assert!(snippet.starts_with("..."));
    }

    #[test]
    fn kwic_empty_content() {
        assert_eq!(kwic_snippet("", "test", 100), "");
    }

    #[test]
    fn kwic_snaps_to_word_boundaries() {
        let content = "The knowledge graph enables powerful entity resolution \
            and relationship extraction from unstructured sources.";
        let snippet = kwic_snippet(content, "entity resolution", 60);
        assert!(snippet.contains("entity resolution"));
        // Should not start or end mid-word.
        if snippet.starts_with("...") {
            let body = &snippet[3..];
            // First char of body should be non-whitespace (start of word).
            assert!(
                body.starts_with(|c: char| !c.is_whitespace()),
                "snippet should start at word boundary: {snippet}"
            );
        }
    }
}

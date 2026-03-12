//! Stage 4: Hierarchical chunking.
//!
//! Decomposes normalized Markdown into a chunk tree:
//! section → paragraph.
//! Primary boundaries from heading structure (structural),
//! with paragraph splitting for oversized sections.
//!
//! Document-level embedding is stored on the [`Source`] record
//! directly, so there is no document-level chunk.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Granularity level of a chunk output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChunkLevel {
    /// Section delimited by headings.
    Section,
    /// Paragraph within a section.
    Paragraph,
}

/// A chunk produced by the chunking stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkOutput {
    /// Unique identifier for this chunk.
    pub id: Uuid,
    /// Parent chunk identifier (None for top-level sections).
    pub parent_id: Option<Uuid>,
    /// Text content of the chunk.
    pub text: String,
    /// Granularity level.
    pub level: ChunkLevel,
    /// Heading path from document root to this chunk.
    pub heading_path: Vec<String>,
    /// Number of leading characters that are overlap context
    /// copied from the preceding chunk. Zero for the first
    /// paragraph chunk and for non-paragraph chunks.
    pub context_prefix_len: usize,
    /// Byte offset in the source normalized text where this
    /// chunk starts (including overlap prefix).
    pub byte_start: usize,
    /// Byte offset in the source normalized text where this
    /// chunk ends.
    pub byte_end: usize,
}

/// Split a Markdown document into hierarchical chunks.
///
/// Sections are split on `#`–`####` headings (H1–H4).
/// The heading hierarchy is tracked in
/// [`ChunkOutput::heading_path`], e.g., `["Title", "Methods",
/// "Data Collection"]`.
///
/// Sections exceeding `max_chunk_size` bytes are further split
/// into paragraphs by `\n\n`. No document-level chunk is
/// created; the document embedding lives on the source record.
///
/// When `overlap` is non-zero, each paragraph chunk after the
/// first within a section is prefixed with the last `overlap`
/// characters of the previous paragraph. The
/// [`ChunkOutput::context_prefix_len`] field records how many
/// leading characters are overlap so consumers can trim them
/// from snippets or highlighting.
pub fn chunk_document(markdown: &str, max_chunk_size: usize, overlap: usize) -> Vec<ChunkOutput> {
    chunk_document_with_merge(markdown, max_chunk_size, overlap, 0)
}

/// Like [`chunk_document`] but with small-section merging.
///
/// When `min_section_size > 0`, consecutive sibling sections
/// (sharing the same parent heading path) whose body is below
/// `min_section_size` are merged into a combined section. This
/// prevents academic papers with many tiny H3/H4 subsections
/// from producing chunks too small for meaningful retrieval.
///
/// The merged section inherits the parent heading path. Bodies
/// are joined with `\n\n` separators. Merging stops when the
/// combined text would exceed `max_chunk_size`.
pub fn chunk_document_with_merge(
    markdown: &str,
    max_chunk_size: usize,
    overlap: usize,
    min_section_size: usize,
) -> Vec<ChunkOutput> {
    let mut chunks = Vec::new();

    if markdown.trim().is_empty() {
        return chunks;
    }

    let raw_sections = split_sections(markdown);
    let sections = if min_section_size > 0 {
        merge_small_siblings(raw_sections, min_section_size, max_chunk_size)
    } else {
        raw_sections
    };

    // Track position in the source text for byte offset computation.
    let mut search_pos: usize = 0;

    for (heading, body) in &sections {
        let heading_path: Vec<String> = heading.iter().map(|s| s.to_string()).collect();

        let section_text = body.trim();
        if section_text.is_empty() {
            continue;
        }

        // Locate the section body in the original markdown.
        let section_byte_start = markdown[search_pos..]
            .find(section_text)
            .map(|i| search_pos + i)
            .unwrap_or(search_pos);
        let section_byte_end = section_byte_start + section_text.len();
        search_pos = section_byte_end;

        let section_id = Uuid::new_v4();
        chunks.push(ChunkOutput {
            id: section_id,
            parent_id: None,
            text: section_text.to_string(),
            level: ChunkLevel::Section,
            heading_path: heading_path.clone(),
            context_prefix_len: 0,
            byte_start: section_byte_start,
            byte_end: section_byte_end,
        });

        if section_text.len() > max_chunk_size {
            let paragraphs: Vec<&str> = section_text
                .split("\n\n")
                .filter(|p| !p.trim().is_empty())
                .collect();

            if paragraphs.len() > 1 {
                let mut prev_text: Option<&str> = None;
                // Track the byte offset of the end of the previous
                // paragraph (used for overlap byte_start computation).
                let mut prev_para_byte_end: usize = section_byte_start;
                let mut para_search_pos: usize = section_byte_start;

                for para in &paragraphs {
                    let para = para.trim();
                    if para.is_empty() {
                        continue;
                    }

                    // Find this paragraph in the markdown.
                    let para_byte_start = markdown[para_search_pos..]
                        .find(para)
                        .map(|i| para_search_pos + i)
                        .unwrap_or(para_search_pos);
                    let para_byte_end = para_byte_start + para.len();
                    para_search_pos = para_byte_end;

                    let (text, prefix_len) = build_overlap_text(prev_text, para, overlap);

                    // For overlap chunks, byte_start reaches back
                    // into the previous paragraph by the overlap
                    // suffix length. The overlap suffix + "\n\n" +
                    // current paragraph forms a contiguous range in
                    // the source text.
                    let chunk_byte_start = if prefix_len > 0 {
                        let overlap_suffix_len = prefix_len.saturating_sub(2);
                        prev_para_byte_end.saturating_sub(overlap_suffix_len)
                    } else {
                        para_byte_start
                    };

                    chunks.push(ChunkOutput {
                        id: Uuid::new_v4(),
                        parent_id: Some(section_id),
                        text,
                        level: ChunkLevel::Paragraph,
                        heading_path: heading_path.clone(),
                        context_prefix_len: prefix_len,
                        byte_start: chunk_byte_start,
                        byte_end: para_byte_end,
                    });

                    prev_para_byte_end = para_byte_end;
                    prev_text = Some(para);
                }
            }
        }
    }

    chunks
}

/// Merge consecutive small sibling sections.
///
/// Two sections are "siblings" if they share the same parent
/// heading path (all elements except the last). When a section's
/// body is below `min_size`, it's merged with the next sibling
/// as long as the combined size stays under `max_size`.
///
/// The merged section inherits the parent heading path (dropping
/// the individual subsection names since the body now spans
/// multiple subsections).
fn merge_small_siblings<'a>(
    sections: Vec<(Vec<&'a str>, String)>,
    min_size: usize,
    max_size: usize,
) -> Vec<(Vec<&'a str>, String)> {
    if sections.is_empty() {
        return sections;
    }

    let mut result: Vec<(Vec<&'a str>, String)> = Vec::new();

    let mut i = 0;
    while i < sections.len() {
        let (ref path, ref body) = sections[i];
        let trimmed_len = body.trim().len();

        // Only consider merging if this section is small.
        if trimmed_len >= min_size || trimmed_len == 0 {
            result.push(sections[i].clone());
            i += 1;
            continue;
        }

        // Parent path = all but the last heading element.
        let parent: Vec<&str> = if path.len() > 1 {
            path[..path.len() - 1].to_vec()
        } else {
            // Top-level sections (H1 only) — don't merge across
            // H1 boundaries.
            result.push(sections[i].clone());
            i += 1;
            continue;
        };

        // Accumulate consecutive small siblings.
        let mut merged_body = body.trim().to_string();
        let mut j = i + 1;

        while j < sections.len() {
            let (ref next_path, ref next_body) = sections[j];
            let next_trimmed = next_body.trim();

            // Must share the same parent to be a sibling.
            let next_parent: Vec<&str> = if next_path.len() > 1 {
                next_path[..next_path.len() - 1].to_vec()
            } else {
                break;
            };
            if next_parent != parent {
                break;
            }

            // Only merge if the next section is also small.
            if next_trimmed.len() >= min_size {
                break;
            }

            // Don't exceed max_size.
            let combined_len =
                merged_body.len() + "\n\n".len() + next_trimmed.len();
            if combined_len > max_size {
                break;
            }

            merged_body.push_str("\n\n");
            merged_body.push_str(next_trimmed);
            j += 1;
        }

        // If we merged anything, use the parent path.
        // Otherwise keep the original.
        if j > i + 1 {
            result.push((parent, merged_body));
        } else {
            result.push(sections[i].clone());
        }
        i = j;
    }

    result
}

/// Build the overlap-prefixed text for a paragraph chunk.
///
/// If there is a previous paragraph and `overlap > 0`, the last
/// `overlap` characters of that paragraph are prepended (separated
/// by `\n\n`) to the current paragraph text. Returns the assembled
/// text and the number of prefix characters that are overlap
/// context (including the `\n\n` separator).
fn build_overlap_text(prev: Option<&str>, current: &str, overlap: usize) -> (String, usize) {
    if overlap == 0 {
        return (current.to_string(), 0);
    }

    let prev = match prev {
        Some(p) if !p.is_empty() => p,
        _ => return (current.to_string(), 0),
    };

    // Take the last `overlap` bytes, snapping forward to a word
    // boundary so the overlap prefix never starts mid-word.
    let mut start = prev.len().saturating_sub(overlap);
    // First snap to a valid UTF-8 char boundary.
    while start > 0 && !prev.is_char_boundary(start) {
        start -= 1;
    }
    // Then snap forward to a word boundary (whitespace or start
    // of string). Without this, overlap prefixes produce chunks
    // like "pological confidence" from "topological confidence".
    if start > 0 {
        if let Some(ws_pos) = prev[start..].find(char::is_whitespace) {
            // Skip past the whitespace character to start at the
            // next word.
            let ws_char_len = prev[start + ws_pos..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(1);
            start += ws_pos + ws_char_len;
        }
    }
    let suffix = &prev[start..];

    // Separator between the overlap prefix and the actual content.
    const SEP: &str = "\n\n";

    let mut text = String::with_capacity(suffix.len() + SEP.len() + current.len());
    text.push_str(suffix);
    text.push_str(SEP);
    text.push_str(current);

    let prefix_len = suffix.len() + SEP.len();
    (text, prefix_len)
}

/// Detect a Markdown heading (H1–H4) and return its level and
/// title text. Returns `None` for non-heading lines or H5+.
fn detect_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim();
    // Check longest prefix first to avoid `# ` matching `## `.
    // All prefixes are ASCII so byte/char boundaries align.
    if let Some(rest) = trimmed.strip_prefix("#### ") {
        Some((4, rest.trim()))
    } else if let Some(rest) = trimmed.strip_prefix("### ") {
        Some((3, rest.trim()))
    } else if let Some(rest) = trimmed.strip_prefix("## ") {
        Some((2, rest.trim()))
    } else if let Some(rest) = trimmed.strip_prefix("# ") {
        Some((1, rest.trim()))
    } else {
        None
    }
}

/// Split markdown into sections by `#`–`####` headings (H1–H4).
///
/// Returns `(heading_path, body)` pairs where `heading_path`
/// tracks the heading hierarchy. For example, an H2 "Methods"
/// under H1 "Paper Title" yields `["Paper Title", "Methods"]`.
fn split_sections(markdown: &str) -> Vec<(Vec<&str>, String)> {
    let mut sections: Vec<(Vec<&str>, String)> = Vec::new();
    // Stack of (level, title) pairs tracking the heading hierarchy.
    let mut heading_stack: Vec<(usize, &str)> = Vec::new();
    let mut current_body = String::new();

    for line in markdown.lines() {
        if let Some((level, title)) = detect_heading(line) {
            // Flush the current section.
            let path: Vec<&str> = heading_stack.iter().map(|(_, t)| *t).collect();
            if !current_body.is_empty() || !path.is_empty() {
                sections.push((path, current_body.clone()));
            }
            current_body = String::new();

            // Pop the stack back to the parent of this heading level.
            // E.g., if we see H2, pop everything at level >= 2.
            while heading_stack.last().is_some_and(|(l, _)| *l >= level) {
                heading_stack.pop();
            }
            heading_stack.push((level, title));
        } else {
            if !current_body.is_empty() {
                current_body.push('\n');
            }
            current_body.push_str(line);
        }
    }

    // Flush the final section.
    let path: Vec<&str> = heading_stack.iter().map(|(_, t)| *t).collect();
    if !current_body.is_empty() || !path.is_empty() {
        sections.push((path, current_body));
    }

    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_document() {
        let chunks = chunk_document("", 500, 0);
        assert!(chunks.is_empty());
    }

    #[test]
    fn single_section() {
        let md = "# Title\n\nSome content here.";
        let chunks = chunk_document(md, 500, 0);
        assert!(chunks.iter().any(|c| c.level == ChunkLevel::Section));
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn no_document_level_chunk() {
        let md = "# Title\n\nSome content here.";
        let chunks = chunk_document(md, 500, 0);
        // Document-level chunks should never appear.
        for c in &chunks {
            assert!(c.level == ChunkLevel::Section || c.level == ChunkLevel::Paragraph);
        }
    }

    #[test]
    fn multiple_sections() {
        let md = "# First\n\nContent 1.\n\n# Second\n\nContent 2.";
        let chunks = chunk_document(md, 500, 0);
        let sections: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Section)
            .collect();
        assert_eq!(sections.len(), 2);
    }

    #[test]
    fn paragraph_splitting() {
        let long_section = format!(
            "# Big Section\n\n{}\n\n{}",
            "a".repeat(100),
            "b".repeat(100)
        );
        let chunks = chunk_document(&long_section, 50, 0);
        let paragraphs: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Paragraph)
            .collect();
        assert_eq!(paragraphs.len(), 2);
    }

    #[test]
    fn heading_path_tracking() {
        let md = "# My Section\n\nContent.";
        let chunks = chunk_document(md, 500, 0);
        let section = chunks
            .iter()
            .find(|c| c.level == ChunkLevel::Section)
            .unwrap();
        assert_eq!(section.heading_path, vec!["My Section"]);
    }

    #[test]
    fn sections_have_no_parent() {
        let md = "# Title\n\nContent.";
        let chunks = chunk_document(md, 500, 0);
        let section = chunks
            .iter()
            .find(|c| c.level == ChunkLevel::Section)
            .unwrap();
        assert_eq!(section.parent_id, None);
    }

    #[test]
    fn paragraph_parent_is_section() {
        let long_section = format!("# Big\n\n{}\n\n{}", "a".repeat(100), "b".repeat(100));
        let chunks = chunk_document(&long_section, 50, 0);
        let section = chunks
            .iter()
            .find(|c| c.level == ChunkLevel::Section)
            .unwrap();
        let para = chunks
            .iter()
            .find(|c| c.level == ChunkLevel::Paragraph)
            .unwrap();
        assert_eq!(para.parent_id, Some(section.id));
    }

    #[test]
    fn context_prefix_len_defaults_to_zero() {
        let md = "# Title\n\nContent.";
        let chunks = chunk_document(md, 500, 0);
        for c in &chunks {
            assert_eq!(c.context_prefix_len, 0);
        }
    }

    #[test]
    fn overlap_zero_gives_no_prefix() {
        let long_section = format!("# Section\n\n{}\n\n{}", "a".repeat(100), "b".repeat(100));
        let chunks = chunk_document(&long_section, 50, 0);
        for c in &chunks {
            assert_eq!(c.context_prefix_len, 0);
        }
    }

    #[test]
    fn overlap_adds_context_prefix() {
        let para_a = "a".repeat(100);
        let para_b = "b".repeat(100);
        let long_section = format!("# Section\n\n{para_a}\n\n{para_b}");
        let overlap = 20;
        let chunks = chunk_document(&long_section, 50, overlap);

        let paragraphs: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Paragraph)
            .collect();

        assert_eq!(paragraphs.len(), 2);

        // First paragraph has no overlap prefix.
        assert_eq!(paragraphs[0].context_prefix_len, 0);
        assert_eq!(paragraphs[0].text, para_a);

        // Second paragraph starts with the last 20 chars of the
        // first paragraph followed by a \n\n separator.
        let expected_prefix = &para_a[para_a.len() - overlap..];
        assert_eq!(paragraphs[1].context_prefix_len, overlap + "\n\n".len());
        assert!(paragraphs[1].text.starts_with(expected_prefix));
        // After the prefix, the original content follows.
        let after_prefix = &paragraphs[1].text[paragraphs[1].context_prefix_len..];
        assert_eq!(after_prefix, para_b);
    }

    #[test]
    fn overlap_larger_than_paragraph_uses_whole_prev() {
        let para_a = "short";
        let para_b = "b".repeat(100);
        let long_section = format!("# Section\n\n{para_a}\n\n{para_b}");
        let overlap = 999; // larger than para_a
        let chunks = chunk_document(&long_section, 5, overlap);

        let paragraphs: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Paragraph)
            .collect();

        assert_eq!(paragraphs.len(), 2);
        // Should use the entirety of para_a as prefix.
        assert_eq!(
            paragraphs[1].context_prefix_len,
            para_a.len() + "\n\n".len()
        );
        assert!(paragraphs[1].text.starts_with(para_a));
    }

    #[test]
    fn overlap_does_not_affect_section_chunks() {
        let md = format!("# Title\n\n{}\n\n{}", "x".repeat(100), "y".repeat(100));
        let chunks = chunk_document(&md, 50, 30);
        for c in &chunks {
            if c.level != ChunkLevel::Paragraph {
                assert_eq!(c.context_prefix_len, 0);
            }
        }
    }

    #[test]
    fn overlap_across_sections_does_not_leak() {
        // Overlap should reset at section boundaries.
        let md = format!(
            "# Sec1\n\n{}\n\n{}\n\n# Sec2\n\n{}\n\n{}",
            "a".repeat(100),
            "b".repeat(100),
            "c".repeat(100),
            "d".repeat(100),
        );
        let chunks = chunk_document(&md, 50, 20);

        let sec2_paragraphs: Vec<_> = chunks
            .iter()
            .filter(|c| {
                c.level == ChunkLevel::Paragraph
                    && c.heading_path.first().map(|s| s.as_str()) == Some("Sec2")
            })
            .collect();

        assert_eq!(sec2_paragraphs.len(), 2);
        // First paragraph of Sec2 has no overlap.
        assert_eq!(sec2_paragraphs[0].context_prefix_len, 0);
        // Second paragraph of Sec2 overlaps from Sec2's first
        // paragraph, NOT from Sec1's last paragraph.
        assert!(sec2_paragraphs[1].text.starts_with(&"c".repeat(20)));
    }

    #[test]
    fn multibyte_utf8_does_not_panic() {
        // Box-drawing chars are 3 bytes each in UTF-8.
        let diagram = "┌──────┐\n│ test │\n└──────┘";
        let para_a = format!("Some text with diagram:\n{diagram}");
        let para_b = "Follow-up paragraph content here.".to_string();
        let md = format!("# Section\n\n{para_a}\n\n{para_b}");

        // Overlap of 20 bytes will likely land inside a 3-byte char.
        let chunks = chunk_document(&md, 10, 20);

        // Should not panic — that's the main assertion.
        let paragraphs: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Paragraph)
            .collect();
        assert!(paragraphs.len() >= 2);

        // The overlap prefix should be valid UTF-8.
        for p in &paragraphs {
            assert!(p.text.is_char_boundary(0));
        }
    }

    #[test]
    fn emoji_overlap_does_not_panic() {
        let para_a = "Hello 🌍🌎🌏 world";
        let para_b = "Another paragraph";
        let md = format!("# Section\n\n{para_a}\n\n{para_b}");
        // Each emoji is 4 bytes; overlap=3 lands inside one.
        let chunks = chunk_document(&md, 5, 3);
        let paragraphs: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Paragraph)
            .collect();
        assert!(paragraphs.len() >= 2);
    }

    #[test]
    fn byte_offsets_match_source_text() {
        let md = "# Title\n\nSome content here.";
        let chunks = chunk_document(md, 500, 0);
        assert_eq!(chunks.len(), 1);
        let section = &chunks[0];
        // Section text should match the slice at the byte offsets.
        assert_eq!(&md[section.byte_start..section.byte_end], section.text);
    }

    #[test]
    fn byte_offsets_multiple_sections() {
        let md = "# First\n\nContent 1.\n\n# Second\n\nContent 2.";
        let chunks = chunk_document(md, 500, 0);
        for chunk in &chunks {
            assert_eq!(
                &md[chunk.byte_start..chunk.byte_end],
                chunk.text,
                "byte offsets should reconstruct chunk text"
            );
        }
    }

    #[test]
    fn byte_offsets_paragraphs_without_overlap() {
        let para_a = "a".repeat(100);
        let para_b = "b".repeat(100);
        let md = format!("# Section\n\n{para_a}\n\n{para_b}");
        let chunks = chunk_document(&md, 50, 0);

        for chunk in &chunks {
            assert_eq!(
                &md[chunk.byte_start..chunk.byte_end],
                chunk.text,
                "byte offsets should match for non-overlap chunks"
            );
        }
    }

    #[test]
    fn byte_offsets_paragraphs_with_overlap() {
        let para_a = "a".repeat(100);
        let para_b = "b".repeat(100);
        let md = format!("# Section\n\n{para_a}\n\n{para_b}");
        let chunks = chunk_document(&md, 50, 20);

        let paragraphs: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Paragraph)
            .collect();
        assert_eq!(paragraphs.len(), 2);

        // First paragraph: no overlap.
        assert_eq!(
            &md[paragraphs[0].byte_start..paragraphs[0].byte_end],
            paragraphs[0].text
        );

        // Second paragraph: overlap chunk spans from prev para
        // into current para. The byte range in the source should
        // match the full chunk text.
        assert_eq!(
            &md[paragraphs[1].byte_start..paragraphs[1].byte_end],
            paragraphs[1].text,
            "overlap chunk byte range should match chunk text"
        );

        // content_offset (context_prefix_len) should mark where
        // unique content begins.
        let unique = &paragraphs[1].text[paragraphs[1].context_prefix_len..];
        assert_eq!(unique, para_b);
    }

    #[test]
    fn byte_offsets_no_heading() {
        let md = "Just plain text without any heading.";
        let chunks = chunk_document(md, 500, 0);
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            &md[chunks[0].byte_start..chunks[0].byte_end],
            chunks[0].text
        );
    }

    // --- H2+ heading hierarchy tests ---

    #[test]
    fn h2_creates_separate_section() {
        let md = "# Title\n\nIntro.\n\n## Methods\n\nMethod content.";
        let chunks = chunk_document(md, 500, 0);
        let sections: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Section)
            .collect();
        assert_eq!(sections.len(), 2);
    }

    #[test]
    fn h2_heading_path_includes_parent() {
        let md = "# Paper\n\nIntro.\n\n## Methods\n\nContent.";
        let chunks = chunk_document(md, 500, 0);
        let methods = chunks
            .iter()
            .find(|c| c.heading_path.last() == Some(&"Methods".to_string()))
            .unwrap();
        assert_eq!(methods.heading_path, vec!["Paper", "Methods"]);
    }

    #[test]
    fn h3_heading_path_includes_ancestors() {
        let md = concat!(
            "# Paper\n\nIntro.\n\n",
            "## Methods\n\nOverview.\n\n",
            "### Data Collection\n\nDetails."
        );
        let chunks = chunk_document(md, 500, 0);
        let data = chunks
            .iter()
            .find(|c| c.heading_path.last() == Some(&"Data Collection".to_string()))
            .unwrap();
        assert_eq!(
            data.heading_path,
            vec!["Paper", "Methods", "Data Collection"]
        );
    }

    #[test]
    fn h4_heading_path_depth() {
        let md = concat!(
            "# A\n\nA body.\n\n",
            "## B\n\nB body.\n\n",
            "### C\n\nC body.\n\n",
            "#### D\n\nD body."
        );
        let chunks = chunk_document(md, 500, 0);
        let d = chunks
            .iter()
            .find(|c| c.heading_path.last() == Some(&"D".to_string()))
            .unwrap();
        assert_eq!(d.heading_path, vec!["A", "B", "C", "D"]);
    }

    #[test]
    fn h2_sibling_resets_path() {
        let md = concat!(
            "# Paper\n\nIntro.\n\n",
            "## Abstract\n\nAbstract text.\n\n",
            "## Methods\n\nMethods text."
        );
        let chunks = chunk_document(md, 500, 0);
        let methods = chunks
            .iter()
            .find(|c| c.heading_path.last() == Some(&"Methods".to_string()))
            .unwrap();
        // Methods should be ["Paper", "Methods"], not
        // ["Paper", "Abstract", "Methods"].
        assert_eq!(methods.heading_path, vec!["Paper", "Methods"]);
    }

    #[test]
    fn h3_under_different_h2s() {
        let md = concat!(
            "# Paper\n\n.\n\n",
            "## Methods\n\n.\n\n",
            "### Data\n\n.\n\n",
            "## Results\n\n.\n\n",
            "### Analysis\n\n."
        );
        let chunks = chunk_document(md, 500, 0);
        let data = chunks
            .iter()
            .find(|c| c.heading_path.last() == Some(&"Data".to_string()))
            .unwrap();
        let analysis = chunks
            .iter()
            .find(|c| c.heading_path.last() == Some(&"Analysis".to_string()))
            .unwrap();
        assert_eq!(data.heading_path, vec!["Paper", "Methods", "Data"]);
        assert_eq!(analysis.heading_path, vec!["Paper", "Results", "Analysis"]);
    }

    #[test]
    fn academic_paper_structure() {
        // Typical academic paper layout.
        let md = concat!(
            "# My Paper Title\n\n",
            "## Abstract\n\nThis paper...\n\n",
            "## 1 Introduction\n\nKnowledge graphs...\n\n",
            "## 2 Methods\n\nWe propose...\n\n",
            "### 2.1 Data\n\nWe collected...\n\n",
            "### 2.2 Model\n\nOur model...\n\n",
            "## 3 Results\n\nResults show...\n\n",
            "## 4 Conclusion\n\nWe conclude..."
        );
        let chunks = chunk_document(md, 500, 0);
        let sections: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Section)
            .collect();
        // 7 sections: abstract, intro, methods,
        // data, model, results, conclusion
        // (title heading has no body text so no section emitted)
        assert_eq!(sections.len(), 7);
    }

    #[test]
    fn detect_heading_h5_ignored() {
        assert!(detect_heading("##### H5 heading").is_none());
    }

    #[test]
    fn detect_heading_levels() {
        assert_eq!(detect_heading("# Title"), Some((1, "Title")));
        assert_eq!(detect_heading("## Sub"), Some((2, "Sub")));
        assert_eq!(detect_heading("### Deep"), Some((3, "Deep")));
        assert_eq!(detect_heading("#### Deeper"), Some((4, "Deeper")));
    }

    #[test]
    fn detect_heading_not_heading() {
        assert!(detect_heading("Not a heading").is_none());
        assert!(detect_heading("#NoSpace").is_none());
        assert!(detect_heading("").is_none());
    }

    #[test]
    fn overlap_snaps_to_word_boundary() {
        // Previous paragraph ends with "topological confidence"
        // Overlap of 15 would land inside "topological" →
        // "ical confidence". After the fix, it should snap to
        // "confidence" (start of next word).
        let para_a = "We use topological confidence measures";
        let para_b = "This paragraph continues.";
        let md = format!("# Section\n\n{para_a}\n\n{para_b}");

        let chunks = chunk_document(&md, 10, 15);
        let paragraphs: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Paragraph)
            .collect();

        assert!(paragraphs.len() >= 2);

        let second = &paragraphs[1];
        let prefix = &second.text[..second.context_prefix_len];
        // The overlap prefix should NOT start mid-word.
        // It should start at "confidence" (after snapping forward).
        assert!(
            !prefix.starts_with("ical"),
            "overlap should not start mid-word, got: {prefix:?}"
        );
        // The prefix should start with a complete word.
        let first_word_char = prefix.trim().chars().next().unwrap_or(' ');
        assert!(
            first_word_char.is_alphanumeric(),
            "prefix should start with a complete word, got: {prefix:?}"
        );
    }

    #[test]
    fn overlap_word_boundary_no_whitespace_in_suffix() {
        // Edge case: overlap lands in a string with no whitespace
        // after the start position. Should fall through gracefully.
        let para_a = "abcdefghijklmnopqrstuvwxyz";
        let para_b = "Second paragraph.";
        let md = format!("# Section\n\n{para_a}\n\n{para_b}");
        // Overlap of 10 lands at position 16 in a 26-char string
        // with no spaces. The find() returns None (no whitespace
        // in suffix), so start stays where it was.
        let chunks = chunk_document(&md, 5, 10);
        let paragraphs: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Paragraph)
            .collect();
        assert!(paragraphs.len() >= 2);
        // Should not panic.
        assert!(paragraphs[1].context_prefix_len > 0);
    }

    // --- Small section merging tests ---

    #[test]
    fn merge_disabled_by_default() {
        // chunk_document() passes min_section_size=0, so no merging.
        let md = concat!(
            "# Paper\n\n.\n\n",
            "## Methods\n\n.\n\n",
            "### Step A\n\nTiny.\n\n",
            "### Step B\n\nAlso tiny."
        );
        let chunks = chunk_document(md, 500, 0);
        let sections: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Section)
            .collect();
        // All four sections remain separate.
        assert_eq!(sections.len(), 4);
    }

    #[test]
    fn merge_combines_small_siblings() {
        let md = concat!(
            "# Paper\n\nIntro text.\n\n",
            "## Methods\n\nOverview.\n\n",
            "### Step A\n\nSmall A.\n\n",
            "### Step B\n\nSmall B.\n\n",
            "## Results\n\nBig results section."
        );
        // min_section_size=200 means "Small A." and "Small B."
        // (both < 200 chars) should merge. "Methods" and "Results"
        // bodies are also small but they're at H2 level and their
        // children are the ones being merged.
        let chunks = chunk_document_with_merge(md, 1000, 0, 200);
        let sections: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Section)
            .collect();

        // "Step A" + "Step B" should merge into one section
        // under parent ["Paper", "Methods"].
        // Remaining: Intro, Methods overview, merged A+B, Results.
        let merged = sections.iter().find(|s| {
            s.text.contains("Small A.") && s.text.contains("Small B.")
        });
        assert!(
            merged.is_some(),
            "small siblings should be merged"
        );
        // The merged section's heading path should be the parent.
        let m = merged.unwrap();
        assert_eq!(m.heading_path, vec!["Paper", "Methods"]);
    }

    #[test]
    fn merge_respects_max_size() {
        // Two small siblings whose combined size exceeds max.
        let body_a = "a".repeat(80);
        let body_b = "b".repeat(80);
        let md = format!(
            "# Paper\n\n.\n\n## Methods\n\n.\n\n### A\n\n{body_a}\n\n### B\n\n{body_b}"
        );
        // min=200, max=100 — each section is 80 chars, combined
        // would be ~162, which exceeds max_chunk_size=100.
        let chunks = chunk_document_with_merge(&md, 100, 0, 200);
        let sections: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Section)
            .collect();
        // Should NOT merge because combined > max_chunk_size.
        let has_a = sections.iter().any(|s| s.text.contains(&body_a));
        let has_b = sections.iter().any(|s| s.text.contains(&body_b));
        assert!(has_a && has_b, "sections should remain separate");
        // Make sure no section contains both.
        let merged = sections
            .iter()
            .any(|s| s.text.contains(&body_a) && s.text.contains(&body_b));
        assert!(!merged, "sections should not be merged past max_size");
    }

    #[test]
    fn merge_does_not_cross_h1_boundaries() {
        // Two small H1 sections should NOT merge.
        let md = "# Section A\n\nSmall.\n\n# Section B\n\nAlso small.";
        let chunks = chunk_document_with_merge(md, 1000, 0, 200);
        let sections: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Section)
            .collect();
        assert_eq!(sections.len(), 2, "H1 sections should not merge");
    }

    #[test]
    fn merge_does_not_cross_parent_boundaries() {
        let md = concat!(
            "# Paper\n\n.\n\n",
            "## Methods\n\n.\n\n",
            "### Data\n\nSmall data.\n\n",
            "## Results\n\n.\n\n",
            "### Analysis\n\nSmall analysis."
        );
        // Data and Analysis are small but have different parents
        // (Methods vs Results). Should not merge.
        let chunks = chunk_document_with_merge(md, 1000, 0, 200);
        let sections: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Section)
            .collect();
        let merged = sections.iter().any(|s| {
            s.text.contains("Small data.") && s.text.contains("Small analysis.")
        });
        assert!(!merged, "siblings under different parents should not merge");
    }

    #[test]
    fn merge_skips_large_sections() {
        let big = "x".repeat(300);
        let md = format!(
            "# Paper\n\n.\n\n## M\n\n.\n\n### A\n\n{big}\n\n### B\n\nSmall B."
        );
        // A is 300 chars (>200 min), B is small. A should not merge.
        let chunks = chunk_document_with_merge(&md, 1000, 0, 200);
        let sections: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Section)
            .collect();
        let merged = sections.iter().any(|s| {
            s.text.contains(&big) && s.text.contains("Small B.")
        });
        assert!(!merged, "large section should not merge with small");
    }

    #[test]
    fn merge_three_consecutive_siblings() {
        let md = concat!(
            "# Paper\n\nIntro.\n\n",
            "## Methods\n\nOverview.\n\n",
            "### Step 1\n\nOne.\n\n",
            "### Step 2\n\nTwo.\n\n",
            "### Step 3\n\nThree."
        );
        let chunks = chunk_document_with_merge(md, 1000, 0, 200);
        let sections: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Section)
            .collect();
        let merged = sections.iter().find(|s| {
            s.text.contains("One.") && s.text.contains("Two.") && s.text.contains("Three.")
        });
        assert!(
            merged.is_some(),
            "three small siblings should merge into one"
        );
    }
}

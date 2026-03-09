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
}

/// Split a Markdown document into hierarchical chunks.
///
/// Sections are split on `# ` headings. Sections exceeding
/// `max_chunk_size` bytes are further split into paragraphs
/// by `\n\n`. No document-level chunk is created; the
/// document embedding lives on the source record.
///
/// When `overlap` is non-zero, each paragraph chunk after the
/// first within a section is prefixed with the last `overlap`
/// characters of the previous paragraph. The
/// [`ChunkOutput::context_prefix_len`] field records how many
/// leading characters are overlap so consumers can trim them
/// from snippets or highlighting.
pub fn chunk_document(markdown: &str, max_chunk_size: usize, overlap: usize) -> Vec<ChunkOutput> {
    let mut chunks = Vec::new();

    if markdown.trim().is_empty() {
        return chunks;
    }

    let sections = split_sections(markdown);

    for (heading, body) in &sections {
        let heading_path: Vec<String> = heading.iter().map(|s| s.to_string()).collect();

        let section_text = body.trim();
        if section_text.is_empty() {
            continue;
        }

        let section_id = Uuid::new_v4();
        chunks.push(ChunkOutput {
            id: section_id,
            parent_id: None,
            text: section_text.to_string(),
            level: ChunkLevel::Section,
            heading_path: heading_path.clone(),
            context_prefix_len: 0,
        });

        if section_text.len() > max_chunk_size {
            let paragraphs: Vec<&str> = section_text
                .split("\n\n")
                .filter(|p| !p.trim().is_empty())
                .collect();

            if paragraphs.len() > 1 {
                let mut prev_text: Option<&str> = None;
                for para in &paragraphs {
                    let para = para.trim();
                    if para.is_empty() {
                        continue;
                    }

                    let (text, prefix_len) = build_overlap_text(prev_text, para, overlap);

                    chunks.push(ChunkOutput {
                        id: Uuid::new_v4(),
                        parent_id: Some(section_id),
                        text,
                        level: ChunkLevel::Paragraph,
                        heading_path: heading_path.clone(),
                        context_prefix_len: prefix_len,
                    });

                    prev_text = Some(para);
                }
            }
        }
    }

    chunks
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

    // Take the last `overlap` characters, snapping to a char
    // boundary (safe because we index from a known char offset).
    let start = prev.len().saturating_sub(overlap);
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

/// Split markdown into sections by `# ` headings.
/// Returns `(heading_path, body)` pairs.
fn split_sections(markdown: &str) -> Vec<(Vec<&str>, String)> {
    let mut sections: Vec<(Vec<&str>, String)> = Vec::new();
    let mut current_heading: Vec<&str> = Vec::new();
    let mut current_body = String::new();

    for line in markdown.lines() {
        let trimmed = line.trim();
        if let Some(title) = trimmed.strip_prefix("# ") {
            if !current_body.is_empty() || !current_heading.is_empty() {
                sections.push((current_heading.clone(), current_body.clone()));
            }
            current_heading = vec![title.trim()];
            current_body = String::new();
        } else {
            if !current_body.is_empty() {
                current_body.push('\n');
            }
            current_body.push_str(line);
        }
    }

    if !current_body.is_empty() || !current_heading.is_empty() {
        sections.push((current_heading, current_body));
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
}

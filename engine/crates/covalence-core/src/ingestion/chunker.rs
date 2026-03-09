//! Stage 4: Hierarchical chunking.
//!
//! Decomposes normalized Markdown into a chunk tree:
//! document → section → paragraph.
//! Primary boundaries from heading structure (structural),
//! with paragraph splitting for oversized sections.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Granularity level of a chunk output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChunkLevel {
    /// Entire document.
    Document,
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
    /// Parent chunk identifier (None for document-level).
    pub parent_id: Option<Uuid>,
    /// Text content of the chunk.
    pub text: String,
    /// Granularity level.
    pub level: ChunkLevel,
    /// Heading path from document root to this chunk.
    pub heading_path: Vec<String>,
}

/// Split a Markdown document into hierarchical chunks.
///
/// Sections are split on `# ` headings. Sections exceeding
/// `max_chunk_size` bytes are further split into paragraphs by `\n\n`.
pub fn chunk_document(markdown: &str, max_chunk_size: usize) -> Vec<ChunkOutput> {
    let mut chunks = Vec::new();

    if markdown.trim().is_empty() {
        return chunks;
    }

    let doc_id = Uuid::new_v4();
    chunks.push(ChunkOutput {
        id: doc_id,
        parent_id: None,
        text: markdown.to_string(),
        level: ChunkLevel::Document,
        heading_path: vec![],
    });

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
            parent_id: Some(doc_id),
            text: section_text.to_string(),
            level: ChunkLevel::Section,
            heading_path: heading_path.clone(),
        });

        if section_text.len() > max_chunk_size {
            let paragraphs: Vec<&str> = section_text
                .split("\n\n")
                .filter(|p| !p.trim().is_empty())
                .collect();

            if paragraphs.len() > 1 {
                for para in paragraphs {
                    let para = para.trim();
                    if para.is_empty() {
                        continue;
                    }
                    chunks.push(ChunkOutput {
                        id: Uuid::new_v4(),
                        parent_id: Some(section_id),
                        text: para.to_string(),
                        level: ChunkLevel::Paragraph,
                        heading_path: heading_path.clone(),
                    });
                }
            }
        }
    }

    chunks
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
        let chunks = chunk_document("", 500);
        assert!(chunks.is_empty());
    }

    #[test]
    fn single_section() {
        let md = "# Title\n\nSome content here.";
        let chunks = chunk_document(md, 500);
        assert!(chunks.iter().any(|c| c.level == ChunkLevel::Document));
        assert!(chunks.iter().any(|c| c.level == ChunkLevel::Section));
    }

    #[test]
    fn multiple_sections() {
        let md = "# First\n\nContent 1.\n\n# Second\n\nContent 2.";
        let chunks = chunk_document(md, 500);
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
        let chunks = chunk_document(&long_section, 50);
        let paragraphs: Vec<_> = chunks
            .iter()
            .filter(|c| c.level == ChunkLevel::Paragraph)
            .collect();
        assert_eq!(paragraphs.len(), 2);
    }

    #[test]
    fn heading_path_tracking() {
        let md = "# My Section\n\nContent.";
        let chunks = chunk_document(md, 500);
        let section = chunks
            .iter()
            .find(|c| c.level == ChunkLevel::Section)
            .unwrap();
        assert_eq!(section.heading_path, vec!["My Section"]);
    }

    #[test]
    fn parent_child_hierarchy() {
        let md = "# Title\n\nContent.";
        let chunks = chunk_document(md, 500);
        let doc = chunks
            .iter()
            .find(|c| c.level == ChunkLevel::Document)
            .unwrap();
        let section = chunks
            .iter()
            .find(|c| c.level == ChunkLevel::Section)
            .unwrap();
        assert_eq!(doc.parent_id, None);
        assert_eq!(section.parent_id, Some(doc.id));
    }
}

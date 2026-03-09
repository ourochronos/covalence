//! Stage 2: Parse raw content into a structured document.
//!
//! Handles format-specific parsing: Markdown passes through with title
//! extraction, plain text is wrapped as-is. Unsupported MIME types
//! produce an error.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A parsed document with extracted metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedDocument {
    /// Document title extracted from content (e.g. first H1 in Markdown).
    pub title: Option<String>,
    /// Main body text.
    pub body: String,
    /// Format-specific metadata key-value pairs.
    pub metadata: HashMap<String, String>,
}

/// Parse raw content bytes into a structured document.
///
/// Supported MIME types:
/// - `text/markdown`: passes through, extracts first `# ` heading as title
/// - `text/plain`: wraps content as-is with no title
pub fn parse(content: &[u8], mime: &str) -> crate::error::Result<ParsedDocument> {
    let text = String::from_utf8_lossy(content);

    match mime {
        "text/markdown" => {
            let title = extract_first_h1(&text);
            Ok(ParsedDocument {
                title,
                body: text.into_owned(),
                metadata: HashMap::new(),
            })
        }
        "text/plain" => Ok(ParsedDocument {
            title: None,
            body: text.into_owned(),
            metadata: HashMap::new(),
        }),
        _ => Err(crate::error::Error::Ingestion(format!(
            "unsupported MIME type: {mime}"
        ))),
    }
}

/// Extract the text of the first `# ` (H1) heading from Markdown.
fn extract_first_h1(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(title) = trimmed.strip_prefix("# ") {
            let title = title.trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_markdown_extracts_title() {
        let content = b"# My Title\n\nSome body text.";
        let doc = parse(content, "text/markdown").unwrap();
        assert_eq!(doc.title, Some("My Title".to_string()));
        assert!(doc.body.contains("Some body text."));
    }

    #[test]
    fn parse_markdown_no_title() {
        let content = b"Just some text without headings.";
        let doc = parse(content, "text/markdown").unwrap();
        assert_eq!(doc.title, None);
    }

    #[test]
    fn parse_plain_text() {
        let content = b"Plain text content here.";
        let doc = parse(content, "text/plain").unwrap();
        assert_eq!(doc.title, None);
        assert_eq!(doc.body, "Plain text content here.");
    }

    #[test]
    fn parse_unsupported_mime() {
        let result = parse(b"<html></html>", "text/html");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unsupported MIME type"));
    }
}

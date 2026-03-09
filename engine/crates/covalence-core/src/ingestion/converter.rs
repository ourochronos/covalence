//! Pluggable format converter system for the ingestion pipeline.
//!
//! Converts raw source content (HTML, plain text, etc.) into Markdown
//! before the parser stage processes it. Each converter handles one or
//! more MIME content types. The [`ConverterRegistry`] dispatches to the
//! appropriate converter based on the content type of incoming data.

use crate::error::{Error, Result};

/// Trait for converting raw source content into Markdown.
///
/// Implementations handle specific content types (e.g. HTML, plain
/// text) and produce Markdown suitable for the downstream parser.
#[async_trait::async_trait]
pub trait SourceConverter: Send + Sync {
    /// Convert raw source content to Markdown for ingestion.
    async fn convert(&self, content: &[u8], content_type: &str) -> Result<String>;

    /// Content types this converter handles.
    ///
    /// Examples: `["text/markdown", "text/x-markdown"]`.
    fn supported_types(&self) -> &[&str];
}

/// Passthrough converter for Markdown content.
///
/// Decodes UTF-8 bytes and returns them unchanged. Invalid UTF-8
/// sequences are replaced with the Unicode replacement character.
pub struct MarkdownConverter;

#[async_trait::async_trait]
impl SourceConverter for MarkdownConverter {
    async fn convert(&self, content: &[u8], _content_type: &str) -> Result<String> {
        Ok(String::from_utf8_lossy(content).into_owned())
    }

    fn supported_types(&self) -> &[&str] {
        &["text/markdown", "text/x-markdown"]
    }
}

/// Converter for plain text content.
///
/// Wraps the text in a simple Markdown structure: an "Untitled
/// Document" heading followed by the body text.
pub struct PlainTextConverter;

#[async_trait::async_trait]
impl SourceConverter for PlainTextConverter {
    async fn convert(&self, content: &[u8], _content_type: &str) -> Result<String> {
        let text = String::from_utf8_lossy(content);
        Ok(format!("# Untitled Document\n\n{text}"))
    }

    fn supported_types(&self) -> &[&str] {
        &["text/plain"]
    }
}

/// Converter for HTML content.
///
/// Performs basic HTML-to-Markdown conversion by stripping tags and
/// preserving structural elements (headings, paragraphs, list items,
/// line breaks). Uses a simple state-machine parser rather than a
/// heavy external dependency.
pub struct HtmlConverter;

#[async_trait::async_trait]
impl SourceConverter for HtmlConverter {
    async fn convert(&self, content: &[u8], _content_type: &str) -> Result<String> {
        let html = String::from_utf8_lossy(content);
        Ok(strip_html(&html))
    }

    fn supported_types(&self) -> &[&str] {
        &["text/html", "application/xhtml+xml"]
    }
}

/// Strip HTML tags and convert structural elements to Markdown.
///
/// Handles: `<h1>`-`<h6>` (to `#`-`######`), `<p>` (blank line),
/// `<br>` / `<br/>` (newline), `<li>` (bullet), and `<script>` /
/// `<style>` (entire block removed). All other tags are removed,
/// preserving inner text. HTML entities `&amp;`, `&lt;`, `&gt;`,
/// `&quot;`, and `&nbsp;` are decoded.
fn strip_html(html: &str) -> String {
    // First, remove <script> and <style> blocks entirely.
    let cleaned = remove_block_elements(html);

    let mut output = String::with_capacity(cleaned.len());
    let mut inside_tag = false;
    let mut current_tag = String::new();

    for ch in cleaned.chars() {
        if ch == '<' {
            inside_tag = true;
            current_tag.clear();
            continue;
        }

        if inside_tag {
            if ch == '>' {
                inside_tag = false;
                let tag_lower = current_tag.to_lowercase();
                let tag_lower = tag_lower.trim();

                // Handle structural tags
                if let Some(rest) = tag_lower.strip_prefix("h") {
                    // Opening heading: h1-h6
                    let level_str = rest.split_whitespace().next().unwrap_or("");
                    if let Ok(level) = level_str.parse::<u8>() {
                        if (1..=6).contains(&level) {
                            let prefix = "#".repeat(level as usize);
                            // Ensure blank line before heading
                            if !output.ends_with('\n') && !output.is_empty() {
                                output.push('\n');
                            }
                            if !output.ends_with("\n\n") && !output.is_empty() {
                                output.push('\n');
                            }
                            output.push_str(&prefix);
                            output.push(' ');
                        }
                    }
                } else if tag_lower.starts_with("/h") {
                    // Closing heading — add newlines
                    output.push('\n');
                } else if tag_lower == "p" || tag_lower.starts_with("p ") {
                    if !output.is_empty() && !output.ends_with("\n\n") {
                        if !output.ends_with('\n') {
                            output.push('\n');
                        }
                        output.push('\n');
                    }
                } else if tag_lower == "/p" {
                    if !output.ends_with('\n') {
                        output.push('\n');
                    }
                    output.push('\n');
                } else if tag_lower == "br" || tag_lower == "br/" || tag_lower == "br /" {
                    output.push('\n');
                } else if tag_lower == "li" || tag_lower.starts_with("li ") {
                    if !output.ends_with('\n') && !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str("- ");
                } else if tag_lower == "/li" && !output.ends_with('\n') {
                    output.push('\n');
                }
                // All other tags are silently consumed.
            } else {
                current_tag.push(ch);
            }
            continue;
        }

        output.push(ch);
    }

    // Decode common HTML entities
    let output = decode_html_entities(&output);

    // Collapse runs of 3+ newlines into 2, and trim.
    collapse_newlines(&output)
}

/// Remove `<script>...</script>` and `<style>...</style>` blocks
/// (case-insensitive).
fn remove_block_elements(html: &str) -> String {
    let mut result = html.to_string();
    for tag in &["script", "style"] {
        loop {
            let lower = result.to_lowercase();
            let open = format!("<{tag}");
            let close = format!("</{tag}>");
            let Some(start) = lower.find(&open) else {
                break;
            };
            let Some(end) = lower[start..].find(&close) else {
                // Unclosed tag — remove from open to end.
                result.truncate(start);
                break;
            };
            let end_abs = start + end + close.len();
            result.replace_range(start..end_abs, "");
        }
    }
    result
}

/// Decode a small set of common HTML entities.
fn decode_html_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&nbsp;", " ")
        .replace("&#39;", "'")
}

/// Collapse runs of 3+ consecutive newlines into exactly 2, and trim
/// leading/trailing whitespace.
fn collapse_newlines(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut newline_count = 0u32;

    for ch in text.chars() {
        if ch == '\n' {
            newline_count += 1;
            if newline_count <= 2 {
                result.push(ch);
            }
        } else {
            newline_count = 0;
            result.push(ch);
        }
    }

    result.trim().to_string()
}

/// Registry of format converters, dispatching by content type.
///
/// On creation, registers the built-in converters ([`MarkdownConverter`],
/// [`PlainTextConverter`], [`HtmlConverter`]). Additional converters can
/// be added via [`register`](ConverterRegistry::register).
pub struct ConverterRegistry {
    /// Registered converters, checked in order.
    converters: Vec<Box<dyn SourceConverter>>,
}

impl ConverterRegistry {
    /// Create a new registry with the default built-in converters.
    pub fn new() -> Self {
        let mut registry = Self {
            converters: Vec::new(),
        };
        registry.converters.push(Box::new(MarkdownConverter));
        registry.converters.push(Box::new(PlainTextConverter));
        registry.converters.push(Box::new(HtmlConverter));
        registry
    }

    /// Register an additional converter.
    ///
    /// The converter is appended to the end of the list. Converters
    /// are checked in order, so earlier registrations take priority
    /// when content types overlap.
    pub fn register(&mut self, converter: Box<dyn SourceConverter>) {
        self.converters.push(converter);
    }

    /// Convert content using the first converter that supports the
    /// given content type.
    ///
    /// Returns an [`Error::Ingestion`] if no converter handles the
    /// content type.
    pub async fn convert(&self, content: &[u8], content_type: &str) -> Result<String> {
        // Normalize: strip parameters (e.g. "text/html; charset=utf-8")
        let base_type = content_type
            .split(';')
            .next()
            .unwrap_or(content_type)
            .trim();

        for converter in &self.converters {
            if converter
                .supported_types()
                .iter()
                .any(|&t| t.eq_ignore_ascii_case(base_type))
            {
                return converter.convert(content, base_type).await;
            }
        }

        Err(Error::Ingestion(format!(
            "no converter registered for content type: \
             {content_type}"
        )))
    }
}

impl Default for ConverterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn markdown_passthrough() {
        let converter = MarkdownConverter;
        let input = b"# Hello\n\nWorld";
        let result = converter
            .convert(input, "text/markdown")
            .await
            .expect("conversion should succeed");
        assert_eq!(result, "# Hello\n\nWorld");
    }

    #[tokio::test]
    async fn markdown_invalid_utf8() {
        let converter = MarkdownConverter;
        // Invalid UTF-8: 0xFF is not a valid start byte.
        let input: &[u8] = &[0x48, 0x65, 0x6C, 0xFF, 0x6F];
        let result = converter
            .convert(input, "text/markdown")
            .await
            .expect("conversion should succeed");
        // The invalid byte should be replaced with U+FFFD.
        assert!(result.contains('\u{FFFD}'));
        assert!(result.starts_with("Hel"));
    }

    #[tokio::test]
    async fn plain_text_wrapping() {
        let converter = PlainTextConverter;
        let input = b"Just some text.";
        let result = converter
            .convert(input, "text/plain")
            .await
            .expect("conversion should succeed");
        assert!(result.starts_with("# Untitled Document"));
        assert!(result.contains("Just some text."));
    }

    #[tokio::test]
    async fn html_strips_tags() {
        let converter = HtmlConverter;
        let input = b"<p>Hello <b>world</b></p>";
        let result = converter
            .convert(input, "text/html")
            .await
            .expect("conversion should succeed");
        assert!(result.contains("Hello world"));
        assert!(!result.contains("<p>"));
        assert!(!result.contains("<b>"));
    }

    #[tokio::test]
    async fn html_converts_headings() {
        let converter = HtmlConverter;
        let input = b"<h1>Title</h1><h2>Subtitle</h2>";
        let result = converter
            .convert(input, "text/html")
            .await
            .expect("conversion should succeed");
        assert!(result.contains("# Title"));
        assert!(result.contains("## Subtitle"));
    }

    #[tokio::test]
    async fn html_converts_lists() {
        let converter = HtmlConverter;
        let input = b"<ul><li>One</li><li>Two</li></ul>";
        let result = converter
            .convert(input, "text/html")
            .await
            .expect("conversion should succeed");
        assert!(result.contains("- One"));
        assert!(result.contains("- Two"));
    }

    #[tokio::test]
    async fn html_removes_script_and_style() {
        let converter = HtmlConverter;
        let input = b"<p>Hello</p><script>alert('xss')</script><p>World</p>";
        let result = converter
            .convert(input, "text/html")
            .await
            .expect("conversion should succeed");
        assert!(!result.contains("alert"));
        assert!(!result.contains("script"));
        assert!(result.contains("Hello"));
        assert!(result.contains("World"));
    }

    #[tokio::test]
    async fn html_decodes_entities() {
        let converter = HtmlConverter;
        let input = b"<p>A &amp; B &lt; C &gt; D</p>";
        let result = converter
            .convert(input, "text/html")
            .await
            .expect("conversion should succeed");
        assert!(result.contains("A & B < C > D"));
    }

    #[tokio::test]
    async fn html_handles_br_tags() {
        let converter = HtmlConverter;
        let input = b"Line one<br>Line two<br/>Line three";
        let result = converter
            .convert(input, "text/html")
            .await
            .expect("conversion should succeed");
        assert!(result.contains("Line one\nLine two\nLine three"));
    }

    #[tokio::test]
    async fn registry_dispatches_by_content_type() {
        let registry = ConverterRegistry::new();

        // Markdown
        let md = registry
            .convert(b"# Test", "text/markdown")
            .await
            .expect("markdown should work");
        assert_eq!(md, "# Test");

        // Plain text
        let txt = registry
            .convert(b"Hello", "text/plain")
            .await
            .expect("plain text should work");
        assert!(txt.contains("# Untitled Document"));
        assert!(txt.contains("Hello"));

        // HTML
        let html = registry
            .convert(b"<p>Hi</p>", "text/html")
            .await
            .expect("html should work");
        assert!(html.contains("Hi"));
        assert!(!html.contains("<p>"));
    }

    #[tokio::test]
    async fn registry_unknown_type_error() {
        let registry = ConverterRegistry::new();
        let result = registry.convert(b"data", "application/pdf").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no converter registered"));
        assert!(err.contains("application/pdf"));
    }

    #[tokio::test]
    async fn registry_strips_charset_parameter() {
        let registry = ConverterRegistry::new();
        let result = registry
            .convert(b"<p>Hello</p>", "text/html; charset=utf-8")
            .await
            .expect("should handle content type with params");
        assert!(result.contains("Hello"));
    }

    #[tokio::test]
    async fn registry_custom_converter() {
        /// Test converter that uppercases content.
        struct UpperConverter;

        #[async_trait::async_trait]
        impl SourceConverter for UpperConverter {
            async fn convert(&self, content: &[u8], _content_type: &str) -> Result<String> {
                let text = String::from_utf8_lossy(content);
                Ok(text.to_uppercase())
            }

            fn supported_types(&self) -> &[&str] {
                &["text/x-upper"]
            }
        }

        let mut registry = ConverterRegistry::new();
        registry.register(Box::new(UpperConverter));

        let result = registry
            .convert(b"hello", "text/x-upper")
            .await
            .expect("custom converter should work");
        assert_eq!(result, "HELLO");
    }

    #[tokio::test]
    async fn registry_default_trait() {
        // Ensure Default impl works.
        let registry = ConverterRegistry::default();
        let result = registry
            .convert(b"test", "text/plain")
            .await
            .expect("default registry should work");
        assert!(result.contains("test"));
    }

    #[test]
    fn strip_html_empty_input() {
        assert_eq!(strip_html(""), "");
    }

    #[test]
    fn strip_html_no_tags() {
        assert_eq!(strip_html("Just plain text"), "Just plain text");
    }

    #[test]
    fn strip_html_nested_tags() {
        let result = strip_html("<div><p>Nested <em>content</em></p></div>");
        assert!(result.contains("Nested content"));
    }

    #[test]
    fn collapse_newlines_works() {
        let input = "a\n\n\n\n\nb";
        let result = collapse_newlines(input);
        assert_eq!(result, "a\n\nb");
    }

    #[test]
    fn decode_entities_all() {
        let input = "&amp; &lt; &gt; &quot; &nbsp; &#39;";
        let result = decode_html_entities(input);
        // &nbsp; decodes to a regular space, so there are two
        // spaces between the quote and apostrophe.
        assert_eq!(result, "& < > \"   '");
    }

    #[test]
    fn remove_block_elements_case_insensitive() {
        let input = "<SCRIPT>bad</SCRIPT>good";
        let result = remove_block_elements(input);
        assert_eq!(result, "good");
    }

    #[test]
    fn supported_types_markdown() {
        let c = MarkdownConverter;
        assert_eq!(c.supported_types(), &["text/markdown", "text/x-markdown"]);
    }

    #[test]
    fn supported_types_plain() {
        let c = PlainTextConverter;
        assert_eq!(c.supported_types(), &["text/plain"]);
    }

    #[test]
    fn supported_types_html() {
        let c = HtmlConverter;
        assert_eq!(c.supported_types(), &["text/html", "application/xhtml+xml"]);
    }
}

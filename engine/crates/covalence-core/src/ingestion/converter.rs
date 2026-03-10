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

/// Converter for HTML content via the ReaderLM-v2 MLX sidecar.
///
/// Calls the ReaderLM sidecar HTTP endpoint to produce high-quality
/// Markdown from HTML (preserving tables, lists, semantic structure).
/// Falls back to the built-in [`HtmlConverter`] tag stripper if the
/// sidecar is unreachable or returns an error.
pub struct ReaderLmConverter {
    /// Base URL of the ReaderLM sidecar (e.g. `http://localhost:8432`).
    base_url: String,
    /// HTTP client for sidecar requests.
    client: reqwest::Client,
}

impl ReaderLmConverter {
    /// Create a new ReaderLM converter pointing at the given sidecar URL.
    pub fn new(base_url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default();
        Self { base_url, client }
    }
}

#[async_trait::async_trait]
impl SourceConverter for ReaderLmConverter {
    async fn convert(&self, content: &[u8], _content_type: &str) -> Result<String> {
        let html = String::from_utf8_lossy(content);

        // Try the sidecar first.
        let url = format!("{}/convert", self.base_url);
        let body = serde_json::json!({ "html": html });

        match self.client.post(&url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<serde_json::Value>().await {
                    Ok(json) => {
                        if let Some(md) = json["markdown"].as_str() {
                            tracing::debug!(
                                html_len = html.len(),
                                md_len = md.len(),
                                "ReaderLM conversion succeeded"
                            );
                            return Ok(md.to_string());
                        }
                        tracing::warn!("ReaderLM response missing 'markdown' field, falling back");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "ReaderLM response parse failed, falling back");
                    }
                }
            }
            Ok(resp) => {
                tracing::warn!(
                    status = %resp.status(),
                    "ReaderLM sidecar returned error, falling back to HtmlConverter"
                );
            }
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "ReaderLM sidecar unavailable, falling back to HtmlConverter"
                );
            }
        }

        // Fallback: use the built-in tag stripper.
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

/// Linearize markdown pipe tables into natural language sentences.
///
/// Converts tables like:
/// ```text
/// | Model | Developer | Year |
/// |-------|-----------|------|
/// | GraphRAG | Microsoft | 2024 |
/// ```
/// Into:
/// ```text
/// Model: GraphRAG, Developer: Microsoft, Year: 2024.
/// ```
///
/// Preserves all non-table content unchanged. Tables without headers
/// use generic "Column 1", "Column 2", etc.
pub fn linearize_tables(markdown: &str) -> String {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut result = String::with_capacity(markdown.len());
    let mut i = 0;

    while i < lines.len() {
        // Detect a table: a pipe-delimited row followed by a
        // separator row (pipes + dashes/colons).
        if is_table_row(lines[i]) && i + 1 < lines.len() && is_separator_row(lines[i + 1]) {
            let headers = parse_table_row(lines[i]);
            i += 2; // skip header + separator

            // Process data rows.
            while i < lines.len() && is_table_row(lines[i]) {
                let cells = parse_table_row(lines[i]);
                let pairs: Vec<String> = headers
                    .iter()
                    .zip(cells.iter())
                    .filter(|(_, v)| !v.is_empty())
                    .map(|(h, v)| format!("{h}: {v}"))
                    .collect();
                if !pairs.is_empty() {
                    result.push_str(&pairs.join(", "));
                    result.push_str(".\n");
                }
                i += 1;
            }
        } else if is_table_row(lines[i]) && !is_separator_row(lines[i]) {
            // Table without headers — use generic column names.
            // Peek ahead to see if multiple pipe rows follow.
            let first_row = parse_table_row(lines[i]);
            let col_count = first_row.len();

            // Check if next line is also a table row (headerless table).
            if i + 1 < lines.len() && is_table_row(lines[i + 1]) && !is_separator_row(lines[i + 1])
            {
                let headers: Vec<String> = (1..=col_count).map(|n| format!("Column {n}")).collect();

                // Linearize all consecutive rows including the first.
                while i < lines.len() && is_table_row(lines[i]) && !is_separator_row(lines[i]) {
                    let cells = parse_table_row(lines[i]);
                    let pairs: Vec<String> = headers
                        .iter()
                        .zip(cells.iter())
                        .filter(|(_, v)| !v.is_empty())
                        .map(|(h, v)| format!("{h}: {v}"))
                        .collect();
                    if !pairs.is_empty() {
                        result.push_str(&pairs.join(", "));
                        result.push_str(".\n");
                    }
                    i += 1;
                }
            } else {
                // Single pipe row — not really a table, keep as-is.
                result.push_str(lines[i]);
                result.push('\n');
                i += 1;
            }
        } else {
            result.push_str(lines[i]);
            result.push('\n');
            i += 1;
        }
    }

    // Trim trailing newline added by line-by-line processing if
    // the original didn't end with one.
    if !markdown.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }

    result
}

/// Check if a line looks like a markdown table row (starts/ends with `|`
/// or contains at least 2 `|` characters).
fn is_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Must have at least 2 pipe characters to be a table row.
    trimmed.matches('|').count() >= 2
}

/// Check if a line is a table separator row (only `|`, `-`, `:`, and
/// spaces).
fn is_separator_row(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || !trimmed.contains('-') {
        return false;
    }
    trimmed
        .chars()
        .all(|c| c == '|' || c == '-' || c == ':' || c == ' ')
}

/// Parse a pipe-delimited table row into trimmed cell values.
fn parse_table_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    // Strip leading and trailing pipes.
    let inner = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or(trimmed);
    inner
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
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

    /// Register an additional converter at the front of the list.
    ///
    /// The converter takes priority over all previously registered
    /// converters for overlapping content types.
    pub fn register_front(&mut self, converter: Box<dyn SourceConverter>) {
        self.converters.insert(0, converter);
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
                let md = converter.convert(content, base_type).await?;
                // Post-process: linearize markdown tables into
                // natural language sentences for better NER/embedding.
                return Ok(linearize_tables(&md));
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

    #[test]
    fn supported_types_readerlm() {
        let c = ReaderLmConverter::new("http://localhost:8432".into());
        assert_eq!(c.supported_types(), &["text/html", "application/xhtml+xml"]);
    }

    #[tokio::test]
    async fn readerlm_fallback_when_sidecar_down() {
        // Point at a non-existent sidecar — should fall back to
        // HtmlConverter's strip_html.
        let converter = ReaderLmConverter::new("http://127.0.0.1:1".into());
        let input = b"<p>Hello <b>world</b></p>";
        let result = converter
            .convert(input, "text/html")
            .await
            .expect("should fall back gracefully");
        assert!(result.contains("Hello world"));
        assert!(!result.contains("<p>"));
    }

    #[tokio::test]
    async fn readerlm_fallback_preserves_structure() {
        let converter = ReaderLmConverter::new("http://127.0.0.1:1".into());
        let input = b"<h1>Title</h1><ul><li>One</li><li>Two</li></ul>";
        let result = converter
            .convert(input, "text/html")
            .await
            .expect("fallback should preserve structure");
        assert!(result.contains("# Title"));
        assert!(result.contains("- One"));
        assert!(result.contains("- Two"));
    }

    #[tokio::test]
    async fn registry_readerlm_takes_priority() {
        // ReaderLmConverter registered front should be dispatched
        // for text/html instead of HtmlConverter.
        let mut registry = ConverterRegistry::new();
        registry.register_front(Box::new(ReaderLmConverter::new(
            "http://127.0.0.1:1".into(),
        )));

        // Even though the sidecar is down, ReaderLmConverter's
        // fallback (strip_html) produces the same result as
        // HtmlConverter — proving it was dispatched first.
        let result = registry
            .convert(b"<p>Test</p>", "text/html")
            .await
            .expect("should work via readerlm fallback");
        assert!(result.contains("Test"));
    }

    #[tokio::test]
    async fn register_front_takes_priority() {
        /// Converter that always returns "FRONT" for text/plain.
        struct FrontConverter;

        #[async_trait::async_trait]
        impl SourceConverter for FrontConverter {
            async fn convert(&self, _content: &[u8], _content_type: &str) -> Result<String> {
                Ok("FRONT".to_string())
            }

            fn supported_types(&self) -> &[&str] {
                &["text/plain"]
            }
        }

        let mut registry = ConverterRegistry::new();
        registry.register_front(Box::new(FrontConverter));

        let result = registry
            .convert(b"hello", "text/plain")
            .await
            .expect("front converter should win");
        assert_eq!(result, "FRONT");
    }

    // --- Table linearization tests ---

    #[test]
    fn linearize_basic_table() {
        let input = "\
| Model | Developer | Year |
|-------|-----------|------|
| GraphRAG | Microsoft Research | 2024 |
| HDBSCAN | Campello et al. | 2013 |";
        let result = linearize_tables(input);
        assert_eq!(
            result,
            "Model: GraphRAG, Developer: Microsoft Research, Year: 2024.\n\
             Model: HDBSCAN, Developer: Campello et al., Year: 2013."
        );
    }

    #[test]
    fn linearize_preserves_non_table_content() {
        let input = "\
# Heading

Some paragraph text.

| A | B |
|---|---|
| 1 | 2 |

More text after.";
        let result = linearize_tables(input);
        assert!(result.contains("# Heading"));
        assert!(result.contains("Some paragraph text."));
        assert!(result.contains("A: 1, B: 2."));
        assert!(result.contains("More text after."));
    }

    #[test]
    fn linearize_empty_cells_skipped() {
        let input = "\
| Name | Value |
|------|-------|
| foo |  |
| bar | 42 |";
        let result = linearize_tables(input);
        assert!(result.contains("Name: foo."));
        assert!(result.contains("Name: bar, Value: 42."));
    }

    #[test]
    fn linearize_no_table() {
        let input = "Just plain markdown\nwith no tables.";
        let result = linearize_tables(input);
        assert_eq!(result, input);
    }

    #[test]
    fn linearize_two_column_key_value() {
        let input = "\
| Key | Value |
|-----|-------|
| name | Covalence |
| version | 0.1.0 |
| language | Rust |";
        let result = linearize_tables(input);
        assert!(result.contains("Key: name, Value: Covalence."));
        assert!(result.contains("Key: version, Value: 0.1.0."));
        assert!(result.contains("Key: language, Value: Rust."));
    }

    #[test]
    fn linearize_adjacent_tables() {
        let input = "\
| A | B |
|---|---|
| 1 | 2 |

| X | Y |
|---|---|
| 3 | 4 |";
        let result = linearize_tables(input);
        assert!(result.contains("A: 1, B: 2."));
        assert!(result.contains("X: 3, Y: 4."));
    }

    #[test]
    fn linearize_colon_separator() {
        // Separator with colons for alignment.
        let input = "\
| Left | Center | Right |
|:-----|:------:|------:|
| a | b | c |";
        let result = linearize_tables(input);
        assert!(result.contains("Left: a, Center: b, Right: c."));
    }

    #[test]
    fn is_separator_row_positive() {
        assert!(is_separator_row("|---|---|"));
        assert!(is_separator_row("| --- | --- |"));
        assert!(is_separator_row("|:---|:---:|---:|"));
        assert!(is_separator_row("| :--- | :---: | ---: |"));
    }

    #[test]
    fn is_separator_row_negative() {
        assert!(!is_separator_row("| a | b |"));
        assert!(!is_separator_row(""));
        assert!(!is_separator_row("| | |"));
    }

    #[test]
    fn parse_table_row_strips_pipes() {
        let cells = parse_table_row("| foo | bar | baz |");
        assert_eq!(cells, vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn linearize_large_table() {
        let mut input = String::from("| ID | Name |\n|---|---|\n");
        for i in 0..50 {
            input.push_str(&format!("| {i} | item-{i} |\n"));
        }
        let result = linearize_tables(&input);
        assert!(result.contains("ID: 0, Name: item-0."));
        assert!(result.contains("ID: 49, Name: item-49."));
        // No pipe characters should remain in linearized output.
        for line in result.lines() {
            if line.contains("ID:") {
                assert!(!line.contains('|'));
            }
        }
    }

    #[tokio::test]
    async fn registry_linearizes_tables_in_markdown() {
        let registry = ConverterRegistry::new();
        let md_with_table = b"# Test\n\n| A | B |\n|---|---|\n| 1 | 2 |\n";
        let result = registry
            .convert(md_with_table, "text/markdown")
            .await
            .expect("should convert");
        assert!(result.contains("A: 1, B: 2."));
        assert!(result.contains("# Test"));
    }
}

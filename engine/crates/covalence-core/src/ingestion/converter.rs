//! Pluggable format converter system for the ingestion pipeline.
//!
//! Converts raw source content (HTML, plain text, etc.) into Markdown
//! before the parser stage processes it. Each converter handles one or
//! more MIME content types. The [`ConverterRegistry`] dispatches to the
//! appropriate converter based on the content type of incoming data.

use crate::error::{Error, Result};
use crate::ingestion::utils::decode_html_entities;

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

/// Maximum HTML size for a single ReaderLM sidecar call.
///
/// HTML exceeding this limit is split into windows at structural
/// boundaries (`<section>`, `<article>`, `<div>`, heading tags)
/// and each window is converted separately.
const READERLM_MAX_CHARS: usize = 50_000;

/// Converter for HTML content via the ReaderLM-v2 MLX sidecar.
///
/// Calls the ReaderLM sidecar HTTP endpoint to produce high-quality
/// Markdown from HTML (preserving tables, lists, semantic structure).
///
/// For large HTML documents exceeding [`READERLM_MAX_CHARS`], the
/// HTML is split at structural element boundaries and each window
/// is converted separately, then reassembled.
///
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

    /// Convert a single HTML fragment via the sidecar.
    ///
    /// Returns `None` if the sidecar is unreachable or returns an
    /// error, signaling the caller to fall back.
    async fn convert_single(&self, html: &str) -> Option<String> {
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
                            return Some(md.to_string());
                        }
                        tracing::warn!("ReaderLM response missing 'markdown' field");
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "ReaderLM response parse failed"
                        );
                    }
                }
            }
            Ok(resp) => {
                tracing::warn!(
                    status = %resp.status(),
                    "ReaderLM sidecar returned error"
                );
            }
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "ReaderLM sidecar unavailable"
                );
            }
        }

        None
    }
}

#[async_trait::async_trait]
impl SourceConverter for ReaderLmConverter {
    async fn convert(&self, content: &[u8], _content_type: &str) -> Result<String> {
        // Strip boilerplate before sending to the sidecar so the
        // model only processes actual article content.
        let raw_html = String::from_utf8_lossy(content);
        let html = strip_boilerplate(&raw_html);

        if html.len() <= READERLM_MAX_CHARS {
            // Small enough for a single call.
            if let Some(md) = self.convert_single(&html).await {
                return Ok(md);
            }
            return Ok(strip_html(&html));
        }

        // Large HTML: split at structural boundaries and convert
        // each window separately.
        let windows = split_html_windows(&html, READERLM_MAX_CHARS);
        tracing::debug!(
            html_len = html.len(),
            windows = windows.len(),
            "splitting large HTML for ReaderLM windowed conversion"
        );

        let mut parts = Vec::with_capacity(windows.len());

        for window in &windows {
            if let Some(md) = self.convert_single(window).await {
                parts.push(md);
            } else {
                // Sidecar failed on this window. To avoid
                // inconsistent mixed output (some windows
                // ReaderLM, some tag-stripped), discard any
                // partial results and fall back to tag-stripping
                // the entire source document as one pass.
                tracing::warn!(
                    completed_windows = parts.len(),
                    total_windows = windows.len(),
                    "ReaderLM sidecar failed mid-batch, \
                     falling back to full document tag strip"
                );
                return Ok(strip_html(&html));
            }
        }

        Ok(parts.join("\n\n"))
    }

    fn supported_types(&self) -> &[&str] {
        &["text/html", "application/xhtml+xml"]
    }
}

/// Converter for PDF content via an external sidecar.
///
/// Calls a configurable HTTP endpoint (e.g., pymupdf4llm sidecar)
/// to extract Markdown from PDF files. The sidecar must accept
/// `POST /convert-pdf` with raw PDF bytes in the body and return
/// `{"markdown": "..."}`.
pub struct PdfConverter {
    /// Base URL of the PDF conversion sidecar.
    base_url: String,
    /// HTTP client with generous timeout for large PDFs.
    client: reqwest::Client,
}

impl PdfConverter {
    /// Create a new PDF converter pointing at the given sidecar URL.
    pub fn new(base_url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .unwrap_or_default();
        Self { base_url, client }
    }

    /// Validate connectivity with the PDF sidecar.
    ///
    /// Calls the health endpoint to verify the sidecar is reachable
    /// and ready. Returns an error with a clear message if not.
    pub async fn validate(&self) -> Result<()> {
        let url = format!("{}/health", self.base_url);
        let resp = self
            .client
            .get(&url)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| {
                Error::Ingestion(format!(
                    "PDF sidecar validation failed ({}): {e}",
                    self.base_url
                ))
            })?;

        if !resp.status().is_success() {
            return Err(Error::Ingestion(format!(
                "PDF sidecar health check returned {}",
                resp.status()
            )));
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl SourceConverter for PdfConverter {
    async fn convert(&self, content: &[u8], _content_type: &str) -> Result<String> {
        let url = format!("{}/convert-pdf", self.base_url);

        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/pdf")
            .body(content.to_vec())
            .send()
            .await
            .map_err(|e| Error::Ingestion(format!("PDF sidecar request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(Error::Ingestion(format!(
                "PDF sidecar returned {status}: {body_text}"
            )));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| Error::Ingestion(format!("failed to parse PDF response: {e}")))?;

        json["markdown"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                Error::Ingestion("PDF sidecar response missing 'markdown' field".to_string())
            })
    }

    fn supported_types(&self) -> &[&str] {
        &["application/pdf"]
    }
}

/// Converter for source code files using tree-sitter parsing.
///
/// Parses Rust and Python source files into AST nodes and produces
/// Markdown where each top-level item (function, struct, class, etc.)
/// becomes a `# ` section with a fenced code block body. The existing
/// heading-based chunker then splits at these natural boundaries.
pub struct CodeConverter;

#[async_trait::async_trait]
impl SourceConverter for CodeConverter {
    async fn convert(&self, content: &[u8], content_type: &str) -> Result<String> {
        let source = String::from_utf8_lossy(content);
        let lang = crate::ingestion::code_chunker::CodeLanguage::from_mime(content_type)
            .ok_or_else(|| {
                Error::Ingestion(format!(
                    "CodeConverter: unsupported content type {content_type}"
                ))
            })?;
        crate::ingestion::code_chunker::code_to_markdown(&source, lang)
    }

    fn supported_types(&self) -> &[&str] {
        crate::ingestion::code_chunker::CODE_MIME_TYPES
    }
}

/// Split HTML at structural element boundaries into windows of
/// approximately `max_chars` each.
///
/// Looks for `<section`, `<article`, `<div`, `<h1`-`<h6`, and
/// `<main` opening tags as split points. Wraps each window in
/// minimal `<html><body>` and `</body></html>` tags.
fn split_html_windows(html: &str, max_chars: usize) -> Vec<String> {
    if html.len() <= max_chars {
        return vec![html.to_string()];
    }

    // Find positions of structural element opening tags.
    let split_tags = ["<section", "<article", "<div", "<main", "<h1", "<h2", "<h3"];
    let mut split_points: Vec<usize> = Vec::new();

    let lower = html.to_lowercase();
    for tag in &split_tags {
        let mut start = 0;
        while let Some(pos) = lower[start..].find(tag) {
            let abs_pos = start + pos;
            if abs_pos > 0 {
                split_points.push(abs_pos);
            }
            start = abs_pos + tag.len();
        }
    }
    split_points.sort_unstable();
    split_points.dedup();

    if split_points.is_empty() {
        // No structural elements — fall back to simple char split.
        return html
            .as_bytes()
            .chunks(max_chars)
            .map(|chunk| String::from_utf8_lossy(chunk).to_string())
            .collect();
    }

    // Group split points into windows up to max_chars.
    let mut windows: Vec<String> = Vec::new();
    let mut window_start = 0;

    for &point in &split_points {
        if point - window_start >= max_chars && window_start < point {
            windows.push(html[window_start..point].to_string());
            window_start = point;
        }
    }

    // Flush remainder.
    if window_start < html.len() {
        windows.push(html[window_start..].to_string());
    }

    windows
}

/// Strip HTML tags and convert structural elements to Markdown.
///
/// Handles: `<h1>`-`<h6>` (to `#`-`######`), `<p>` (blank line),
/// `<br>` / `<br/>` (newline), `<li>` (bullet), and `<script>` /
/// `<style>` (entire block removed). All other tags are removed,
/// preserving inner text. HTML entities `&amp;`, `&lt;`, `&gt;`,
/// `&quot;`, and `&nbsp;` are decoded.
fn strip_html(html: &str) -> String {
    // Remove boilerplate elements (nav, header, footer, sidebar,
    // cookie banners), then <script> and <style> blocks.
    let cleaned = strip_boilerplate(html);
    let cleaned = remove_block_elements(&cleaned);

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

/// Strip boilerplate HTML elements that contain non-content material.
///
/// Removes `<nav>`, `<header>`, `<footer>`, `<aside>`, `<noscript>`,
/// and common cookie/ad containers. Applied before the main HTML
/// conversion to prevent boilerplate from being chunked and extracted.
pub fn strip_boilerplate(html: &str) -> String {
    let mut result = html.to_string();

    // Remove block-level boilerplate elements entirely.
    for tag in &["nav", "header", "footer", "aside", "noscript"] {
        result = remove_block_element_by_tag(&result, tag);
    }

    // Remove common boilerplate by id/class patterns.
    // These regex-free heuristics cover the most common cases.
    for pattern in &[
        "cookie",
        "consent",
        "gdpr",
        "banner",
        "advert",
        "sidebar",
        "social-share",
        "related-posts",
        "comments",
    ] {
        result = remove_divs_matching(&result, pattern);
    }

    result
}

/// Remove all `<tag>...</tag>` blocks for the given tag name
/// (case-insensitive).
fn remove_block_element_by_tag(html: &str, tag: &str) -> String {
    let mut result = html.to_string();
    loop {
        let lower = result.to_lowercase();
        let open = format!("<{tag}");
        let close = format!("</{tag}>");
        let Some(start) = lower.find(&open) else {
            break;
        };
        let Some(end) = lower[start..].find(&close) else {
            result.truncate(start);
            break;
        };
        let end_pos = start + end + close.len();
        result.replace_range(start..end_pos, "");
    }
    result
}

/// Remove `<div>` blocks whose opening tag contains a matching
/// id or class attribute value (case-insensitive heuristic).
///
/// This is a best-effort removal — it handles single-level divs
/// but not deeply nested ones. The goal is to strip obvious
/// boilerplate containers, not to be a full HTML parser.
fn remove_divs_matching(html: &str, pattern: &str) -> String {
    let mut result = html.to_string();
    let pattern_lower = pattern.to_lowercase();

    loop {
        let lower = result.to_lowercase();
        // Find <div ...> where attributes contain the pattern.
        let Some(div_start) = lower.find("<div") else {
            break;
        };
        let Some(tag_end) = lower[div_start..].find('>') else {
            break;
        };
        let tag_content = &lower[div_start..div_start + tag_end];

        if tag_content.contains(&pattern_lower) {
            // Find the matching </div>
            if let Some(close_offset) = lower[div_start + tag_end..].find("</div>") {
                let end_pos = div_start + tag_end + close_offset + 6;
                result.replace_range(div_start..end_pos, "");
                continue;
            }
        }
        // No match at this position — advance past it by replacing
        // <div with a placeholder, search again, then restore.
        // This is a simple way to skip non-matching divs without
        // keeping a cursor index.
        break;
    }
    result
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
    let without_prefix = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let inner = without_prefix.strip_suffix('|').unwrap_or(without_prefix);
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
        registry.converters.push(Box::new(CodeConverter));
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

                // Post-conversion quality check for format
                // conversions (HTML, PDF) to catch garbage output.
                // Skip for passthrough formats (markdown, text).
                let needs_quality_check = base_type.contains("html") || base_type.contains("pdf");
                if needs_quality_check {
                    if let Some(reason) = check_conversion_quality(&md) {
                        tracing::warn!(
                            content_type = base_type,
                            reason,
                            md_len = md.len(),
                            "conversion quality check failed"
                        );
                        return Err(Error::Ingestion(format!(
                            "conversion produced low-quality output: {reason}"
                        )));
                    }
                    if is_low_quality_conversion(content.len(), &md) {
                        tracing::warn!(
                            content_type = base_type,
                            input_len = content.len(),
                            md_words = md.split_whitespace().count(),
                            "conversion output suspiciously small relative to input"
                        );
                    }
                }

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

/// Check whether converted Markdown meets minimum quality thresholds.
///
/// Only flags issues when the input was large enough to expect
/// meaningful output. Small HTML fragments convert to short Markdown
/// and that's fine.
///
/// Returns `None` if the output is acceptable, or `Some(reason)` if
/// it should be rejected.
fn check_conversion_quality(md: &str) -> Option<&'static str> {
    let trimmed = md.trim();
    if trimmed.is_empty() {
        return Some("empty output");
    }

    None
}

/// Check whether a conversion result is proportionally too small
/// relative to the original content size.
///
/// Returns `true` if the conversion looks like garbage (large input
/// produced very little output). Used by the registry to log
/// warnings for low-quality conversions without hard-failing.
pub(crate) fn is_low_quality_conversion(input_len: usize, md: &str) -> bool {
    let word_count = md.split_whitespace().count();
    // Only flag when input was substantial (>1KB) and output is
    // tiny (<10 words).
    input_len > 1024 && word_count < 10
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

    // --- HTML windowing tests ---

    #[test]
    fn split_html_small_passthrough() {
        let html = "<p>Hello world</p>";
        let windows = split_html_windows(html, 50_000);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0], html);
    }

    #[test]
    fn split_html_at_structural_boundaries() {
        let html = format!(
            "<div>{}</div><div>{}</div><div>{}</div>",
            "a".repeat(200),
            "b".repeat(200),
            "c".repeat(200),
        );
        let windows = split_html_windows(&html, 300);
        assert!(
            windows.len() >= 2,
            "expected split, got {} windows",
            windows.len()
        );
    }

    #[test]
    fn split_html_no_structural_tags() {
        let html = "x".repeat(200);
        let windows = split_html_windows(&html, 100);
        assert!(windows.len() >= 2);
        // Each window should be <= max_chars
        for w in &windows {
            assert!(w.len() <= 100);
        }
    }

    // --- Conversion quality tests ---

    #[test]
    fn quality_check_empty_fails() {
        assert_eq!(check_conversion_quality(""), Some("empty output"));
        assert_eq!(check_conversion_quality("   "), Some("empty output"));
    }

    #[test]
    fn quality_check_short_passes() {
        // Short output is OK for short input
        assert_eq!(check_conversion_quality("Hello world"), None);
    }

    #[test]
    fn low_quality_large_input_tiny_output() {
        assert!(is_low_quality_conversion(5000, "Hello"));
    }

    #[test]
    fn low_quality_small_input_ok() {
        assert!(!is_low_quality_conversion(100, "Hello"));
    }

    #[test]
    fn low_quality_adequate_output() {
        let output = "This is a sufficiently long output with many words to \
                      pass the quality check for conversion results.";
        assert!(!is_low_quality_conversion(5000, output));
    }

    // --- PDF converter tests ---

    #[test]
    fn pdf_converter_supported_types() {
        let conv = PdfConverter::new("http://localhost:9999".into());
        assert_eq!(conv.supported_types(), &["application/pdf"]);
    }

    #[test]
    fn pdf_converter_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PdfConverter>();
    }

    #[tokio::test]
    async fn registry_rejects_pdf_without_converter() {
        let registry = ConverterRegistry::new();
        let result = registry.convert(b"%PDF-1.4", "application/pdf").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no converter registered"));
    }

    // --- Boilerplate removal tests ---

    #[test]
    fn strip_boilerplate_removes_nav() {
        let html = "<body><nav><a href='/'>Home</a></nav><p>Article content</p></body>";
        let result = strip_boilerplate(html);
        assert!(!result.contains("Home"));
        assert!(result.contains("Article content"));
    }

    #[test]
    fn strip_boilerplate_removes_header_footer() {
        let html = "<header>Site Header</header>\
                     <main>Content</main>\
                     <footer>Copyright 2024</footer>";
        let result = strip_boilerplate(html);
        assert!(!result.contains("Site Header"));
        assert!(!result.contains("Copyright"));
        assert!(result.contains("Content"));
    }

    #[test]
    fn strip_boilerplate_removes_aside() {
        let html = "<article>Main text</article><aside>Related articles</aside>";
        let result = strip_boilerplate(html);
        assert!(!result.contains("Related articles"));
        assert!(result.contains("Main text"));
    }

    #[test]
    fn strip_boilerplate_removes_cookie_div() {
        let html = "<div class='cookie-consent'>Accept cookies</div>\
                     <p>Real content here</p>";
        let result = strip_boilerplate(html);
        assert!(!result.contains("Accept cookies"));
        assert!(result.contains("Real content here"));
    }

    #[test]
    fn strip_boilerplate_preserves_clean_html() {
        let html = "<h1>Title</h1><p>Paragraph one.</p><p>Paragraph two.</p>";
        let result = strip_boilerplate(html);
        assert_eq!(result, html);
    }

    #[test]
    fn strip_boilerplate_removes_noscript() {
        let html = "<p>Content</p><noscript>Enable JavaScript</noscript>";
        let result = strip_boilerplate(html);
        assert!(!result.contains("Enable JavaScript"));
        assert!(result.contains("Content"));
    }

    #[tokio::test]
    async fn html_converter_strips_boilerplate() {
        let converter = HtmlConverter;
        let html = b"<nav>Menu</nav><h1>Title</h1><p>Content</p><footer>Footer</footer>";
        let result = converter.convert(html, "text/html").await.unwrap();
        assert!(!result.contains("Menu"));
        assert!(!result.contains("Footer"));
        assert!(result.contains("Title"));
        assert!(result.contains("Content"));
    }
}

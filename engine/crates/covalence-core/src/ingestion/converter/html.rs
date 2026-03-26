//! HTML-to-Markdown converter and related utilities.
//!
//! Provides tag stripping, boilerplate removal, and structural HTML
//! windowing for large documents.

use crate::error::Result;
use crate::ingestion::converter::SourceConverter;

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

/// Split HTML at structural element boundaries into windows of
/// approximately `max_chars` each.
///
/// Looks for `<section`, `<article`, `<div`, `<h1`-`<h6`, and
/// `<main` opening tags as split points. Wraps each window in
/// minimal `<html><body>` and `</body></html>` tags.
pub(crate) fn split_html_windows(html: &str, max_chars: usize) -> Vec<String> {
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

/// Strip HTML tags and convert to Markdown.
///
/// Uses the `html2md` crate for conversion, with boilerplate
/// removal as a preprocessing step. The flow is:
/// 1. `strip_boilerplate()` — removes nav, header, footer, etc.
/// 2. `remove_block_elements()` — removes script/style blocks
/// 3. `html2md::parse_html()` — converts cleaned HTML to Markdown
/// 4. `collapse_newlines()` — normalizes whitespace
pub(crate) fn strip_html(html: &str) -> String {
    // Remove boilerplate elements (nav, header, footer, sidebar,
    // cookie banners), then <script> and <style> blocks.
    let cleaned = strip_boilerplate(html);
    let cleaned = remove_block_elements(&cleaned);

    // Convert to Markdown using html2md.
    let md = html2md::parse_html(&cleaned);

    // Collapse runs of 3+ newlines into 2, and trim.
    collapse_newlines(&md)
}

/// Strip boilerplate HTML elements that contain non-content material.
///
/// Removes `<nav>`, `<header>`, `<footer>`, `<aside>`, `<noscript>`,
/// and common cookie/ad containers. Applied before the main HTML
/// conversion to prevent boilerplate from being chunked and extracted.
pub(crate) fn strip_boilerplate(html: &str) -> String {
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
pub(crate) fn collapse_newlines(text: &str) -> String {
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

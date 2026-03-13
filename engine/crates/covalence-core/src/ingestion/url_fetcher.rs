//! URL fetching and metadata extraction for URL-based ingestion.
//!
//! Fetches remote content via HTTP, detects MIME type and source
//! classification from headers and URL patterns, and extracts basic
//! metadata (title, author, date) from HTML content.

use crate::error::{Error, Result};
use crate::ingestion::utils::decode_html_entities;

/// Result of fetching a URL.
pub struct FetchResult {
    /// Raw response bytes.
    pub bytes: Vec<u8>,
    /// Detected MIME type from Content-Type header.
    pub mime: String,
    /// Auto-detected source type based on URL patterns.
    pub source_type: String,
    /// Extracted metadata from content and headers.
    pub metadata: UrlMetadata,
}

/// Metadata extracted from fetched content.
#[derive(Debug, Default)]
pub struct UrlMetadata {
    /// Title extracted from HTML or content.
    pub title: Option<String>,
    /// Author extracted from meta tags.
    pub author: Option<String>,
    /// Publication or last-modified date.
    pub date: Option<String>,
}

/// Fetch a URL with reasonable defaults.
///
/// Uses a 30-second timeout and identifies as Covalence's ingestion
/// agent. Extracts MIME type from the Content-Type header, detects
/// source type from URL patterns, and parses basic metadata from
/// HTML content.
pub async fn fetch_url(url: &str) -> Result<FetchResult> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Covalence/1.0 (knowledge-engine)")
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| Error::Ingestion(format!("failed to build HTTP client: {e}")))?;

    // Request markdown from CloudFlare-fronted sites. This returns
    // clean markdown directly, bypassing the HTML conversion pipeline
    // and eliminating boilerplate noise.
    let mut request = client.get(url);
    if wants_markdown(url) {
        request = request.header(reqwest::header::ACCEPT, "text/markdown");
    }

    let response: reqwest::Response = request.send().await.map_err(|e| {
        if e.is_timeout() {
            Error::Ingestion(format!("URL fetch timed out: {url}"))
        } else if e.is_connect() {
            Error::Ingestion(format!("failed to connect to URL: {url}"))
        } else {
            Error::Ingestion(format!("failed to fetch URL {url}: {e}"))
        }
    })?;

    let status = response.status();
    if !status.is_success() {
        return Err(Error::Ingestion(format!(
            "URL returned HTTP {status}: {url}"
        )));
    }

    // Extract MIME from Content-Type header.
    let content_type: String = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v: &reqwest::header::HeaderValue| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    // Extract Last-Modified for date metadata.
    let last_modified: Option<String> = response
        .headers()
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|v: &reqwest::header::HeaderValue| v.to_str().ok())
        .map(|s| s.to_string());

    let mime = parse_mime(&content_type);
    let source_type = detect_source_type(url);
    let bytes: Vec<u8> =
        response.bytes().await.map(|b| b.to_vec()).map_err(|e| {
            Error::Ingestion(format!("failed to read response body from {url}: {e}"))
        })?;

    // Extract metadata from HTML content.
    let mut metadata = if mime == "text/html" {
        if let Ok(text) = std::str::from_utf8(&bytes) {
            extract_html_metadata(text)
        } else {
            UrlMetadata::default()
        }
    } else if mime == "text/markdown" || mime == "text/plain" {
        if let Ok(text) = std::str::from_utf8(&bytes) {
            extract_text_metadata(text)
        } else {
            UrlMetadata::default()
        }
    } else {
        UrlMetadata::default()
    };

    // Use Last-Modified header as date fallback.
    if metadata.date.is_none() {
        metadata.date = last_modified;
    }

    Ok(FetchResult {
        bytes,
        mime,
        source_type,
        metadata,
    })
}

/// Check whether a URL is served through CloudFlare and supports the
/// `Accept: text/markdown` header for clean markdown responses.
///
/// Known CloudFlare-fronted domains that return markdown:
/// - arxiv.org (abstract pages, paper pages)
fn wants_markdown(url: &str) -> bool {
    let lower = url.to_lowercase();
    // ArXiv abstract/paper pages are CloudFlare-fronted.
    // PDF URLs should NOT get the markdown header (they serve
    // binary content directly).
    if lower.contains("arxiv.org") && !lower.contains("/pdf/") {
        return true;
    }
    false
}

/// Parse the MIME type from a Content-Type header value.
///
/// Strips parameters like charset: `"text/html; charset=utf-8"` →
/// `"text/html"`.
fn parse_mime(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or("application/octet-stream")
        .trim()
        .to_lowercase()
}

/// Detect source type from URL patterns.
///
/// Falls back to `"web_page"` for unrecognized URLs.
fn detect_source_type(url: &str) -> String {
    let lower = url.to_lowercase();

    if lower.contains("arxiv.org")
        || lower.contains("doi.org")
        || lower.ends_with(".pdf")
        || lower.contains("scholar.google")
        || lower.contains("semanticscholar")
        || lower.contains("pubmed")
        || lower.contains("ieee.org")
        || lower.contains("acm.org")
    {
        return "document".to_string();
    }

    if lower.contains("github.com")
        || lower.contains("gitlab.com")
        || lower.contains("bitbucket.org")
        || lower.contains("raw.githubusercontent.com")
    {
        return "code".to_string();
    }

    if lower.contains("news.ycombinator.com")
        || lower.contains("reddit.com")
        || lower.contains("twitter.com")
        || lower.contains("x.com")
        || lower.contains("mastodon")
    {
        return "conversation".to_string();
    }

    if lower.contains("/api/")
        || lower.contains("/v1/")
        || lower.contains("/v2/")
        || lower.contains("swagger")
        || lower.contains("openapi")
    {
        return "api".to_string();
    }

    "web_page".to_string()
}

/// Extract metadata from HTML content using simple string parsing.
///
/// Looks for `<title>`, `<meta name="author">`, `<meta
/// name="description">`, Open Graph tags, and common date meta tags.
fn extract_html_metadata(html: &str) -> UrlMetadata {
    let title = extract_html_title(html);
    let author = extract_meta_content(html, "author")
        .or_else(|| extract_meta_property(html, "article:author"));
    let date = extract_meta_content(html, "date")
        .or_else(|| extract_meta_property(html, "article:published_time"))
        .or_else(|| extract_meta_content(html, "publication_date"))
        .or_else(|| extract_meta_content(html, "DC.date"));

    UrlMetadata {
        title,
        author,
        date,
    }
}

/// Extract metadata from plain text or markdown.
///
/// Uses the first `# heading` as title.
fn extract_text_metadata(text: &str) -> UrlMetadata {
    let title = text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("# ")
            .map(|rest| rest.trim().to_string())
    });

    UrlMetadata {
        title,
        ..Default::default()
    }
}

/// Extract `<title>...</title>` from HTML.
fn extract_html_title(html: &str) -> Option<String> {
    // Use ASCII-only lowercasing to preserve byte-offset alignment
    // between `lower` and `html`. Full `to_lowercase()` can change
    // byte lengths for non-ASCII chars (e.g., 'İ' → "i̇"), making
    // byte positions from `lower` invalid for slicing `html`.
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let after_tag = lower[start..].find('>')?;
    let content_start = start + after_tag + 1;
    let end = lower[content_start..].find("</title>")?;
    let raw = &html[content_start..content_start + end];
    let decoded = decode_html_entities(raw.trim());
    if decoded.is_empty() {
        None
    } else {
        Some(decoded)
    }
}

/// Extract content from `<meta name="X" content="...">`.
fn extract_meta_content(html: &str, name: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let pattern = format!("name=\"{name}\"");
    let pos = lower.find(&pattern)?;

    // Search for `content="..."` near this meta tag.
    let region_start = lower[..pos].rfind('<').unwrap_or(0);
    let region_end = lower[pos..].find('>').map(|i| pos + i + 1)?;
    let region = &lower[region_start..region_end];

    if let Some(content_pos) = region.find("content=\"") {
        let value_start = content_pos + 9;
        let original_region = &html[region_start..region_end];
        if let Some(value_end) = original_region[value_start..].find('"') {
            let value = original_region[value_start..value_start + value_end]
                .trim()
                .to_string();
            if value.is_empty() {
                return None;
            }
            return Some(decode_html_entities(&value));
        }
    }

    None
}

/// Extract content from `<meta property="X" content="...">`.
fn extract_meta_property(html: &str, property: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let pattern = format!("property=\"{property}\"");
    let pos = lower.find(&pattern)?;

    let region_start = lower[..pos].rfind('<').unwrap_or(0);
    let region_end = lower[pos..].find('>').map(|i| pos + i + 1)?;
    let region = &lower[region_start..region_end];

    if let Some(content_pos) = region.find("content=\"") {
        let value_start = content_pos + 9;
        let original_region = &html[region_start..region_end];
        if let Some(value_end) = original_region[value_start..].find('"') {
            let value = original_region[value_start..value_start + value_end]
                .trim()
                .to_string();
            if value.is_empty() {
                return None;
            }
            return Some(decode_html_entities(&value));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mime_strips_charset() {
        assert_eq!(parse_mime("text/html; charset=utf-8"), "text/html");
        assert_eq!(parse_mime("application/pdf"), "application/pdf");
        assert_eq!(parse_mime("TEXT/HTML"), "text/html");
    }

    #[test]
    fn detect_source_type_arxiv() {
        assert_eq!(
            detect_source_type("https://arxiv.org/abs/2404.16130"),
            "document"
        );
        assert_eq!(
            detect_source_type("https://arxiv.org/pdf/2404.16130"),
            "document"
        );
    }

    #[test]
    fn detect_source_type_github() {
        assert_eq!(detect_source_type("https://github.com/user/repo"), "code");
        assert_eq!(
            detect_source_type("https://raw.githubusercontent.com/a/b/main/f.rs"),
            "code"
        );
    }

    #[test]
    fn detect_source_type_conversation() {
        assert_eq!(
            detect_source_type("https://news.ycombinator.com/item?id=123"),
            "conversation"
        );
        assert_eq!(
            detect_source_type("https://reddit.com/r/rust/comments/abc"),
            "conversation"
        );
    }

    #[test]
    fn detect_source_type_api() {
        assert_eq!(
            detect_source_type("https://example.com/api/v1/users"),
            "api"
        );
    }

    #[test]
    fn detect_source_type_generic_web() {
        assert_eq!(
            detect_source_type("https://example.com/blog/post"),
            "web_page"
        );
    }

    #[test]
    fn detect_source_type_pdf_extension() {
        assert_eq!(
            detect_source_type("https://example.com/paper.pdf"),
            "document"
        );
    }

    #[test]
    fn extract_html_title_basic() {
        let html = "<html><head><title>My Page</title></head></html>";
        assert_eq!(extract_html_title(html), Some("My Page".to_string()));
    }

    #[test]
    fn extract_html_title_with_entities() {
        let html = "<title>Rust &amp; GraphRAG</title>";
        assert_eq!(
            extract_html_title(html),
            Some("Rust & GraphRAG".to_string())
        );
    }

    #[test]
    fn extract_html_title_empty() {
        let html = "<title></title>";
        assert_eq!(extract_html_title(html), None);
    }

    #[test]
    fn extract_meta_content_author() {
        let html = r#"<meta name="author" content="Jane Doe">"#;
        assert_eq!(
            extract_meta_content(html, "author"),
            Some("Jane Doe".to_string())
        );
    }

    #[test]
    fn extract_meta_content_missing() {
        let html = r#"<meta name="description" content="A page">"#;
        assert_eq!(extract_meta_content(html, "author"), None);
    }

    #[test]
    fn extract_meta_property_og() {
        let html = r#"<meta property="article:author" content="John Smith">"#;
        assert_eq!(
            extract_meta_property(html, "article:author"),
            Some("John Smith".to_string())
        );
    }

    #[test]
    fn extract_text_metadata_heading() {
        let text = "# My Document\n\nSome content here.";
        let meta = extract_text_metadata(text);
        assert_eq!(meta.title, Some("My Document".to_string()));
    }

    #[test]
    fn extract_text_metadata_no_heading() {
        let text = "Just some plain text without headings.";
        let meta = extract_text_metadata(text);
        assert_eq!(meta.title, None);
    }

    #[test]
    fn full_html_metadata_extraction() {
        let html = r#"
            <html>
            <head>
                <title>Research Paper</title>
                <meta name="author" content="Alice">
                <meta name="date" content="2024-01-15">
            </head>
            <body>Content</body>
            </html>
        "#;
        let meta = extract_html_metadata(html);
        assert_eq!(meta.title, Some("Research Paper".to_string()));
        assert_eq!(meta.author, Some("Alice".to_string()));
        assert_eq!(meta.date, Some("2024-01-15".to_string()));
    }

    #[test]
    fn decode_entities() {
        assert_eq!(decode_html_entities("a &amp; b &lt; c"), "a & b < c");
    }

    #[test]
    fn extract_title_with_non_ascii_before_tag() {
        // Non-ASCII chars before <title> could misalign byte offsets
        // if we used full to_lowercase() (e.g., 'İ' changes byte
        // length when lowercased). to_ascii_lowercase() avoids this.
        let html = "<html><!-- Ünïcödé --><title>My Title</title></html>";
        assert_eq!(extract_html_title(html), Some("My Title".to_string()));
    }

    #[test]
    fn extract_title_with_non_ascii_in_title() {
        let html = "<title>Über den Dächern</title>";
        assert_eq!(
            extract_html_title(html),
            Some("Über den Dächern".to_string())
        );
    }

    #[test]
    fn extract_title_with_cjk_content() {
        let html = "<html><head><title>知识图谱</title></head></html>";
        assert_eq!(extract_html_title(html), Some("知识图谱".to_string()));
    }

    #[test]
    fn extract_meta_with_non_ascii_prefix() {
        // Non-ASCII before the meta tag must not misalign offsets.
        let html = r#"<html><!-- Ünïcödé --><meta name="author" content="José García"></html>"#;
        assert_eq!(
            extract_meta_content(html, "author"),
            Some("José García".to_string())
        );
    }

    #[test]
    fn extract_meta_property_with_non_ascii() {
        let html =
            r#"<html><!-- Ünïcödé --><meta property="article:author" content="Müller"></html>"#;
        assert_eq!(
            extract_meta_property(html, "article:author"),
            Some("Müller".to_string())
        );
    }

    #[test]
    fn wants_markdown_arxiv_abstract() {
        assert!(wants_markdown("https://arxiv.org/abs/2404.16130"));
        assert!(wants_markdown("https://arxiv.org/abs/2404.16130v2"));
    }

    #[test]
    fn wants_markdown_arxiv_html() {
        assert!(wants_markdown("https://arxiv.org/html/2404.16130"));
    }

    #[test]
    fn wants_markdown_arxiv_pdf_excluded() {
        // PDF URLs should NOT get the markdown header.
        assert!(!wants_markdown("https://arxiv.org/pdf/2404.16130"));
    }

    #[test]
    fn wants_markdown_non_cloudflare() {
        assert!(!wants_markdown("https://example.com/page"));
        assert!(!wants_markdown("https://github.com/user/repo"));
    }
}

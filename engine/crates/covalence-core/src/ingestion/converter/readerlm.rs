//! ReaderLM-v2 MLX sidecar HTML converter.

use crate::error::Result;
use crate::ingestion::converter::SourceConverter;
use crate::ingestion::converter::html::{split_html_windows, strip_boilerplate, strip_html};

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
/// Falls back to the built-in [`HtmlConverter`](super::HtmlConverter)
/// tag stripper if the sidecar is unreachable or returns an error.
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

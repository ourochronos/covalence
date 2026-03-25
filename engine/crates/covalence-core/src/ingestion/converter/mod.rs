//! Pluggable format converter system for the ingestion pipeline.
//!
//! Converts raw source content (HTML, plain text, etc.) into Markdown
//! before the parser stage processes it. Each converter handles one or
//! more MIME content types. The [`ConverterRegistry`] dispatches to the
//! appropriate converter based on the content type of incoming data.

mod code;
pub mod html;
mod markdown;
mod pdf;
mod plaintext;
mod readerlm;
pub mod table;

#[cfg(test)]
mod tests;

use crate::error::{Error, Result};

pub use code::CodeConverter;
pub use html::HtmlConverter;
pub use markdown::MarkdownConverter;
pub use pdf::PdfConverter;
pub use plaintext::PlainTextConverter;
pub use readerlm::ReaderLmConverter;
pub use table::linearize_tables;

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

//! Passthrough converter for Markdown content.

use crate::error::Result;
use crate::ingestion::converter::SourceConverter;

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

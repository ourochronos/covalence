//! Converter for plain text content.

use crate::error::Result;
use crate::ingestion::converter::SourceConverter;

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

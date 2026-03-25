//! Converter for source code files using tree-sitter parsing.

use crate::error::{Error, Result};
use crate::ingestion::converter::SourceConverter;

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

//! Deterministic AST-based entity extraction for source code.
//!
//! Walks a tree-sitter AST to extract structured entities and
//! relationships from Rust and Python source code. Unlike the LLM
//! extractor, all extractions are deterministic with confidence 1.0.
//!
//! Design principle: struct/class fields become metadata properties
//! on their parent entity, NOT separate graph nodes.

mod common;
mod go;
mod python;
mod rust;

#[cfg(test)]
mod tests;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::ingestion::code_chunker::CodeLanguage;
use crate::ingestion::extractor::{ExtractionContext, ExtractionResult, Extractor};

/// Compute a SHA-256 hash of a tree-sitter node's source text.
///
/// Used to fingerprint individual AST items (functions, structs, etc.)
/// so that incremental re-ingestion can skip unchanged code entities.
/// The hash is stored in `ExtractedEntity.metadata.ast_hash` and
/// persisted on graph nodes in `properties.ast_hash`.
fn compute_ast_hash(source: &str, node: &tree_sitter::Node) -> String {
    let text = &source[node.start_byte()..node.end_byte()];
    let hash = Sha256::digest(text.as_bytes());
    format!("{hash:x}")
}

/// Build the metadata JSON carrying an AST hash for a code entity.
fn ast_metadata(source: &str, node: &tree_sitter::Node) -> Option<serde_json::Value> {
    Some(serde_json::json!({ "ast_hash": compute_ast_hash(source, node) }))
}

/// Deterministic extractor that walks tree-sitter ASTs to extract
/// code entities and relationships.
///
/// Produces entities for structs, enums, traits, functions, impl
/// blocks, modules, constants, macros (Rust) and classes, functions
/// (Python). Relationships include `implements`, `extends`,
/// `imports`, `calls`, and `contains`.
///
/// All extractions have confidence 1.0 since they are derived from
/// deterministic AST parsing rather than probabilistic LLM output.
pub struct AstExtractor;

impl AstExtractor {
    /// Create a new AST extractor.
    pub fn new() -> Self {
        Self
    }

    /// Extract entities and relationships from source code.
    ///
    /// Detects the language from the extraction context (source URI
    /// or source type) and delegates to the appropriate
    /// language-specific extractor.
    fn extract_code(&self, text: &str, context: &ExtractionContext) -> Result<ExtractionResult> {
        let lang = self.detect_language(context);
        let lang = match lang {
            Some(l) => l,
            None => return Ok(ExtractionResult::default()),
        };

        let mut parser = tree_sitter::Parser::new();
        let ts_language = match lang {
            CodeLanguage::Rust => tree_sitter_rust::LANGUAGE,
            CodeLanguage::Python => tree_sitter_python::LANGUAGE,
            CodeLanguage::Go => tree_sitter_go::LANGUAGE,
        };
        parser
            .set_language(&ts_language.into())
            .map_err(|e| Error::Ingestion(format!("tree-sitter language error: {e}")))?;

        // The input may be Markdown-wrapped code from the code
        // chunker. Extract raw code from fenced blocks before
        // parsing.
        let raw_code = unwrap_markdown_code(text);

        let tree = parser
            .parse(raw_code.as_bytes(), None)
            .ok_or_else(|| Error::Ingestion("tree-sitter parse failed".into()))?;

        match lang {
            CodeLanguage::Rust => rust::extract_rust(&raw_code, &tree),
            CodeLanguage::Python => python::extract_python(&raw_code, &tree),
            CodeLanguage::Go => go::extract_go(&raw_code, &tree),
        }
    }

    /// Detect the code language from extraction context.
    fn detect_language(&self, context: &ExtractionContext) -> Option<CodeLanguage> {
        // Try URI-based detection first.
        if let Some(ref uri) = context.source_uri {
            if let Some(lang) = CodeLanguage::from_uri(uri) {
                return Some(lang);
            }
        }
        // Try source_type as a MIME type.
        if let Some(ref st) = context.source_type {
            if let Some(lang) = CodeLanguage::from_mime(st) {
                return Some(lang);
            }
        }
        None
    }
}

impl Default for AstExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Extractor for AstExtractor {
    async fn extract(&self, text: &str, context: &ExtractionContext) -> Result<ExtractionResult> {
        self.extract_code(text, context)
    }
}

/// Extract raw code from Markdown-fenced code blocks.
///
/// The code chunker wraps source in Markdown sections with fenced
/// blocks. This function strips those wrappers to recover the
/// original source for AST parsing.
fn unwrap_markdown_code(text: &str) -> String {
    let mut code_parts: Vec<&str> = Vec::new();
    let mut in_fence = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            code_parts.push(line);
        }
    }

    if code_parts.is_empty() {
        // No fenced blocks found — treat the whole input as code.
        text.to_string()
    } else {
        code_parts.join("\n")
    }
}

//! Content preparation and conflict detection for the ingestion
//! pipeline.
//!
//! Contains the `prepare_content()` stage (convert -> parse ->
//! normalize), the `check_and_invalidate_conflicts()` helper for
//! temporal edge invalidation, and the `sniff_html()` content-type
//! detector.

use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::ingestion::code_chunker;
use crate::models::edge::Edge;
use crate::storage::traits::EdgeRepo;

use super::super::source::SourceService;
use super::types::PreparedContent;

impl SourceService {
    /// Shared convert -> parse -> normalize stages.
    ///
    /// Returns the normalized text, its hash, and whether this is
    /// a code source.
    pub(crate) async fn prepare_content(
        &self,
        content: &[u8],
        mime: &str,
        uri: Option<&str>,
    ) -> Result<PreparedContent> {
        // Content-sniff: override MIME if raw content is clearly HTML
        // but was declared as something else (e.g., text/plain).
        let mime = if !mime.contains("html") && sniff_html(content) {
            tracing::debug!(
                declared_mime = mime,
                "content-sniffed as HTML, overriding MIME"
            );
            "text/html"
        } else {
            mime
        };

        let code_lang = code_chunker::detect_code_language(mime, uri);
        let is_code = code_lang.is_some();

        // Stage 1.5: Convert
        let (parse_content, parse_mime): (std::borrow::Cow<'_, [u8]>, &str) =
            if let Some(lang) = code_lang {
                let source_text = String::from_utf8_lossy(content);
                let md = code_chunker::code_to_markdown(&source_text, lang)?;
                (std::borrow::Cow::Owned(md.into_bytes()), "text/markdown")
            } else if self.pipeline.convert_enabled {
                if let Some(ref registry) = self.converter_registry {
                    let converted = registry.convert(content, mime).await?;
                    (
                        std::borrow::Cow::Owned(converted.into_bytes()),
                        "text/markdown",
                    )
                } else {
                    (std::borrow::Cow::Borrowed(content), mime)
                }
            } else {
                (std::borrow::Cow::Borrowed(content), mime)
            };

        // Stage 2: Parse
        let parsed = crate::ingestion::parser::parse(&parse_content, parse_mime)?;

        // Stage 3: Normalize via composable pass chain.
        //
        // The profile registry selects the right normalization chain
        // based on source type + URI (e.g., arXiv gets MathJax
        // stripping, code gets minimal normalization).
        let normalized = if !self.pipeline.normalize_enabled {
            parsed.body.clone()
        } else {
            let source_type = if is_code {
                crate::models::source::SourceType::Code
            } else {
                crate::models::source::SourceType::Document
            };
            let registry = crate::ingestion::source_profile::ProfileRegistry::new();
            let profile = registry.match_profile(&source_type, uri);
            tracing::debug!(
                profile = profile.name,
                uri = uri.unwrap_or("-"),
                "selected normalization profile"
            );
            profile.normalize_chain().run(&parsed.body)
        };

        let normalized_hash = Sha256::digest(normalized.as_bytes()).to_vec();

        Ok(PreparedContent {
            normalized,
            normalized_hash,
            is_code,
            parsed_title: parsed.title,
            parsed_metadata: parsed.metadata,
        })
    }

    /// Check for conflicting edges and invalidate any that contradict
    /// the new edge after it has been created.
    ///
    /// Queries existing active edges sharing the same `(source_node,
    /// rel_type)` and runs temporal conflict detection. Edges with
    /// different targets and overlapping temporal ranges are
    /// invalidated — superseded by the new edge.
    ///
    /// Must be called **after** `EdgeRepo::create()` so the new edge
    /// exists for the `invalidated_by` foreign key.
    pub(crate) async fn check_and_invalidate_conflicts(&self, edge: &Edge) -> Result<usize> {
        use crate::epistemic::invalidation::{ExistingEdgeRecord, detect_conflicts};

        let existing =
            EdgeRepo::find_by_source_and_rel_type(&*self.repo, edge.source_node_id, &edge.rel_type)
                .await?;

        if existing.is_empty() {
            return Ok(0);
        }

        // Exclude the new edge itself from the candidates.
        let records: Vec<ExistingEdgeRecord> = existing
            .iter()
            .filter(|e| e.id != edge.id)
            .map(|e| {
                (
                    e.id,
                    e.target_node_id,
                    e.valid_from,
                    e.valid_until,
                    e.invalid_at,
                )
            })
            .collect();

        let check = detect_conflicts(
            edge.id,
            edge.target_node_id,
            &edge.rel_type,
            edge.valid_from,
            &records,
        );

        let mut invalidated = 0;
        for conflict in &check.conflicts {
            tracing::info!(
                existing_edge = %conflict.existing_edge_id,
                new_edge = %conflict.new_edge_id,
                conflict_type = ?conflict.conflict_type,
                rel_type = %edge.rel_type,
                "invalidating conflicting edge"
            );
            EdgeRepo::invalidate(&*self.repo, conflict.existing_edge_id, edge.id).await?;
            invalidated += 1;
        }

        Ok(invalidated)
    }
}

/// Check if raw content looks like HTML by inspecting leading bytes.
///
/// Skips whitespace/BOM then checks for `<!DOCTYPE` or `<html`.
pub(super) fn sniff_html(content: &[u8]) -> bool {
    // Skip BOM + whitespace.
    let trimmed = content
        .iter()
        .position(|&b| !b.is_ascii_whitespace() && b != 0xEF && b != 0xBB && b != 0xBF)
        .map(|pos| &content[pos..])
        .unwrap_or(content);

    let prefix: Vec<u8> = trimmed
        .iter()
        .take(15)
        .map(|b| b.to_ascii_lowercase())
        .collect();
    prefix.starts_with(b"<!doctype") || prefix.starts_with(b"<html")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_html_detects_doctype() {
        assert!(sniff_html(b"<!DOCTYPE html>\n<html>"));
        assert!(sniff_html(b"<!doctype html>"));
    }

    #[test]
    fn sniff_html_detects_html_tag() {
        assert!(sniff_html(b"<html lang=\"en\">"));
        assert!(sniff_html(b"  <html>"));
    }

    #[test]
    fn sniff_html_rejects_markdown() {
        assert!(!sniff_html(b"# Heading\n\nSome text"));
        assert!(!sniff_html(b"Hello world"));
    }

    #[test]
    fn sniff_html_skips_bom() {
        assert!(sniff_html(b"\xEF\xBB\xBF<!DOCTYPE html>"));
    }

    #[test]
    fn sniff_html_empty_content() {
        assert!(!sniff_html(b""));
        assert!(!sniff_html(b"   "));
    }
}

//! Type definitions for the shared ingestion pipeline.
//!
//! These types are used by both `ingest()` and `reprocess()` to
//! communicate between pipeline stages.

use crate::types::ids::{ChunkId, SourceId, StatementId};

/// Result of the content preparation stage (convert → parse →
/// normalize).
pub(crate) struct PreparedContent {
    /// Normalized text ready for chunking.
    pub normalized: String,
    /// SHA-256 hash of the normalized content.
    pub normalized_hash: Vec<u8>,
    /// Whether this is a code source.
    pub is_code: bool,
    /// Title extracted during parsing (if any).
    pub parsed_title: Option<String>,
    /// Metadata extracted during parsing.
    pub parsed_metadata: std::collections::HashMap<String, String>,
}

/// Input to the shared ingestion pipeline.
pub(crate) struct PipelineInput<'a> {
    /// The source ID (already created or loaded).
    pub source_id: SourceId,
    /// Source type string (e.g., "document", "code").
    pub source_type: &'a str,
    /// Optional URI for context in extraction.
    pub source_uri: Option<String>,
    /// Optional title for extraction context.
    pub source_title: Option<String>,
    /// Knowledge domain (code, spec, design, research, external).
    pub source_domain: Option<String>,
    /// Normalized text to chunk and process.
    pub normalized: &'a str,
    /// Whether this is a code source (skips coref).
    pub is_code: bool,
}

/// Output from the shared ingestion pipeline.
pub(crate) struct PipelineOutput {
    /// Number of chunks created.
    pub chunks_created: usize,
}

/// Provenance source for entity extraction records.
pub(crate) enum ExtractionProvenance {
    /// Entity was extracted from a chunk.
    Chunk(ChunkId),
    /// Entity was extracted from a statement.
    Statement(StatementId),
}

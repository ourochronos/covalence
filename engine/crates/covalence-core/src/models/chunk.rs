//! Chunk model -- text segment at a specific granularity level.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::clearance::ClearanceLevel;
use crate::types::ids::{ChunkId, SourceId};

/// Granularity level of a text chunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkLevel {
    /// Entire document.
    Document,
    /// Section or heading-delimited block.
    Section,
    /// Paragraph.
    Paragraph,
    /// Single sentence.
    Sentence,
}

impl ChunkLevel {
    /// String representation for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Section => "section",
            Self::Paragraph => "paragraph",
            Self::Sentence => "sentence",
        }
    }

    /// Parse from database string.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "document" => Some(Self::Document),
            "section" => Some(Self::Section),
            "paragraph" => Some(Self::Paragraph),
            "sentence" => Some(Self::Sentence),
            _ => None,
        }
    }
}

/// A text segment with hierarchical structure and embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    /// Unique identifier.
    pub id: ChunkId,
    /// FK to the source this chunk came from.
    pub source_id: SourceId,
    /// FK to the parent chunk (null for document-level).
    pub parent_chunk_id: Option<ChunkId>,
    /// Granularity level as string.
    pub level: String,
    /// Position within parent.
    pub ordinal: i32,
    /// Markdown-normalized text content.
    pub content: String,
    /// SHA-256 of content for structural refactoring detection.
    pub content_hash: Vec<u8>,
    /// LLM-generated context summary prepended before embedding.
    pub contextual_prefix: Option<String>,
    /// Token count for the content.
    pub token_count: i32,
    /// Hierarchical path (e.g. `"Title > Chapter 2 > Section 2.1"`).
    pub structural_hierarchy: String,
    /// Federation clearance level.
    pub clearance_level: ClearanceLevel,
    /// Cosine similarity between child and parent embeddings (null for document-level).
    pub parent_alignment: Option<f64>,
    /// Extraction method determined by landscape analysis.
    pub extraction_method: Option<String>,
    /// Additional landscape metrics (adjacent_similarity, sibling_outlier_score, etc.).
    pub landscape_metrics: Option<serde_json::Value>,
    /// Additional metadata (heading text, page number, speaker, etc.).
    pub metadata: serde_json::Value,
    /// Byte offset in `Source.normalized_content` where this chunk
    /// starts (including overlap prefix). `None` for legacy chunks.
    pub byte_start: Option<i32>,
    /// Byte offset in `Source.normalized_content` where this chunk
    /// ends. `None` for legacy chunks.
    pub byte_end: Option<i32>,
    /// Number of overlap prefix bytes at the start of this chunk.
    /// Unique content starts at `byte_start + content_offset`.
    /// Zero for the first chunk in a section.
    pub content_offset: Option<i32>,
    /// When this chunk was created.
    pub created_at: DateTime<Utc>,
}

impl Chunk {
    /// Create a new chunk.
    pub fn new(
        source_id: SourceId,
        level: ChunkLevel,
        ordinal: i32,
        content: String,
        content_hash: Vec<u8>,
        token_count: i32,
    ) -> Self {
        Self {
            id: ChunkId::new(),
            source_id,
            parent_chunk_id: None,
            level: level.as_str().to_string(),
            ordinal,
            content,
            content_hash,
            contextual_prefix: None,
            token_count,
            structural_hierarchy: String::new(),
            clearance_level: ClearanceLevel::default(),
            parent_alignment: None,
            extraction_method: None,
            landscape_metrics: None,
            metadata: serde_json::Value::Object(Default::default()),
            byte_start: None,
            byte_end: None,
            content_offset: None,
            created_at: Utc::now(),
        }
    }

    /// Set the parent chunk and return self for chaining.
    pub fn with_parent(mut self, parent_id: ChunkId) -> Self {
        self.parent_chunk_id = Some(parent_id);
        self
    }

    /// Set the structural hierarchy and return self for chaining.
    pub fn with_hierarchy(mut self, hierarchy: String) -> Self {
        self.structural_hierarchy = hierarchy;
        self
    }

    /// Set the metadata and return self for chaining.
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Extraction method determined by landscape analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionMethod {
    /// Skip extraction -- embedding linkage is sufficient.
    EmbeddingLinkage,
    /// Quick delta check against existing graph.
    DeltaCheck,
    /// Full entity/relationship extraction.
    FullExtraction,
    /// Full extraction with second-pass review (gleaning).
    FullExtractionWithReview,
}

impl ExtractionMethod {
    /// String representation for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EmbeddingLinkage => "embedding_linkage",
            Self::DeltaCheck => "delta_check",
            Self::FullExtraction => "full_extraction",
            Self::FullExtractionWithReview => "full_extraction_with_review",
        }
    }

    /// Parse from database string.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "embedding_linkage" => Some(Self::EmbeddingLinkage),
            "delta_check" => Some(Self::DeltaCheck),
            "full_extraction" => Some(Self::FullExtraction),
            "full_extraction_with_review" => Some(Self::FullExtractionWithReview),
            _ => None,
        }
    }
}

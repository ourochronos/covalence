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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ids::SourceId;

    #[test]
    fn chunk_level_roundtrip() {
        let levels = [
            ChunkLevel::Document,
            ChunkLevel::Section,
            ChunkLevel::Paragraph,
            ChunkLevel::Sentence,
        ];
        for level in &levels {
            let s = level.as_str();
            let parsed = ChunkLevel::from_str_opt(s);
            assert_eq!(parsed, Some(level.clone()), "roundtrip failed for {s}");
        }
    }

    #[test]
    fn chunk_level_unknown() {
        assert!(ChunkLevel::from_str_opt("unknown").is_none());
        assert!(ChunkLevel::from_str_opt("").is_none());
    }

    #[test]
    fn chunk_new_defaults() {
        let source_id = SourceId::new();
        let chunk = Chunk::new(
            source_id,
            ChunkLevel::Paragraph,
            0,
            "Hello world".into(),
            vec![0u8; 32],
            3,
        );

        assert_eq!(chunk.source_id, source_id);
        assert_eq!(chunk.level, "paragraph");
        assert_eq!(chunk.ordinal, 0);
        assert_eq!(chunk.content, "Hello world");
        assert_eq!(chunk.token_count, 3);
        assert!(chunk.parent_chunk_id.is_none());
        assert!(chunk.contextual_prefix.is_none());
        assert!(chunk.byte_start.is_none());
        assert_eq!(chunk.clearance_level, ClearanceLevel::default());
    }

    #[test]
    fn chunk_builder_chain() {
        let source_id = SourceId::new();
        let parent_id = ChunkId::new();
        let meta = serde_json::json!({"heading": "Chapter 1"});

        let chunk = Chunk::new(
            source_id,
            ChunkLevel::Section,
            1,
            "Content".into(),
            vec![0u8; 32],
            2,
        )
        .with_parent(parent_id)
        .with_hierarchy("Doc > Chapter 1".into())
        .with_metadata(meta.clone());

        assert_eq!(chunk.parent_chunk_id, Some(parent_id));
        assert_eq!(chunk.structural_hierarchy, "Doc > Chapter 1");
        assert_eq!(chunk.metadata, meta);
    }

    #[test]
    fn chunk_serde_roundtrip() {
        let chunk = Chunk::new(
            SourceId::new(),
            ChunkLevel::Document,
            0,
            "Test content".into(),
            vec![1, 2, 3],
            5,
        );
        let json = serde_json::to_string(&chunk).unwrap();
        let restored: Chunk = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.content, "Test content");
        assert_eq!(restored.level, "document");
        assert_eq!(restored.token_count, 5);
    }
}

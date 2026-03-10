-- Migration 010: Normalized content storage and chunk byte offsets
--
-- Adds normalized_content and normalized_hash to sources for
-- deterministic re-ingestion and change detection. Adds byte
-- offset columns to chunks so chunk text can be reconstructed
-- from the source's normalized_content without duplication.

-- Sources: store the normalized (post-parse, post-normalize) text
-- and its SHA-256 hash for fast change detection.
ALTER TABLE sources ADD COLUMN normalized_content TEXT;
ALTER TABLE sources ADD COLUMN normalized_hash BYTEA;

CREATE INDEX idx_sources_normalized_hash
    ON sources (normalized_hash)
    WHERE normalized_hash IS NOT NULL;

-- Chunks: byte offsets into source.normalized_content.
-- byte_start..byte_end = full chunk text (including overlap).
-- content_offset = number of overlap prefix bytes (0 for first chunk).
-- Unique content = normalized_content[byte_start + content_offset .. byte_end].
ALTER TABLE chunks ADD COLUMN byte_start INTEGER;
ALTER TABLE chunks ADD COLUMN byte_end INTEGER;
ALTER TABLE chunks ADD COLUMN content_offset INTEGER DEFAULT 0;

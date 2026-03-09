-- Fix embedding column dimensions to 1024 (matching text-embedding-3-large
-- truncated output). Also ensures sources.embedding exists.

-- Add sources.embedding if migration 003 was not applied.
ALTER TABLE sources ADD COLUMN IF NOT EXISTS embedding halfvec(1024);

-- Drop HNSW indexes before retyping columns.
DROP INDEX IF EXISTS idx_chunks_embedding;
DROP INDEX IF EXISTS idx_nodes_embedding;
DROP INDEX IF EXISTS idx_articles_embedding;
DROP INDEX IF EXISTS idx_aliases_embedding;
DROP INDEX IF EXISTS idx_sources_embedding;

-- Retype all embedding columns to halfvec(1024).
ALTER TABLE chunks ALTER COLUMN embedding TYPE halfvec(1024);
ALTER TABLE nodes ALTER COLUMN embedding TYPE halfvec(1024);
ALTER TABLE articles ALTER COLUMN embedding TYPE halfvec(1024);
ALTER TABLE node_aliases ALTER COLUMN alias_embedding TYPE halfvec(1024);
ALTER TABLE sources ALTER COLUMN embedding TYPE halfvec(1024);

-- Recreate HNSW indexes.
CREATE INDEX idx_chunks_embedding
    ON chunks USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

CREATE INDEX idx_nodes_embedding
    ON nodes USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

CREATE INDEX idx_articles_embedding
    ON articles USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

CREATE INDEX idx_aliases_embedding
    ON node_aliases USING hnsw (alias_embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

CREATE INDEX idx_sources_embedding
    ON sources USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

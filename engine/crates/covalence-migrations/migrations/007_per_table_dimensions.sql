-- Per-table embedding dimension tiering.
--
-- Optimizes quality vs storage by using different vector dimensions
-- for each table based on content richness and query frequency:
--   sources:      2048 (fewest records, richest content)
--   chunks:       1024 (most records, balance quality/performance)
--   articles:     1024 (searched alongside chunks)
--   nodes:         256 (short entity text, resolution lookups)
--   node_aliases:  256 (must match nodes for cosine comparison)

-- Drop HNSW indexes before retyping columns.
DROP INDEX IF EXISTS idx_sources_embedding;
DROP INDEX IF EXISTS idx_chunks_embedding;
DROP INDEX IF EXISTS idx_articles_embedding;
DROP INDEX IF EXISTS idx_nodes_embedding;
DROP INDEX IF EXISTS idx_aliases_embedding;

-- Retype each table to its target dimension.
ALTER TABLE sources ALTER COLUMN embedding TYPE halfvec(2048);
ALTER TABLE chunks ALTER COLUMN embedding TYPE halfvec(1024);
ALTER TABLE articles ALTER COLUMN embedding TYPE halfvec(1024);
ALTER TABLE nodes ALTER COLUMN embedding TYPE halfvec(256);
ALTER TABLE node_aliases ALTER COLUMN alias_embedding TYPE halfvec(256);

-- Recreate HNSW indexes at new dimensions.
CREATE INDEX idx_sources_embedding
    ON sources USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

CREATE INDEX idx_chunks_embedding
    ON chunks USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

CREATE INDEX idx_articles_embedding
    ON articles USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

CREATE INDEX idx_nodes_embedding
    ON nodes USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

CREATE INDEX idx_aliases_embedding
    ON node_aliases USING hnsw (alias_embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

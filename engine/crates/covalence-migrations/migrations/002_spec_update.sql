-- Spec update: bi-temporal edges, 2048d embeddings, landscape analysis, query cache

-- 1. Edge bi-temporal columns
ALTER TABLE edges ADD COLUMN valid_from TIMESTAMPTZ;
ALTER TABLE edges ADD COLUMN valid_until TIMESTAMPTZ;
ALTER TABLE edges ADD COLUMN invalid_at TIMESTAMPTZ;
ALTER TABLE edges ADD COLUMN invalidated_by UUID REFERENCES edges(id);

CREATE INDEX idx_edges_valid_from ON edges(valid_from) WHERE valid_from IS NOT NULL;
CREATE INDEX idx_edges_invalid_at ON edges(invalid_at) WHERE invalid_at IS NOT NULL;
CREATE INDEX idx_edges_active ON edges(source_node_id, target_node_id, rel_type) WHERE invalid_at IS NULL;

DROP INDEX IF EXISTS idx_edges_temporal;

-- 2. Embedding dimension 768 -> 2048
-- NOTE: Dimension (2048) must match COVALENCE_EMBED_DIM (default 2048).
-- If using a different dimension, adjust these ALTER statements before running.
DROP INDEX IF EXISTS idx_chunks_embedding;
ALTER TABLE chunks ALTER COLUMN embedding TYPE halfvec(2048);
CREATE INDEX idx_chunks_embedding ON chunks USING hnsw (embedding halfvec_cosine_ops) WITH (m = 16, ef_construction = 64);

DROP INDEX IF EXISTS idx_nodes_embedding;
ALTER TABLE nodes ALTER COLUMN embedding TYPE halfvec(2048);
CREATE INDEX idx_nodes_embedding ON nodes USING hnsw (embedding halfvec_cosine_ops) WITH (m = 16, ef_construction = 64);

DROP INDEX IF EXISTS idx_articles_embedding;
ALTER TABLE articles ALTER COLUMN embedding TYPE halfvec(2048);
CREATE INDEX idx_articles_embedding ON articles USING hnsw (embedding halfvec_cosine_ops) WITH (m = 16, ef_construction = 64);

DROP INDEX IF EXISTS idx_aliases_embedding;
ALTER TABLE node_aliases ALTER COLUMN alias_embedding TYPE halfvec(2048);
CREATE INDEX idx_aliases_embedding ON node_aliases USING hnsw (alias_embedding halfvec_cosine_ops) WITH (m = 16, ef_construction = 64);

-- 3. Chunk landscape fields
ALTER TABLE chunks ADD COLUMN parent_alignment FLOAT;
ALTER TABLE chunks ADD COLUMN extraction_method TEXT;
ALTER TABLE chunks ADD COLUMN landscape_metrics JSONB;

CREATE INDEX idx_chunks_extraction_method ON chunks(extraction_method) WHERE extraction_method IS NOT NULL AND extraction_method != 'embedding_linkage';
CREATE INDEX idx_chunks_parent_alignment ON chunks(parent_alignment) WHERE parent_alignment IS NOT NULL;

-- 4. Model calibrations table
CREATE TABLE model_calibrations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    model_name TEXT NOT NULL UNIQUE,
    parent_child_p25 FLOAT NOT NULL,
    parent_child_p50 FLOAT NOT NULL,
    parent_child_p75 FLOAT NOT NULL,
    adjacent_mean FLOAT NOT NULL,
    adjacent_stddev FLOAT NOT NULL,
    sample_size INT NOT NULL,
    calibrated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 5. Query cache table
CREATE TABLE query_cache (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    query_text TEXT NOT NULL,
    query_embedding halfvec(2048) NOT NULL,
    response JSONB NOT NULL,
    strategy_used TEXT NOT NULL,
    hit_count INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_query_cache_embedding ON query_cache USING hnsw (query_embedding halfvec_cosine_ops) WITH (m = 16, ef_construction = 64);

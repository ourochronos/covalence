-- Move document-level embedding from chunk to source record.
-- The doc-level chunk duplicated the full source text and produced
-- low-quality embeddings for large documents. Storing the embedding
-- on the source itself avoids both issues.

ALTER TABLE sources ADD COLUMN IF NOT EXISTS embedding halfvec(2048);

CREATE INDEX IF NOT EXISTS idx_sources_embedding
    ON sources USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- Migration 009: Section embeddings for tree-indexed sources
-- Ports Valence's tree_index + section_embeddings to Covalence.
--
-- Design:
--   1. tree_index is stored in nodes.metadata->'tree_index' (JSONB)
--   2. Section embeddings are stored in a new table: node_sections
--   3. Each section references a (start_char, end_char) range in the source content
--   4. Section embeddings participate in vector search alongside node-level embeddings
--   5. Composed embeddings (mean of sections) replace the single-content embedding
--
-- No truncation: sections are sized to fit embedding model context windows.
-- Overlap: configurable, defaults to 20%.

BEGIN;

-- Section embeddings table
CREATE TABLE IF NOT EXISTS covalence.node_sections (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    node_id         UUID        NOT NULL REFERENCES covalence.nodes(id) ON DELETE CASCADE,
    tree_path       TEXT        NOT NULL,       -- "0", "0.1", "0.2.3" etc.
    depth           INT         NOT NULL DEFAULT 0,
    title           TEXT,
    summary         TEXT,
    start_char      INT         NOT NULL,
    end_char        INT         NOT NULL,
    content_hash    TEXT,                       -- MD5 of slice for change detection
    embedding       halfvec(1536),
    model           TEXT        DEFAULT 'text-embedding-3-small',
    created_at      TIMESTAMPTZ DEFAULT now(),

    -- Unique constraint: one section per tree_path per node
    CONSTRAINT node_sections_unique UNIQUE (node_id, tree_path)
);

-- Index for vector search over sections (same HNSW params as node_embeddings)
CREATE INDEX IF NOT EXISTS node_sections_hnsw_idx
    ON covalence.node_sections
    USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- Index for fast lookup by node_id
CREATE INDEX IF NOT EXISTS node_sections_node_id_idx
    ON covalence.node_sections (node_id);

-- Index for depth-filtered search (leaf sections only)
CREATE INDEX IF NOT EXISTS node_sections_depth_idx
    ON covalence.node_sections (depth);

COMMENT ON TABLE covalence.node_sections IS
    'Tree-indexed section embeddings for multi-granularity vector search. '
    'Each row is a slice of a source/article content with its own embedding. '
    'Leaf sections enable precise sub-document matching; parent sections enable '
    'broader thematic matching. Composed (mean) embeddings are written back to node_embeddings.';

COMMIT;

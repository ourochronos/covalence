-- Wave 8 foundation: offset projection ledgers, unresolved entity pool,
-- and removal of legacy landscape analysis columns.

-- Offset Projection Ledger: tracks character-level mutations from
-- coreference resolution (fastcoref) so that byte spans in mutated text
-- can be reverse-projected to canonical source byte offsets.
CREATE TABLE IF NOT EXISTS offset_projection_ledgers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id UUID NOT NULL REFERENCES sources(id),
    -- Canonical (original) text span.
    canonical_span_start INT NOT NULL,
    canonical_span_end INT NOT NULL,
    canonical_token TEXT NOT NULL,
    -- Mutated (post-coref) text span.
    mutated_span_start INT NOT NULL,
    mutated_span_end INT NOT NULL,
    mutated_token TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_opl_source_id
    ON offset_projection_ledgers(source_id);
CREATE INDEX IF NOT EXISTS idx_opl_mutated_span
    ON offset_projection_ledgers(source_id, mutated_span_start, mutated_span_end);

-- Unresolved Entity Pool: entities that missed all 4 resolution tiers
-- are held here for HDBSCAN batch clustering (Tier 5).
CREATE TABLE IF NOT EXISTS unresolved_entities (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id UUID NOT NULL REFERENCES sources(id),
    statement_id UUID REFERENCES statements(id),
    chunk_id UUID REFERENCES chunks(id),
    extracted_name TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    description TEXT,
    embedding halfvec(256),
    confidence FLOAT NOT NULL DEFAULT 1.0,
    resolved_node_id UUID REFERENCES nodes(id),
    resolved_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_unresolved_source_id
    ON unresolved_entities(source_id);
CREATE INDEX IF NOT EXISTS idx_unresolved_pending
    ON unresolved_entities(resolved_node_id) WHERE resolved_node_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_unresolved_embedding
    ON unresolved_entities USING hnsw(embedding halfvec_cosine_ops);

-- Drop legacy landscape analysis columns from chunks.
-- These are obsolete under the statement-first paradigm.
ALTER TABLE chunks DROP COLUMN IF EXISTS parent_alignment;
ALTER TABLE chunks DROP COLUMN IF EXISTS adjacent_similarity;
ALTER TABLE chunks DROP COLUMN IF EXISTS sibling_outlier_score;
ALTER TABLE chunks DROP COLUMN IF EXISTS extraction_method;
ALTER TABLE chunks DROP COLUMN IF EXISTS landscape_metrics;

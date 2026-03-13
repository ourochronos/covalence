-- Statement-first extraction pipeline: statements and sections tables.
-- ADR-0015: Extract atomic statements from source text, cluster into
-- sections, then use as primary retrieval units.

-- Atomic knowledge claims extracted from source text.
CREATE TABLE IF NOT EXISTS statements (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id UUID NOT NULL REFERENCES sources(id),
    content TEXT NOT NULL,
    content_hash BYTEA NOT NULL,
    embedding halfvec(1024),
    byte_start INT NOT NULL,
    byte_end INT NOT NULL,
    heading_path TEXT,
    paragraph_index INT,
    ordinal INT NOT NULL,
    confidence FLOAT NOT NULL DEFAULT 1.0,
    section_id UUID,
    clearance_level INT NOT NULL DEFAULT 0,
    is_evicted BOOLEAN NOT NULL DEFAULT false,
    extraction_method TEXT NOT NULL DEFAULT 'llm_statement',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    content_tsv tsvector GENERATED ALWAYS AS (to_tsvector('english', content)) STORED
);

-- Compiled clusters of related statements.
CREATE TABLE IF NOT EXISTS sections (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id UUID NOT NULL REFERENCES sources(id),
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    content_hash BYTEA NOT NULL,
    embedding halfvec(1024),
    statement_ids UUID[] NOT NULL DEFAULT '{}',
    cluster_label TEXT,
    ordinal INT NOT NULL,
    clearance_level INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    body_tsv tsvector GENERATED ALWAYS AS (to_tsvector('english', title || ' ' || summary)) STORED
);

-- FK from statements to sections (deferred to allow creation order flexibility).
ALTER TABLE statements
    ADD CONSTRAINT fk_statements_section
    FOREIGN KEY (section_id) REFERENCES sections(id);

-- Indexes for statements.
CREATE INDEX IF NOT EXISTS idx_statements_source_id ON statements(source_id);
CREATE INDEX IF NOT EXISTS idx_statements_section_id ON statements(section_id);
CREATE INDEX IF NOT EXISTS idx_statements_content_hash ON statements(source_id, content_hash);
CREATE INDEX IF NOT EXISTS idx_statements_content_tsv ON statements USING gin(content_tsv);
CREATE INDEX IF NOT EXISTS idx_statements_embedding ON statements USING hnsw(embedding halfvec_cosine_ops);

-- Indexes for sections.
CREATE INDEX IF NOT EXISTS idx_sections_source_id ON sections(source_id);
CREATE INDEX IF NOT EXISTS idx_sections_body_tsv ON sections USING gin(body_tsv);
CREATE INDEX IF NOT EXISTS idx_sections_embedding ON sections USING hnsw(embedding halfvec_cosine_ops);

-- Dual provenance: extractions can link to either a chunk or a statement.
ALTER TABLE extractions ADD COLUMN IF NOT EXISTS statement_id UUID REFERENCES statements(id);
ALTER TABLE extractions ALTER COLUMN chunk_id DROP NOT NULL;
CREATE INDEX IF NOT EXISTS idx_extractions_statement_id ON extractions(statement_id);

-- Source summary (compiled from section summaries).
ALTER TABLE sources ADD COLUMN IF NOT EXISTS summary TEXT;

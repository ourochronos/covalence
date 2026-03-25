-- 003: Statements and sections tables
--
-- Statement-first extraction pipeline per ADR-0015. Atomic
-- statements extracted from source text, clustered into sections.

-- ================================================================
-- Statements: atomic knowledge claims
-- ================================================================

CREATE TABLE IF NOT EXISTS statements (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id         UUID NOT NULL REFERENCES sources(id),
    content           TEXT NOT NULL,
    content_hash      BYTEA NOT NULL,
    embedding         halfvec(1024),
    byte_start        INT NOT NULL,
    byte_end          INT NOT NULL,
    heading_path      TEXT,
    paragraph_index   INT,
    ordinal           INT NOT NULL,
    confidence        FLOAT NOT NULL DEFAULT 1.0,
    section_id        UUID,
    clearance_level   INT NOT NULL DEFAULT 0,
    is_evicted        BOOLEAN NOT NULL DEFAULT false,
    extraction_method TEXT NOT NULL DEFAULT 'llm_statement',
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Processing metadata (migration 016)
    processing        JSONB DEFAULT '{}',
    -- Full-text search (generated column)
    content_tsv       tsvector GENERATED ALWAYS AS (to_tsvector('english', content)) STORED
);

-- ================================================================
-- Sections: compiled clusters of related statements
-- ================================================================

CREATE TABLE IF NOT EXISTS sections (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id       UUID NOT NULL REFERENCES sources(id),
    title           TEXT NOT NULL,
    summary         TEXT NOT NULL,
    content_hash    BYTEA NOT NULL,
    embedding       halfvec(1024),
    statement_ids   UUID[] NOT NULL DEFAULT '{}',
    cluster_label   TEXT,
    ordinal         INT NOT NULL,
    clearance_level INT NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Full-text search (generated column)
    body_tsv        tsvector GENERATED ALWAYS AS (
        to_tsvector('english', title || ' ' || summary)
    ) STORED
);

-- ================================================================
-- Deferred foreign keys
-- ================================================================

-- FK from statements to sections
ALTER TABLE statements
    ADD CONSTRAINT fk_statements_section
    FOREIGN KEY (section_id) REFERENCES sections(id);

-- FK from extractions to statements (dual provenance)
ALTER TABLE extractions
    ADD CONSTRAINT fk_extractions_statement
    FOREIGN KEY (statement_id) REFERENCES statements(id);

-- FK from unresolved_entities to statements
ALTER TABLE unresolved_entities
    ADD CONSTRAINT fk_unresolved_entities_statement
    FOREIGN KEY (statement_id) REFERENCES statements(id);

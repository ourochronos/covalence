-- 001: Core schema — all tables at final shape
--
-- Extensions, core tables (sources, chunks, nodes, edges, articles),
-- supporting tables (extractions, node_aliases, audit_logs,
-- offset_projection_ledgers, unresolved_entities, outbox_events),
-- views, and trigger functions.

-- ================================================================
-- Extensions
-- ================================================================

CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE EXTENSION IF NOT EXISTS ltree;

-- ================================================================
-- Sources
-- ================================================================

CREATE TABLE IF NOT EXISTS sources (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_type       TEXT NOT NULL,
    uri               TEXT,
    title             TEXT,
    author            TEXT,
    created_date      TIMESTAMPTZ,
    ingested_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    content_hash      BYTEA NOT NULL,
    metadata          JSONB NOT NULL DEFAULT '{}',
    raw_content       TEXT,
    trust_alpha       FLOAT NOT NULL DEFAULT 4.0,
    trust_beta        FLOAT NOT NULL DEFAULT 1.0,
    reliability_score FLOAT NOT NULL DEFAULT 0.8,
    clearance_level   INT NOT NULL DEFAULT 0,
    update_class      TEXT,
    supersedes_id     UUID REFERENCES sources(id),
    content_version   INT NOT NULL DEFAULT 1,
    -- Source-level embedding (Voyage voyage-3-large, 2048d)
    embedding         halfvec(2048),
    -- Normalized content for deterministic re-ingestion (migration 010)
    normalized_content TEXT,
    normalized_hash    BYTEA,
    -- Source summary compiled from section summaries (migration 011)
    summary           TEXT,
    -- Graph type system labels (ADR-0018, migration 014)
    project           TEXT NOT NULL DEFAULT 'covalence',
    domain            TEXT,
    -- Supersession tracking (migration 017)
    superseded_by     UUID REFERENCES sources(id),
    superseded_at     TIMESTAMPTZ,
    -- Async processing status (migration 018)
    status            TEXT NOT NULL DEFAULT 'complete',
    -- Processing metadata (migration 016)
    processing        JSONB DEFAULT '{}',
    UNIQUE(content_hash)
);

-- ================================================================
-- Chunks
-- ================================================================

CREATE TABLE IF NOT EXISTS chunks (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id           UUID NOT NULL REFERENCES sources(id),
    parent_chunk_id     UUID REFERENCES chunks(id),
    level               TEXT NOT NULL,
    ordinal             INT NOT NULL,
    content             TEXT NOT NULL,
    content_hash        BYTEA NOT NULL,
    embedding           halfvec(1024),
    contextual_prefix   TEXT,
    token_count         INT NOT NULL,
    structural_hierarchy ltree NOT NULL DEFAULT '',
    clearance_level     INT NOT NULL DEFAULT 0,
    metadata            JSONB NOT NULL DEFAULT '{}',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Byte offsets into source.normalized_content (migration 010)
    byte_start          INTEGER,
    byte_end            INTEGER,
    content_offset      INTEGER DEFAULT 0,
    -- Processing metadata (migration 016)
    processing          JSONB DEFAULT '{}',
    -- Full-text search (generated column)
    content_tsv         tsvector GENERATED ALWAYS AS (to_tsvector('english', content)) STORED
);

-- ================================================================
-- Nodes
-- ================================================================

CREATE TABLE IF NOT EXISTS nodes (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    canonical_name       TEXT NOT NULL,
    node_type            TEXT NOT NULL,
    description          TEXT,
    properties           JSONB NOT NULL DEFAULT '{}',
    embedding            halfvec(256),
    confidence_breakdown JSONB,
    clearance_level      INT NOT NULL DEFAULT 0,
    first_seen           TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen            TIMESTAMPTZ NOT NULL DEFAULT now(),
    mention_count        INT NOT NULL DEFAULT 1,
    -- Ontology clustering (migration 008)
    canonical_type       TEXT,
    cluster_id           UUID,
    -- Graph type system (ADR-0018, migration 014)
    entity_class         TEXT,
    -- Domain entropy (migration 015)
    domain_entropy       REAL,
    primary_domain       TEXT,
    -- Processing metadata (migration 016)
    processing           JSONB DEFAULT '{}',
    -- Full-text search (generated column)
    name_tsv             tsvector GENERATED ALWAYS AS (
        to_tsvector('english', canonical_name || ' ' || COALESCE(description, ''))
    ) STORED
);

-- ================================================================
-- Edges
-- ================================================================

CREATE TABLE IF NOT EXISTS edges (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_node_id    UUID NOT NULL REFERENCES nodes(id),
    target_node_id    UUID NOT NULL REFERENCES nodes(id),
    rel_type          TEXT NOT NULL,
    causal_level      TEXT CHECK (causal_level IN ('association', 'intervention', 'counterfactual')),
    properties        JSONB NOT NULL DEFAULT '{}',
    weight            FLOAT NOT NULL DEFAULT 1.0,
    confidence        FLOAT NOT NULL DEFAULT 1.0,
    confidence_breakdown JSONB,
    clearance_level   INT NOT NULL DEFAULT 0,
    is_synthetic      BOOLEAN NOT NULL DEFAULT false,
    recorded_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Bi-temporal columns (migration 002)
    valid_from        TIMESTAMPTZ,
    valid_until       TIMESTAMPTZ,
    invalid_at        TIMESTAMPTZ,
    invalidated_by    UUID REFERENCES edges(id),
    -- Ontology clustering (migration 008)
    canonical_rel_type TEXT
);

-- ================================================================
-- Articles
-- ================================================================

CREATE TABLE IF NOT EXISTS articles (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    title             TEXT NOT NULL,
    body              TEXT NOT NULL,
    embedding         halfvec(1024),
    confidence        FLOAT NOT NULL DEFAULT 1.0,
    confidence_breakdown JSONB,
    domain_path       TEXT[] NOT NULL DEFAULT '{}',
    version           INT NOT NULL DEFAULT 1,
    content_hash      BYTEA NOT NULL,
    source_node_ids   UUID[] NOT NULL DEFAULT '{}',
    clearance_level   INT NOT NULL DEFAULT 0,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Full-text search (generated column)
    body_tsv          tsvector GENERATED ALWAYS AS (
        to_tsvector('english', title || ' ' || body)
    ) STORED
);

-- ================================================================
-- Extractions
-- ================================================================

CREATE TABLE IF NOT EXISTS extractions (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    chunk_id          UUID REFERENCES chunks(id),
    entity_type       TEXT NOT NULL,
    entity_id         UUID NOT NULL,
    extraction_method TEXT NOT NULL,
    confidence        FLOAT NOT NULL DEFAULT 1.0,
    is_superseded     BOOLEAN NOT NULL DEFAULT false,
    extracted_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Dual provenance: can link to chunk or statement (migration 011)
    statement_id      UUID
);

-- ================================================================
-- Node Aliases
-- ================================================================

CREATE TABLE IF NOT EXISTS node_aliases (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    node_id         UUID NOT NULL REFERENCES nodes(id),
    alias           TEXT NOT NULL,
    alias_embedding halfvec(256),
    source_chunk_id UUID REFERENCES chunks(id)
);

-- ================================================================
-- Audit Logs
-- ================================================================

CREATE TABLE IF NOT EXISTS audit_logs (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    action      TEXT NOT NULL,
    actor       TEXT NOT NULL,
    target_type TEXT,
    target_id   UUID,
    payload     JSONB NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ================================================================
-- Outbox Events (Graph Sidecar Sync)
-- ================================================================

CREATE TABLE IF NOT EXISTS outbox_events (
    seq_id      BIGSERIAL PRIMARY KEY,
    entity_type TEXT NOT NULL,
    entity_id   UUID NOT NULL,
    operation   TEXT NOT NULL,
    payload     JSONB NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ================================================================
-- Offset Projection Ledger
-- ================================================================

CREATE TABLE IF NOT EXISTS offset_projection_ledgers (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id           UUID NOT NULL REFERENCES sources(id),
    canonical_span_start INT NOT NULL,
    canonical_span_end   INT NOT NULL,
    canonical_token      TEXT NOT NULL,
    mutated_span_start   INT NOT NULL,
    mutated_span_end     INT NOT NULL,
    mutated_token        TEXT NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ================================================================
-- Unresolved Entities (Tier 5 HDBSCAN batch pool)
-- ================================================================

CREATE TABLE IF NOT EXISTS unresolved_entities (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id        UUID NOT NULL REFERENCES sources(id),
    statement_id     UUID,
    chunk_id         UUID REFERENCES chunks(id),
    extracted_name   TEXT NOT NULL,
    entity_type      TEXT NOT NULL,
    description      TEXT,
    embedding        halfvec(256),
    confidence       FLOAT NOT NULL DEFAULT 1.0,
    resolved_node_id UUID REFERENCES nodes(id),
    resolved_at      TIMESTAMPTZ,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ================================================================
-- Ontology Clusters
-- ================================================================

CREATE TABLE IF NOT EXISTS ontology_clusters (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    level            TEXT NOT NULL,
    canonical_label  TEXT NOT NULL,
    member_labels    JSONB NOT NULL DEFAULT '[]',
    member_count     INT NOT NULL DEFAULT 0,
    min_cluster_size INT NOT NULL DEFAULT 2,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- FK: nodes.cluster_id -> ontology_clusters (deferred because nodes defined before ontology_clusters)
ALTER TABLE nodes ADD CONSTRAINT fk_nodes_cluster
    FOREIGN KEY (cluster_id) REFERENCES ontology_clusters(id);

-- ================================================================
-- Model Calibrations (embedding landscape analysis)
-- ================================================================

CREATE TABLE IF NOT EXISTS model_calibrations (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    model_name        TEXT NOT NULL UNIQUE,
    parent_child_p25  FLOAT NOT NULL,
    parent_child_p50  FLOAT NOT NULL,
    parent_child_p75  FLOAT NOT NULL,
    adjacent_mean     FLOAT NOT NULL,
    adjacent_stddev   FLOAT NOT NULL,
    sample_size       INT NOT NULL,
    calibrated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ================================================================
-- Trigger: outbox notification for graph sidecar sync
-- ================================================================

CREATE OR REPLACE FUNCTION notify_outbox() RETURNS TRIGGER AS $$
BEGIN
    INSERT INTO outbox_events (entity_type, entity_id, operation, payload)
    VALUES (TG_TABLE_NAME, COALESCE(NEW.id, OLD.id), TG_OP, row_to_json(COALESCE(NEW, OLD)));
    NOTIFY graph_sync_ping;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_nodes_outbox AFTER INSERT OR UPDATE OR DELETE ON nodes
    FOR EACH ROW EXECUTE FUNCTION notify_outbox();
CREATE TRIGGER trg_edges_outbox AFTER INSERT OR UPDATE OR DELETE ON edges
    FOR EACH ROW EXECUTE FUNCTION notify_outbox();

-- ================================================================
-- Trigger: auto-compute reliability_score from trust params
-- ================================================================

CREATE OR REPLACE FUNCTION update_reliability_score()
RETURNS TRIGGER AS $$
BEGIN
    NEW.reliability_score := NEW.trust_alpha / (NEW.trust_alpha + NEW.trust_beta);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_reliability_score
    BEFORE INSERT OR UPDATE OF trust_alpha, trust_beta ON sources
    FOR EACH ROW EXECUTE FUNCTION update_reliability_score();

-- ================================================================
-- View: provenance triples
-- ================================================================

CREATE VIEW provenance_triples AS
SELECT
    e.id AS triple_id,
    e.source_node_id AS subject,
    e.rel_type AS predicate,
    e.target_node_id AS object,
    e.causal_level,
    e.confidence,
    ex.chunk_id,
    ex.confidence AS extraction_confidence,
    s.reliability_score AS source_reliability
FROM edges e
JOIN extractions ex ON ex.entity_id = e.id AND ex.entity_type = 'edge'
JOIN chunks c ON c.id = ex.chunk_id
JOIN sources s ON s.id = c.source_id
WHERE NOT ex.is_superseded;

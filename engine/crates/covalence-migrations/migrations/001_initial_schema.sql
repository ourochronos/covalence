-- Covalence initial schema
-- Spec reference: spec/03-storage.md

-- Required extensions
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE EXTENSION IF NOT EXISTS ltree;

-- === Sources ===

CREATE TABLE sources (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_type TEXT NOT NULL,
    uri TEXT,
    title TEXT,
    author TEXT,
    created_date TIMESTAMPTZ,
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    content_hash BYTEA NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}',
    raw_content TEXT,
    trust_alpha FLOAT NOT NULL DEFAULT 4.0,
    trust_beta FLOAT NOT NULL DEFAULT 1.0,
    reliability_score FLOAT NOT NULL DEFAULT 0.8,
    clearance_level INT NOT NULL DEFAULT 0,
    update_class TEXT,
    supersedes_id UUID REFERENCES sources(id),
    content_version INT NOT NULL DEFAULT 1,
    UNIQUE(content_hash)
);

CREATE INDEX idx_sources_type ON sources(source_type);
CREATE INDEX idx_sources_ingested ON sources(ingested_at);
CREATE INDEX idx_sources_metadata ON sources USING GIN(metadata);
CREATE INDEX idx_sources_clearance ON sources(clearance_level);
CREATE INDEX idx_sources_supersedes ON sources(supersedes_id);

-- === Chunks ===

CREATE TABLE chunks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id UUID NOT NULL REFERENCES sources(id),
    parent_chunk_id UUID REFERENCES chunks(id),
    level TEXT NOT NULL,
    ordinal INT NOT NULL,
    content TEXT NOT NULL,
    content_hash BYTEA NOT NULL,
    embedding halfvec(768),
    contextual_prefix TEXT,
    token_count INT NOT NULL,
    structural_hierarchy ltree NOT NULL DEFAULT '',
    clearance_level INT NOT NULL DEFAULT 0,
    metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_chunks_source ON chunks(source_id);
CREATE INDEX idx_chunks_parent ON chunks(parent_chunk_id);
CREATE INDEX idx_chunks_level ON chunks(level);
CREATE INDEX idx_chunks_hash ON chunks(content_hash);
CREATE INDEX idx_chunks_clearance ON chunks(clearance_level);
CREATE INDEX idx_chunks_hierarchy ON chunks USING GIST(structural_hierarchy);
CREATE INDEX idx_chunks_embedding ON chunks
    USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- === Nodes ===

CREATE TABLE nodes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    canonical_name TEXT NOT NULL,
    node_type TEXT NOT NULL,
    description TEXT,
    properties JSONB NOT NULL DEFAULT '{}',
    embedding halfvec(768),
    confidence_breakdown JSONB,
    clearance_level INT NOT NULL DEFAULT 0,
    first_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
    mention_count INT NOT NULL DEFAULT 1
);

CREATE INDEX idx_nodes_type ON nodes(node_type);
CREATE INDEX idx_nodes_name ON nodes(canonical_name);
CREATE INDEX idx_nodes_name_trgm ON nodes USING GIN(canonical_name gin_trgm_ops);
CREATE INDEX idx_nodes_clearance ON nodes(clearance_level);
CREATE INDEX idx_nodes_embedding ON nodes
    USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);
CREATE INDEX idx_nodes_properties ON nodes USING GIN(properties);

-- === Edges ===

CREATE TABLE edges (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_node_id UUID NOT NULL REFERENCES nodes(id),
    target_node_id UUID NOT NULL REFERENCES nodes(id),
    rel_type TEXT NOT NULL,
    causal_level TEXT CHECK (causal_level IN ('association', 'intervention', 'counterfactual')),
    properties JSONB NOT NULL DEFAULT '{}',
    weight FLOAT NOT NULL DEFAULT 1.0,
    confidence FLOAT NOT NULL DEFAULT 1.0,
    confidence_breakdown JSONB,
    clearance_level INT NOT NULL DEFAULT 0,
    is_synthetic BOOLEAN NOT NULL DEFAULT false,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_edges_source ON edges(source_node_id);
CREATE INDEX idx_edges_target ON edges(target_node_id);
CREATE INDEX idx_edges_type ON edges(rel_type);
CREATE INDEX idx_edges_causal ON edges(causal_level) WHERE causal_level IS NOT NULL;
CREATE INDEX idx_edges_pair ON edges(source_node_id, target_node_id, rel_type);
CREATE INDEX idx_edges_clearance ON edges(clearance_level);
CREATE INDEX idx_edges_temporal ON edges USING GIN(properties)
    WHERE properties ? 'valid_from';

-- === Articles ===

CREATE TABLE articles (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    embedding halfvec(768),
    confidence FLOAT NOT NULL DEFAULT 1.0,
    confidence_breakdown JSONB,
    domain_path TEXT[] NOT NULL DEFAULT '{}',
    version INT NOT NULL DEFAULT 1,
    content_hash BYTEA NOT NULL,
    source_node_ids UUID[] NOT NULL DEFAULT '{}',
    clearance_level INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_articles_domain ON articles USING GIN(domain_path);
CREATE INDEX idx_articles_clearance ON articles(clearance_level);
CREATE INDEX idx_articles_embedding ON articles
    USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- === Outbox Events (Graph Sidecar Sync) ===

CREATE TABLE outbox_events (
    seq_id BIGSERIAL PRIMARY KEY,
    entity_type TEXT NOT NULL,
    entity_id UUID NOT NULL,
    operation TEXT NOT NULL,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

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

-- === Audit Logs ===

CREATE TABLE audit_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    action TEXT NOT NULL,
    actor TEXT NOT NULL,
    target_type TEXT,
    target_id UUID,
    payload JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_audit_action ON audit_logs(action);
CREATE INDEX idx_audit_target ON audit_logs(target_type, target_id);
CREATE INDEX idx_audit_time ON audit_logs(created_at);

-- === Extractions ===

CREATE TABLE extractions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    chunk_id UUID NOT NULL REFERENCES chunks(id),
    entity_type TEXT NOT NULL,
    entity_id UUID NOT NULL,
    extraction_method TEXT NOT NULL,
    confidence FLOAT NOT NULL DEFAULT 1.0,
    is_superseded BOOLEAN NOT NULL DEFAULT false,
    extracted_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_extractions_chunk ON extractions(chunk_id);
CREATE INDEX idx_extractions_entity ON extractions(entity_type, entity_id);
CREATE INDEX idx_extractions_active ON extractions(entity_type, entity_id)
    WHERE NOT is_superseded;

-- === Node Aliases ===

CREATE TABLE node_aliases (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    node_id UUID NOT NULL REFERENCES nodes(id),
    alias TEXT NOT NULL,
    alias_embedding halfvec(768),
    source_chunk_id UUID REFERENCES chunks(id)
);

CREATE INDEX idx_aliases_node ON node_aliases(node_id);
CREATE INDEX idx_aliases_text ON node_aliases(alias);
CREATE INDEX idx_aliases_text_trgm ON node_aliases USING GIN(alias gin_trgm_ops);
CREATE INDEX idx_aliases_embedding ON node_aliases
    USING hnsw (alias_embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- === Full-Text Search Support ===

ALTER TABLE chunks ADD COLUMN content_tsv tsvector
    GENERATED ALWAYS AS (to_tsvector('english', content)) STORED;
CREATE INDEX idx_chunks_tsv ON chunks USING GIN(content_tsv);

ALTER TABLE nodes ADD COLUMN name_tsv tsvector
    GENERATED ALWAYS AS (to_tsvector('english', canonical_name || ' ' || COALESCE(description, ''))) STORED;
CREATE INDEX idx_nodes_tsv ON nodes USING GIN(name_tsv);

ALTER TABLE articles ADD COLUMN body_tsv tsvector
    GENERATED ALWAYS AS (to_tsvector('english', title || ' ' || body)) STORED;
CREATE INDEX idx_articles_tsv ON articles USING GIN(body_tsv);

-- === Stored Procedures ===

-- Confidence trigger: recompute reliability_score on trust changes
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

-- Provenance triples view
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

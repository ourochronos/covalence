-- =============================================================================
-- Test database schema for covalence_test
-- Applied by setup_test_database() before each test run.
-- Idempotent: every statement uses IF NOT EXISTS / DO-blocks.
-- AGE extension is intentionally omitted; integration tests use the
-- relational layer only and do not require Cypher traversal.
-- =============================================================================

CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS vector;

CREATE SCHEMA IF NOT EXISTS covalence;

-- -----------------------------------------------------------------------------
-- nodes
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.nodes (
    id              UUID             PRIMARY KEY DEFAULT gen_random_uuid(),
    age_id          BIGINT,
    node_type       TEXT             NOT NULL
                                     CONSTRAINT nodes_node_type_check
                                     CHECK (node_type IN ('article', 'source', 'entity')),
    title           TEXT,
    content         TEXT,
    status          TEXT             DEFAULT 'active'
                                     CONSTRAINT nodes_status_check
                                     CHECK (status IN (
                                         'active', 'superseded', 'archived',
                                         'disputed', 'tombstone'
                                     )),
    confidence      DOUBLE PRECISION DEFAULT 0.5,
    epistemic_type  TEXT
                                     CONSTRAINT nodes_epistemic_type_check
                                     CHECK (epistemic_type IN (
                                         'semantic', 'episodic', 'procedural', 'declarative'
                                     )),
    domain_path     TEXT[],
    metadata        JSONB            DEFAULT '{}',
    source_type     TEXT,
    reliability     DOUBLE PRECISION,
    content_hash    TEXT,
    fingerprint     TEXT,
    size_tokens     INTEGER,
    pinned          BOOLEAN          DEFAULT false,
    version         INTEGER          NOT NULL DEFAULT 1,
    created_at      TIMESTAMPTZ      DEFAULT now(),
    modified_at     TIMESTAMPTZ      DEFAULT now(),
    accessed_at     TIMESTAMPTZ      DEFAULT now(),
    archived_at     TIMESTAMPTZ,
    usage_score             DOUBLE PRECISION DEFAULT 0.5,
    last_reconsolidated_at  TIMESTAMPTZ,
    namespace               TEXT             NOT NULL DEFAULT 'default',
    content_tsv             TSVECTOR         GENERATED ALWAYS AS (
                                to_tsvector('english',
                                    COALESCE(title, '') || ' ' || COALESCE(content, ''))
                            ) STORED
);

-- Migration 018: add last_reconsolidated_at to pre-existing test databases.
ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS last_reconsolidated_at TIMESTAMPTZ;

-- Migration 020: expanding-interval consolidation schedule (covalence#67).
ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS next_consolidation_at TIMESTAMPTZ NULL;

ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS consolidation_count INT NOT NULL DEFAULT 0;

-- Migration 024: Faceted classification (covalence#92).
ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS facet_function TEXT[] NULL DEFAULT NULL;

ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS facet_scope TEXT[] NULL DEFAULT NULL;

-- Migration 021: index for heartbeat query (covalence#67 final).
CREATE INDEX IF NOT EXISTS idx_nodes_next_consolidation_at
    ON covalence.nodes (next_consolidation_at)
    WHERE next_consolidation_at IS NOT NULL
      AND status = 'active'
      AND node_type = 'article';

-- -----------------------------------------------------------------------------
-- edges
-- (No edge_type CHECK constraint — extensible string label, validated in Rust)
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.edges (
    id              UUID             PRIMARY KEY DEFAULT gen_random_uuid(),
    age_id          BIGINT,
    source_node_id  UUID             REFERENCES covalence.nodes(id),
    target_node_id  UUID             REFERENCES covalence.nodes(id),
    edge_type       TEXT             NOT NULL,
    weight          DOUBLE PRECISION DEFAULT 1.0,
    confidence      DOUBLE PRECISION DEFAULT 1.0,
    metadata        JSONB            DEFAULT '{}',
    created_at      TIMESTAMPTZ      DEFAULT now(),
    created_by      TEXT,
    namespace       TEXT             NOT NULL DEFAULT 'default',
    -- Migration 019: temporal edges (covalence#60)
    valid_from      TIMESTAMPTZ      NOT NULL DEFAULT now(),
    valid_to        TIMESTAMPTZ      NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS edges_dedup_idx
    ON covalence.edges (source_node_id, target_node_id, edge_type);

-- Migration 019: add temporal columns to pre-existing test databases.
ALTER TABLE covalence.edges ADD COLUMN IF NOT EXISTS valid_from TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE covalence.edges ADD COLUMN IF NOT EXISTS valid_to   TIMESTAMPTZ NULL;

-- Partial index for superseded-edge lookups.
CREATE INDEX IF NOT EXISTS idx_edges_valid_to
    ON covalence.edges (valid_to)
    WHERE valid_to IS NOT NULL;

-- -----------------------------------------------------------------------------
-- node_embeddings
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.node_embeddings (
    node_id         UUID             PRIMARY KEY
                                     REFERENCES covalence.nodes(id) ON DELETE CASCADE,
    embedding       halfvec(1536),
    model           TEXT             DEFAULT 'text-embedding-3-small',
    created_at      TIMESTAMPTZ      DEFAULT now(),
    namespace       TEXT             NOT NULL DEFAULT 'default'
);

-- -----------------------------------------------------------------------------
-- usage_traces
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.usage_traces (
    id              UUID             PRIMARY KEY DEFAULT gen_random_uuid(),
    node_id         UUID             REFERENCES covalence.nodes(id),
    session_id      TEXT,
    query_text      TEXT,
    retrieval_rank  INTEGER,
    accessed_at     TIMESTAMPTZ      DEFAULT now(),
    namespace       TEXT             NOT NULL DEFAULT 'default'
);

-- -----------------------------------------------------------------------------
-- contentions
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.contentions (
    id              UUID             PRIMARY KEY DEFAULT gen_random_uuid(),
    node_id         UUID             REFERENCES covalence.nodes(id),
    source_node_id  UUID             REFERENCES covalence.nodes(id),
    edge_id         UUID             REFERENCES covalence.edges(id),
    type            TEXT,
    description     TEXT,
    severity        TEXT,
    status          TEXT             DEFAULT 'detected',
    resolution      TEXT,
    materiality     DOUBLE PRECISION,
    detected_at     TIMESTAMPTZ      DEFAULT now(),
    resolved_at     TIMESTAMPTZ,
    namespace       TEXT             NOT NULL DEFAULT 'default',
    contention_type TEXT             NOT NULL DEFAULT 'rebuttal'
                                     CHECK (contention_type IN ('rebuttal', 'undermining', 'undercutting')),
    CONSTRAINT contentions_article_source_uniq UNIQUE (node_id, source_node_id)
);

-- -----------------------------------------------------------------------------
-- slow_path_queue
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.slow_path_queue (
    id              UUID             PRIMARY KEY DEFAULT gen_random_uuid(),
    task_type       TEXT             NOT NULL
                                     CONSTRAINT slow_path_queue_task_type_check
                                     CHECK (task_type IN (
                                         'compile', 'infer_edges', 'resolve_contention',
                                         'split', 'merge', 'embed', 'contention_check',
                                         'tree_index', 'tree_embed', 'recompile'
                                     )),
    node_id         UUID             REFERENCES covalence.nodes(id),
    payload         JSONB            DEFAULT '{}',
    status          TEXT             DEFAULT 'pending'
                                     CONSTRAINT slow_path_queue_status_check
                                     CHECK (status IN (
                                         'pending', 'processing', 'complete', 'failed'
                                     )),
    priority        INTEGER          DEFAULT 0,
    created_at      TIMESTAMPTZ      DEFAULT now(),
    started_at      TIMESTAMPTZ,
    completed_at    TIMESTAMPTZ,
    result          JSONB
);

-- Migration 020: execute_after for delayed task scheduling (covalence#67).
ALTER TABLE covalence.slow_path_queue
    ADD COLUMN IF NOT EXISTS execute_after TIMESTAMPTZ NULL;

CREATE INDEX IF NOT EXISTS idx_slow_path_queue_execute_after
    ON covalence.slow_path_queue (execute_after)
    WHERE execute_after IS NOT NULL;

-- Idempotently refresh the task_type CHECK constraint so that new task types
-- (e.g. 'reconsolidate', 'consolidate_article') are accepted even on databases
-- created before this update.
ALTER TABLE covalence.slow_path_queue
    DROP CONSTRAINT IF EXISTS slow_path_queue_task_type_check;
ALTER TABLE covalence.slow_path_queue
    ADD CONSTRAINT slow_path_queue_task_type_check
    CHECK (task_type IN (
        'compile', 'infer_edges', 'resolve_contention',
        'split', 'merge', 'embed', 'contention_check',
        'tree_index', 'tree_embed', 'recompile',
        'decay_check', 'divergence_scan', 'recompute_graph_embeddings',
        'reconsolidate', 'consolidate_article'
    ));

-- -----------------------------------------------------------------------------
-- inference_log
-- (inputs/output carry DEFAULT '{}' so the engine's newer input_node_ids-based
--  inserts that omit those columns still satisfy NOT NULL.)
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.inference_log (
    id                  UUID             PRIMARY KEY DEFAULT gen_random_uuid(),
    operation           TEXT             NOT NULL,
    inputs              JSONB            NOT NULL DEFAULT '{}',
    output              JSONB            NOT NULL DEFAULT '{}',
    input_node_ids      UUID[],
    input_summary       TEXT,
    output_decision     TEXT,
    output_confidence   DOUBLE PRECISION,
    output_rationale    TEXT,
    model               TEXT,
    latency_ms          INTEGER,
    created_at          TIMESTAMPTZ      DEFAULT now()
);

-- -----------------------------------------------------------------------------
-- sessions
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.sessions (
    id              UUID             PRIMARY KEY DEFAULT gen_random_uuid(),
    label           TEXT             UNIQUE,
    created_at      TIMESTAMPTZ      DEFAULT now(),
    last_active_at  TIMESTAMPTZ      DEFAULT now(),
    metadata        JSONB            DEFAULT '{}',
    status          TEXT             DEFAULT 'active'
                                     CHECK (status IN ('active', 'expired', 'closed'))
);

-- migration 011: add platform/channel/parent fields (idempotent)
ALTER TABLE covalence.sessions ADD COLUMN IF NOT EXISTS platform TEXT;
ALTER TABLE covalence.sessions ADD COLUMN IF NOT EXISTS channel TEXT;
ALTER TABLE covalence.sessions ADD COLUMN IF NOT EXISTS parent_session_id UUID REFERENCES covalence.sessions(id);

-- -----------------------------------------------------------------------------
-- session_nodes
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.session_nodes (
    session_id      UUID             REFERENCES covalence.sessions(id),
    node_id         UUID             REFERENCES covalence.nodes(id),
    first_accessed  TIMESTAMPTZ      DEFAULT now(),
    access_count    INTEGER          DEFAULT 1,
    PRIMARY KEY (session_id, node_id)
);

-- -----------------------------------------------------------------------------
-- node_sections
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.node_sections (
    id              UUID             PRIMARY KEY DEFAULT gen_random_uuid(),
    node_id         UUID             NOT NULL
                                     REFERENCES covalence.nodes(id) ON DELETE CASCADE,
    tree_path       TEXT             NOT NULL,
    depth           INT              NOT NULL DEFAULT 0,
    title           TEXT,
    summary         TEXT,
    start_char      INT              NOT NULL,
    end_char        INT              NOT NULL,
    content_hash    TEXT,
    embedding       halfvec(1536),
    model           TEXT             DEFAULT 'text-embedding-3-small',
    created_at      TIMESTAMPTZ      DEFAULT now(),
    namespace       TEXT             NOT NULL DEFAULT 'default',
    CONSTRAINT node_sections_unique UNIQUE (node_id, tree_path)
);

-- -----------------------------------------------------------------------------
-- standing_concerns
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.standing_concerns (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT        NOT NULL UNIQUE,
    status      TEXT        NOT NULL CHECK (status IN ('green', 'yellow', 'red')),
    notes       TEXT,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- =============================================================================
-- INDEXES
-- =============================================================================

-- HNSW for approximate nearest-neighbour semantic search
CREATE INDEX IF NOT EXISTS node_embeddings_hnsw_idx
    ON covalence.node_embeddings
    USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

CREATE INDEX IF NOT EXISTS node_sections_hnsw_idx
    ON covalence.node_sections
    USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- GIN indexes
CREATE INDEX IF NOT EXISTS nodes_metadata_gin_idx
    ON covalence.nodes USING gin (metadata);

CREATE INDEX IF NOT EXISTS nodes_content_tsv_gin_idx
    ON covalence.nodes USING gin (content_tsv);

CREATE INDEX IF NOT EXISTS nodes_facet_function_gin_idx
    ON covalence.nodes USING gin (facet_function)
    WHERE facet_function IS NOT NULL;

CREATE INDEX IF NOT EXISTS nodes_facet_scope_gin_idx
    ON covalence.nodes USING gin (facet_scope)
    WHERE facet_scope IS NOT NULL;

-- B-tree indexes
CREATE INDEX IF NOT EXISTS nodes_type_status_idx
    ON covalence.nodes (node_type, status);

CREATE INDEX IF NOT EXISTS nodes_active_status_idx
    ON covalence.nodes (status)
    WHERE status = 'active';

CREATE INDEX IF NOT EXISTS edges_source_node_idx
    ON covalence.edges (source_node_id);

CREATE INDEX IF NOT EXISTS edges_target_node_idx
    ON covalence.edges (target_node_id);

CREATE INDEX IF NOT EXISTS edges_edge_type_idx
    ON covalence.edges (edge_type);

CREATE INDEX IF NOT EXISTS contentions_status_idx
    ON covalence.contentions (status);

CREATE INDEX IF NOT EXISTS slow_path_queue_status_priority_idx
    ON covalence.slow_path_queue (status, priority);

CREATE INDEX IF NOT EXISTS usage_traces_node_accessed_idx
    ON covalence.usage_traces (node_id, accessed_at);

CREATE INDEX IF NOT EXISTS inference_log_operation_created_idx
    ON covalence.inference_log (operation, created_at DESC);

CREATE INDEX IF NOT EXISTS sessions_label_idx
    ON covalence.sessions (label);

CREATE INDEX IF NOT EXISTS sessions_status_idx
    ON covalence.sessions (status);

-- -----------------------------------------------------------------------------
-- session_messages
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.session_messages (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id  UUID        NOT NULL REFERENCES covalence.sessions(id) ON DELETE CASCADE,
    speaker     TEXT,
    role        TEXT        NOT NULL,
    content     TEXT        NOT NULL,
    chunk_index INTEGER,
    created_at  TIMESTAMPTZ DEFAULT now(),
    flushed_at  TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS session_messages_session_id_created_at_idx
    ON covalence.session_messages (session_id, created_at);
CREATE INDEX IF NOT EXISTS session_messages_unflushed_idx
    ON covalence.session_messages (session_id)
    WHERE flushed_at IS NULL;

CREATE INDEX IF NOT EXISTS node_sections_node_id_idx
    ON covalence.node_sections (node_id);

CREATE INDEX IF NOT EXISTS node_sections_depth_idx
    ON covalence.node_sections (depth);

-- covalence#47: Namespace isolation indexes
CREATE INDEX IF NOT EXISTS nodes_namespace_idx
    ON covalence.nodes (namespace);

CREATE INDEX IF NOT EXISTS nodes_namespace_status_idx
    ON covalence.nodes (namespace, status);

CREATE INDEX IF NOT EXISTS edges_namespace_idx
    ON covalence.edges (namespace);

CREATE INDEX IF NOT EXISTS node_embeddings_namespace_idx
    ON covalence.node_embeddings (namespace);

CREATE INDEX IF NOT EXISTS node_sections_namespace_idx
    ON covalence.node_sections (namespace);

CREATE INDEX IF NOT EXISTS usage_traces_namespace_idx
    ON covalence.usage_traces (namespace);

CREATE INDEX IF NOT EXISTS contentions_namespace_idx
    ON covalence.contentions (namespace);

-- Migration 022: UNDERCUTS + contention_type (covalence#87)
-- Idempotent: safe to run against existing test databases.
ALTER TABLE covalence.contentions
    ADD COLUMN IF NOT EXISTS contention_type TEXT NOT NULL DEFAULT 'rebuttal'
        CHECK (contention_type IN ('rebuttal', 'undermining', 'undercutting'));

-- Migration 023: content_hash (SHA-256) for tamper detection (covalence#78)
-- The column already appears in the CREATE TABLE above; this ensures existing
-- test databases (created before 023) also have the column.
ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS content_hash TEXT;

CREATE INDEX IF NOT EXISTS idx_nodes_content_hash
    ON covalence.nodes (content_hash)
    WHERE content_hash IS NOT NULL;

-- Migration 025: OCC Phase 0 (covalence#98)
-- Harden version column + deduplicate contentions at DB level.

-- 1. Back-fill any NULL version values, then enforce NOT NULL.
UPDATE covalence.nodes SET version = 1 WHERE version IS NULL;
ALTER TABLE covalence.nodes ALTER COLUMN version SET NOT NULL;
ALTER TABLE covalence.nodes ALTER COLUMN version SET DEFAULT 1;

-- 2. UNIQUE constraint on (node_id, source_node_id) for contentions.
--    Drop first for idempotency, then re-add.
ALTER TABLE covalence.contentions
    DROP CONSTRAINT IF EXISTS contentions_article_source_uniq;
ALTER TABLE covalence.contentions
    ADD CONSTRAINT contentions_article_source_uniq
        UNIQUE (node_id, source_node_id);

-- Migration 026: KG inference rules (covalence#99)
-- contends_derived materialized view: A CONFIRMS B ∧ B CONTRADICTS C → (A, C)
CREATE MATERIALIZED VIEW IF NOT EXISTS covalence.contends_derived AS
SELECT
    e1.source_node_id  AS node_a_id,
    e2.target_node_id  AS node_c_id,
    e1.id              AS source_edge_1_id,
    e2.id              AS source_edge_2_id
FROM  covalence.edges e1
JOIN  covalence.edges e2
      ON  e1.target_node_id = e2.source_node_id
WHERE e1.edge_type = 'CONFIRMS'
  AND e2.edge_type = 'CONTRADICTS'
  AND e1.valid_to  IS NULL
  AND e2.valid_to  IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS contends_derived_edges_uniq
    ON covalence.contends_derived (source_edge_1_id, source_edge_2_id);

CREATE INDEX IF NOT EXISTS contends_derived_node_a_idx
    ON covalence.contends_derived (node_a_id);

CREATE INDEX IF NOT EXISTS contends_derived_node_c_idx
    ON covalence.contends_derived (node_c_id);

CREATE INDEX IF NOT EXISTS contends_derived_node_a_c_idx
    ON covalence.contends_derived (node_a_id, node_c_id);

-- =============================================================================
-- PERMISSIONS
-- =============================================================================

GRANT USAGE ON SCHEMA covalence TO covalence;
GRANT ALL ON ALL TABLES IN SCHEMA covalence TO covalence;
GRANT ALL ON ALL SEQUENCES IN SCHEMA covalence TO covalence;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA covalence TO covalence;

-- Migration 027: Persistent gap registry (covalence#100)
CREATE TABLE IF NOT EXISTS covalence.gap_log (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    query        TEXT        NOT NULL,
    top_score    FLOAT,
    result_count INTEGER,
    session_id   TEXT,
    namespace    TEXT        NOT NULL DEFAULT 'default',
    created_at   TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX IF NOT EXISTS gap_log_created_at_idx  ON covalence.gap_log (created_at DESC);
CREATE INDEX IF NOT EXISTS gap_log_namespace_idx   ON covalence.gap_log (namespace);
CREATE INDEX IF NOT EXISTS gap_log_query_idx       ON covalence.gap_log (lower(trim(query)));

CREATE TABLE IF NOT EXISTS covalence.gap_registry (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    topic           TEXT        NOT NULL,
    namespace       TEXT        NOT NULL DEFAULT 'default',
    query_count     INTEGER     DEFAULT 0,
    avg_top_score   FLOAT,
    last_queried_at TIMESTAMPTZ,
    gap_score       FLOAT,
    status          TEXT        DEFAULT 'open',
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now(),
    UNIQUE (topic, namespace)
);

CREATE INDEX IF NOT EXISTS gap_registry_gap_score_idx  ON covalence.gap_registry (gap_score DESC);
CREATE INDEX IF NOT EXISTS gap_registry_status_idx     ON covalence.gap_registry (status);
CREATE INDEX IF NOT EXISTS gap_registry_namespace_idx  ON covalence.gap_registry (namespace);

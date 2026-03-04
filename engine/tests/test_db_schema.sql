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

-- Migration 033: causal semantics (covalence#75).
ALTER TABLE covalence.edges ADD COLUMN IF NOT EXISTS causal_weight FLOAT NOT NULL DEFAULT 0.5;
ALTER TABLE covalence.nodes ADD COLUMN IF NOT EXISTS provenance_confidence FLOAT;

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
        'reconsolidate', 'consolidate_article', 'critique_article'
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

-- Migration 028: EWC structural importance (covalence#101)
ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS structural_importance FLOAT DEFAULT 0.0;

CREATE INDEX IF NOT EXISTS idx_nodes_structural_importance
    ON covalence.nodes(structural_importance);

-- Migration 030: KB Navigation Landmarks (covalence#112)
ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS is_landmark BOOLEAN NOT NULL DEFAULT false;

CREATE INDEX IF NOT EXISTS idx_nodes_is_landmark
    ON covalence.nodes(is_landmark)
    WHERE is_landmark = true;

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
    structural_score FLOAT      NOT NULL DEFAULT 0.0,
    horizon_score   FLOAT       NOT NULL DEFAULT 0.0,
    status          TEXT        DEFAULT 'open',
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now(),
    UNIQUE (topic, namespace)
);

CREATE INDEX IF NOT EXISTS gap_registry_gap_score_idx  ON covalence.gap_registry (gap_score DESC);
CREATE INDEX IF NOT EXISTS gap_registry_status_idx     ON covalence.gap_registry (status);
CREATE INDEX IF NOT EXISTS gap_registry_namespace_idx  ON covalence.gap_registry (namespace);

-- Migration 032: Gap Registry Phase 2 — structural and horizon scores (covalence#120)
ALTER TABLE covalence.gap_registry
  ADD COLUMN IF NOT EXISTS structural_score FLOAT NOT NULL DEFAULT 0.0,
  ADD COLUMN IF NOT EXISTS horizon_score FLOAT NOT NULL DEFAULT 0.0;

-- Migration 031: Task state machine (covalence#114)
CREATE TABLE IF NOT EXISTS covalence.tasks (
  id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
  label               TEXT        NOT NULL,
  issue_ref           TEXT,
  status              TEXT        NOT NULL DEFAULT 'pending'
                                  CHECK (status IN ('pending','assigned','running','done','failed')),
  assigned_session_id TEXT,
  started_at          TIMESTAMPTZ,
  completed_at        TIMESTAMPTZ,
  timeout_at          TIMESTAMPTZ,
  failure_class       TEXT,
  result_summary      TEXT,
  metadata            JSONB       NOT NULL DEFAULT '{}',
  created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE OR REPLACE FUNCTION covalence.tasks_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
  NEW.updated_at = now();
  RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_tasks_updated_at ON covalence.tasks;
CREATE TRIGGER trg_tasks_updated_at
  BEFORE UPDATE ON covalence.tasks
  FOR EACH ROW EXECUTE FUNCTION covalence.tasks_set_updated_at();

CREATE INDEX IF NOT EXISTS idx_tasks_status      ON covalence.tasks (status);
CREATE INDEX IF NOT EXISTS idx_tasks_created_at  ON covalence.tasks (created_at DESC);

-- Migration 033: Causal semantics (covalence#75)
ALTER TABLE covalence.edges
    ADD COLUMN IF NOT EXISTS causal_weight FLOAT NOT NULL DEFAULT 0.5;

ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS provenance_confidence FLOAT;

-- Migration 034: edge_causal_metadata (covalence#116)
DO $$ BEGIN
  CREATE TYPE covalence.causal_level_enum AS ENUM (
    'association', 'intervention', 'counterfactual'
  );
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
  CREATE TYPE covalence.causal_evidence_type_enum AS ENUM (
    'structural_prior', 'expert_assertion', 'statistical', 'experimental',
    'granger_temporal', 'llm_extracted', 'domain_rule'
  );
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

CREATE TABLE IF NOT EXISTS covalence.edge_causal_metadata (
    edge_id           UUID                                  NOT NULL PRIMARY KEY,
    causal_level      covalence.causal_level_enum           NOT NULL DEFAULT 'association',
    causal_strength   FLOAT                                 NOT NULL DEFAULT 0.5
                        CHECK (causal_strength >= 0.0 AND causal_strength <= 1.0),
    evidence_type     covalence.causal_evidence_type_enum   NOT NULL DEFAULT 'structural_prior',
    direction_conf    FLOAT                                 NOT NULL DEFAULT 0.5
                        CHECK (direction_conf >= 0.0 AND direction_conf <= 1.0),
    hidden_conf_risk  FLOAT                                 NOT NULL DEFAULT 0.5
                        CHECK (hidden_conf_risk >= 0.0 AND hidden_conf_risk <= 1.0),
    temporal_lag_ms   INT                                   CHECK (temporal_lag_ms IS NULL OR temporal_lag_ms >= 0),
    notes             TEXT,
    created_at        TIMESTAMPTZ                           NOT NULL DEFAULT NOW(),
    updated_at        TIMESTAMPTZ                           NOT NULL DEFAULT NOW(),

    CONSTRAINT fk_ecm_edge
        FOREIGN KEY (edge_id)
        REFERENCES covalence.edges(id)
        ON DELETE CASCADE
        DEFERRABLE INITIALLY DEFERRED
);

CREATE INDEX IF NOT EXISTS idx_ecm_causal_level
    ON covalence.edge_causal_metadata (causal_level);

CREATE INDEX IF NOT EXISTS idx_ecm_causal_strength
    ON covalence.edge_causal_metadata (causal_strength DESC);

CREATE INDEX IF NOT EXISTS idx_ecm_evidence_type
    ON covalence.edge_causal_metadata (evidence_type);

CREATE INDEX IF NOT EXISTS idx_ecm_level_strength
    ON covalence.edge_causal_metadata (causal_level, causal_strength DESC);

CREATE OR REPLACE FUNCTION covalence._ecm_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_ecm_updated_at ON covalence.edge_causal_metadata;
CREATE TRIGGER trg_ecm_updated_at
    BEFORE UPDATE ON covalence.edge_causal_metadata
    FOR EACH ROW EXECUTE FUNCTION covalence._ecm_set_updated_at();

-- =============================================================================
-- Migration 035: search_intent_enum (mirrors migration 035)
-- =============================================================================
DO $$ BEGIN
  CREATE TYPE covalence.search_intent_enum AS ENUM (
    'factual',
    'temporal',
    'causal',
    'entity'
  );
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- =============================================================================
-- Function: graph_traverse (mirrors migration 035)
-- =============================================================================
-- Recursive CTE breadth-first search from a set of start nodes.
--
-- Parameters:
--   p_start_nodes   — anchor node UUIDs to begin traversal from
--   p_max_hops      — maximum hop depth (1–3; hard-capped in Rust)
--   p_min_weight    — minimum causal_weight threshold (COALESCE NULL → 0.0)
--   p_intent_filter — optional intent name ('factual'|'temporal'|'causal'|'entity');
--                     NULL means traverse all edge types
--
-- Returns TABLE (node_id, edge_id, hop_depth, causal_weight, edge_type).

CREATE OR REPLACE FUNCTION covalence.graph_traverse(
    p_start_nodes   UUID[],
    p_max_hops      INT     DEFAULT 1,
    p_min_weight    FLOAT8  DEFAULT 0.0,
    p_intent_filter TEXT    DEFAULT NULL
)
RETURNS TABLE (
    node_id       UUID,
    edge_id       UUID,
    hop_depth     INT,
    causal_weight FLOAT8,
    edge_type     TEXT
)
LANGUAGE plpgsql AS $$
DECLARE
    v_namespace TEXT;
BEGIN
    -- Guard: empty array → no results
    IF p_start_nodes IS NULL OR array_length(p_start_nodes, 1) IS NULL THEN
        RETURN;
    END IF;

    -- Derive namespace from the first start node for edge scoping
    SELECT n.namespace INTO v_namespace
    FROM covalence.nodes n
    WHERE n.id = p_start_nodes[1];

    RETURN QUERY
    WITH RECURSIVE bfs(
        node_id,
        edge_id,
        hop_depth,
        causal_weight,
        edge_type,
        visited
    ) AS (
        -- ── Base case: seed with all start nodes at depth 0 ────────────────
        SELECT
            n.id          AS node_id,
            NULL::UUID    AS edge_id,
            0             AS hop_depth,
            NULL::FLOAT8  AS causal_weight,
            NULL::TEXT    AS edge_type,
            ARRAY[n.id]   AS visited
        FROM covalence.nodes n
        WHERE n.id = ANY(p_start_nodes)

        UNION ALL

        -- ── Recursive case: one hop from current frontier ──────────────────
        SELECT
            CASE WHEN e.source_node_id = bfs.node_id
                 THEN e.target_node_id
                 ELSE e.source_node_id END            AS node_id,
            e.id                                      AS edge_id,
            bfs.hop_depth + 1                         AS hop_depth,
            COALESCE(e.causal_weight, 0.0)            AS causal_weight,
            e.edge_type                               AS edge_type,
            bfs.visited || (
                CASE WHEN e.source_node_id = bfs.node_id
                     THEN e.target_node_id
                     ELSE e.source_node_id END
            )                                         AS visited
        FROM bfs
        JOIN covalence.edges e ON (
            e.source_node_id = bfs.node_id OR
            e.target_node_id = bfs.node_id
        )
        WHERE bfs.hop_depth < p_max_hops
          AND e.valid_to IS NULL
          AND e.namespace = v_namespace
          AND COALESCE(e.causal_weight, 0.0) >= p_min_weight
          -- Intent-based edge filtering (maps intent name → allowed edge types)
          AND (
              p_intent_filter IS NULL
              OR (p_intent_filter = 'factual'  AND e.edge_type IN ('CONFIRMS', 'ORIGINATES', 'COMPILED_FROM'))
              OR (p_intent_filter = 'temporal' AND e.edge_type IN ('PRECEDES', 'FOLLOWS'))
              OR (p_intent_filter = 'causal'   AND e.edge_type IN ('CAUSES', 'MOTIVATED_BY', 'IMPLEMENTS'))
              OR (p_intent_filter = 'entity'   AND e.edge_type IN ('INVOLVES', 'CAPTURED_IN'))
          )
          -- Cycle detection: skip nodes already on this path
          AND NOT (
              CASE WHEN e.source_node_id = bfs.node_id
                   THEN e.target_node_id
                   ELSE e.source_node_id END
          ) = ANY(bfs.visited)
    )
    -- Deduplicate by node_id: keep shortest-path (min hop_depth) entry
    SELECT DISTINCT ON (bfs.node_id)
        bfs.node_id,
        bfs.edge_id,
        bfs.hop_depth,
        bfs.causal_weight,
        bfs.edge_type
    FROM bfs
    -- Only active nodes in the same namespace
    JOIN covalence.nodes n ON n.id = bfs.node_id
                           AND n.status    = 'active'
                           AND n.namespace = v_namespace
    WHERE bfs.hop_depth > 0
    ORDER BY bfs.node_id, bfs.hop_depth ASC;
END;
$$;

-- =============================================================================
-- Procedure: process_queue_cleanup (mirrors migration 035)
-- =============================================================================
-- Two-pass DELETE that removes stale failed slow_path_queue entries.
--
-- Pass A — failed embed tasks whose node now has an embedding.
-- Pass B — failed tasks with a NULL node_id (unretriable).
--
-- Parameters:
--   p_stale_hours_embed     — age threshold (hours) for Pass A (default 1.0)
--   p_stale_hours_null_node — age threshold (hours) for Pass B (default 24.0)

CREATE OR REPLACE PROCEDURE covalence.process_queue_cleanup(
    p_stale_hours_embed      FLOAT8 DEFAULT 1.0,
    p_stale_hours_null_node  FLOAT8 DEFAULT 24.0
)
LANGUAGE plpgsql AS $$
DECLARE
    v_embed_deleted     INT;
    v_null_node_deleted INT;
BEGIN
    -- ── Pass A: stale failed embed tasks for nodes that now have embeddings ──
    DELETE FROM covalence.slow_path_queue
    WHERE task_type = 'embed'
      AND status    = 'failed'
      AND node_id IS NOT NULL
      AND COALESCE(completed_at, started_at, created_at) <
              now() - (p_stale_hours_embed * interval '1 hour')
      AND EXISTS (
          SELECT 1
          FROM   covalence.node_embeddings ne
          WHERE  ne.node_id = slow_path_queue.node_id
      );

    GET DIAGNOSTICS v_embed_deleted = ROW_COUNT;
    RAISE NOTICE 'process_queue_cleanup: pass A deleted % stale embed jobs (node now embedded)',
        v_embed_deleted;

    -- ── Pass B: stale failed tasks with a NULL node_id (unretriable) ─────────
    DELETE FROM covalence.slow_path_queue
    WHERE status  = 'failed'
      AND node_id IS NULL
      AND COALESCE(completed_at, started_at, created_at) <
              now() - (p_stale_hours_null_node * interval '1 hour');

    GET DIAGNOSTICS v_null_node_deleted = ROW_COUNT;
    RAISE NOTICE 'process_queue_cleanup: pass B deleted % stale null-node failed jobs',
        v_null_node_deleted;
END;
$$;

-- =============================================================================
-- Function: upsert_causal_metadata (mirrors migration 035)
-- =============================================================================
CREATE OR REPLACE FUNCTION covalence.upsert_causal_metadata(
    p_edge_id           UUID,
    p_causal_level      covalence.causal_level_enum          DEFAULT NULL,
    p_evidence_type     covalence.causal_evidence_type_enum  DEFAULT NULL,
    p_causal_strength   FLOAT8                               DEFAULT NULL,
    p_direction_conf    FLOAT8                               DEFAULT NULL,
    p_hidden_conf_risk  FLOAT8                               DEFAULT NULL,
    p_temporal_lag_ms   INT                                  DEFAULT NULL,
    p_notes             TEXT                                 DEFAULT NULL
)
RETURNS SETOF covalence.edge_causal_metadata
LANGUAGE plpgsql AS $$
BEGIN
    RETURN QUERY
    INSERT INTO covalence.edge_causal_metadata
        (edge_id, causal_level, causal_strength, evidence_type,
         direction_conf, hidden_conf_risk, temporal_lag_ms, notes)
    VALUES (
        p_edge_id,
        COALESCE(p_causal_level,    'association'::covalence.causal_level_enum),
        COALESCE(p_causal_strength, 0.5),
        COALESCE(p_evidence_type,   'structural_prior'::covalence.causal_evidence_type_enum),
        COALESCE(p_direction_conf,   0.5),
        COALESCE(p_hidden_conf_risk, 0.5),
        p_temporal_lag_ms,
        p_notes
    )
    ON CONFLICT (edge_id) DO UPDATE SET
        causal_level     = COALESCE(p_causal_level,    edge_causal_metadata.causal_level),
        causal_strength  = COALESCE(p_causal_strength, edge_causal_metadata.causal_strength),
        evidence_type    = COALESCE(p_evidence_type,   edge_causal_metadata.evidence_type),
        direction_conf   = COALESCE(p_direction_conf,  edge_causal_metadata.direction_conf),
        hidden_conf_risk = COALESCE(p_hidden_conf_risk, edge_causal_metadata.hidden_conf_risk),
        temporal_lag_ms  = COALESCE(p_temporal_lag_ms, edge_causal_metadata.temporal_lag_ms),
        notes            = COALESCE(p_notes,            edge_causal_metadata.notes),
        updated_at       = NOW()
    RETURNING *;
END;
$$;

-- =============================================================================
-- Covalence: Graph-Native Knowledge Substrate
-- Migration: 001_initial_schema.sql
-- Description: Initial schema — AGE graph labels + relational metadata tables
-- =============================================================================

-- =============================================================================
-- SECTION 1: EXTENSIONS
-- =============================================================================

CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS age;

-- =============================================================================
-- SECTION 2: SCHEMA
-- All Covalence relational tables live in the 'covalence' schema, isolated from
-- Valence v2's 'public' schema for coexistence during migration.
-- =============================================================================

CREATE SCHEMA IF NOT EXISTS covalence;

-- =============================================================================
-- SECTION 3: AGE GRAPH SETUP
-- The Apache AGE property graph 'covalence' stores all typed edges and provides
-- openCypher traversal. AGE is canonical for topology; the relational 'edges'
-- table is a SQL mirror for fast non-traversal queries.
--
-- DO blocks with exception handlers make the migration re-entrant:
-- already-existing graphs/labels are silently skipped.
-- =============================================================================

LOAD 'age';
SET search_path = ag_catalog, "$user", public;

-- Create the covalence graph (no-op if already exists)
DO $$
BEGIN
    PERFORM ag_catalog.create_graph('covalence');
EXCEPTION
    WHEN SQLSTATE '42P04' THEN NULL;
    WHEN others THEN NULL;
END;
$$;

-- Vertex labels:
--   Article : compiled, mutable knowledge units (LLM-synthesized from sources)
--   Source  : raw, immutable input material (docs, conversations, web pages)
--   Entity  : canonical named entities (v1 feature; label created now for topology)
DO $$
DECLARE
    v_labels TEXT[] := ARRAY['Article', 'Source', 'Entity'];
    lbl TEXT;
BEGIN
    FOREACH lbl IN ARRAY v_labels LOOP
        BEGIN
            PERFORM ag_catalog.create_vlabel('covalence', lbl);
        EXCEPTION WHEN others THEN NULL;
        END;
    END LOOP;
END;
$$;

-- Edge labels — the initial typed relationship vocabulary.
-- New types can be added without schema migration (just create_elabel + extend CHECK).
--
--   SUPERSEDES    : This node replaces/supersedes another node
--   SPLIT_FROM    : This article was produced by splitting a parent article
--   COMPILED_FROM : This article was compiled from this source
--   CONFIRMS      : Source corroborates an existing article claim
--   CONTRADICTS   : These nodes make conflicting claims (triggers contention)
--   CONTENDS      : Softer disagreement; alternative interpretation
--   RELATES_TO    : Generic semantic relatedness (non-directional)
--   ELABORATES    : This node expands on another without superseding it
--   GENERALIZES   : This node abstracts a more specific node
--   PRECEDES      : Temporal ordering: this node is earlier than target
--   FOLLOWS       : Temporal ordering: this node is later than target
--   INVOLVES      : Node references a named entity node
DO $$
DECLARE
    e_labels TEXT[] := ARRAY[
        'SUPERSEDES', 'SPLIT_FROM', 'COMPILED_FROM', 'CONFIRMS',
        'CONTRADICTS', 'CONTENDS', 'RELATES_TO', 'ELABORATES',
        'GENERALIZES', 'PRECEDES', 'FOLLOWS', 'INVOLVES'
    ];
    lbl TEXT;
BEGIN
    FOREACH lbl IN ARRAY e_labels LOOP
        BEGIN
            PERFORM ag_catalog.create_elabel('covalence', lbl);
        EXCEPTION WHEN others THEN NULL;
        END;
    END LOOP;
END;
$$;

-- Restore standard search path for the relational section
SET search_path = "$user", public;

-- =============================================================================
-- SECTION 4: RELATIONAL TABLES
-- =============================================================================

-- -----------------------------------------------------------------------------
-- TABLE: covalence.nodes
-- Unified relational store for all graph vertices (articles, sources, entities).
-- Every AGE vertex has a corresponding row here; age_id provides the bridge.
-- Rich metadata (content, confidence, FTS) lives here for query efficiency.
-- Valid node_type values: 'article' | 'source' | 'entity'
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.nodes (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    age_id          BIGINT,

    -- Classification (CHECK documents valid values)
    node_type       TEXT        NOT NULL
                    CONSTRAINT nodes_node_type_check
                    CHECK (node_type IN ('article', 'source', 'entity')),

    title           TEXT,
    content         TEXT,

    -- Lifecycle: 'active' | 'archived' | 'tombstone'
    status          TEXT        DEFAULT 'active'
                    CONSTRAINT nodes_status_check
                    CHECK (status IN ('active', 'archived', 'tombstone')),

    -- Confidence decomposition — single canonical representation per node.
    -- Eliminates the three-representation drift problem in Valence v2.
    confidence_overall          FLOAT,
    confidence_source           FLOAT,
    confidence_method           FLOAT,
    confidence_consistency      FLOAT,
    confidence_freshness        FLOAT,
    confidence_corroboration    FLOAT,
    confidence_applicability    FLOAT,

    -- Knowledge classification: 'semantic' | 'episodic' | 'procedural' | 'declarative'
    epistemic_type  TEXT
                    CONSTRAINT nodes_epistemic_type_check
                    CHECK (epistemic_type IN ('semantic', 'episodic', 'procedural', 'declarative')),

    domain_path     TEXT[],
    metadata        JSONB       DEFAULT '{}',

    -- Source-specific fields (populated when node_type = 'source')
    source_type     TEXT,       -- document|conversation|web|code|observation|tool_output|user_input
    reliability     FLOAT,

    content_hash    TEXT,
    fingerprint     TEXT,
    size_tokens     INTEGER,
    pinned          BOOLEAN     DEFAULT false,
    version         INTEGER     DEFAULT 1,

    created_at      TIMESTAMPTZ DEFAULT now(),
    modified_at     TIMESTAMPTZ DEFAULT now(),
    accessed_at     TIMESTAMPTZ DEFAULT now(),
    archived_at     TIMESTAMPTZ,

    -- Retrieval frequency x recency; drives organic eviction
    usage_score     FLOAT       DEFAULT 0.5,

    -- Generated tsvector for FTS (always current, zero application logic required)
    content_tsv     TSVECTOR    GENERATED ALWAYS AS (
                        to_tsvector('english',
                            COALESCE(title, '') || ' ' || COALESCE(content, ''))
                    ) STORED
);

COMMENT ON TABLE covalence.nodes IS
    'Unified node table for all Covalence graph vertices (articles, sources, entities). '
    'Mirrors AGE vertices with structured relational metadata. AGE is canonical for graph '
    'topology; this table is canonical for content, confidence, and structured metadata.';

COMMENT ON COLUMN covalence.nodes.content_tsv IS
    'Generated tsvector for native PostgreSQL FTS (ts_rank fallback). GIN-indexed.';

COMMENT ON COLUMN covalence.nodes.usage_score IS
    'Retrieval frequency x recency. Drives organic eviction at capacity.';

-- -----------------------------------------------------------------------------
-- TABLE: covalence.edges
-- SQL mirror of AGE graph edges for fast non-traversal queries.
-- AGE is canonical for Cypher traversal; this table enables O(log n) SQL lookups.
-- Valid edge_type values documented in the CHECK constraint below.
-- To add a new type: (1) create_elabel in AGE, (2) extend the CHECK.
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.edges (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    age_id          BIGINT,

    source_node_id  UUID        REFERENCES covalence.nodes(id),
    target_node_id  UUID        REFERENCES covalence.nodes(id),

    -- Initial edge vocabulary enforced by CHECK; extensible without data migration.
    edge_type       TEXT        NOT NULL
                    CONSTRAINT edges_edge_type_check
                    CHECK (edge_type IN (
                        'SUPERSEDES',       -- replaces another node
                        'SPLIT_FROM',       -- produced by splitting a parent
                        'COMPILED_FROM',    -- article compiled from source
                        'CONFIRMS',         -- corroborates an article claim
                        'CONTRADICTS',      -- conflicting claims (triggers contention)
                        'CONTENDS',         -- softer disagreement / alternative view
                        'RELATES_TO',       -- generic semantic relatedness
                        'ELABORATES',       -- expands without superseding
                        'GENERALIZES',      -- abstracts a more specific node
                        'PRECEDES',         -- temporal: this node is earlier
                        'FOLLOWS',          -- temporal: this node is later
                        'INVOLVES'          -- references a named entity node
                    )),

    weight          FLOAT       DEFAULT 1.0,
    confidence      FLOAT       DEFAULT 1.0,
    metadata        JSONB       DEFAULT '{}',
    created_at      TIMESTAMPTZ DEFAULT now(),
    created_by      TEXT        -- 'agent' | 'system' | 'user'
);

COMMENT ON TABLE covalence.edges IS
    'SQL mirror of AGE graph edges. Enables fast O(log n) edge lookups without Cypher. '
    'Written synchronously whenever an AGE edge is created. AGE is canonical for traversal. '
    'Extend the edge_type CHECK and call create_elabel to add new relationship types.';

-- -----------------------------------------------------------------------------
-- TABLE: covalence.node_embeddings
-- Vector embeddings isolated from nodes for clean partial HNSW indexing.
-- halfvec(1536): half-precision = 3 KB/node; ~375 MB at 50K nodes incl. HNSW.
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.node_embeddings (
    node_id         UUID        PRIMARY KEY REFERENCES covalence.nodes(id) ON DELETE CASCADE,
    embedding       halfvec(1536),
    model           TEXT        DEFAULT 'text-embedding-3-small',
    created_at      TIMESTAMPTZ DEFAULT now()
);

COMMENT ON TABLE covalence.node_embeddings IS
    'Vector embeddings for semantic ANN search. Stored separately from nodes for '
    'partial HNSW indexing. halfvec = half-precision, 50% storage vs float32.';

-- -----------------------------------------------------------------------------
-- TABLE: covalence.usage_traces
-- Per-retrieval event log powering usage_score and organic forgetting.
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.usage_traces (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    node_id         UUID        REFERENCES covalence.nodes(id),
    session_id      TEXT,
    query_text      TEXT,
    retrieval_rank  INTEGER,
    accessed_at     TIMESTAMPTZ DEFAULT now()
);

COMMENT ON TABLE covalence.usage_traces IS
    'Per-retrieval event log. Aggregated into nodes.usage_score by maintenance worker. '
    'Drives organic eviction: lowest-score non-pinned nodes archived at capacity.';

-- -----------------------------------------------------------------------------
-- TABLE: covalence.contentions
-- Denormalized contradiction registry. AGE CONTRADICTS/CONTENDS edges are
-- canonical; this table is a query-optimized projection for fast status queries.
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.contentions (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    node_id         UUID        REFERENCES covalence.nodes(id),         -- article node
    source_node_id  UUID        REFERENCES covalence.nodes(id),         -- contradicting node
    edge_id         UUID        REFERENCES covalence.edges(id),         -- CONTRADICTS/CONTENDS edge
    type            TEXT,       -- 'contradiction' | 'contention'
    description     TEXT,
    severity        TEXT,       -- 'low' | 'medium' | 'high'
    status          TEXT        DEFAULT 'detected',  -- 'detected' | 'resolved' | 'dismissed'
    resolution      TEXT,
    materiality     FLOAT,
    detected_at     TIMESTAMPTZ DEFAULT now(),
    resolved_at     TIMESTAMPTZ
);

COMMENT ON TABLE covalence.contentions IS
    'Denormalized contention registry. AGE edges are canonical; this table enables fast '
    'SQL queries over contention status without Cypher traversal overhead. '
    'Retained after resolution for audit and graduation training data.';

-- -----------------------------------------------------------------------------
-- TABLE: covalence.slow_path_queue
-- Async inference work queue for the dual-stream write architecture.
-- Fast path enqueues; background worker dequeues and processes.
-- Provides explicit queue visibility for monitoring and backpressure detection.
-- Valid task_type: 'compile' | 'infer_edges' | 'resolve_contention' | 'split' | 'merge'
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.slow_path_queue (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),

    task_type       TEXT        NOT NULL
                    CONSTRAINT slow_path_queue_task_type_check
                    CHECK (task_type IN (
                        'compile',              -- LLM article compilation
                        'infer_edges',          -- causal/entity edge detection
                        'resolve_contention',   -- contention resolution
                        'split',                -- article split
                        'merge',                -- article merge
                        'embed',                -- vector embedding
                        'contention_check',     -- contention detection
                        'tree_index',           -- hierarchical section indexing
                        'tree_embed'            -- section embedding
                    )),

    node_id         UUID        REFERENCES covalence.nodes(id),
    payload         JSONB       DEFAULT '{}',

    -- 'pending' | 'processing' | 'complete' | 'failed'
    status          TEXT        DEFAULT 'pending'
                    CONSTRAINT slow_path_queue_status_check
                    CHECK (status IN ('pending', 'processing', 'complete', 'failed')),

    priority        INTEGER     DEFAULT 0,   -- higher = more urgent
    created_at      TIMESTAMPTZ DEFAULT now(),
    started_at      TIMESTAMPTZ,
    completed_at    TIMESTAMPTZ,
    result          JSONB
);

COMMENT ON TABLE covalence.slow_path_queue IS
    'Async inference work queue (dual-stream write architecture, MAGMA-validated). '
    'Fast write path enqueues tasks; background worker dequeues by (pending, priority DESC). '
    'Queue depth surfaced in GET /admin/stats. Failed tasks retain result for debugging.';

-- -----------------------------------------------------------------------------
-- TABLE: covalence.inference_log
-- Graduation training dataset. Every slow-path inference decision logged here.
-- Frequent high-confidence patterns promoted to fast-path algorithms in v1.
-- -----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS covalence.inference_log (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    operation       TEXT        NOT NULL,   -- e.g. 'causal_edge_inference'
    inputs          JSONB       NOT NULL,   -- node ids, content excerpts, embeddings
    output          JSONB       NOT NULL,   -- edge_label, confidence, rationale
    model           TEXT,                   -- LLM used, e.g. 'gpt-4o'
    latency_ms      INTEGER,
    created_at      TIMESTAMPTZ DEFAULT now()
);

COMMENT ON TABLE covalence.inference_log IS
    'Graduation training dataset. Every slow-path inference decision is logged here. '
    'At ~500 examples per operation type, patterns are promoted to fast-path algorithms '
    'in v1. See SPEC.md §8.3 for the graduation pathway.';

-- =============================================================================
-- SECTION 5: INDEXES
-- =============================================================================

-- HNSW for approximate nearest neighbor semantic search.
-- halfvec_cosine_ops = cosine similarity. m=16, ef_construction=64.
CREATE INDEX IF NOT EXISTS node_embeddings_hnsw_idx
    ON covalence.node_embeddings
    USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- GIN on metadata JSONB for containment queries (tags, memory flags, etc.)
CREATE INDEX IF NOT EXISTS nodes_metadata_gin_idx
    ON covalence.nodes
    USING gin (metadata);

-- GIN on content_tsv for native PostgreSQL ts_rank FTS
CREATE INDEX IF NOT EXISTS nodes_content_tsv_gin_idx
    ON covalence.nodes
    USING gin (content_tsv);

-- Compound B-tree: (node_type, status) for filtered list queries
CREATE INDEX IF NOT EXISTS nodes_type_status_idx
    ON covalence.nodes (node_type, status);

-- Partial B-tree: active nodes only (smaller, faster than full-table status index)
CREATE INDEX IF NOT EXISTS nodes_active_status_idx
    ON covalence.nodes (status)
    WHERE status = 'active';

-- B-tree on edges for fast SQL edge lookups
CREATE INDEX IF NOT EXISTS edges_source_node_idx
    ON covalence.edges (source_node_id);

CREATE INDEX IF NOT EXISTS edges_target_node_idx
    ON covalence.edges (target_node_id);

CREATE INDEX IF NOT EXISTS edges_edge_type_idx
    ON covalence.edges (edge_type);

-- B-tree on contentions status
CREATE INDEX IF NOT EXISTS contentions_status_idx
    ON covalence.contentions (status);

-- B-tree on slow_path_queue for worker dequeue pattern
CREATE INDEX IF NOT EXISTS slow_path_queue_status_priority_idx
    ON covalence.slow_path_queue (status, priority);

-- B-tree on usage_traces for score aggregation
CREATE INDEX IF NOT EXISTS usage_traces_node_accessed_idx
    ON covalence.usage_traces (node_id, accessed_at);

-- =============================================================================
-- SECTION 6: HELPER FUNCTIONS
-- =============================================================================

-- get_chain_tips(): active article nodes with no incoming SUPERSEDES edge.
-- "Chain tips" are the current authoritative versions of knowledge — the heads
-- of any supersession chain. Use for default article retrieval to exclude
-- superseded articles from results.
--
-- SQL anti-join on edges table avoids Cypher overhead for this common query.
-- Equivalent Cypher: MATCH (a:Article) WHERE NOT ()-[:SUPERSEDES]->(a) RETURN a
CREATE OR REPLACE FUNCTION covalence.get_chain_tips()
RETURNS TABLE (
    id                  UUID,
    title               TEXT,
    node_type           TEXT,
    status              TEXT,
    confidence_overall  FLOAT,
    epistemic_type      TEXT,
    domain_path         TEXT[],
    usage_score         FLOAT,
    created_at          TIMESTAMPTZ,
    modified_at         TIMESTAMPTZ
)
LANGUAGE plpgsql
STABLE
AS $$
BEGIN
    RETURN QUERY
    SELECT
        n.id,
        n.title,
        n.node_type,
        n.status,
        n.confidence_overall,
        n.epistemic_type,
        n.domain_path,
        n.usage_score,
        n.created_at,
        n.modified_at
    FROM covalence.nodes n
    WHERE n.node_type = 'article'
      AND n.status    = 'active'
      AND NOT EXISTS (
          SELECT 1
          FROM   covalence.edges e
          WHERE  e.target_node_id = n.id
            AND  e.edge_type      = 'SUPERSEDES'
      )
    ORDER BY n.usage_score DESC, n.modified_at DESC;
END;
$$;

COMMENT ON FUNCTION covalence.get_chain_tips() IS
    'Returns active article nodes with no incoming SUPERSEDES edge — the current '
    'authoritative head of each knowledge chain. Excludes superseded articles. '
    'Ordered by usage_score DESC, modified_at DESC.';

-- =============================================================================
-- SECTION 7: PERMISSIONS
-- =============================================================================

GRANT USAGE ON SCHEMA covalence TO covalence;
GRANT ALL ON ALL TABLES IN SCHEMA covalence TO covalence;
GRANT ALL ON ALL SEQUENCES IN SCHEMA covalence TO covalence;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA covalence TO covalence;

-- =============================================================================
-- END OF MIGRATION 001_initial_schema.sql
-- =============================================================================
--
-- Objects created:
--   AGE Graph:         covalence
--   AGE Vertex Labels: Article, Source, Entity
--   AGE Edge Labels:   SUPERSEDES, SPLIT_FROM, COMPILED_FROM, CONFIRMS,
--                      CONTRADICTS, CONTENDS, RELATES_TO, ELABORATES,
--                      GENERALIZES, PRECEDES, FOLLOWS, INVOLVES
--
--   Tables (schema: covalence):
--     nodes            — all vertices + FTS tsvector generated column
--     edges            — SQL mirror of AGE edges
--     node_embeddings  — halfvec(1536) embeddings
--     usage_traces     — per-retrieval event log
--     contentions      — detected contradictions
--     slow_path_queue  — async inference work queue
--     inference_log    — graduation training dataset
--
--   Indexes (12):
--     HNSW: node_embeddings(embedding halfvec_cosine_ops, m=16, ef=64)
--     GIN:  nodes(metadata), nodes(content_tsv)
--     B-tree: nodes(node_type,status), nodes(status) WHERE active [partial]
--             edges(source_node_id), edges(target_node_id), edges(edge_type)
--             contentions(status)
--             slow_path_queue(status,priority)
--             usage_traces(node_id,accessed_at)
--
--   Functions:
--     covalence.get_chain_tips() — active articles with no incoming SUPERSEDES
-- =============================================================================

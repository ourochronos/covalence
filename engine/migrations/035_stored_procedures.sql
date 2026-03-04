-- Migration 035: stored procedures — Phase 1 (covalence#143)
--
-- Introduces three PostgreSQL stored procedures / functions that replace
-- dynamic SQL construction in Rust (closes #143, #144, #145, #147).
--
--   1. covalence.graph_traverse   — recursive CTE BFS (fixes #144: SQL injection)
--   2. covalence.upsert_causal_metadata — COALESCE partial upsert (fixes #145)
--   3. covalence.process_queue_cleanup  — two-pass queue DELETE (closes #147 NULL fix)
--
-- Also adds:
--   * search_intent_enum CREATE TYPE (idempotent)
--   * notes TEXT column on edge_causal_metadata

BEGIN;

-- =============================================================================
-- Enum: search_intent_enum
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
-- Schema addition: notes column on edge_causal_metadata
-- =============================================================================

ALTER TABLE covalence.edge_causal_metadata
  ADD COLUMN IF NOT EXISTS notes TEXT DEFAULT NULL;

-- =============================================================================
-- Function: graph_traverse
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
-- Returns TABLE (node_id, edge_id, hop_depth, causal_weight, edge_type):
--   • node_id       — reached neighbor node
--   • edge_id       — edge traversed to reach node (NULL only for seed rows)
--   • hop_depth     — 1-based hop count from any start node
--   • causal_weight — COALESCE(e.causal_weight, 0.0) — never NULL (fixes #144, #147)
--   • edge_type     — edge type label
--
-- Cycle detection uses a visited UUID[] path array; each node is visited at
-- most once per traversal root.  The final output is deduplicated by node_id,
-- keeping the shortest-path entry (minimum hop_depth).
--
-- Namespace is derived from the first start node and applied as an edge filter,
-- keeping traversal within a single namespace boundary.

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
-- Function: upsert_causal_metadata
-- =============================================================================
-- Partial upsert for edge_causal_metadata using COALESCE semantics:
--   • On INSERT: uses the passed value, falling back to the column default.
--   • On UPDATE: COALESCE(new_value, existing_value) — NULL params leave the
--     existing field unchanged, fixing the silent-reset bug (covalence#145).
--
-- Parameters:
--   p_edge_id        — edge to enrich (PK / FK)
--   p_causal_level   — Pearl hierarchy level (NULL → preserve existing / default)
--   p_evidence_type  — evidence classification (NULL → preserve existing / default)
--   p_causal_strength — [0.0, 1.0] strength estimate (NULL → preserve existing / default)
--   p_notes          — free-text annotation (NULL → preserve existing)
--
-- Returns the resulting row (after INSERT or UPDATE).

CREATE OR REPLACE FUNCTION covalence.upsert_causal_metadata(
    p_edge_id         UUID,
    p_causal_level    covalence.causal_level_enum          DEFAULT NULL,
    p_evidence_type   covalence.causal_evidence_type_enum  DEFAULT NULL,
    p_causal_strength FLOAT8                               DEFAULT NULL,
    p_notes           TEXT                                 DEFAULT NULL
)
RETURNS SETOF covalence.edge_causal_metadata
LANGUAGE plpgsql AS $$
BEGIN
    RETURN QUERY
    INSERT INTO covalence.edge_causal_metadata
        (edge_id, causal_level, causal_strength, evidence_type, notes)
    VALUES (
        p_edge_id,
        COALESCE(p_causal_level,    'association'::covalence.causal_level_enum),
        COALESCE(p_causal_strength, 0.5),
        COALESCE(p_evidence_type,   'structural_prior'::covalence.causal_evidence_type_enum),
        p_notes
    )
    ON CONFLICT (edge_id) DO UPDATE SET
        -- COALESCE: use new value only when caller explicitly provided it;
        -- otherwise preserve the existing row value (fixes covalence#145).
        causal_level    = COALESCE(p_causal_level,    edge_causal_metadata.causal_level),
        causal_strength = COALESCE(p_causal_strength, edge_causal_metadata.causal_strength),
        evidence_type   = COALESCE(p_evidence_type,   edge_causal_metadata.evidence_type),
        notes           = COALESCE(p_notes,            edge_causal_metadata.notes),
        updated_at      = NOW()
    RETURNING *;
END;
$$;

-- =============================================================================
-- Procedure: process_queue_cleanup
-- =============================================================================
-- Two-pass DELETE that removes stale failed slow_path_queue entries.
-- Replaces the two inline DELETE statements in admin_service.rs (covalence#143).
--
-- Pass A — embed tasks whose node now has an embedding (stale after p_stale_hours_embed).
-- Pass B — failed tasks with a NULL node_id (stale after p_stale_hours_null_node).
--
-- RAISE NOTICE emits row counts for each pass (visible in server logs).
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

COMMIT;

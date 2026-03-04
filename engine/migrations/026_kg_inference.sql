-- =============================================================================
-- Migration 026: KG inference rules (covalence#99)
--
-- Implements two forward-chaining inference rules from Dung 1995:
--
--  1. CONTRADICTS symmetry enforcement:
--     Backfill inverse edges for any existing A→B CONTRADICTS that has no
--     matching B→A CONTRADICTS.  Going forward the service layer enforces
--     this on every new edge write.
--
--  2. confirms-and-contradicts derives contends (materialized view):
--     A CONFIRMS B  ∧  B CONTRADICTS C  →  A CONTENDS with C
--     Exposed as covalence.contends_derived (node_a_id, node_c_id,
--     source_edge_1_id, source_edge_2_id).
-- =============================================================================

-- ── Step 1: Backfill missing CONTRADICTS inverse edges ────────────────────
-- For every active A→B CONTRADICTS edge that lacks a corresponding B→A
-- CONTRADICTS edge, insert the inverse.  ON CONFLICT … DO NOTHING keeps
-- this idempotent on re-runs.
INSERT INTO covalence.edges
    (id, source_node_id, target_node_id, edge_type,
     weight, confidence, metadata, created_by, valid_from)
SELECT
    gen_random_uuid(),
    e.target_node_id,                                        -- B → new source
    e.source_node_id,                                        -- A → new target
    'CONTRADICTS',
    e.weight,
    e.confidence,
    jsonb_build_object(
        'inferred_by',    'contradicts_symmetry',
        'source_edge_id', e.id
    ),
    'kg_inference_026',
    now()
FROM  covalence.edges e
WHERE e.edge_type  = 'CONTRADICTS'
  AND e.valid_to  IS NULL
  AND NOT EXISTS (
      SELECT 1
      FROM   covalence.edges inv
      WHERE  inv.source_node_id = e.target_node_id
        AND  inv.target_node_id = e.source_node_id
        AND  inv.edge_type      = 'CONTRADICTS'
        AND  inv.valid_to      IS NULL
  )
ON CONFLICT (source_node_id, target_node_id, edge_type) DO NOTHING;

-- ── Step 2: contends_derived materialized view ────────────────────────────
-- Produces (A, C) pairs where A confirms some B that contradicts C.
-- Refreshed on demand via POST /admin/refresh-inference or the
-- maintenance API (refresh_inference: true).
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

-- Unique index required for REFRESH MATERIALIZED VIEW CONCURRENTLY.
-- The pair (source_edge_1_id, source_edge_2_id) is naturally unique because
-- each CONFIRMS edge and each CONTRADICTS edge pair produces exactly one row.
CREATE UNIQUE INDEX IF NOT EXISTS contends_derived_edges_uniq
    ON covalence.contends_derived (source_edge_1_id, source_edge_2_id);

-- Lookup indexes for the most common access patterns.
CREATE INDEX IF NOT EXISTS contends_derived_node_a_idx
    ON covalence.contends_derived (node_a_id);

CREATE INDEX IF NOT EXISTS contends_derived_node_c_idx
    ON covalence.contends_derived (node_c_id);

CREATE INDEX IF NOT EXISTS contends_derived_node_a_c_idx
    ON covalence.contends_derived (node_a_id, node_c_id);

COMMENT ON MATERIALIZED VIEW covalence.contends_derived IS
    'Derived CONTENDS tuples: A CONFIRMS B ∧ B CONTRADICTS C → (A, C). '
    'Refresh via POST /admin/refresh-inference or maintenance API.';

-- Migration 036: Fix overly-broad bootstrap predicate in edge_causal_metadata (covalence#146)
--
-- Problem
-- -------
-- Migration 034 seeded edge_causal_metadata with a WHERE clause that contained:
--
--     WHERE LOWER(e.edge_type) IN ('originates', 'compiled_from', ...)
--        OR COALESCE(e.causal_weight, 0.0) > 0
--
-- Because migration 033 added causal_weight with DEFAULT 0.5, the second
-- branch (COALESCE(e.causal_weight, 0.0) > 0) is always TRUE for every edge
-- in the database.  As a result every edge received a metadata row, regardless
-- of whether its relationship type carries any genuine causal semantics.
--
-- Fix
-- ---
-- 1. DELETE spurious rows whose edge_type is not in the causal-worthy set.
-- 2. UPDATE the rows for EXTENDS / ELABORATES (which legitimately belong but
--    received the generic ELSE-branch defaults) with better calibrated values.
--
-- Causal-worthy edge types (derived from the 034 explicit whitelist plus the
-- causal-intent filter defined in migration 035 stored procedures):
--
--   Provenance / intervention : ORIGINATES, COMPILED_FROM, SUPERSEDES
--   Epistemic / association   : CONFIRMS, CONTRADICTS, CONTENDS
--   Explicit causal           : CAUSES, MOTIVATED_BY, IMPLEMENTS
--   Elaborative (kept)        : EXTENDS, ELABORATES
--
-- All other types (structural, temporal, semantic, entity, quality) are
-- removed.

BEGIN;

-- =============================================================================
-- Step 1: Remove spurious rows for non-causal edge types
-- =============================================================================

DELETE FROM covalence.edge_causal_metadata ecm
USING covalence.edges e
WHERE ecm.edge_id = e.id
  AND LOWER(e.edge_type) NOT IN (
      'originates',
      'compiled_from',
      'supersedes',
      'confirms',
      'contradicts',
      'contends',
      'causes',
      'motivated_by',
      'implements',
      'extends',
      'elaborates'
  );

-- =============================================================================
-- Step 2: Correct the EXTENDS / ELABORATES rows that were seeded with the
--         generic ELSE-branch defaults (causal_strength=0.5, direction_conf=0.5,
--         hidden_conf_risk=0.5, evidence_type='structural_prior').
--         EXTENDS is an elaborative provenance edge — assign values consistent
--         with how CONFIRMS is treated (association-level, moderate strength).
-- =============================================================================

UPDATE covalence.edge_causal_metadata ecm
SET
    causal_level     = 'association',
    causal_strength  = 0.55,
    evidence_type    = 'structural_prior',
    direction_conf   = 0.75,
    hidden_conf_risk = 0.30,
    updated_at       = NOW()
FROM covalence.edges e
WHERE ecm.edge_id = e.id
  AND LOWER(e.edge_type) IN ('extends', 'elaborates');

COMMIT;

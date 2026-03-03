-- =============================================================================
-- Migration 019: temporal edges (covalence#60 — Graph lifecycle)
--
-- Add valid_from / valid_to columns to covalence.edges so that edges can be
-- *superseded* rather than physically deleted.  This enables historical queries
-- and underpins the reconsolidation/consolidation architecture.
--
-- Semantics:
--   valid_to IS NULL  → edge is currently active  (default behaviour)
--   valid_to IS NOT NULL → edge has been superseded / expired
--
-- Zero-downtime: additive schema change only.  All existing edges remain active
-- (valid_to stays NULL).  Default behaviour is unchanged — queries that don't
-- explicitly request historical data continue to see only active edges.
-- =============================================================================

-- ── Add temporal columns ──────────────────────────────────────────────────────
ALTER TABLE covalence.edges
    ADD COLUMN IF NOT EXISTS valid_from TIMESTAMPTZ NOT NULL DEFAULT now();

ALTER TABLE covalence.edges
    ADD COLUMN IF NOT EXISTS valid_to TIMESTAMPTZ NULL;

-- ── Partial index for efficient "is this edge superseded?" lookups ─────────────
CREATE INDEX IF NOT EXISTS idx_edges_valid_to
    ON covalence.edges (valid_to)
    WHERE valid_to IS NOT NULL;

-- ── Backfill: existing edges get valid_from = created_at ─────────────────────
-- The ADD COLUMN above set valid_from = now() (the migration transaction start)
-- for every pre-existing row.  Reset those rows to use created_at instead so
-- that historical edges carry their true origination timestamp.
UPDATE covalence.edges
    SET valid_from = created_at
    WHERE valid_from = now();

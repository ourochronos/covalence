-- =============================================================================
-- Migration 025: OCC Phase 0 — version guard + queue safety + contention dedup
--                (covalence#98)
-- =============================================================================
--
-- Three targeted fixes:
--
--  1. Ensure `covalence.nodes.version` is NOT NULL with DEFAULT 1 so that
--     optimistic concurrency control (OCC) WHERE clauses are safe.
--
--  2. UNIQUE constraint on `covalence.contentions(node_id, source_node_id)`
--     to deduplicate contention rows at the DB level.  The application layer
--     uses INSERT … ON CONFLICT (node_id, source_node_id) DO NOTHING.
--
-- NOTE: FOR UPDATE SKIP LOCKED was already present in the claim_task
-- SELECT before this migration landed.  It is recorded here for
-- audit-trail completeness.
-- =============================================================================

-- 1. Harden the version column: NOT NULL + default 1 on every node row.
--    Back-fill any pre-existing NULLs first so the NOT NULL constraint lands
--    cleanly on both fresh and migrated databases.
UPDATE covalence.nodes
   SET version = 1
 WHERE version IS NULL;

ALTER TABLE covalence.nodes
    ALTER COLUMN version SET NOT NULL,
    ALTER COLUMN version SET DEFAULT 1;

COMMENT ON COLUMN covalence.nodes.version IS
    'Monotonically increasing write counter.  Used by OCC: callers supply '
    'their last-read version; the UPDATE WHERE version = $expected returns '
    '0 rows on conflict, which the application layer maps to HTTP 409.';

-- 2. Add UNIQUE constraint on contentions to prevent duplicate rows for the
--    same (article, source) pair.  Idempotent via IF NOT EXISTS.
ALTER TABLE covalence.contentions
    DROP CONSTRAINT IF EXISTS contentions_article_source_uniq;

ALTER TABLE covalence.contentions
    ADD CONSTRAINT contentions_article_source_uniq
        UNIQUE (node_id, source_node_id);

COMMENT ON CONSTRAINT contentions_article_source_uniq ON covalence.contentions IS
    'Prevents duplicate contention rows for the same (article, source) pair.  '
    'Insert path uses ON CONFLICT (node_id, source_node_id) DO NOTHING.';

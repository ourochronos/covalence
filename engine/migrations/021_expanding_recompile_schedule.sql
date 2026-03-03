-- =============================================================================
-- Migration 021: Expanding-interval recompilation schedule (covalence#67)
-- =============================================================================
--
-- The columns next_consolidation_at and consolidation_count were added in
-- migration 020 (020_consolidation_schedule.sql).  This migration documents
-- the final schedule semantics and adds a supporting index for the heartbeat
-- query that picks up due articles.
--
-- Schedule (Smolen et al. 2017 spacing effect):
--   consolidation_count=0  (after compile)   → next in 1 hour
--   consolidation_count=1  (after pass 1)    → next in 12 hours
--   consolidation_count=2  (after pass 2)    → next in 3 days
--   consolidation_count=3  (after pass 3)    → next in 1 week
--   consolidation_count=4+ (after pass 4+)   → next in 30 days (monthly)
--
-- The schedule is perpetual: next_consolidation_at is NEVER set to NULL
-- (unlike the 020 draft which terminated after pass 3).
--
-- Orphan articles (no linked sources) are skipped by the handler; their
-- consolidation state is not advanced.
-- =============================================================================

-- Ensure the columns exist (idempotent — they were created in migration 020).
ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS next_consolidation_at TIMESTAMPTZ NULL;

ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS consolidation_count INT NOT NULL DEFAULT 0;

-- Index to accelerate the heartbeat query that finds due articles.
CREATE INDEX IF NOT EXISTS idx_nodes_next_consolidation_at
    ON covalence.nodes (next_consolidation_at)
    WHERE next_consolidation_at IS NOT NULL
      AND status = 'active'
      AND node_type = 'article';

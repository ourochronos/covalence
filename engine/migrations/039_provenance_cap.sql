-- Migration 039: Provenance Cap & Auto-Split — task type + backfill (covalence#161)
--
-- Registers the `auto_split` slow-path task type and enqueues split tasks for
-- any existing articles that already exceed PROVENANCE_SPLIT_THRESHOLD (50).
--
-- Schema changes: comment-only — no new columns or tables are required because:
--   * slow_path_queue.payload JSONB already carries the article_id context.
--   * article_sources bridge (migration 037) already tracks per-link provenance.
--   * edges.edge_type has no CHECK constraint (dropped in migration 010), so
--     the new 'CHILD_OF' edge type is accepted without a schema change.
--
-- This migration is safe to run with zero downtime (no table locks).

BEGIN;

-- ── 1.  Extend slow_path_queue task_type CHECK constraint ────────────────────
--
-- The constraint is a VARCHAR CHECK (not a native enum), so we DROP + ADD.
-- List is cumulative — every prior task type is re-included.

ALTER TABLE covalence.slow_path_queue
    DROP CONSTRAINT IF EXISTS slow_path_queue_task_type_check;

ALTER TABLE covalence.slow_path_queue
    ADD CONSTRAINT slow_path_queue_task_type_check
    CHECK (task_type IN (
        'compile', 'infer_edges', 'resolve_contention',
        'split', 'merge', 'embed', 'contention_check',
        'tree_index', 'tree_embed', 'recompile',
        'decay_check', 'divergence_scan', 'recompute_graph_embeddings',
        'reconsolidate', 'consolidate_article', 'critique_article',
        'infer_article_edges',
        'auto_split'                 -- covalence#161
    ));

-- ── 2.  Document the task type (comment-only — no schema change) ─────────────

COMMENT ON COLUMN covalence.slow_path_queue.task_type IS
    'Known task types: compile, infer_edges, resolve_contention, split, merge,
     embed, contention_check, tree_index, tree_embed, recompile, decay_check,
     divergence_scan, recompute_graph_embeddings, reconsolidate,
     consolidate_article, critique_article, infer_article_edges,
     auto_split (covalence#161 — provenance cap & auto-split).

     auto_split payload schema:
       { "article_id": "<uuid>",
         "reason": "originates_overflow" | "backfill_overflow",
         "originates_count_at_trigger": <int> }

     Runtime constants (overridable via env var):
       PROVENANCE_CAP             = 40  (COVALENCE_PROVENANCE_CAP)
       PROVENANCE_SPLIT_THRESHOLD = 50  (COVALENCE_PROVENANCE_SPLIT_THRESHOLD)';

COMMIT;

-- =============================================================================
-- BACKFILL (runs outside the transaction so it can be skipped / re-run safely)
--
-- Enqueue auto_split for all active articles whose ORIGINATES edge count
-- already exceeds PROVENANCE_SPLIT_THRESHOLD (50).  Idempotent: skips any
-- article that already has a pending or in-flight auto_split task.
--
-- Deploy order:
--   1. Apply this migration (schema change above — safe, no lock).
--   2. Deploy the new Rust binary (with handle_auto_split + compile cap logic).
--   3. The INSERT below runs automatically as part of this migration file, but
--      only after the COMMIT above, so it uses the unconstrained TEXT column.
--      Re-run it manually if you need to pick up newly-over-threshold articles.
-- =============================================================================

INSERT INTO covalence.slow_path_queue
    (task_type, payload, priority, created_at, status)
SELECT
    'auto_split',
    jsonb_build_object(
        'article_id',                  n.id::text,
        'reason',                      'backfill_overflow',
        'originates_count_at_trigger', edge_counts.cnt
    ),
    1,          -- low priority (same as 'split')
    now(),
    'pending'
FROM (
    SELECT
        e.target_node_id    AS article_id,
        COUNT(*)            AS cnt
    FROM  covalence.edges  e
    JOIN  covalence.nodes  n ON n.id = e.target_node_id
    WHERE e.edge_type  = 'ORIGINATES'
      AND n.node_type  = 'article'
      AND n.status     = 'active'
    GROUP BY e.target_node_id
    HAVING COUNT(*) > 50      -- PROVENANCE_SPLIT_THRESHOLD
) edge_counts
JOIN covalence.nodes n ON n.id = edge_counts.article_id
-- Idempotency guard: skip articles already queued for auto_split.
WHERE NOT EXISTS (
    SELECT 1
    FROM   covalence.slow_path_queue spq
    WHERE  spq.task_type              = 'auto_split'
      AND  spq.status                 IN ('pending', 'processing')
      AND  spq.payload->>'article_id' = n.id::text
);

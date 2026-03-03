-- =============================================================================
-- Migration 018: retrieval-triggered reconsolidation (covalence#66)
--
-- 1. Add last_reconsolidated_at to covalence.nodes so the engine can enforce
--    a 6-hour cooldown before re-queuing reconsolidation work.
--
-- 2. Widen the slow_path_queue.task_type CHECK constraint to include all task
--    types that the worker already handles (decay_check, divergence_scan,
--    recompute_graph_embeddings) plus the new reconsolidate task type.
--    The original constraint was too narrow and silently broke those tasks.
-- =============================================================================

-- ── nodes: add cooldown timestamp ────────────────────────────────────────────
ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS last_reconsolidated_at TIMESTAMPTZ;

-- ── slow_path_queue: widen task_type constraint ───────────────────────────────
-- DROP + re-ADD is the only portable way to change a CHECK body in Postgres.
ALTER TABLE covalence.slow_path_queue
    DROP CONSTRAINT IF EXISTS slow_path_queue_task_type_check;

ALTER TABLE covalence.slow_path_queue
    ADD CONSTRAINT slow_path_queue_task_type_check
    CHECK (task_type IN (
        'compile',
        'infer_edges',
        'resolve_contention',
        'split',
        'merge',
        'embed',
        'contention_check',
        'tree_index',
        'tree_embed',
        'recompile',
        'decay_check',
        'divergence_scan',
        'recompute_graph_embeddings',
        'reconsolidate'
    ));

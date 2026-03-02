-- =============================================================================
-- Migration 012 — Staleness detection (covalence#36)
--
-- Adds the 'recompile' task_type to the slow_path_queue CHECK constraint so
-- AdminService::staleness_scan() can enqueue articles for recompilation.
-- =============================================================================

BEGIN;

-- Drop the existing constraint and recreate it with 'recompile' added.
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
        'recompile'
    ));

COMMIT;

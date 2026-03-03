-- covalence#67: Expanding-interval article recompilation schedule.
--
-- Adds the data model for the spacing-effect consolidation schedule:
--   Pass 1: ~1 hour  after creation
--   Pass 2: ~18 hours after creation
--   Pass 3: ~5 days   after creation
--   Pass 4+: weekly (rolled into admin_maintenance)

-- ── 1. Consolidation tracking columns on nodes ────────────────────────────
ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS next_consolidation_at TIMESTAMPTZ NULL;

ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS consolidation_count INT NOT NULL DEFAULT 0;

-- ── 2. execute_after for delayed slow-path tasks ─────────────────────────
--   Worker only claims a task when execute_after IS NULL OR execute_after <= now().
ALTER TABLE covalence.slow_path_queue
    ADD COLUMN IF NOT EXISTS execute_after TIMESTAMPTZ NULL;

-- ── 3. Add consolidate_article to the task_type CHECK constraint ──────────
ALTER TABLE covalence.slow_path_queue
    DROP CONSTRAINT IF EXISTS slow_path_queue_task_type_check;

ALTER TABLE covalence.slow_path_queue
    ADD CONSTRAINT slow_path_queue_task_type_check
    CHECK (task_type = ANY (ARRAY[
        'compile', 'infer_edges', 'resolve_contention', 'split', 'merge',
        'embed', 'contention_check', 'tree_index', 'tree_embed', 'recompile',
        'decay_check', 'divergence_scan', 'recompute_graph_embeddings',
        'reconsolidate', 'consolidate_article'
    ]));

-- ── 4. Indexes ─────────────────────────────────────────────────────────────
CREATE INDEX IF NOT EXISTS idx_nodes_next_consolidation_at
    ON covalence.nodes (next_consolidation_at)
    WHERE next_consolidation_at IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_slow_path_queue_execute_after
    ON covalence.slow_path_queue (execute_after)
    WHERE execute_after IS NOT NULL;

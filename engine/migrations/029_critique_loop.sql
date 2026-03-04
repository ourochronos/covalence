-- 029: Reflexion-style critique loop (covalence#105)
--
-- Adds `critique_article` to the slow_path_queue task_type CHECK constraint.
-- The CRITIQUES edge type needs no DB-level constraint (edge_type is a free
-- text label validated in Rust — see the comment in test_db_schema.sql).

-- Refresh the task_type CHECK constraint to include 'critique_article'.
ALTER TABLE covalence.slow_path_queue
    DROP CONSTRAINT IF EXISTS slow_path_queue_task_type_check;
ALTER TABLE covalence.slow_path_queue
    ADD CONSTRAINT slow_path_queue_task_type_check
    CHECK (task_type IN (
        'compile', 'infer_edges', 'resolve_contention',
        'split', 'merge', 'embed', 'contention_check',
        'tree_index', 'tree_embed', 'recompile',
        'decay_check', 'divergence_scan', 'recompute_graph_embeddings',
        'reconsolidate', 'consolidate_article', 'critique_article'
    ));

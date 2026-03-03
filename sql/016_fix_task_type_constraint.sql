-- covalence#61: Add 'recompute_graph_embeddings' to slow_path_queue task_type constraint.
-- The admin maintenance endpoint queues this task type, but the CHECK constraint
-- previously did not include it, causing HTTP 500 on POST /admin/maintenance.

ALTER TABLE covalence.slow_path_queue
  DROP CONSTRAINT slow_path_queue_task_type_check;

ALTER TABLE covalence.slow_path_queue
  ADD CONSTRAINT slow_path_queue_task_type_check
  CHECK (task_type = ANY (ARRAY[
    'compile', 'infer_edges', 'resolve_contention', 'split', 'merge',
    'embed', 'contention_check', 'tree_index', 'tree_embed',
    'recompute_graph_embeddings'
  ]));

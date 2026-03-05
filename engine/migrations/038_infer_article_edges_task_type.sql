-- Migration 038: Article-to-Article Semantic Edge Inference — task type (covalence#160)
--
-- Adds the `infer_article_edges` slow-path task type and a composite index on
-- covalence.edges to support the edge-firewall existence check and edge-type-
-- filtered graph traversal introduced by the new worker.
--
-- The `edge_type` CHECK constraint on covalence.edges was dropped in migration
-- 010, so no schema change is needed there — RELATES_TO, EXTENDS, CONFIRMS,
-- and CONTRADICTS are all already accepted by the GraphRepository layer.

BEGIN;

-- ── 1.  Extend slow_path_queue task_type CHECK constraint ────────────────────
-- The constraint is a VARCHAR CHECK (not a native enum) so we DROP + ADD.
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
        'infer_article_edges'
    ));

-- ── 2.  Composite index for edge-firewall + edge-type-filtered traversal ─────
CREATE INDEX IF NOT EXISTS idx_edges_type_src_tgt
    ON covalence.edges (edge_type, source_node_id, target_node_id);

COMMIT;

-- =============================================================================
-- BACKFILL (run manually after migration):
-- =============================================================================
-- INSERT INTO covalence.slow_path_queue (task_type, node_id, created_at)
-- SELECT 'infer_article_edges', id, NOW()
-- FROM covalence.nodes
-- WHERE node_type = 'article'
--   AND status = 'active'
--   AND NOT EXISTS (
--     SELECT 1 FROM covalence.edges
--     WHERE source_node_id = covalence.nodes.id
--       AND edge_type != 'ORIGINATES'
--   )
-- ON CONFLICT DO NOTHING;

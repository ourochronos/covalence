-- Migration 024: Faceted classification — multi-dimensional knowledge organisation.
--
-- Adds two nullable TEXT[] columns to covalence.nodes implementing Phase 1 of
-- Ranganathan-inspired faceted classification (covalence#92):
--
--   facet_function  — what does this knowledge DO?
--                     e.g. "retrieval", "storage", "evaluation", "design"
--
--   facet_scope     — at what abstraction level does it operate?
--                     e.g. "theoretical", "practical", "operational", "historical"
--
-- Both columns are nullable and default NULL so existing nodes are unaffected
-- (backward-compatible).  GIN indexes enable efficient array-containment
-- queries (facet_function @> '{retrieval}').
--
-- Phase 2 (scoring / ranking integration) is tracked separately.

ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS facet_function TEXT[] NULL DEFAULT NULL;

ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS facet_scope TEXT[] NULL DEFAULT NULL;

-- GIN indexes for array containment queries (@>).
CREATE INDEX IF NOT EXISTS nodes_facet_function_gin_idx
    ON covalence.nodes USING gin (facet_function)
    WHERE facet_function IS NOT NULL;

CREATE INDEX IF NOT EXISTS nodes_facet_scope_gin_idx
    ON covalence.nodes USING gin (facet_scope)
    WHERE facet_scope IS NOT NULL;

-- Migration 009: Fix sp_find_cooccurrence_pairs performance
--
-- The original SP in 006 used correlated subqueries to check node degree:
--
--   AND (SELECT COUNT(*) FROM edges WHERE source_node_id = pf.src OR target_node_id = pf.src) < p_max_degree
--   AND (SELECT COUNT(*) FROM edges WHERE source_node_id = pf.tgt OR target_node_id = pf.tgt) < p_max_degree
--
-- These scan the entire edges table for EVERY candidate pair, turning the
-- degree filter into an O(pairs * edges) operation. On a graph with 88K+
-- edges this dominates execution time.
--
-- Fix: pre-compute node degrees once in a CTE and JOIN against it.

CREATE OR REPLACE FUNCTION sp_find_cooccurrence_pairs(
    p_min_cooccurrences INT DEFAULT 2,
    p_max_degree INT DEFAULT 500
) RETURNS TABLE(
    source_node_id UUID,
    target_node_id UUID,
    cooccurrence_count BIGINT
) AS $$
    WITH node_degrees AS (
        SELECT node_id, COUNT(*) AS degree FROM (
            SELECT source_node_id AS node_id FROM edges
            UNION ALL
            SELECT target_node_id AS node_id FROM edges
        ) all_nodes GROUP BY node_id
    ),
    chunk_pairs AS (
        SELECT e1.entity_id AS src, e2.entity_id AS tgt
        FROM extractions e1
        JOIN extractions e2 ON e1.chunk_id = e2.chunk_id
        WHERE e1.entity_type = 'node'
          AND e2.entity_type = 'node'
          AND e1.entity_id < e2.entity_id
          AND e1.is_superseded = false
          AND e2.is_superseded = false
    ),
    stmt_pairs AS (
        SELECT e1.entity_id AS src, e2.entity_id AS tgt
        FROM extractions e1
        JOIN extractions e2 ON e1.statement_id = e2.statement_id
        WHERE e1.entity_type = 'node'
          AND e2.entity_type = 'node'
          AND e1.entity_id < e2.entity_id
          AND e1.is_superseded = false
          AND e2.is_superseded = false
          AND e1.statement_id IS NOT NULL
    ),
    pair_freq AS (
        SELECT src, tgt, COUNT(*) AS freq
        FROM (SELECT * FROM chunk_pairs UNION ALL SELECT * FROM stmt_pairs) combined
        GROUP BY src, tgt
        HAVING COUNT(*) >= p_min_cooccurrences
    )
    SELECT pf.src, pf.tgt, pf.freq
    FROM pair_freq pf
    LEFT JOIN node_degrees nd_src ON nd_src.node_id = pf.src
    LEFT JOIN node_degrees nd_tgt ON nd_tgt.node_id = pf.tgt
    WHERE NOT EXISTS (
        SELECT 1 FROM edges e
        WHERE e.source_node_id = pf.src
          AND e.target_node_id = pf.tgt
          AND e.rel_type = 'co_occurs'
    )
    AND COALESCE(nd_src.degree, 0) < p_max_degree
    AND COALESCE(nd_tgt.degree, 0) < p_max_degree;
$$ LANGUAGE sql STABLE;

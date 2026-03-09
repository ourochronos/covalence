-- Stored procedures for graph traversal and temporal search.

CREATE OR REPLACE FUNCTION graph_traverse(
    start_id UUID, max_hops INT DEFAULT 3
) RETURNS TABLE(node_id UUID, depth INT, path UUID[]) AS $$
WITH RECURSIVE traverse AS (
    SELECT id AS node_id, 0 AS depth, ARRAY[id] AS path
    FROM nodes WHERE id = start_id
    UNION ALL
    SELECT CASE
        WHEN e.source_node_id = t.node_id THEN e.target_node_id
        ELSE e.source_node_id
    END, t.depth + 1, t.path || CASE
        WHEN e.source_node_id = t.node_id THEN e.target_node_id
        ELSE e.source_node_id
    END
    FROM traverse t
    JOIN edges e ON (e.source_node_id = t.node_id OR e.target_node_id = t.node_id)
    WHERE t.depth < max_hops
    AND NOT CASE
        WHEN e.source_node_id = t.node_id THEN e.target_node_id
        ELSE e.source_node_id
    END = ANY(t.path)
)
SELECT * FROM traverse;
$$ LANGUAGE SQL STABLE;

CREATE OR REPLACE FUNCTION temporal_search(
    query_start TIMESTAMPTZ,
    query_end TIMESTAMPTZ,
    max_results INT DEFAULT 20
) RETURNS TABLE(id UUID, score FLOAT8) AS $$
SELECT n.id,
    1.0 / (1.0 + LEAST(
        ABS(EXTRACT(EPOCH FROM n.first_seen - query_start)),
        ABS(EXTRACT(EPOCH FROM n.last_seen - query_end))
    ) / 86400.0) AS score
FROM nodes n
WHERE n.first_seen <= query_end AND n.last_seen >= query_start
ORDER BY score DESC
LIMIT max_results;
$$ LANGUAGE SQL STABLE;

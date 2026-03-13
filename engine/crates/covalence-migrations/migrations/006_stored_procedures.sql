-- Stored procedures for temporal search.

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

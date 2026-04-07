-- Fix sp_data_health_report: the `duplicate_sources` count used
-- (title, domains) as the dedup key, which flagged legitimately
-- distinct files that happen to share a filename (e.g., 24 different
-- `mod.rs` files across crates) as duplicates. The real definition
-- of a duplicate is "multiple non-superseded rows for the same URI"
-- — `content_hash` is already UNIQUE at the schema level, and URI
-- is the stable per-file identifier.

CREATE OR REPLACE FUNCTION sp_data_health_report()
RETURNS TABLE(
    superseded_sources BIGINT,
    superseded_chunks BIGINT,
    orphan_nodes BIGINT,
    orphan_nodes_with_edges BIGINT,
    duplicate_sources BIGINT,
    unembedded_nodes BIGINT,
    unsummarized_code BIGINT,
    unsummarized_sources BIGINT
) AS $$
    SELECT
        (SELECT COUNT(*) FROM sources WHERE superseded_by IS NOT NULL)::bigint,
        (SELECT COUNT(*) FROM chunks c JOIN sources s ON s.id = c.source_id
         WHERE s.superseded_by IS NOT NULL)::bigint,
        (SELECT COUNT(*) FROM nodes n
         WHERE NOT EXISTS (SELECT 1 FROM extractions ex WHERE ex.entity_id = n.id))::bigint,
        (SELECT COUNT(*) FROM nodes n
         WHERE NOT EXISTS (SELECT 1 FROM extractions ex WHERE ex.entity_id = n.id)
           AND EXISTS (SELECT 1 FROM edges e
                       WHERE e.source_node_id = n.id OR e.target_node_id = n.id))::bigint,
        -- Duplicate = multiple non-superseded rows sharing a URI.
        (SELECT COALESCE(SUM(n), 0) FROM (
           SELECT COUNT(*) AS n FROM sources
           WHERE uri IS NOT NULL AND superseded_by IS NULL
           GROUP BY uri HAVING COUNT(*) > 1
         ) dup)::bigint,
        (SELECT COUNT(*) FROM nodes WHERE embedding IS NULL)::bigint,
        (SELECT COUNT(*) FROM nodes
         WHERE entity_class = 'code'
           AND (properties->>'semantic_summary' IS NULL
                OR properties->>'semantic_summary' = ''))::bigint,
        (SELECT COUNT(*) FROM sources
         WHERE (summary IS NULL OR summary = '')
           AND superseded_by IS NULL)::bigint;
$$ LANGUAGE sql STABLE;

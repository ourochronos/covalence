-- Fix sp_list_nodes_without_embeddings: the original filter
-- excluded any node whose `description` was NULL or empty,
-- which permanently stranded actor/domain nodes that only have
-- a canonical_name (e.g. extracted authors, single-word concepts).
-- The Rust backfill in `backfill_node_embeddings` already falls
-- back to canonical_name when description is missing — the SP
-- side filter just hid those rows from it.
--
-- After this migration, `POST /admin/nodes/backfill-embeddings`
-- will see every node where `embedding IS NULL`, matching what
-- `sp_data_health_report` already counts as `unembedded_nodes`.

CREATE OR REPLACE FUNCTION sp_list_nodes_without_embeddings(
    p_limit INT DEFAULT 500
) RETURNS TABLE(id UUID, canonical_name TEXT, description TEXT) AS $$
    SELECT id, canonical_name, description
    FROM nodes
    WHERE embedding IS NULL
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

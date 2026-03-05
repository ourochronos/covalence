-- covalence#196: GIN index on source node metadata for JSONB containment queries.
-- Enables efficient `metadata @> $filter::jsonb` filtering on source_list.
CREATE INDEX IF NOT EXISTS idx_nodes_metadata_gin
    ON covalence.nodes USING gin (metadata jsonb_path_ops)
    WHERE node_type = 'source';

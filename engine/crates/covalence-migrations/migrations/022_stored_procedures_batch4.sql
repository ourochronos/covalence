-- Migration 022: Stored procedures — Batch 4
--
-- Enhanced SPs for intelligence, graph sync, resolver,
-- global search, pipeline, node service, ontology.

-- ================================================================
-- Graph sync (fixed column mappings)
-- ================================================================

DROP FUNCTION IF EXISTS sp_load_all_nodes();
CREATE OR REPLACE FUNCTION sp_load_all_nodes()
RETURNS TABLE(
    id UUID,
    canonical_name TEXT,
    node_type TEXT,
    canonical_type TEXT,
    clearance_level INT,
    entity_class TEXT
) AS $$
    SELECT id, canonical_name, node_type,
           COALESCE(canonical_type, node_type),
           clearance_level, entity_class
    FROM nodes;
$$ LANGUAGE sql STABLE;

DROP FUNCTION IF EXISTS sp_load_all_edges();
CREATE OR REPLACE FUNCTION sp_load_all_edges()
RETURNS TABLE(
    id UUID, source_node_id UUID, target_node_id UUID,
    rel_type TEXT, canonical_rel_type TEXT,
    confidence FLOAT8, weight FLOAT8,
    is_synthetic BOOLEAN, has_valid_from BOOLEAN,
    causal_level INT, clearance_level INT
) AS $$
    SELECT id, source_node_id, target_node_id,
           rel_type, COALESCE(canonical_rel_type, rel_type),
           confidence, COALESCE(weight, 1.0),
           (properties->>'synthetic')::boolean IS TRUE,
           valid_from IS NOT NULL,
           COALESCE(causal_level, 0),
           clearance_level
    FROM edges
    WHERE invalid_at IS NULL;
$$ LANGUAGE sql STABLE;

DROP FUNCTION IF EXISTS sp_poll_outbox_events(BIGINT, INT);
CREATE OR REPLACE FUNCTION sp_poll_outbox_events(
    p_after_seq BIGINT,
    p_limit INT DEFAULT 1000
) RETURNS TABLE(seq_id BIGINT, entity_type TEXT, operation TEXT, entity_id UUID, payload JSONB) AS $$
    SELECT seq_id, entity_type, operation, entity_id, payload
    FROM outbox_events
    WHERE seq_id > p_after_seq
    ORDER BY seq_id
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Intelligence: node resolution
-- ================================================================

CREATE OR REPLACE FUNCTION sp_resolve_node_by_name(
    p_name TEXT
) RETURNS TABLE(id UUID, canonical_name TEXT, node_type TEXT) AS $$
    -- Exact match first
    SELECT id, canonical_name, node_type
    FROM nodes
    WHERE LOWER(canonical_name) = LOWER(p_name)
    LIMIT 1;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_resolve_node_fuzzy(
    p_name TEXT,
    p_limit INT DEFAULT 1
) RETURNS TABLE(id UUID, canonical_name TEXT, node_type TEXT) AS $$
    SELECT id, canonical_name, node_type
    FROM nodes
    WHERE LOWER(canonical_name) LIKE '%' || LOWER(p_name) || '%'
    ORDER BY mention_count DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Intelligence: blast radius
-- ================================================================

CREATE OR REPLACE FUNCTION sp_get_node_component(
    p_node_id UUID
) RETURNS TABLE(component_id UUID, component_name TEXT) AS $$
    SELECT n.id, n.canonical_name
    FROM edges e
    JOIN nodes n ON n.id = e.target_node_id
    WHERE e.source_node_id = p_node_id
      AND e.rel_type = 'PART_OF_COMPONENT'
      AND e.invalid_at IS NULL
    LIMIT 1;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_get_invalidated_neighbors(
    p_node_id UUID,
    p_limit INT DEFAULT 50
) RETURNS TABLE(
    neighbor_id UUID, neighbor_name TEXT, rel_type TEXT, direction TEXT
) AS $$
    SELECT n.id, n.canonical_name, e.rel_type, 'outgoing'::text
    FROM edges e
    JOIN nodes n ON n.id = e.target_node_id
    WHERE e.source_node_id = p_node_id AND e.invalid_at IS NOT NULL
    UNION ALL
    SELECT n.id, n.canonical_name, e.rel_type, 'incoming'::text
    FROM edges e
    JOIN nodes n ON n.id = e.source_node_id
    WHERE e.target_node_id = p_node_id AND e.invalid_at IS NOT NULL
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Intelligence: evidence queries (vector + domain filter)
-- ================================================================

CREATE OR REPLACE FUNCTION sp_search_nodes_by_domain_vector(
    p_embedding halfvec,
    p_domains TEXT[],
    p_entity_class TEXT,
    p_limit INT DEFAULT 10
) RETURNS TABLE(
    id UUID, canonical_name TEXT, description TEXT, distance FLOAT8
) AS $$
    SELECT n.id, n.canonical_name, n.description,
           (n.embedding <=> p_embedding) AS distance
    FROM nodes n
    WHERE n.embedding IS NOT NULL
      AND EXISTS (
        SELECT 1 FROM extractions ex
        JOIN chunks c ON c.id = ex.chunk_id
        JOIN sources s ON s.id = c.source_id
        WHERE ex.entity_id = n.id
          AND s.domain = ANY(p_domains)
      )
      AND (p_entity_class IS NULL OR n.entity_class = p_entity_class)
    ORDER BY distance
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Resolver: fuzzy with type preference
-- ================================================================

CREATE OR REPLACE FUNCTION sp_search_nodes_fuzzy_typed(
    p_name TEXT,
    p_threshold FLOAT4,
    p_preferred_type TEXT,
    p_limit INT DEFAULT 5
) RETURNS TABLE(id UUID, canonical_name TEXT, node_type TEXT, sim FLOAT4) AS $$
    SELECT id, canonical_name, node_type,
           similarity(canonical_name, p_name) AS sim
    FROM nodes
    WHERE similarity(canonical_name, p_name) >= p_threshold
    ORDER BY (node_type = p_preferred_type) DESC, sim DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Global search: embedding similarity
-- ================================================================

CREATE OR REPLACE FUNCTION sp_search_community_summaries_vector(
    p_embedding halfvec,
    p_limit INT DEFAULT 10
) RETURNS TABLE(id UUID, canonical_name TEXT, description TEXT, distance FLOAT8) AS $$
    SELECT id, canonical_name, description,
           (embedding <=> p_embedding) AS distance
    FROM nodes
    WHERE node_type = 'community_summary'
      AND embedding IS NOT NULL
    ORDER BY distance
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_search_sections_vector(
    p_embedding halfvec,
    p_limit INT DEFAULT 10
) RETURNS TABLE(id UUID, source_id UUID, title TEXT, summary TEXT, distance FLOAT8) AS $$
    SELECT id, source_id, title, summary,
           (embedding <=> p_embedding) AS distance
    FROM sections
    WHERE embedding IS NOT NULL
    ORDER BY distance
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_search_articles_vector(
    p_embedding halfvec,
    p_limit INT DEFAULT 10
) RETURNS TABLE(id UUID, title TEXT, body TEXT, distance FLOAT8) AS $$
    SELECT id, title, body,
           (embedding <=> p_embedding) AS distance
    FROM articles
    WHERE embedding IS NOT NULL
    ORDER BY distance
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Node service
-- ================================================================

CREATE OR REPLACE FUNCTION sp_tombstone_node(
    p_node_id UUID
) RETURNS VOID AS $$
    UPDATE nodes SET clearance_level = -1 WHERE id = p_node_id;
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION sp_get_invalidated_edges_for_node(
    p_node_id UUID,
    p_limit INT DEFAULT 50
) RETURNS TABLE(
    id UUID, source_node_id UUID, target_node_id UUID,
    rel_type TEXT, invalid_at TIMESTAMPTZ, invalidated_by UUID
) AS $$
    SELECT id, source_node_id, target_node_id,
           rel_type, invalid_at, invalidated_by
    FROM edges
    WHERE (source_node_id = p_node_id OR target_node_id = p_node_id)
      AND invalid_at IS NOT NULL
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Pipeline: vector search for hierarchical/source-gated
-- ================================================================

CREATE OR REPLACE FUNCTION sp_search_table_vector(
    p_table TEXT,
    p_embedding halfvec,
    p_limit INT DEFAULT 20
) RETURNS TABLE(id UUID, distance FLOAT8) AS $$
BEGIN
    -- Dynamic table query for multi-table vector search
    RETURN QUERY EXECUTE format(
        'SELECT id, (embedding <=> $1) AS distance
         FROM %I WHERE embedding IS NOT NULL
         ORDER BY distance LIMIT $2',
        p_table
    ) USING p_embedding, p_limit;
END;
$$ LANGUAGE plpgsql STABLE;

-- ================================================================
-- Resolver: rel_type resolution
-- ================================================================

CREATE OR REPLACE FUNCTION sp_resolve_rel_type_exact(
    p_rel_type TEXT
) RETURNS TEXT AS $$
    SELECT COALESCE(canonical_rel_type, rel_type)
    FROM edges
    WHERE LOWER(rel_type) = LOWER(p_rel_type)
       OR LOWER(canonical_rel_type) = LOWER(p_rel_type)
    LIMIT 1;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_resolve_rel_type_fuzzy(
    p_rel_type TEXT,
    p_threshold FLOAT4
) RETURNS TEXT AS $$
    SELECT rel_type
    FROM (
        SELECT rel_type, similarity(rel_type, p_rel_type) AS sim,
               COUNT(*) AS freq
        FROM edges
        WHERE similarity(rel_type, p_rel_type) >= p_threshold
        GROUP BY rel_type
    ) candidates
    ORDER BY sim * LN(freq + 1) DESC
    LIMIT 1;
$$ LANGUAGE sql STABLE;

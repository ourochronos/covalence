-- Migration 021: Stored procedures — Batch 3
--
-- Search dimensions, cache, graph sync, entity resolution,
-- alignment, bootstrap, ask enrichment.

-- ================================================================
-- Search: Lexical dimension
-- ================================================================

CREATE OR REPLACE FUNCTION sp_search_nodes_lexical(
    p_query TEXT,
    p_limit INT DEFAULT 20
) RETURNS TABLE(id UUID, canonical_name TEXT, node_type TEXT, description TEXT,
                rank REAL, snippet TEXT) AS $$
    SELECT n.id, n.canonical_name, n.node_type, n.description,
           ts_rank_cd(n.name_tsv, plainto_tsquery('english', p_query)) AS rank,
           ts_headline('english', n.canonical_name, plainto_tsquery('english', p_query)) AS snippet
    FROM nodes n
    WHERE n.name_tsv @@ plainto_tsquery('english', p_query)
    ORDER BY rank DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_search_chunks_lexical(
    p_query TEXT,
    p_limit INT DEFAULT 20
) RETURNS TABLE(id UUID, source_id UUID, content TEXT, rank REAL, snippet TEXT) AS $$
    SELECT c.id, c.source_id, c.content,
           ts_rank_cd(c.content_tsv, plainto_tsquery('english', p_query)) AS rank,
           ts_headline('english', c.content, plainto_tsquery('english', p_query),
                       'MaxFragments=2, MaxWords=40, MinWords=15') AS snippet
    FROM chunks c
    WHERE c.content_tsv @@ plainto_tsquery('english', p_query)
    ORDER BY rank DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_search_statements_lexical(
    p_query TEXT,
    p_limit INT DEFAULT 20
) RETURNS TABLE(id UUID, source_id UUID, content TEXT, rank REAL, snippet TEXT) AS $$
    SELECT s.id, s.source_id, s.content,
           ts_rank_cd(s.content_tsv, plainto_tsquery('english', p_query)) AS rank,
           ts_headline('english', s.content, plainto_tsquery('english', p_query),
                       'MaxFragments=1, MaxWords=30') AS snippet
    FROM statements s
    WHERE s.content_tsv @@ plainto_tsquery('english', p_query)
      AND s.is_evicted = false
    ORDER BY rank DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_search_sections_lexical(
    p_query TEXT,
    p_limit INT DEFAULT 20
) RETURNS TABLE(id UUID, source_id UUID, title TEXT, summary TEXT, rank REAL) AS $$
    SELECT s.id, s.source_id, s.title, s.summary,
           ts_rank_cd(s.body_tsv, plainto_tsquery('english', p_query)) AS rank
    FROM sections s
    WHERE s.body_tsv @@ plainto_tsquery('english', p_query)
    ORDER BY rank DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Search: Temporal dimension
-- ================================================================

CREATE OR REPLACE FUNCTION sp_search_chunks_temporal(
    p_start TIMESTAMPTZ,
    p_end TIMESTAMPTZ,
    p_limit INT DEFAULT 20
) RETURNS TABLE(id UUID, source_id UUID, content TEXT, created_at TIMESTAMPTZ) AS $$
    SELECT c.id, c.source_id, c.content, s.ingested_at
    FROM chunks c
    JOIN sources s ON s.id = c.source_id
    WHERE s.ingested_at BETWEEN p_start AND p_end
    ORDER BY s.ingested_at DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_search_chunks_recent(
    p_limit INT DEFAULT 20
) RETURNS TABLE(id UUID, source_id UUID, content TEXT, created_at TIMESTAMPTZ) AS $$
    SELECT c.id, c.source_id, c.content, s.ingested_at
    FROM chunks c
    JOIN sources s ON s.id = c.source_id
    ORDER BY s.ingested_at DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Search: Cache
-- ================================================================

CREATE OR REPLACE FUNCTION sp_lookup_query_cache(
    p_embedding halfvec,
    p_strategy TEXT,
    p_max_distance FLOAT8,
    p_ttl_secs INT
) RETURNS TABLE(id UUID, response JSONB) AS $$
    SELECT id, response FROM query_cache
    WHERE strategy_used = p_strategy
      AND query_embedding <=> p_embedding < p_max_distance
      AND created_at > NOW() - (p_ttl_secs || ' seconds')::interval
    ORDER BY query_embedding <=> p_embedding
    LIMIT 1;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_bump_cache_hit_count(
    p_cache_id UUID
) RETURNS VOID AS $$
    UPDATE query_cache SET hit_count = hit_count + 1 WHERE id = p_cache_id;
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION sp_clear_query_cache()
RETURNS BIGINT AS $$
    WITH deleted AS (DELETE FROM query_cache RETURNING id)
    SELECT COUNT(*) FROM deleted;
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION sp_store_query_cache(
    p_id UUID,
    p_embedding halfvec,
    p_strategy TEXT,
    p_results JSONB,
    p_query_text TEXT
) RETURNS VOID AS $$
    INSERT INTO query_cache (id, query_embedding, strategy_used, response, query_text)
    VALUES (p_id, p_embedding, p_strategy, p_results, p_query_text);
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION sp_evict_old_cache_entries(
    p_max_entries INT
) RETURNS BIGINT AS $$
    WITH to_delete AS (
        SELECT id FROM query_cache
        ORDER BY created_at DESC
        OFFSET p_max_entries
    ),
    deleted AS (
        DELETE FROM query_cache WHERE id IN (SELECT id FROM to_delete)
        RETURNING id
    )
    SELECT COUNT(*) FROM deleted;
$$ LANGUAGE sql;

-- ================================================================
-- Graph sync
-- ================================================================

CREATE OR REPLACE FUNCTION sp_poll_outbox_events(
    p_after_seq BIGINT,
    p_limit INT DEFAULT 1000
) RETURNS TABLE(seq_id BIGINT, event_type TEXT, entity_id UUID) AS $$
    SELECT seq_id, event_type, entity_id
    FROM outbox_events
    WHERE seq_id > p_after_seq
    ORDER BY seq_id
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_load_all_nodes()
RETURNS TABLE(
    id UUID, canonical_name TEXT, node_type TEXT,
    clearance_level INT, entity_class TEXT
) AS $$
    SELECT id, COALESCE(canonical_type, node_type),
           node_type, clearance_level,
           entity_class
    FROM nodes;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_load_all_edges()
RETURNS TABLE(
    id UUID, source_node_id UUID, target_node_id UUID,
    rel_type TEXT, confidence FLOAT8, is_synthetic BOOLEAN,
    has_valid_from BOOLEAN, clearance_level INT
) AS $$
    SELECT id, source_node_id, target_node_id,
           COALESCE(canonical_rel_type, rel_type),
           confidence,
           properties->>'synthetic' = 'true',
           valid_from IS NOT NULL,
           clearance_level
    FROM edges
    WHERE invalid_at IS NULL;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Entity resolution (pg_resolver)
-- ================================================================

CREATE OR REPLACE FUNCTION sp_find_closest_node_embedding(
    p_embedding halfvec,
    p_threshold FLOAT8,
    p_limit INT DEFAULT 5
) RETURNS TABLE(id UUID, canonical_name TEXT, node_type TEXT, distance FLOAT8) AS $$
    SELECT id, canonical_name, node_type,
           (embedding <=> p_embedding) AS distance
    FROM nodes
    WHERE embedding IS NOT NULL
      AND (embedding <=> p_embedding) < p_threshold
    ORDER BY distance
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_search_nodes_fuzzy_trigram(
    p_name TEXT,
    p_threshold FLOAT4,
    p_limit INT DEFAULT 5
) RETURNS TABLE(id UUID, canonical_name TEXT, node_type TEXT, sim FLOAT4) AS $$
    SELECT id, canonical_name, node_type,
           similarity(canonical_name, p_name) AS sim
    FROM nodes
    WHERE similarity(canonical_name, p_name) >= p_threshold
    ORDER BY sim DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Alignment checks (supplement batch 2)
-- ================================================================

CREATE OR REPLACE FUNCTION sp_find_code_ahead(
    p_distance_threshold FLOAT8,
    p_limit INT DEFAULT 20
) RETURNS TABLE(
    canonical_name TEXT, node_type TEXT, domain TEXT,
    closest_dist FLOAT8, closest_name TEXT
) AS $$
    SELECT n.canonical_name, n.node_type,
           COALESCE(n.primary_domain, 'code'),
           (SELECT MIN(n.embedding <=> s.embedding)
            FROM nodes s
            WHERE s.primary_domain IN ('spec', 'design')
              AND s.entity_class = 'domain'
              AND s.embedding IS NOT NULL) AS closest_dist,
           (SELECT s.canonical_name
            FROM nodes s
            WHERE s.primary_domain IN ('spec', 'design')
              AND s.entity_class = 'domain'
              AND s.embedding IS NOT NULL
            ORDER BY n.embedding <=> s.embedding ASC
            LIMIT 1) AS closest_name
    FROM nodes n
    WHERE n.entity_class = 'code'
      AND n.embedding IS NOT NULL
      AND n.node_type NOT IN ('code_test', 'module')
      AND n.canonical_name NOT LIKE 'test_%'
      AND n.mention_count >= 2
      AND (
        (SELECT MIN(n.embedding <=> s.embedding)
         FROM nodes s
         WHERE s.primary_domain IN ('spec', 'design')
           AND s.entity_class = 'domain'
           AND s.embedding IS NOT NULL) > p_distance_threshold
        OR
        (SELECT MIN(n.embedding <=> s.embedding)
         FROM nodes s
         WHERE s.primary_domain IN ('spec', 'design')
           AND s.entity_class = 'domain'
           AND s.embedding IS NOT NULL) IS NULL
      )
    ORDER BY n.mention_count DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_find_design_contradictions(
    p_distance_threshold FLOAT8,
    p_limit INT DEFAULT 20
) RETURNS TABLE(
    design_name TEXT, design_type TEXT, distance FLOAT8, research_name TEXT
) AS $$
    SELECT d.canonical_name, d.node_type,
           (d.embedding <=> r.embedding) AS dist,
           r.canonical_name
    FROM nodes d
    JOIN nodes r ON r.primary_domain = 'research'
      AND r.entity_class = 'domain'
      AND r.embedding IS NOT NULL
      AND (d.embedding <=> r.embedding) < p_distance_threshold
    WHERE d.primary_domain = 'design'
      AND d.entity_class = 'domain'
      AND d.embedding IS NOT NULL
      AND d.mention_count >= 2
    ORDER BY dist ASC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_find_stale_design(
    p_limit INT DEFAULT 20
) RETURNS TABLE(
    title TEXT, days_behind FLOAT8, code_entities TEXT
) AS $$
    SELECT s.title,
           (EXTRACT(EPOCH FROM (MAX(n.last_seen) - s.ingested_at)) / 86400.0)::FLOAT8,
           STRING_AGG(DISTINCT n.canonical_name, ', ' ORDER BY n.canonical_name)
    FROM sources s
    JOIN chunks c ON c.source_id = s.id
    JOIN extractions ex ON ex.chunk_id = c.id AND ex.entity_type = 'node'
    JOIN nodes n ON n.id = ex.entity_id AND n.entity_class = 'code'
    WHERE s.domain = 'design'
    GROUP BY s.id, s.title, s.ingested_at
    HAVING MAX(n.last_seen) > s.ingested_at
    ORDER BY 2 DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Bootstrap: component linking
-- ================================================================

CREATE OR REPLACE FUNCTION sp_check_edge_exists(
    p_source_id UUID,
    p_target_id UUID,
    p_rel_type TEXT
) RETURNS BOOLEAN AS $$
    SELECT EXISTS (
        SELECT 1 FROM edges
        WHERE source_node_id = p_source_id
          AND target_node_id = p_target_id
          AND rel_type = p_rel_type
          AND invalid_at IS NULL
    );
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_list_component_nodes()
RETURNS TABLE(id UUID, canonical_name TEXT, embedding halfvec) AS $$
    SELECT id, canonical_name, embedding
    FROM nodes
    WHERE entity_class = 'analysis'
      AND node_type = 'component';
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Ask enrichment: graph edges for context
-- ================================================================

CREATE OR REPLACE FUNCTION sp_get_outgoing_edges(
    p_node_id UUID,
    p_rel_types TEXT[],
    p_limit INT DEFAULT 10
) RETURNS TABLE(target_name TEXT, rel_type TEXT) AS $$
    SELECT n.canonical_name, e.rel_type
    FROM edges e
    JOIN nodes n ON n.id = e.target_node_id
    WHERE e.source_node_id = p_node_id
      AND e.rel_type = ANY(p_rel_types)
      AND e.invalid_at IS NULL
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_get_incoming_edges(
    p_node_id UUID,
    p_rel_types TEXT[],
    p_limit INT DEFAULT 10
) RETURNS TABLE(source_name TEXT, rel_type TEXT) AS $$
    SELECT n.canonical_name, e.rel_type
    FROM edges e
    JOIN nodes n ON n.id = e.source_node_id
    WHERE e.target_node_id = p_node_id
      AND e.rel_type = ANY(p_rel_types)
      AND e.invalid_at IS NULL
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Admin: node queries
-- ================================================================

CREATE OR REPLACE FUNCTION sp_list_nodes_without_embeddings(
    p_limit INT DEFAULT 500
) RETURNS TABLE(id UUID, canonical_name TEXT, description TEXT) AS $$
    SELECT id, canonical_name, description
    FROM nodes
    WHERE embedding IS NULL
      AND description IS NOT NULL
      AND description != ''
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_list_nodes_with_embeddings(
    p_node_ids UUID[]
) RETURNS TABLE(id UUID) AS $$
    SELECT id FROM nodes
    WHERE id = ANY(p_node_ids)
      AND embedding IS NOT NULL;
$$ LANGUAGE sql STABLE;

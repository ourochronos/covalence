-- 006: All stored procedures
--
-- Organized by domain. ~67 stored procedures covering source
-- management, node CRUD, aliases, extractions, queue management,
-- cache, search dimensions, entity resolution, graph sync,
-- data health, coverage analysis, alignment, and pipeline.
--
-- Excluded (unused): sp_get_invalidated_neighbors,
-- sp_list_nodes_with_embeddings, sp_search_nodes_by_domain_vector,
-- sp_search_nodes_fuzzy_trigram, sp_search_table_vector,
-- temporal_search.

-- ================================================================
-- Source management
-- ================================================================

CREATE OR REPLACE FUNCTION sp_update_source_status(
    p_source_id UUID,
    p_status TEXT
) RETURNS VOID AS $$
    UPDATE sources SET status = p_status WHERE id = p_source_id;
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION sp_update_source_status_conditional(
    p_source_id UUID,
    p_new_status TEXT,
    p_unless_status TEXT
) RETURNS VOID AS $$
    UPDATE sources SET status = p_new_status
    WHERE id = p_source_id AND status != p_unless_status;
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION sp_update_source_processing(
    p_source_id UUID,
    p_stage TEXT,
    p_metadata JSONB
) RETURNS VOID AS $$
    UPDATE sources SET processing = jsonb_set(
        COALESCE(processing, '{}'), ARRAY[p_stage], p_metadata
    ) WHERE id = p_source_id;
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION sp_source_has_summary(
    p_source_id UUID
) RETURNS BOOLEAN AS $$
    SELECT summary IS NOT NULL FROM sources WHERE id = p_source_id;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_delete_source_cascade(
    p_source_id UUID
) RETURNS TABLE(
    extractions_deleted BIGINT,
    statements_deleted BIGINT,
    sections_deleted BIGINT,
    aliases_cleared BIGINT,
    chunks_deleted BIGINT,
    ledger_deleted BIGINT,
    jobs_cancelled BIGINT
) AS $$
DECLARE
    v_ext BIGINT;
    v_stmt BIGINT;
    v_sect BIGINT;
    v_alias BIGINT;
    v_chunk BIGINT;
    v_ledger BIGINT;
    v_jobs BIGINT;
BEGIN
    -- Cancel pending queue jobs
    DELETE FROM retry_jobs
    WHERE status IN ('pending', 'failed')
      AND payload->>'source_id' = p_source_id::text;
    GET DIAGNOSTICS v_jobs = ROW_COUNT;

    -- Extractions (via chunks)
    DELETE FROM extractions WHERE chunk_id IN (
        SELECT id FROM chunks WHERE source_id = p_source_id
    );
    GET DIAGNOSTICS v_ext = ROW_COUNT;

    -- Extractions (via statements)
    DELETE FROM extractions WHERE statement_id IN (
        SELECT id FROM statements WHERE source_id = p_source_id
    );
    v_ext := v_ext + ROW_COUNT;

    -- Statements, sections
    DELETE FROM sections WHERE source_id = p_source_id;
    GET DIAGNOSTICS v_sect = ROW_COUNT;

    DELETE FROM statements WHERE source_id = p_source_id;
    GET DIAGNOSTICS v_stmt = ROW_COUNT;

    -- Unresolved entities
    DELETE FROM unresolved_entities WHERE source_id = p_source_id;

    -- Clear alias chunk refs
    UPDATE node_aliases SET source_chunk_id = NULL
    WHERE source_chunk_id IN (
        SELECT id FROM chunks WHERE source_id = p_source_id
    );
    GET DIAGNOSTICS v_alias = ROW_COUNT;

    -- Chunks
    DELETE FROM chunks WHERE source_id = p_source_id;
    GET DIAGNOSTICS v_chunk = ROW_COUNT;

    -- Offset projection ledger
    DELETE FROM offset_projection_ledgers WHERE source_id = p_source_id;
    GET DIAGNOSTICS v_ledger = ROW_COUNT;

    -- Clear source embedding
    UPDATE sources SET embedding = NULL WHERE id = p_source_id;

    RETURN QUERY SELECT v_ext, v_stmt, v_sect, v_alias, v_chunk, v_ledger, v_jobs;
END;
$$ LANGUAGE plpgsql;

-- ================================================================
-- Node CRUD
-- ================================================================

CREATE OR REPLACE FUNCTION sp_get_node_by_name_exact(
    p_name TEXT
) RETURNS UUID AS $$
    SELECT id FROM nodes
    WHERE LOWER(canonical_name) = LOWER(p_name)
    LIMIT 1;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_bump_node_mention(
    p_node_id UUID
) RETURNS VOID AS $$
    UPDATE nodes
    SET mention_count = mention_count + 1,
        last_seen = NOW()
    WHERE id = p_node_id;
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION sp_get_node_properties(
    p_node_id UUID
) RETURNS JSONB AS $$
    SELECT properties FROM nodes WHERE id = p_node_id;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_update_node_ast_hash(
    p_node_id UUID,
    p_properties JSONB,
    p_description TEXT DEFAULT NULL
) RETURNS VOID AS $$
    UPDATE nodes
    SET properties = p_properties,
        description = COALESCE(p_description, description)
    WHERE id = p_node_id;
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION sp_update_node_ast_hash_only(
    p_node_id UUID,
    p_ast_hash TEXT
) RETURNS VOID AS $$
    UPDATE nodes
    SET properties = jsonb_set(
        COALESCE(properties, '{}'),
        '{ast_hash}', to_jsonb(p_ast_hash)
    )
    WHERE id = p_node_id;
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION sp_update_node_semantic_summary(
    p_node_id UUID,
    p_summary TEXT
) RETURNS VOID AS $$
    UPDATE nodes SET
        properties = jsonb_set(
            COALESCE(properties, '{}'),
            '{semantic_summary}',
            to_jsonb(p_summary)
        ),
        description = p_summary,
        embedding = NULL
    WHERE id = p_node_id;
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION sp_tombstone_node(
    p_node_id UUID
) RETURNS VOID AS $$
    UPDATE nodes SET clearance_level = -1 WHERE id = p_node_id;
$$ LANGUAGE sql;

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

-- ================================================================
-- Alias management
-- ================================================================

CREATE OR REPLACE FUNCTION sp_get_alias_by_text(
    p_alias TEXT
) RETURNS UUID AS $$
    SELECT node_id FROM node_aliases
    WHERE LOWER(alias) = LOWER(p_alias)
    LIMIT 1;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Extraction records
-- ================================================================

CREATE OR REPLACE FUNCTION sp_create_extraction_from_chunk(
    p_id UUID,
    p_chunk_id UUID,
    p_entity_id UUID,
    p_method TEXT,
    p_confidence FLOAT8
) RETURNS VOID AS $$
    INSERT INTO extractions (
        id, chunk_id, entity_type, entity_id,
        extraction_method, confidence, is_superseded
    ) VALUES (p_id, p_chunk_id, 'node', p_entity_id, p_method, p_confidence, false);
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION sp_create_extraction_from_statement(
    p_id UUID,
    p_statement_id UUID,
    p_entity_id UUID,
    p_method TEXT,
    p_confidence FLOAT8
) RETURNS VOID AS $$
    INSERT INTO extractions (
        id, statement_id, entity_type, entity_id,
        extraction_method, confidence, is_superseded
    ) VALUES (p_id, p_statement_id, 'node', p_entity_id, p_method, p_confidence, false);
$$ LANGUAGE sql;

-- ================================================================
-- Queue management (fan-in helpers)
-- ================================================================

CREATE OR REPLACE FUNCTION sp_count_pending_jobs_for_source(
    p_kind TEXT,
    p_source_id TEXT
) RETURNS BIGINT AS $$
    SELECT COUNT(*) FROM retry_jobs
    WHERE kind = p_kind::job_kind
      AND status IN ('pending', 'running')
      AND payload->>'source_id' = p_source_id;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_count_failed_jobs_for_source(
    p_kind TEXT,
    p_source_id TEXT
) RETURNS BIGINT AS $$
    SELECT COUNT(*) FROM retry_jobs
    WHERE kind = p_kind::job_kind
      AND status IN ('dead', 'failed')
      AND payload->>'source_id' = p_source_id;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Chunk queries (for extraction workers)
-- ================================================================

CREATE OR REPLACE FUNCTION sp_get_chunks_by_source(
    p_source_id UUID
) RETURNS TABLE(id UUID) AS $$
    SELECT id FROM chunks WHERE source_id = p_source_id;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_get_chunk_content_for_entity(
    p_node_id UUID,
    p_def_pattern TEXT
) RETURNS TEXT AS $$
    SELECT c.content FROM extractions ex
    JOIN chunks c ON c.id = ex.chunk_id
    WHERE ex.entity_id = p_node_id AND ex.entity_type = 'node'
      AND ex.chunk_id IS NOT NULL
      AND c.content LIKE '%' || p_def_pattern || '%'
    ORDER BY ex.confidence DESC
    LIMIT 1;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_get_chunk_by_source_pattern(
    p_source_id UUID,
    p_def_pattern TEXT
) RETURNS TEXT AS $$
    SELECT c.content FROM chunks c
    WHERE c.source_id = p_source_id
      AND c.content LIKE '%' || p_def_pattern || '%'
    ORDER BY LENGTH(c.content) ASC
    LIMIT 1;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_update_chunk_processing(
    p_chunk_id UUID,
    p_stage TEXT,
    p_metadata JSONB
) RETURNS VOID AS $$
    UPDATE chunks SET processing = jsonb_set(
        COALESCE(processing, '{}'), ARRAY[p_stage], p_metadata
    ) WHERE id = p_chunk_id;
$$ LANGUAGE sql;

-- ================================================================
-- Entity summary queries (for summarization workers)
-- ================================================================

CREATE OR REPLACE FUNCTION sp_get_unsummarized_entities_by_source(
    p_source_id UUID
) RETURNS TABLE(id UUID) AS $$
    SELECT DISTINCT n.id
    FROM nodes n
    JOIN extractions ex ON ex.entity_id = n.id AND ex.entity_type = 'node'
    JOIN chunks c ON c.id = ex.chunk_id
    WHERE c.source_id = p_source_id
      AND n.entity_class = 'code'
      AND (n.properties->>'semantic_summary' IS NULL
           OR n.properties->>'semantic_summary' = '')
      AND n.node_type != 'code_test'
      AND n.canonical_name NOT LIKE 'test_%';
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_get_entity_summaries_by_source(
    p_source_id UUID
) RETURNS TABLE(
    canonical_name TEXT,
    node_type TEXT,
    summary TEXT
) AS $$
    SELECT canonical_name, node_type,
           COALESCE(properties->>'semantic_summary',
                    description, canonical_name)
    FROM nodes n
    JOIN extractions ex ON ex.entity_id = n.id
      AND ex.entity_type = 'node'
    JOIN chunks c ON c.id = ex.chunk_id
    WHERE c.source_id = p_source_id
      AND n.entity_class = 'code'
    GROUP BY n.id, canonical_name, node_type,
             properties, description
    ORDER BY n.node_type, n.canonical_name;
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
-- Search: Vector (global dimension)
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

CREATE OR REPLACE FUNCTION sp_resolve_node_by_name(
    p_name TEXT
) RETURNS TABLE(id UUID, canonical_name TEXT, node_type TEXT) AS $$
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

-- ================================================================
-- Graph sync
-- ================================================================

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
           CASE causal_level
             WHEN 'intervention' THEN 1
             WHEN 'counterfactual' THEN 2
             ELSE 0
           END,
           clearance_level
    FROM edges
    WHERE invalid_at IS NULL;
$$ LANGUAGE sql STABLE;

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
-- Data health
-- ================================================================

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
        (SELECT COUNT(*) FROM (
           SELECT title, domains FROM sources
           WHERE title IS NOT NULL
           GROUP BY title, domains HAVING COUNT(*) > 1
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

CREATE OR REPLACE FUNCTION sp_invalidated_edge_stats()
RETURNS TABLE(
    total_invalidated BIGINT,
    total_valid BIGINT
) AS $$
    SELECT
        (SELECT COUNT(*) FROM edges WHERE invalid_at IS NOT NULL)::bigint,
        (SELECT COUNT(*) FROM edges WHERE invalid_at IS NULL)::bigint;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_top_invalidated_rel_types(
    p_limit INT DEFAULT 10
) RETURNS TABLE(
    rel_type TEXT,
    cnt BIGINT
) AS $$
    SELECT rel_type, COUNT(*) as cnt
    FROM edges
    WHERE invalid_at IS NOT NULL
    GROUP BY rel_type
    ORDER BY cnt DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Coverage analysis (parameterized domain versions)
-- ================================================================

CREATE OR REPLACE FUNCTION sp_get_orphan_code_nodes(
    p_code_domains TEXT[] DEFAULT ARRAY['code']
)
RETURNS TABLE(
    id UUID,
    canonical_name TEXT,
    node_type TEXT,
    mention_count INT
) AS $$
    SELECT n.id, n.canonical_name, n.node_type, n.mention_count
    FROM nodes n
    WHERE n.entity_class = 'code'
      AND n.primary_domain = ANY(p_code_domains)
      AND n.node_type != 'code_test'
      AND n.canonical_name NOT LIKE 'test_%'
      AND NOT EXISTS (
        SELECT 1 FROM edges e
        WHERE e.source_node_id = n.id
          AND e.rel_type = 'PART_OF_COMPONENT'
          AND e.invalid_at IS NULL
      )
    ORDER BY n.mention_count DESC;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_get_unimplemented_specs(
    p_spec_domains TEXT[] DEFAULT ARRAY['spec', 'design']
)
RETURNS TABLE(
    id UUID,
    canonical_name TEXT,
    node_type TEXT,
    mention_count INT
) AS $$
    SELECT n.id, n.canonical_name, n.node_type, n.mention_count
    FROM nodes n
    WHERE n.entity_class = 'domain'
      AND n.primary_domain = ANY(p_spec_domains)
      AND n.mention_count >= 2
      AND LENGTH(n.canonical_name) >= 3
      AND NOT EXISTS (
        SELECT 1 FROM edges e
        WHERE e.target_node_id = n.id
          AND e.rel_type = 'IMPLEMENTS_INTENT'
          AND e.invalid_at IS NULL
      )
    ORDER BY n.mention_count DESC;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_count_spec_concepts(
    p_spec_domains TEXT[] DEFAULT ARRAY['spec', 'design']
)
RETURNS BIGINT AS $$
    SELECT COUNT(*)
    FROM nodes n
    WHERE n.entity_class = 'domain'
      AND n.primary_domain = ANY(p_spec_domains)
      AND n.mention_count >= 2
      AND LENGTH(n.canonical_name) >= 3;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_count_implemented_specs(
    p_spec_domains TEXT[] DEFAULT ARRAY['spec', 'design']
)
RETURNS BIGINT AS $$
    SELECT COUNT(DISTINCT n.id)
    FROM nodes n
    WHERE n.entity_class = 'domain'
      AND n.primary_domain = ANY(p_spec_domains)
      AND n.mention_count >= 2
      AND LENGTH(n.canonical_name) >= 3
      AND EXISTS (
        SELECT 1 FROM edges e
        WHERE e.target_node_id = n.id
          AND e.rel_type = 'IMPLEMENTS_INTENT'
          AND e.invalid_at IS NULL
      );
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Node provenance
-- ================================================================

CREATE OR REPLACE FUNCTION sp_get_node_provenance_sources(
    p_node_ids UUID[]
) RETURNS TABLE(
    entity_id UUID,
    source_uri TEXT,
    source_title TEXT
) AS $$
    SELECT DISTINCT e.entity_id, s.uri, s.title
    FROM extractions e
    JOIN chunks c ON c.id = e.chunk_id
    JOIN sources s ON s.id = c.source_id
    WHERE e.entity_type = 'node'
      AND e.entity_id = ANY(p_node_ids)
      AND e.is_superseded = false;
$$ LANGUAGE sql STABLE;

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
-- Alignment checks (parameterized domain versions)
-- ================================================================

CREATE OR REPLACE FUNCTION sp_check_spec_ahead(
    p_limit INT DEFAULT 20,
    p_spec_domains TEXT[] DEFAULT ARRAY['spec', 'design']
)
RETURNS TABLE(
    canonical_name TEXT,
    node_type TEXT,
    mention_count INT
) AS $$
    SELECT n.canonical_name, n.node_type, n.mention_count
    FROM nodes n
    WHERE n.entity_class = 'domain'
      AND n.primary_domain = ANY(p_spec_domains)
      AND n.node_type NOT IN ('technology', 'actor')
      AND n.mention_count >= 2
      AND LENGTH(n.canonical_name) >= 3
      AND n.canonical_name !~ '^[a-z]+$'
      AND NOT EXISTS (
        SELECT 1 FROM edges e
        WHERE e.target_node_id = n.id
          AND e.rel_type = 'IMPLEMENTS_INTENT'
          AND e.invalid_at IS NULL
      )
    ORDER BY n.mention_count DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_find_code_ahead(
    p_distance_threshold FLOAT8,
    p_limit INT DEFAULT 20,
    p_code_class TEXT DEFAULT 'code',
    p_spec_domains TEXT[] DEFAULT ARRAY['spec', 'design']
) RETURNS TABLE(
    canonical_name TEXT, node_type TEXT, domain TEXT,
    closest_dist FLOAT8, closest_name TEXT
) AS $$
    SELECT n.canonical_name, n.node_type,
           COALESCE(n.primary_domain, 'code'),
           (SELECT MIN(n.embedding <=> s.embedding)
            FROM nodes s
            WHERE s.primary_domain = ANY(p_spec_domains)
              AND s.entity_class = 'domain'
              AND s.embedding IS NOT NULL) AS closest_dist,
           (SELECT s.canonical_name
            FROM nodes s
            WHERE s.primary_domain = ANY(p_spec_domains)
              AND s.entity_class = 'domain'
              AND s.embedding IS NOT NULL
            ORDER BY n.embedding <=> s.embedding ASC
            LIMIT 1) AS closest_name
    FROM nodes n
    WHERE n.entity_class = p_code_class
      AND n.embedding IS NOT NULL
      AND n.node_type NOT IN ('code_test', 'module')
      AND n.canonical_name NOT LIKE 'test_%'
      AND n.mention_count >= 2
      AND (
        (SELECT MIN(n.embedding <=> s.embedding)
         FROM nodes s
         WHERE s.primary_domain = ANY(p_spec_domains)
           AND s.entity_class = 'domain'
           AND s.embedding IS NOT NULL) > p_distance_threshold
        OR
        (SELECT MIN(n.embedding <=> s.embedding)
         FROM nodes s
         WHERE s.primary_domain = ANY(p_spec_domains)
           AND s.entity_class = 'domain'
           AND s.embedding IS NOT NULL) IS NULL
      )
    ORDER BY n.mention_count DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_find_design_contradictions(
    p_distance_threshold FLOAT8,
    p_limit INT DEFAULT 20,
    p_design_domains TEXT[] DEFAULT ARRAY['design'],
    p_research_domains TEXT[] DEFAULT ARRAY['research']
) RETURNS TABLE(
    design_name TEXT, design_type TEXT, distance FLOAT8, research_name TEXT
) AS $$
    SELECT d.canonical_name, d.node_type,
           (d.embedding <=> r.embedding) AS dist,
           r.canonical_name
    FROM nodes d
    JOIN nodes r ON r.primary_domain = ANY(p_research_domains)
      AND r.entity_class = 'domain'
      AND r.embedding IS NOT NULL
      AND (d.embedding <=> r.embedding) < p_distance_threshold
    WHERE d.primary_domain = ANY(p_design_domains)
      AND d.entity_class = 'domain'
      AND d.embedding IS NOT NULL
      AND d.mention_count >= 2
    ORDER BY dist ASC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_find_stale_design(
    p_limit INT DEFAULT 20,
    p_design_domains TEXT[] DEFAULT ARRAY['design']
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
    WHERE s.domains && p_design_domains
    GROUP BY s.id, s.title, s.ingested_at
    HAVING MAX(n.last_seen) > s.ingested_at
    ORDER BY 2 DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Research whitespace analysis (parameterized domain version)
-- ================================================================

CREATE OR REPLACE FUNCTION sp_get_research_source_bridges(
    p_min_cluster_size BIGINT DEFAULT 3,
    p_limit INT DEFAULT 100,
    p_domain_filter TEXT DEFAULT NULL,
    p_research_domains TEXT[] DEFAULT ARRAY['research', 'external']
)
RETURNS TABLE(
    source_id UUID,
    source_title TEXT,
    source_uri TEXT,
    total_entities BIGINT,
    bridged_entities BIGINT
) AS $$
    SELECT s.id, s.title, s.uri,
        COUNT(DISTINCT n.id) as total_entities,
        COUNT(DISTINCT CASE
            WHEN EXISTS (
                SELECT 1 FROM edges e
                WHERE (e.source_node_id = n.id OR e.target_node_id = n.id)
                  AND e.rel_type = 'THEORETICAL_BASIS'
                  AND e.invalid_at IS NULL
            ) THEN n.id END
        ) as bridged_entities
    FROM sources s
    JOIN chunks c ON c.source_id = s.id
    JOIN extractions ex ON ex.chunk_id = c.id AND ex.entity_type = 'node'
    JOIN nodes n ON n.id = ex.entity_id
    WHERE s.domains && p_research_domains
      AND s.superseded_by IS NULL
      AND n.mention_count >= 2
      AND (p_domain_filter IS NULL OR p_domain_filter = ANY(s.domains))
    GROUP BY s.id, s.title, s.uri
    HAVING COUNT(DISTINCT n.id) >= p_min_cluster_size
    ORDER BY total_entities DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Edge operations: cooccurrence synthesis (CTE-optimized)
-- ================================================================

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

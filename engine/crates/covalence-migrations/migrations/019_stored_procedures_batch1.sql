-- Migration 019: Stored procedures — Batch 1
--
-- Entity resolution + queue management SPs.
-- These are the hot path for ingestion and the first
-- requirement for the covalence-worker binary split (#175).

-- ================================================================
-- Source status management
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

-- ================================================================
-- Node CRUD (used by entity resolution)
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
-- Queue fan-in helpers
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
-- Source processing metadata
-- ================================================================

CREATE OR REPLACE FUNCTION sp_update_source_processing(
    p_source_id UUID,
    p_stage TEXT,
    p_metadata JSONB
) RETURNS VOID AS $$
    UPDATE sources SET processing = jsonb_set(
        COALESCE(processing, '{}'), ARRAY[p_stage], p_metadata
    ) WHERE id = p_source_id;
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

CREATE OR REPLACE FUNCTION sp_source_has_summary(
    p_source_id UUID
) RETURNS BOOLEAN AS $$
    SELECT summary IS NOT NULL FROM sources WHERE id = p_source_id;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Source cascade delete (the FK nightmare we hit manually)
-- ================================================================

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

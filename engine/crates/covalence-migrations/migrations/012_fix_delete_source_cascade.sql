-- Fix sp_delete_source_cascade: ROW_COUNT can only be read via
-- GET DIAGNOSTICS. The original used it as a bare identifier on
-- the statement-extractions branch, which PostgreSQL parses as a
-- column reference, failing with "column row_count does not exist"
-- and killing every process_source job that re-ingests an existing
-- source.

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
    v_ext2 BIGINT;
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

    -- Extractions (via statements). ROW_COUNT can only be read via
    -- GET DIAGNOSTICS — using it as a bare identifier fails with
    -- "column row_count does not exist".
    DELETE FROM extractions WHERE statement_id IN (
        SELECT id FROM statements WHERE source_id = p_source_id
    );
    GET DIAGNOSTICS v_ext2 = ROW_COUNT;
    v_ext := v_ext + v_ext2;

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

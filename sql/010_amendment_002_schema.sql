-- 010_amendment_002_schema.sql
-- Amendment 002: Handler Design Fixes — Schema Migration
-- Items: 1 (drop edge_type CHECK), 2 (collapse confidence), 4 (expand node status),
--        5 (AGE sync trigger), 6 (edge dedup constraint)
-- Idempotent: safe to re-run.

BEGIN;

--------------------------------------------------------------------
-- Item 1: Drop edges.edge_type CHECK constraint
--------------------------------------------------------------------
ALTER TABLE covalence.edges
    DROP CONSTRAINT IF EXISTS edges_edge_type_check;

COMMENT ON COLUMN covalence.edges.edge_type IS
    'Extensible string label. Validated in application (Rust EdgeType enum). No CHECK constraint — new edge types require no schema migration.';

--------------------------------------------------------------------
-- Item 2: Collapse 7 confidence columns → single `confidence`
--------------------------------------------------------------------

-- Migrate decomposed columns into metadata JSONB before dropping
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_schema = 'covalence' AND table_name = 'nodes'
          AND column_name = 'confidence_source'
    ) THEN
        UPDATE covalence.nodes SET metadata = jsonb_set(
            COALESCE(metadata, '{}'::jsonb),
            '{confidence_detail}',
            jsonb_build_object(
                'source', confidence_source,
                'method', confidence_method,
                'consistency', confidence_consistency,
                'freshness', confidence_freshness,
                'corroboration', confidence_corroboration,
                'applicability', confidence_applicability
            ),
            true
        ) WHERE confidence_source IS NOT NULL
           OR confidence_method IS NOT NULL;
    END IF;
END;
$$;

-- Rename confidence_overall → confidence (only if old name still exists)
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_schema = 'covalence' AND table_name = 'nodes'
          AND column_name = 'confidence_overall'
    ) THEN
        ALTER TABLE covalence.nodes RENAME COLUMN confidence_overall TO confidence;
    END IF;
END;
$$;

ALTER TABLE covalence.nodes ALTER COLUMN confidence SET DEFAULT 0.5;

-- Drop decomposed columns
ALTER TABLE covalence.nodes
    DROP COLUMN IF EXISTS confidence_source,
    DROP COLUMN IF EXISTS confidence_method,
    DROP COLUMN IF EXISTS confidence_consistency,
    DROP COLUMN IF EXISTS confidence_freshness,
    DROP COLUMN IF EXISTS confidence_corroboration,
    DROP COLUMN IF EXISTS confidence_applicability;

--------------------------------------------------------------------
-- Item 4: Expand nodes.status CHECK to add 'superseded' and 'disputed'
--------------------------------------------------------------------
ALTER TABLE covalence.nodes
    DROP CONSTRAINT IF EXISTS nodes_status_check;

ALTER TABLE covalence.nodes
    ADD CONSTRAINT nodes_status_check
    CHECK (status IN ('active', 'superseded', 'archived', 'disputed', 'tombstone'));

--------------------------------------------------------------------
-- Item 5: AGE sync trigger on edges (best-effort, skip if AGE not loaded)
--------------------------------------------------------------------
DO $outer$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'age') THEN

        EXECUTE $exec$
        CREATE OR REPLACE FUNCTION covalence.sync_edge_to_age()
        RETURNS TRIGGER
        LANGUAGE plpgsql AS $body$
        DECLARE
            _src_age_id BIGINT;
            _tgt_age_id BIGINT;
            _edge_result agtype;
        BEGIN
            SELECT age_id INTO _src_age_id FROM covalence.nodes WHERE id = NEW.source_node_id;
            SELECT age_id INTO _tgt_age_id FROM covalence.nodes WHERE id = NEW.target_node_id;

            IF _src_age_id IS NULL OR _tgt_age_id IS NULL THEN
                RETURN NEW;
            END IF;

            BEGIN
                EXECUTE format(
                    $cypher$
                    SELECT * FROM cypher('covalence', $age$
                        MATCH (s), (t)
                        WHERE id(s) = %s AND id(t) = %s
                        CREATE (s)-[e:%I {
                            sql_id: '%s',
                            confidence: %s,
                            created_at: '%s'
                        }]->(t)
                        RETURN id(e)
                    $age$) AS (edge_id agtype)
                    $cypher$,
                    _src_age_id, _tgt_age_id,
                    NEW.edge_type,
                    NEW.id,
                    COALESCE(NEW.confidence, 1.0),
                    COALESCE(NEW.created_at, now())
                ) INTO _edge_result;

                UPDATE covalence.edges SET age_id = (_edge_result::text)::bigint WHERE id = NEW.id;
            EXCEPTION WHEN others THEN
                RAISE WARNING 'AGE edge sync failed for edge %: %', NEW.id, SQLERRM;
            END;

            RETURN NEW;
        END;
        $body$;
        $exec$;

        DROP TRIGGER IF EXISTS trg_sync_edge_to_age ON covalence.edges;
        CREATE TRIGGER trg_sync_edge_to_age
            AFTER INSERT ON covalence.edges
            FOR EACH ROW
            EXECUTE FUNCTION covalence.sync_edge_to_age();

        RAISE NOTICE 'AGE sync trigger installed on covalence.edges';
    ELSE
        RAISE NOTICE 'AGE extension not loaded — skipping sync trigger installation';
    END IF;
EXCEPTION WHEN others THEN
    RAISE WARNING 'AGE sync trigger setup failed: %. Continuing without it.', SQLERRM;
END;
$outer$;

--------------------------------------------------------------------
-- Item 6: Edge dedup — remove duplicates, add unique constraint
--------------------------------------------------------------------

-- Remove existing duplicates (keep oldest by created_at, then smallest id)
DO $$
DECLARE
    _deleted INT := 1;
BEGIN
    WHILE _deleted > 0 LOOP
        DELETE FROM covalence.edges
        WHERE id IN (
            SELECT a.id FROM covalence.edges a
            JOIN covalence.edges b
              ON a.source_node_id = b.source_node_id
             AND a.target_node_id = b.target_node_id
             AND a.edge_type = b.edge_type
             AND (a.created_at > b.created_at
                  OR (a.created_at = b.created_at AND a.id > b.id))
            LIMIT 1000
        );
        GET DIAGNOSTICS _deleted = ROW_COUNT;
    END LOOP;
END;
$$;

CREATE UNIQUE INDEX IF NOT EXISTS edges_dedup_idx
    ON covalence.edges (source_node_id, target_node_id, edge_type);

COMMIT;

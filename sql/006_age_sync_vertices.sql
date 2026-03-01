LOAD 'age';
SET search_path = ag_catalog, "$user", public;

-- Batch create Source vertices
DO $body$
DECLARE
    r RECORD;
BEGIN
    FOR r IN SELECT id::text as uid, COALESCE(left(title, 200), '') as title
             FROM covalence.nodes WHERE status = 'active' AND node_type = 'source'
    LOOP
        BEGIN
            EXECUTE format(
                'SELECT * FROM cypher(''covalence'', $c$ CREATE (:Source {uid: %L, title: %L}) $c$) as (v agtype)',
                r.uid, replace(r.title, '''', '')
            );
        EXCEPTION WHEN OTHERS THEN
            RAISE NOTICE 'skip source %: %', r.uid, SQLERRM;
        END;
    END LOOP;
    RAISE NOTICE 'Sources done';
END $body$;

-- Batch create Article vertices
DO $body$
DECLARE
    r RECORD;
BEGIN
    FOR r IN SELECT id::text as uid, COALESCE(left(title, 200), '') as title
             FROM covalence.nodes WHERE status = 'active' AND node_type = 'article'
    LOOP
        BEGIN
            EXECUTE format(
                'SELECT * FROM cypher(''covalence'', $c$ CREATE (:Article {uid: %L, title: %L}) $c$) as (v agtype)',
                r.uid, replace(r.title, '''', '')
            );
        EXCEPTION WHEN OTHERS THEN
            RAISE NOTICE 'skip article %: %', r.uid, SQLERRM;
        END;
    END LOOP;
    RAISE NOTICE 'Articles done';
END $body$;

SELECT * FROM cypher('covalence', $$ MATCH (n) RETURN count(n) $$) as (cnt agtype);

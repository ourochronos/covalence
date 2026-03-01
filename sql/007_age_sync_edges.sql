LOAD 'age';
SET search_path = ag_catalog, "$user", public;

-- Batch create edges by matching source_node_id → target_node_id
DO $body$
DECLARE
    r RECORD;
    edge_label TEXT;
    q TEXT;
    created INT := 0;
    skipped INT := 0;
BEGIN
    FOR r IN SELECT e.source_node_id::text as src, e.target_node_id::text as tgt, e.edge_type
             FROM covalence.edges e
             JOIN covalence.nodes ns ON ns.id = e.source_node_id AND ns.status = 'active'
             JOIN covalence.nodes nt ON nt.id = e.target_node_id AND nt.status = 'active'
    LOOP
        edge_label := r.edge_type;
        BEGIN
            q := format(
                'SELECT * FROM cypher(''covalence'', $c$
                    MATCH (a {uid: %L}), (b {uid: %L})
                    CREATE (a)-[:%s]->(b)
                $c$) as (v agtype)',
                r.src, r.tgt, edge_label
            );
            EXECUTE q;
            created := created + 1;
        EXCEPTION WHEN OTHERS THEN
            skipped := skipped + 1;
        END;
    END LOOP;
    RAISE NOTICE 'Edges created: %, skipped: %', created, skipped;
END $body$;

SELECT * FROM cypher('covalence', $$ MATCH ()-[e]->() RETURN count(e) $$) as (cnt agtype);

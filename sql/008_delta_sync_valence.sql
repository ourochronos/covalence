-- ============================================================================
-- Covalence: Delta Sync from Valence v2
-- ============================================================================
-- Re-syncs new/updated sources, articles, edges, contentions, usage traces.
-- Safe to run repeatedly — uses ON CONFLICT + change detection.
-- Run ON the covalence DB (port 5434).
-- After running: curl -X POST http://localhost:8430/admin/embed-all
-- Then re-run 006 + 007 for AGE graph sync.
-- ============================================================================

-- Step 1: Delta Sources
INSERT INTO covalence.nodes (
    id, node_type, source_type, title, content, content_hash, fingerprint,
    size_tokens, reliability, metadata, status, created_at, modified_at
)
SELECT id, 'source', type, title, content,
    COALESCE(content_hash, fingerprint), fingerprint,
    COALESCE(length(content)/4, 0), COALESCE(reliability::float8, 0.5),
    COALESCE(metadata, '{}'), 'active', created_at, created_at
FROM dblink(
    'host=host.docker.internal port=5433 dbname=valence user=valence password=valence',
    'SELECT id, type, title, content, content_hash, fingerprint, reliability, metadata, created_at FROM sources'
) AS s(id uuid, type text, title text, content text, content_hash text,
       fingerprint text, reliability numeric, metadata jsonb, created_at timestamptz)
ON CONFLICT (id) DO UPDATE SET
    content = EXCLUDED.content, content_hash = EXCLUDED.content_hash,
    title = EXCLUDED.title, metadata = EXCLUDED.metadata, modified_at = now()
WHERE covalence.nodes.content_hash IS DISTINCT FROM EXCLUDED.content_hash;

-- Step 2: Delta Articles
INSERT INTO covalence.nodes (
    id, node_type, title, content, content_hash, size_tokens,
    confidence_overall, confidence_source, confidence_method,
    confidence_consistency, confidence_freshness, confidence_corroboration,
    confidence_applicability, domain_path, version, status, pinned,
    usage_score, created_at, modified_at
)
SELECT id, 'article', title, content, COALESCE(content_hash, md5(content)),
    COALESCE(size_tokens, length(content)/4),
    COALESCE(confidence_source*0.30 + confidence_method*0.15 + confidence_consistency*0.20 +
             confidence_freshness*0.10 + confidence_corroboration*0.15 + confidence_applicability*0.10, 0.5),
    confidence_source::float8, confidence_method::float8, confidence_consistency::float8,
    confidence_freshness::float8, confidence_corroboration::float8, confidence_applicability::float8,
    domain_path, COALESCE(version, 1), status, COALESCE(pinned, false),
    COALESCE(usage_score::float8, 0.5), created_at, COALESCE(modified_at, created_at)
FROM dblink(
    'host=host.docker.internal port=5433 dbname=valence user=valence password=valence',
    'SELECT id, title, content, content_hash, size_tokens,
            confidence_source, confidence_method, confidence_consistency,
            confidence_freshness, confidence_corroboration, confidence_applicability,
            domain_path, version, status, pinned, usage_score, created_at, modified_at
     FROM articles'
) AS a(id uuid, title text, content text, content_hash text, size_tokens int,
       confidence_source real, confidence_method real, confidence_consistency real,
       confidence_freshness real, confidence_corroboration real, confidence_applicability real,
       domain_path text[], version int, status text, pinned boolean, usage_score numeric,
       created_at timestamptz, modified_at timestamptz)
ON CONFLICT (id) DO UPDATE SET
    content = EXCLUDED.content, content_hash = EXCLUDED.content_hash,
    title = EXCLUDED.title, version = EXCLUDED.version, status = EXCLUDED.status,
    confidence_overall = EXCLUDED.confidence_overall, usage_score = EXCLUDED.usage_score,
    modified_at = now()
WHERE covalence.nodes.content_hash IS DISTINCT FROM EXCLUDED.content_hash
   OR covalence.nodes.version IS DISTINCT FROM EXCLUDED.version
   OR covalence.nodes.status IS DISTINCT FROM EXCLUDED.status;

-- Step 3: Delta Provenance Edges
INSERT INTO covalence.edges (
    id, source_node_id, target_node_id, edge_type, weight, confidence, created_by, created_at
)
SELECT id, source_id, article_id,
    CASE relationship
        WHEN 'originates' THEN 'ORIGINATES' WHEN 'compiled_from' THEN 'ORIGINATES'
        WHEN 'confirms' THEN 'CONFIRMS' WHEN 'contradicts' THEN 'CONTRADICTS'
        WHEN 'contends' THEN 'CONTENDS' ELSE 'ORIGINATES'
    END, 1.0, 1.0, 'delta_sync', COALESCE(added_at, now())
FROM dblink(
    'host=host.docker.internal port=5433 dbname=valence user=valence password=valence',
    'SELECT id, article_id, source_id, relationship, added_at FROM article_sources'
) AS p(id uuid, article_id uuid, source_id uuid, relationship text, added_at timestamptz)
ON CONFLICT (id) DO NOTHING;

-- Step 4: Delta Supersession Edges
INSERT INTO covalence.edges (
    source_node_id, target_node_id, edge_type, weight, confidence, created_by
)
SELECT id, supersedes_id, 'SUPERSEDES', 1.0, 1.0, 'delta_sync'
FROM dblink(
    'host=host.docker.internal port=5433 dbname=valence user=valence password=valence',
    'SELECT id, supersedes_id FROM sources WHERE supersedes_id IS NOT NULL'
) AS s(id uuid, supersedes_id uuid)
WHERE NOT EXISTS (
    SELECT 1 FROM covalence.edges e
    WHERE e.source_node_id = s.id AND e.target_node_id = s.supersedes_id AND e.edge_type = 'SUPERSEDES'
);

-- Step 5: Delta Contentions
INSERT INTO covalence.contentions (
    id, node_id, source_node_id, type, description, severity,
    status, resolution, detected_at, resolved_at, materiality
)
SELECT id, article_id, source_id, type, description, severity,
    status, resolution, detected_at, resolved_at, materiality::float8
FROM dblink(
    'host=host.docker.internal port=5433 dbname=valence user=valence password=valence',
    'SELECT id, article_id, source_id, type, description, severity, status,
            resolution, detected_at, resolved_at, materiality FROM contentions'
) AS c(id uuid, article_id uuid, source_id uuid, type text, description text,
       severity text, status text, resolution text, detected_at timestamptz,
       resolved_at timestamptz, materiality numeric)
ON CONFLICT (id) DO NOTHING;

-- Step 6: Delta Usage Traces
INSERT INTO covalence.usage_traces (id, node_id, session_id, query_text, retrieval_rank, accessed_at)
SELECT id, node_id, session_id, query_text, retrieval_rank, accessed_at
FROM dblink(
    'host=host.docker.internal port=5433 dbname=valence user=valence password=valence',
    'SELECT id, article_id, session_id::text, query_text, NULL::int, retrieved_at
     FROM usage_traces WHERE article_id IS NOT NULL'
) AS u(id uuid, node_id uuid, session_id text, query_text text, retrieval_rank int, accessed_at timestamptz)
ON CONFLICT (id) DO NOTHING;

-- Verification
SELECT 'nodes' as tbl, count(*) as total,
       count(*) FILTER (WHERE node_type = 'source') as sources,
       count(*) FILTER (WHERE node_type = 'article') as articles
FROM covalence.nodes
UNION ALL
SELECT 'edges', count(*), NULL, NULL FROM covalence.edges
UNION ALL
SELECT 'contentions', count(*), NULL, NULL FROM covalence.contentions
UNION ALL
SELECT 'usage_traces', count(*), NULL, NULL FROM covalence.usage_traces;

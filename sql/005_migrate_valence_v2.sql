-- ============================================================================
-- Covalence: Valence v2 → Covalence Migration (Issue #8)
-- ============================================================================
-- Source: valence-pg (port 5433), public schema
-- Target: covalence-pg (port 5434), covalence schema
--
-- This script runs ON the covalence DB using dblink to read from valence.
-- Prerequisites: CREATE EXTENSION IF NOT EXISTS dblink;
-- ============================================================================

-- Step 0: dblink extension
CREATE EXTENSION IF NOT EXISTS dblink;

-- ============================================================================
-- Step 1: Migrate Sources (278 rows)
-- ============================================================================
-- Valence sources → Covalence nodes (node_type='source')
INSERT INTO covalence.nodes (
    id, node_type, source_type, title, content, content_hash, fingerprint,
    size_tokens, reliability, metadata, status, created_at, modified_at
)
SELECT
    id,
    'source'::text,
    type,
    title,
    content,
    COALESCE(content_hash, fingerprint),
    fingerprint,
    COALESCE(length(content) / 4, 0),  -- rough token estimate
    COALESCE(reliability::double precision, 0.5),
    COALESCE(metadata, '{}'),
    'active'::text,
    created_at,
    created_at
FROM dblink(
    'host=host.docker.internal port=5433 dbname=valence user=valence password=valence',
    'SELECT id, type, title, content, content_hash, fingerprint, reliability, metadata, created_at FROM sources'
) AS s(
    id uuid, type text, title text, content text, content_hash text,
    fingerprint text, reliability numeric, metadata jsonb, created_at timestamptz
)
ON CONFLICT (id) DO NOTHING;

-- ============================================================================
-- Step 2: Migrate Articles (298 rows)
-- ============================================================================
INSERT INTO covalence.nodes (
    id, node_type, title, content, content_hash, size_tokens,
    confidence_overall, confidence_source, confidence_method,
    confidence_consistency, confidence_freshness, confidence_corroboration,
    confidence_applicability, domain_path, version, status, pinned,
    usage_score, created_at, modified_at
)
SELECT
    id,
    'article'::text,
    title,
    content,
    COALESCE(content_hash, md5(content)),
    COALESCE(size_tokens, length(content) / 4),
    COALESCE(confidence_source * 0.30 + confidence_method * 0.15 +
             confidence_consistency * 0.20 + confidence_freshness * 0.10 +
             confidence_corroboration * 0.15 + confidence_applicability * 0.10, 0.5),
    confidence_source::double precision,
    confidence_method::double precision,
    confidence_consistency::double precision,
    confidence_freshness::double precision,
    confidence_corroboration::double precision,
    confidence_applicability::double precision,
    domain_path,
    COALESCE(version, 1),
    status,
    COALESCE(pinned, false),
    COALESCE(usage_score::double precision, 0.5),
    created_at,
    COALESCE(modified_at, created_at)
FROM dblink(
    'host=host.docker.internal port=5433 dbname=valence user=valence password=valence',
    'SELECT id, title, content, content_hash, size_tokens,
            confidence_source, confidence_method, confidence_consistency,
            confidence_freshness, confidence_corroboration, confidence_applicability,
            domain_path, version, status, pinned, usage_score,
            created_at, modified_at
     FROM articles'
) AS a(
    id uuid, title text, content text, content_hash text, size_tokens int,
    confidence_source real, confidence_method real, confidence_consistency real,
    confidence_freshness real, confidence_corroboration real, confidence_applicability real,
    domain_path text[], version int, status text, pinned boolean, usage_score numeric,
    created_at timestamptz, modified_at timestamptz
)
ON CONFLICT (id) DO NOTHING;

-- ============================================================================
-- Step 3: Provenance Edges — article_sources → ORIGINATES edges (603 rows)
-- ============================================================================
INSERT INTO covalence.edges (
    id, source_node_id, target_node_id, edge_type, weight, confidence, created_by, created_at
)
SELECT
    id,
    source_id,     -- source is the origin
    article_id,    -- article is the target
    CASE relationship
        WHEN 'originates' THEN 'ORIGINATES'
        WHEN 'compiled_from' THEN 'ORIGINATES'  -- canonical mapping
        WHEN 'confirms' THEN 'CONFIRMS'
        WHEN 'contradicts' THEN 'CONTRADICTS'
        WHEN 'contends' THEN 'CONTENDS'
        ELSE 'ORIGINATES'
    END,
    1.0,
    1.0,
    'migration',
    COALESCE(added_at, now())
FROM dblink(
    'host=host.docker.internal port=5433 dbname=valence user=valence password=valence',
    'SELECT id, article_id, source_id, relationship, added_at FROM article_sources'
) AS p(id uuid, article_id uuid, source_id uuid, relationship text, added_at timestamptz)
ON CONFLICT (id) DO NOTHING;

-- ============================================================================
-- Step 4: Supersession Edges (sources with supersedes_id)
-- ============================================================================
INSERT INTO covalence.edges (
    id, source_node_id, target_node_id, edge_type, weight, confidence, created_by
)
SELECT
    gen_random_uuid(),
    id,               -- new source supersedes old
    supersedes_id,    -- old source being superseded
    'SUPERSEDES',
    1.0, 1.0, 'migration'
FROM dblink(
    'host=host.docker.internal port=5433 dbname=valence user=valence password=valence',
    'SELECT id, supersedes_id FROM sources WHERE supersedes_id IS NOT NULL'
) AS s(id uuid, supersedes_id uuid)
ON CONFLICT DO NOTHING;

-- ============================================================================
-- Step 5: Contentions (33 rows)
-- ============================================================================
INSERT INTO covalence.contentions (
    id, node_id, source_node_id, type, description, severity,
    status, resolution, detected_at, resolved_at, materiality
)
SELECT
    id,
    article_id,
    source_id,
    type,
    description,
    severity,
    status,
    resolution,
    detected_at,
    resolved_at,
    materiality::double precision
FROM dblink(
    'host=host.docker.internal port=5433 dbname=valence user=valence password=valence',
    'SELECT id, article_id, source_id, type, description, severity, status,
            resolution, detected_at, resolved_at, materiality
     FROM contentions'
) AS c(
    id uuid, article_id uuid, source_id uuid, type text, description text,
    severity text, status text, resolution text, detected_at timestamptz,
    resolved_at timestamptz, materiality numeric
)
ON CONFLICT (id) DO NOTHING;

-- ============================================================================
-- Step 6: Usage Traces (3351 rows)
-- ============================================================================
INSERT INTO covalence.usage_traces (id, node_id, session_id, query_text, retrieval_rank, accessed_at)
SELECT
    id, node_id, session_id, query_text, retrieval_rank, accessed_at
FROM dblink(
    'host=host.docker.internal port=5433 dbname=valence user=valence password=valence',
    'SELECT id, article_id, session_id::text, query_text, retrieval_rank, accessed_at
     FROM usage_traces WHERE article_id IS NOT NULL'
) AS u(id uuid, node_id uuid, session_id text, query_text text, retrieval_rank int, accessed_at timestamptz)
ON CONFLICT (id) DO NOTHING;

-- ============================================================================
-- Step 7: Tombstones
-- ============================================================================
-- (0 rows in valence, skip)

-- ============================================================================
-- Step 8: Generate content_tsv for all migrated nodes
-- ============================================================================
-- content_tsv is a GENERATED column, so it auto-populates on INSERT. No action needed.

-- ============================================================================
-- Verification queries
-- ============================================================================
-- Run these manually after migration:
-- SELECT 'nodes' as tbl, count(*) FROM covalence.nodes
-- UNION ALL SELECT 'edges', count(*) FROM covalence.edges
-- UNION ALL SELECT 'contentions', count(*) FROM covalence.contentions
-- UNION ALL SELECT 'usage_traces', count(*) FROM covalence.usage_traces;

-- Migration 020: Stored procedures — Batch 2
--
-- Analysis, health, and alignment SPs.

-- ================================================================
-- Data health report
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
           SELECT title, domain FROM sources
           WHERE title IS NOT NULL
           GROUP BY title, domain HAVING COUNT(*) > 1
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

-- ================================================================
-- Coverage analysis
-- ================================================================

CREATE OR REPLACE FUNCTION sp_get_orphan_code_nodes()
RETURNS TABLE(
    id UUID,
    canonical_name TEXT,
    node_type TEXT,
    mention_count INT
) AS $$
    SELECT n.id, n.canonical_name, n.node_type, n.mention_count
    FROM nodes n
    WHERE n.entity_class = 'code'
      AND n.primary_domain = 'code'
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

CREATE OR REPLACE FUNCTION sp_get_unimplemented_specs()
RETURNS TABLE(
    id UUID,
    canonical_name TEXT,
    node_type TEXT,
    mention_count INT
) AS $$
    SELECT n.id, n.canonical_name, n.node_type, n.mention_count
    FROM nodes n
    WHERE n.entity_class = 'domain'
      AND n.primary_domain IN ('spec', 'design')
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

CREATE OR REPLACE FUNCTION sp_count_spec_concepts()
RETURNS BIGINT AS $$
    SELECT COUNT(*)
    FROM nodes n
    WHERE n.entity_class = 'domain'
      AND n.primary_domain IN ('spec', 'design')
      AND n.mention_count >= 2
      AND LENGTH(n.canonical_name) >= 3;
$$ LANGUAGE sql STABLE;

CREATE OR REPLACE FUNCTION sp_count_implemented_specs()
RETURNS BIGINT AS $$
    SELECT COUNT(DISTINCT n.id)
    FROM nodes n
    WHERE n.entity_class = 'domain'
      AND n.primary_domain IN ('spec', 'design')
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
-- Alignment checks
-- ================================================================

CREATE OR REPLACE FUNCTION sp_check_spec_ahead(
    p_limit INT DEFAULT 20
) RETURNS TABLE(
    canonical_name TEXT,
    node_type TEXT,
    mention_count INT
) AS $$
    SELECT n.canonical_name, n.node_type, n.mention_count
    FROM nodes n
    WHERE n.entity_class = 'domain'
      AND n.primary_domain IN ('spec', 'design')
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

-- ================================================================
-- Cooccurrence edge synthesis
-- ================================================================

CREATE OR REPLACE FUNCTION sp_find_cooccurrence_pairs(
    p_min_cooccurrences INT DEFAULT 2,
    p_max_degree INT DEFAULT 500
) RETURNS TABLE(
    source_node_id UUID,
    target_node_id UUID,
    cooccurrence_count BIGINT
) AS $$
    WITH chunk_pairs AS (
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
    WHERE NOT EXISTS (
        SELECT 1 FROM edges e
        WHERE e.source_node_id = pf.src
          AND e.target_node_id = pf.tgt
          AND e.rel_type = 'co_occurs'
    )
    AND (SELECT COUNT(*) FROM edges WHERE source_node_id = pf.src OR target_node_id = pf.src) < p_max_degree
    AND (SELECT COUNT(*) FROM edges WHERE source_node_id = pf.tgt OR target_node_id = pf.tgt) < p_max_degree;
$$ LANGUAGE sql STABLE;

-- ================================================================
-- Invalidated edge stats
-- ================================================================

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
-- Node provenance lookup
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

-- ================================================================
-- Research whitespace analysis
-- ================================================================

CREATE OR REPLACE FUNCTION sp_get_research_source_bridges()
RETURNS TABLE(
    source_id UUID,
    source_title TEXT,
    total_entities BIGINT,
    bridged_entities BIGINT
) AS $$
    SELECT s.id, s.title,
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
    WHERE s.domain IN ('research', 'external')
      AND s.superseded_by IS NULL
      AND n.mention_count >= 2
    GROUP BY s.id, s.title
    HAVING COUNT(DISTINCT n.id) >= 3;
$$ LANGUAGE sql STABLE;

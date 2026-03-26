-- Migration 012: Parameterize domain-filtering stored procedures
--
-- Replaces hardcoded domain literals in 9 SPs with configurable
-- parameters that default to the current values. This lets custom
-- domain configurations work without changing the SP definitions.

-- 1. sp_get_orphan_code_nodes — add p_code_domains
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

-- 2. sp_get_unimplemented_specs — add p_spec_domains
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

-- 3. sp_count_spec_concepts — add p_spec_domains
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

-- 4. sp_count_implemented_specs — add p_spec_domains
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

-- 5. sp_check_spec_ahead — add p_spec_domains
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

-- 6. sp_find_code_ahead — add p_code_class, p_spec_domains
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

-- 7. sp_find_design_contradictions — add p_design_domains, p_research_domains
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

-- 8. sp_find_stale_design — add p_design_domains
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
    WHERE s.domain = ANY(p_design_domains)
    GROUP BY s.id, s.title, s.ingested_at
    HAVING MAX(n.last_seen) > s.ingested_at
    ORDER BY 2 DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

-- 9. sp_get_research_source_bridges — add p_research_domains
--    Also add missing p_min_cluster_size, p_limit, p_domain_filter
--    params that the Rust code already passes.
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
    WHERE s.domain = ANY(p_research_domains)
      AND s.superseded_by IS NULL
      AND n.mention_count >= 2
      AND (p_domain_filter IS NULL OR s.domain = p_domain_filter)
    GROUP BY s.id, s.title, s.uri
    HAVING COUNT(DISTINCT n.id) >= p_min_cluster_size
    ORDER BY total_entities DESC
    LIMIT p_limit;
$$ LANGUAGE sql STABLE;

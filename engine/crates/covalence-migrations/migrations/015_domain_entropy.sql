-- 015: Domain entropy per node
-- Measures how cross-cutting an entity is across knowledge domains.
-- Low entropy = internal concept (primarily one domain).
-- High entropy = cross-cutting (appears in many domains).
-- Used by DDSS search routing for self-referential intent detection.

ALTER TABLE nodes ADD COLUMN IF NOT EXISTS domain_entropy REAL;
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS primary_domain TEXT;

-- Backfill domain_entropy and primary_domain from extraction provenance.
-- For each node, count mentions per source domain, compute Shannon entropy,
-- and identify the primary (most common) domain.
WITH domain_counts AS (
    SELECT
        n.id AS node_id,
        s.domain,
        COUNT(*) AS cnt
    FROM nodes n
    JOIN extractions ex ON ex.entity_id = n.id AND ex.entity_type = 'node'
    LEFT JOIN chunks c ON c.id = ex.chunk_id
    LEFT JOIN statements st ON st.id = ex.statement_id
    JOIN sources s ON s.id = COALESCE(c.source_id, st.source_id)
    WHERE s.domain IS NOT NULL
    GROUP BY n.id, s.domain
),
node_totals AS (
    SELECT
        node_id,
        SUM(cnt) AS total,
        MAX(cnt) AS max_cnt
    FROM domain_counts
    GROUP BY node_id
),
entropy_calc AS (
    SELECT
        dc.node_id,
        -- Shannon entropy: -sum(p * log2(p))
        -SUM(
            (dc.cnt::float / nt.total::float) *
            LN(dc.cnt::float / nt.total::float) / LN(2)
        ) AS entropy,
        -- Primary domain: domain with the most mentions
        (SELECT domain FROM domain_counts dc2
         WHERE dc2.node_id = dc.node_id
         ORDER BY dc2.cnt DESC LIMIT 1) AS primary_domain
    FROM domain_counts dc
    JOIN node_totals nt ON nt.node_id = dc.node_id
    GROUP BY dc.node_id
)
UPDATE nodes
SET domain_entropy = ec.entropy,
    primary_domain = ec.primary_domain
FROM entropy_calc ec
WHERE nodes.id = ec.node_id;

-- Index for filtering by primary domain
CREATE INDEX IF NOT EXISTS idx_nodes_primary_domain ON nodes (primary_domain);

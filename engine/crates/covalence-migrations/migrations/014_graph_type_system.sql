-- 014: Graph Type System — Labels, Layers, and Enforcement
-- ADR-0018: Adds project/domain to sources, entity_class to nodes.
-- All additive — no existing columns modified or removed.

-- ============================================================
-- Source labels: project and domain
-- ============================================================

-- Project namespace (default 'covalence' for all existing sources)
ALTER TABLE sources ADD COLUMN IF NOT EXISTS project TEXT NOT NULL DEFAULT 'covalence';

-- Domain classification: code, spec, design, research, external
ALTER TABLE sources ADD COLUMN IF NOT EXISTS domain TEXT;

-- Index for domain-based filtering in search and analysis
CREATE INDEX IF NOT EXISTS idx_sources_domain ON sources (domain);
CREATE INDEX IF NOT EXISTS idx_sources_project ON sources (project);

-- Backfill domain from URI patterns
UPDATE sources SET domain = CASE
    -- Code sources (by source_type)
    WHEN source_type = 'code' THEN 'code'
    -- Spec documents
    WHEN uri LIKE 'file://spec/%' THEN 'spec'
    -- Design documents (ADRs, vision, design docs, project meta)
    WHEN uri LIKE 'file://docs/adr/%' THEN 'design'
    WHEN uri LIKE 'file://VISION%' THEN 'design'
    WHEN uri LIKE 'file://CLAUDE%' THEN 'design'
    WHEN uri LIKE 'file://MILESTONES%' THEN 'design'
    WHEN uri LIKE 'file://design/%' THEN 'design'
    -- Code files by URI pattern (file://engine/, file://cli/, file://dashboard/)
    WHEN uri LIKE 'file://engine/%' THEN 'code'
    WHEN uri LIKE 'file://cli/%' THEN 'code'
    WHEN uri LIKE 'file://dashboard/%' THEN 'code'
    -- Research papers (arxiv, doi)
    WHEN uri LIKE 'https://arxiv%' THEN 'research'
    WHEN uri LIKE 'https://doi%' THEN 'research'
    -- Other HTTP sources default to research
    WHEN uri LIKE 'http://%' OR uri LIKE 'https://%' THEN 'research'
    -- Remaining documents default to external
    WHEN source_type = 'document' THEN 'external'
    -- Fallback
    ELSE NULL
END
WHERE domain IS NULL;

-- ============================================================
-- Entity classification on nodes
-- ============================================================

ALTER TABLE nodes ADD COLUMN IF NOT EXISTS entity_class TEXT;

-- Index for entity_class filtering in search
CREATE INDEX IF NOT EXISTS idx_nodes_entity_class ON nodes (entity_class);

-- Backfill entity_class from node_type
-- Use COALESCE(canonical_type, node_type) to prefer the ontology-clustered type
UPDATE nodes SET entity_class = CASE
    -- Code entities
    WHEN COALESCE(canonical_type, node_type) IN (
        'function', 'struct', 'trait', 'enum', 'impl_block',
        'constant', 'module', 'class', 'macro',
        'code_function', 'code_struct', 'code_trait', 'code_module',
        'code_impl', 'code_type', 'code_test'
    ) THEN 'code'
    -- Actor entities
    WHEN COALESCE(canonical_type, node_type) IN (
        'person', 'organization', 'location', 'role'
    ) THEN 'actor'
    -- Analysis entities (system-generated)
    WHEN COALESCE(canonical_type, node_type) = 'component' THEN 'analysis'
    -- Everything else is a domain concept
    ELSE 'domain'
END
WHERE entity_class IS NULL;

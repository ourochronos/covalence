-- Migration 011: Domain Generalization
--
-- Transforms domains from a fixed 5-value taxonomy into configurable
-- visibility scopes. Sources can belong to multiple domains. Analysis
-- rules are data-driven. The graph stores facts; domains determine
-- how you view them.

-- 1. Multi-domain sources
ALTER TABLE sources ADD COLUMN IF NOT EXISTS domains TEXT[] NOT NULL DEFAULT '{}';
UPDATE sources SET domains = ARRAY[domain] WHERE domain IS NOT NULL AND domains = '{}';
CREATE INDEX IF NOT EXISTS idx_sources_domains_gin ON sources USING GIN (domains);

-- 2. Domain groups (named sets of domains for analysis scoping)
CREATE TABLE IF NOT EXISTS domain_groups (
    group_name TEXT NOT NULL,
    domain_id  TEXT NOT NULL REFERENCES ontology_domains(id),
    sort_order INT DEFAULT 0,
    PRIMARY KEY (group_name, domain_id)
);

INSERT INTO domain_groups (group_name, domain_id, sort_order) VALUES
    ('specification', 'spec',     1),
    ('specification', 'design',   2),
    ('evidence',      'research', 1),
    ('evidence',      'external', 2),
    ('implementation','code',     1)
ON CONFLICT DO NOTHING;

-- 3. Alignment rules (replace hardcoded checks)
CREATE TABLE IF NOT EXISTS alignment_rules (
    id          SERIAL PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    description TEXT,
    check_type  TEXT NOT NULL CHECK (check_type IN ('ahead', 'contradiction', 'staleness')),
    source_group TEXT NOT NULL,
    target_group TEXT NOT NULL,
    parameters  JSONB NOT NULL DEFAULT '{}',
    is_active   BOOLEAN NOT NULL DEFAULT true,
    sort_order  INT DEFAULT 0
);

INSERT INTO alignment_rules (name, description, check_type, source_group, target_group, parameters) VALUES
    ('code_ahead',
     'Code entities with no matching spec concept',
     'ahead', 'implementation', 'specification', '{}'),
    ('spec_ahead',
     'Spec concepts with no implementing code',
     'ahead', 'specification', 'implementation', '{}'),
    ('design_contradicted',
     'Design decisions potentially contradicted by research',
     'contradiction', 'specification', 'evidence',
     '{"source_domain": "design", "target_domain": "research"}'),
    ('stale_design',
     'Design docs whose descriptions diverge from code reality',
     'staleness', 'specification', 'implementation',
     '{"source_domain": "design"}')
ON CONFLICT (name) DO NOTHING;

-- 4. Domain classification rules (replace hardcoded derive_domain)
CREATE TABLE IF NOT EXISTS domain_rules (
    id          SERIAL PRIMARY KEY,
    priority    INT NOT NULL DEFAULT 100,
    match_type  TEXT NOT NULL CHECK (match_type IN ('source_type', 'uri_prefix', 'uri_regex')),
    match_value TEXT NOT NULL,
    domain_id   TEXT NOT NULL REFERENCES ontology_domains(id),
    description TEXT,
    is_active   BOOLEAN NOT NULL DEFAULT true
);

INSERT INTO domain_rules (priority, match_type, match_value, domain_id, description) VALUES
    (10,  'source_type', 'code',              'code',     'Code sources'),
    (20,  'uri_prefix',  'file://spec/',       'spec',     'Spec documents'),
    (30,  'uri_prefix',  'file://docs/adr/',   'design',   'ADR design docs'),
    (31,  'uri_prefix',  'file://VISION',      'design',   'Vision doc'),
    (32,  'uri_prefix',  'file://CLAUDE',      'design',   'Project instructions'),
    (33,  'uri_prefix',  'file://MILESTONES',  'design',   'Milestone docs'),
    (34,  'uri_prefix',  'file://design/',     'design',   'Design docs'),
    (40,  'uri_prefix',  'file://engine/',     'code',     'Engine source'),
    (41,  'uri_prefix',  'file://cli/',        'code',     'CLI source'),
    (42,  'uri_prefix',  'file://dashboard/',  'code',     'Dashboard source'),
    (50,  'uri_prefix',  'https://arxiv',      'research', 'ArXiv papers'),
    (51,  'uri_prefix',  'https://doi',        'research', 'DOI papers'),
    (60,  'uri_prefix',  'http://',            'research', 'HTTP URLs'),
    (61,  'uri_prefix',  'https://',           'research', 'HTTPS URLs'),
    (100, 'source_type', 'document',           'external', 'Remaining documents')
ON CONFLICT DO NOTHING;

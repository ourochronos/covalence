-- Migration 025: Ontology Schema
--
-- Configurable knowledge schema per ADR-0022. Entity types,
-- relationship types, entity categories, domains, and view
-- mappings are stored in the database instead of hardcoded.
--
-- The "default" ontology is seeded with current Covalence values
-- so existing behavior is preserved.

-- ================================================================
-- Entity categories (Level 1 universals from ADR-0022)
-- ================================================================

CREATE TABLE IF NOT EXISTS ontology_categories (
    id          TEXT PRIMARY KEY,  -- 'concept', 'process', etc.
    label       TEXT NOT NULL,
    description TEXT,
    sort_order  INT DEFAULT 0
);

INSERT INTO ontology_categories (id, label, description, sort_order) VALUES
    ('concept',    'Concept',    'An abstract idea, definition, or named thing', 1),
    ('process',    'Process',    'Something that transforms, computes, or produces', 2),
    ('artifact',   'Artifact',   'A concrete, addressable, citable thing', 3),
    ('agent',      'Agent',      'Something that acts, decides, or creates', 4),
    ('property',   'Property',   'A measurable attribute or quality', 5),
    ('collection', 'Collection', 'A grouping, containment, or namespace', 6)
ON CONFLICT (id) DO NOTHING;

-- ================================================================
-- Entity types (Level 2 domain-specific, maps to categories)
-- ================================================================

CREATE TABLE IF NOT EXISTS ontology_entity_types (
    id          TEXT PRIMARY KEY,  -- 'function', 'theorem', etc.
    category    TEXT NOT NULL REFERENCES ontology_categories(id),
    label       TEXT NOT NULL,
    description TEXT,
    is_active   BOOLEAN NOT NULL DEFAULT true
);

-- Seed with current Covalence entity types
INSERT INTO ontology_entity_types (id, category, label) VALUES
    -- Code (process)
    ('function',   'process',    'Function'),
    ('method',     'process',    'Method'),
    ('macro',      'process',    'Macro'),
    -- Code (concept)
    ('struct',     'concept',    'Struct'),
    ('enum',       'concept',    'Enum'),
    ('trait',      'concept',    'Trait'),
    ('constant',   'concept',    'Constant'),
    ('impl_block', 'concept',    'Impl Block'),
    -- Code (collection)
    ('module',     'collection', 'Module'),
    -- Domain concepts
    ('concept',    'concept',    'Concept'),
    ('technology', 'concept',    'Technology'),
    -- Artifacts
    ('component',  'collection', 'Component'),
    ('community_summary', 'artifact', 'Community Summary'),
    -- Agents
    ('person',     'agent',      'Person'),
    ('organization', 'agent',    'Organization'),
    -- Research
    ('dataset',    'artifact',   'Dataset'),
    ('metric',     'property',   'Metric'),
    -- Analysis
    ('code_test',  'process',    'Code Test')
ON CONFLICT (id) DO NOTHING;

-- ================================================================
-- Universal relationship types (Level 1 from ADR-0022)
-- ================================================================

CREATE TABLE IF NOT EXISTS ontology_rel_universals (
    id          TEXT PRIMARY KEY,  -- 'is_a', 'part_of', etc.
    label       TEXT NOT NULL,
    description TEXT,
    is_symmetric BOOLEAN DEFAULT false
);

INSERT INTO ontology_rel_universals (id, label, description, is_symmetric) VALUES
    ('is_a',         'Is A',          'Classification, taxonomy', false),
    ('part_of',      'Part Of',       'Composition, containment', false),
    ('depends_on',   'Depends On',    'Dependency, prerequisite', false),
    ('derived_from', 'Derived From',  'Derivation, production', false),
    ('supports',     'Supports',      'Epistemic agreement, evidence', false),
    ('contradicts',  'Contradicts',   'Epistemic disagreement', true),
    ('precedes',     'Precedes',      'Temporal ordering', false),
    ('uses',         'Uses',          'Reference, invocation', false)
ON CONFLICT (id) DO NOTHING;

-- ================================================================
-- Relationship types (Level 2 domain-specific, maps to universals)
-- ================================================================

CREATE TABLE IF NOT EXISTS ontology_rel_types (
    id          TEXT PRIMARY KEY,  -- 'calls', 'IMPLEMENTS_INTENT', etc.
    universal   TEXT REFERENCES ontology_rel_universals(id),
    label       TEXT NOT NULL,
    description TEXT,
    is_active   BOOLEAN NOT NULL DEFAULT true
);

-- Seed with current Covalence relationship types
INSERT INTO ontology_rel_types (id, universal, label) VALUES
    -- Code structural
    ('calls',              'uses',         'Calls'),
    ('uses_type',          'uses',         'Uses Type'),
    ('imports',            'depends_on',   'Imports'),
    ('implements',         'derived_from', 'Implements'),
    ('extends',            'is_a',         'Extends'),
    ('contains',           'part_of',      'Contains'),
    -- Analysis bridges
    ('PART_OF_COMPONENT',  'part_of',      'Part Of Component'),
    ('IMPLEMENTS_INTENT',  'derived_from', 'Implements Intent'),
    ('THEORETICAL_BASIS',  'derived_from', 'Theoretical Basis'),
    -- Epistemic
    ('supports',           'supports',     'Supports'),
    ('contradicts',        'contradicts',  'Contradicts'),
    ('evaluated_on',       'uses',         'Evaluated On'),
    ('outperformed_on',    'supports',     'Outperformed On'),
    ('compared_to',        'uses',         'Compared To'),
    -- General
    ('co_occurs',          NULL,           'Co-occurs'),
    ('is_part_of',         'part_of',      'Is Part Of'),
    ('applies_to',         'uses',         'Applies To'),
    ('enables',            'supports',     'Enables'),
    ('used_in',            'uses',         'Used In'),
    ('mentioned_at',       'uses',         'Mentioned At')
ON CONFLICT (id) DO NOTHING;

-- ================================================================
-- Domain classification
-- ================================================================

CREATE TABLE IF NOT EXISTS ontology_domains (
    id          TEXT PRIMARY KEY,  -- 'code', 'research', etc.
    label       TEXT NOT NULL,
    description TEXT,
    is_internal BOOLEAN DEFAULT false,  -- for DDSS self-referential boost
    sort_order  INT DEFAULT 0
);

INSERT INTO ontology_domains (id, label, is_internal, sort_order) VALUES
    ('code',     'Code',     true,  1),
    ('spec',     'Spec',     true,  2),
    ('design',   'Design',   true,  3),
    ('research', 'Research', false, 4),
    ('external', 'External', false, 5)
ON CONFLICT (id) DO NOTHING;

-- ================================================================
-- View → edge type mappings (which edges appear in which view)
-- ================================================================

CREATE TABLE IF NOT EXISTS ontology_view_edges (
    view_name   TEXT NOT NULL,     -- 'causal', 'temporal', 'entity', 'structural'
    rel_type    TEXT NOT NULL REFERENCES ontology_rel_types(id),
    PRIMARY KEY (view_name, rel_type)
);

-- Causal view edges
INSERT INTO ontology_view_edges (view_name, rel_type) VALUES
    ('causal', 'enables'),
    ('causal', 'supports'),
    ('causal', 'contradicts')
ON CONFLICT DO NOTHING;

-- Entity view edges
INSERT INTO ontology_view_edges (view_name, rel_type) VALUES
    ('entity', 'implements'),
    ('entity', 'extends'),
    ('entity', 'is_part_of'),
    ('entity', 'uses_type'),
    ('entity', 'applies_to'),
    ('entity', 'used_in')
ON CONFLICT DO NOTHING;

-- Structural view edges
INSERT INTO ontology_view_edges (view_name, rel_type) VALUES
    ('structural', 'calls'),
    ('structural', 'uses_type'),
    ('structural', 'contains'),
    ('structural', 'imports')
ON CONFLICT DO NOTHING;

-- ================================================================
-- Noise patterns (domain-specific, replaces hardcoded noise_filter)
-- ================================================================

CREATE TABLE IF NOT EXISTS ontology_noise_patterns (
    id          SERIAL PRIMARY KEY,
    pattern     TEXT NOT NULL,          -- regex or literal
    pattern_type TEXT NOT NULL DEFAULT 'literal', -- 'literal', 'regex', 'prefix', 'suffix'
    description TEXT,
    is_active   BOOLEAN NOT NULL DEFAULT true
);

-- Seed with a few representative patterns (the full 57-test noise
-- filter will be migrated incrementally)
INSERT INTO ontology_noise_patterns (pattern, pattern_type, description) VALUES
    ('the', 'literal', 'Common article'),
    ('this', 'literal', 'Common demonstrative'),
    ('true', 'literal', 'Boolean literal'),
    ('false', 'literal', 'Boolean literal'),
    ('GET', 'literal', 'HTTP method'),
    ('POST', 'literal', 'HTTP method'),
    ('PUT', 'literal', 'HTTP method'),
    ('DELETE', 'literal', 'HTTP method')
ON CONFLICT DO NOTHING;

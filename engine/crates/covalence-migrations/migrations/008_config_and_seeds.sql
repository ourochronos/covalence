-- 008: Configuration table and seed data
--
-- Runtime config table, ontology seed data (categories, entity types,
-- relationship types, domains, view edges, noise patterns), and
-- default source adapter seeds.

-- ================================================================
-- Config table (runtime-adjustable settings)
-- ================================================================

CREATE TABLE IF NOT EXISTS config (
    key         TEXT PRIMARY KEY,
    value       JSONB NOT NULL,
    description TEXT,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Seed with current defaults
INSERT INTO config (key, value, description) VALUES
    ('queue.process_concurrency', '4', 'Max concurrent process_source/reprocess jobs'),
    ('queue.extract_concurrency', '24', 'Max concurrent extract_chunk jobs (I/O bound)'),
    ('queue.summarize_concurrency', '10', 'Max concurrent summarize_entity jobs'),
    ('queue.compose_concurrency', '5', 'Max concurrent compose_source_summary jobs'),
    ('queue.edge_concurrency', '1', 'Max concurrent synthesize_edges jobs'),
    ('queue.embed_concurrency', '4', 'Max concurrent embed_batch jobs'),
    ('queue.job_timeout_secs', '900', 'Max seconds per job before timeout'),
    ('pipeline.coref_enabled', 'true', 'Enable neural coreference resolution'),
    ('pipeline.statement_enabled', 'true', 'Enable statement extraction pipeline'),
    ('pipeline.tier5_enabled', 'true', 'Enable HDBSCAN Tier 5 entity resolution'),
    ('search.cache_ttl_secs', '3600', 'Search cache TTL in seconds'),
    ('search.cache_max_entries', '10000', 'Max entries in search cache'),
    ('search.rerank_weight', '0.6', 'Reranker weight in CC fusion (0-1)')
ON CONFLICT (key) DO NOTHING;

-- ================================================================
-- Ontology: Entity categories
-- ================================================================

INSERT INTO ontology_categories (id, label, description, sort_order) VALUES
    ('concept',    'Concept',    'An abstract idea, definition, or named thing', 1),
    ('process',    'Process',    'Something that transforms, computes, or produces', 2),
    ('artifact',   'Artifact',   'A concrete, addressable, citable thing', 3),
    ('agent',      'Agent',      'Something that acts, decides, or creates', 4),
    ('property',   'Property',   'A measurable attribute or quality', 5),
    ('collection', 'Collection', 'A grouping, containment, or namespace', 6)
ON CONFLICT (id) DO NOTHING;

-- ================================================================
-- Ontology: Entity types
-- ================================================================

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
-- Ontology: Universal relationship types
-- ================================================================

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
-- Ontology: Relationship types
-- ================================================================

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
-- Ontology: Domains
-- ================================================================

INSERT INTO ontology_domains (id, label, is_internal, sort_order) VALUES
    ('code',     'Code',     true,  1),
    ('spec',     'Spec',     true,  2),
    ('design',   'Design',   true,  3),
    ('research', 'Research', false, 4),
    ('external', 'External', false, 5)
ON CONFLICT (id) DO NOTHING;

-- ================================================================
-- Ontology: View-to-edge-type mappings
-- ================================================================

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
-- Ontology: Noise patterns
-- ================================================================

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

-- ================================================================
-- Source adapter seeds
-- ================================================================

INSERT INTO source_adapters (name, description, match_domain, match_mime, converter, default_source_type, default_domain) VALUES
    ('arxiv-pdf', 'ArXiv research papers (PDF)', 'arxiv.org', 'application/pdf', 'pdf', 'document', 'research'),
    ('arxiv-html', 'ArXiv research papers (HTML)', 'arxiv.org', 'text/html', 'passthrough', 'document', 'research'),
    ('github-code', 'GitHub source code files', 'github.com', NULL, 'code', 'code', 'code'),
    ('markdown', 'Markdown documents', NULL, 'text/markdown', 'passthrough', 'document', NULL),
    ('html-web', 'Web pages (HTML)', NULL, 'text/html', 'readerlm', 'document', 'external'),
    ('pdf-generic', 'PDF documents', NULL, 'application/pdf', 'pdf', 'document', NULL),
    ('code-rust', 'Rust source files', NULL, 'text/x-rust', 'code', 'code', 'code'),
    ('code-go', 'Go source files', NULL, 'text/x-go', 'code', 'code', 'code'),
    ('code-python', 'Python source files', NULL, 'text/x-python', 'code', 'code', 'code')
ON CONFLICT (name) DO NOTHING;

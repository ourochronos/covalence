-- 002: Ontology type system tables (no seed data)
--
-- Configurable knowledge schema per ADR-0022. Entity types,
-- relationship types, entity categories, domains, view edge
-- mappings, and noise patterns. Seed data is in 008_config_and_seeds.sql.

-- ================================================================
-- Entity categories (Level 1 universals)
-- ================================================================

CREATE TABLE IF NOT EXISTS ontology_categories (
    id          TEXT PRIMARY KEY,
    label       TEXT NOT NULL,
    description TEXT,
    sort_order  INT DEFAULT 0
);

-- ================================================================
-- Entity types (Level 2 domain-specific, maps to categories)
-- ================================================================

CREATE TABLE IF NOT EXISTS ontology_entity_types (
    id          TEXT PRIMARY KEY,
    category    TEXT NOT NULL REFERENCES ontology_categories(id),
    label       TEXT NOT NULL,
    description TEXT,
    is_active   BOOLEAN NOT NULL DEFAULT true
);

-- ================================================================
-- Universal relationship types (Level 1)
-- ================================================================

CREATE TABLE IF NOT EXISTS ontology_rel_universals (
    id           TEXT PRIMARY KEY,
    label        TEXT NOT NULL,
    description  TEXT,
    is_symmetric BOOLEAN DEFAULT false
);

-- ================================================================
-- Relationship types (Level 2 domain-specific, maps to universals)
-- ================================================================

CREATE TABLE IF NOT EXISTS ontology_rel_types (
    id          TEXT PRIMARY KEY,
    universal   TEXT REFERENCES ontology_rel_universals(id),
    label       TEXT NOT NULL,
    description TEXT,
    is_active   BOOLEAN NOT NULL DEFAULT true
);

-- ================================================================
-- Domain classification
-- ================================================================

CREATE TABLE IF NOT EXISTS ontology_domains (
    id          TEXT PRIMARY KEY,
    label       TEXT NOT NULL,
    description TEXT,
    is_internal BOOLEAN DEFAULT false,
    sort_order  INT DEFAULT 0
);

-- ================================================================
-- View-to-edge-type mappings
-- ================================================================

CREATE TABLE IF NOT EXISTS ontology_view_edges (
    view_name TEXT NOT NULL,
    rel_type  TEXT NOT NULL REFERENCES ontology_rel_types(id),
    PRIMARY KEY (view_name, rel_type)
);

-- ================================================================
-- Noise patterns (domain-specific, replaces hardcoded noise_filter)
-- ================================================================

CREATE TABLE IF NOT EXISTS ontology_noise_patterns (
    id           SERIAL PRIMARY KEY,
    pattern      TEXT NOT NULL,
    pattern_type TEXT NOT NULL DEFAULT 'literal',
    description  TEXT,
    is_active    BOOLEAN NOT NULL DEFAULT true
);

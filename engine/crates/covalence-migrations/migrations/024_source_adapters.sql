-- Migration 024: Source adapter configuration
--
-- Data-driven source adapters stored as JSONB. Each adapter
-- defines how to handle a specific source type: which converter
-- to use, which prompt template, which normalization profile.
--
-- No code plugins needed for 80% of cases — just config.

CREATE TABLE IF NOT EXISTS source_adapters (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL UNIQUE,
    description TEXT,
    -- Matching rules: when should this adapter be used?
    match_domain    TEXT,          -- e.g., 'arxiv.org', 'github.com'
    match_mime      TEXT,          -- e.g., 'application/pdf', 'text/html'
    match_uri_regex TEXT,          -- e.g., '^https://arxiv\.org/'
    -- Pipeline configuration
    converter       TEXT,          -- 'pdf', 'readerlm', 'code', 'passthrough'
    normalization   TEXT DEFAULT 'default', -- normalization profile name
    prompt_template TEXT,          -- extraction prompt template name (from engine/prompts/)
    -- Source metadata defaults
    default_source_type TEXT DEFAULT 'document',
    default_domain      TEXT,      -- 'research', 'code', 'spec', etc.
    -- Webhook for custom processing (Phase 4)
    webhook_url     TEXT,          -- external HTTP endpoint for custom conversion
    -- Feature flags
    coref_enabled   BOOLEAN DEFAULT true,
    statement_enabled BOOLEAN DEFAULT true,
    -- Metadata
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    is_active   BOOLEAN NOT NULL DEFAULT true
);

-- Seed with built-in adapters matching current behavior
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

CREATE INDEX IF NOT EXISTS idx_source_adapters_active
    ON source_adapters(is_active) WHERE is_active = true;

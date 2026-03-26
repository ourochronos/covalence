-- 009: Metadata schema storage for extension-declared validation.
--
-- Stores JSON Schemas declared by extensions for validating entity
-- and source metadata during ingestion. Each schema is scoped to
-- either an entity type or a source domain, with a UNIQUE constraint
-- ensuring one schema per (scope, scope_id).
--
-- The `metadata_enforcement` config key controls validation behavior:
-- 'ignore' (default), 'warn', or 'strict'.

CREATE TABLE IF NOT EXISTS metadata_schemas (
    id          SERIAL PRIMARY KEY,
    scope       TEXT NOT NULL,  -- 'entity_type' or 'source_domain'
    scope_id    TEXT NOT NULL,  -- entity type id or domain id
    schema      JSONB NOT NULL,
    extension   TEXT NOT NULL,  -- which extension declared this
    UNIQUE (scope, scope_id)
);

-- Seed the metadata_enforcement config key with 'ignore' default.
INSERT INTO config (key, value, description) VALUES
    ('metadata_enforcement', '"warn"',
     'Metadata schema enforcement level: ignore, warn, or strict')
ON CONFLICT (key) DO NOTHING;

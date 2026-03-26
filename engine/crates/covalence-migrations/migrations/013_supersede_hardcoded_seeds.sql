-- Migration 013: Document seed supersession
--
-- The ontology seeds in migration 008 (entity types, relationship types,
-- domains, view edges, noise patterns) are now superseded by extension
-- manifests in extensions/. Migration 008 used ON CONFLICT DO NOTHING
-- so its seeds are harmless — this migration documents the transition.
--
-- Source of truth: extensions/{core,code-analysis,spec-design,research}/extension.yaml
SELECT 1;  -- No-op migration for documentation

-- Lifecycle hooks for extensible /ask pipeline integration.
-- External HTTP endpoints called at pre_search, post_search, and
-- post_synthesis phases.

CREATE TABLE IF NOT EXISTS lifecycle_hooks (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT NOT NULL UNIQUE,
    phase       TEXT NOT NULL CHECK (phase IN ('pre_search', 'post_search', 'post_synthesis')),
    hook_url    TEXT NOT NULL,
    adapter_id  UUID REFERENCES source_adapters(id) ON DELETE CASCADE,
    timeout_ms  INT NOT NULL DEFAULT 2000,
    fail_open   BOOLEAN NOT NULL DEFAULT true,
    is_active   BOOLEAN NOT NULL DEFAULT true,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS idx_hooks_phase ON lifecycle_hooks (phase) WHERE is_active = true;

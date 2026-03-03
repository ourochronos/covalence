-- covalence#47: Namespace isolation.
-- Adds a `namespace` column to all node-related tables so multiple isolated
-- knowledge graphs can share one Covalence instance.
--
-- All existing rows receive the DEFAULT 'default' namespace, which is
-- backward-compatible with every existing query path.

-- ── nodes ─────────────────────────────────────────────────────────────────────
ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS namespace TEXT NOT NULL DEFAULT 'default';

CREATE INDEX IF NOT EXISTS nodes_namespace_idx
    ON covalence.nodes (namespace);

CREATE INDEX IF NOT EXISTS nodes_namespace_status_idx
    ON covalence.nodes (namespace, status);

-- ── edges ─────────────────────────────────────────────────────────────────────
-- Both endpoints of an edge share the same namespace (same graph), so the
-- column lives here for efficient filtering without requiring a JOIN to nodes.
ALTER TABLE covalence.edges
    ADD COLUMN IF NOT EXISTS namespace TEXT NOT NULL DEFAULT 'default';

CREATE INDEX IF NOT EXISTS edges_namespace_idx
    ON covalence.edges (namespace);

-- ── node_embeddings ───────────────────────────────────────────────────────────
ALTER TABLE covalence.node_embeddings
    ADD COLUMN IF NOT EXISTS namespace TEXT NOT NULL DEFAULT 'default';

CREATE INDEX IF NOT EXISTS node_embeddings_namespace_idx
    ON covalence.node_embeddings (namespace);

-- ── node_sections ─────────────────────────────────────────────────────────────
ALTER TABLE covalence.node_sections
    ADD COLUMN IF NOT EXISTS namespace TEXT NOT NULL DEFAULT 'default';

CREATE INDEX IF NOT EXISTS node_sections_namespace_idx
    ON covalence.node_sections (namespace);

-- ── usage_traces ──────────────────────────────────────────────────────────────
ALTER TABLE covalence.usage_traces
    ADD COLUMN IF NOT EXISTS namespace TEXT NOT NULL DEFAULT 'default';

CREATE INDEX IF NOT EXISTS usage_traces_namespace_idx
    ON covalence.usage_traces (namespace);

-- ── contentions ───────────────────────────────────────────────────────────────
ALTER TABLE covalence.contentions
    ADD COLUMN IF NOT EXISTS namespace TEXT NOT NULL DEFAULT 'default';

CREATE INDEX IF NOT EXISTS contentions_namespace_idx
    ON covalence.contentions (namespace);

-- =============================================================================
-- Migration 030: KB Navigation Landmarks (covalence#112)
-- 
-- Adds is_landmark column to nodes table so auto-generated domain overview
-- and topology map articles can be flagged as landmark nodes that are never
-- evicted from the knowledge base.
-- =============================================================================

ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS is_landmark BOOLEAN NOT NULL DEFAULT false;

COMMENT ON COLUMN covalence.nodes.is_landmark IS
    'True for auto-generated KB topology and domain landmark articles (covalence#112). '
    'Landmark nodes are excluded from organic eviction.';

CREATE INDEX IF NOT EXISTS idx_nodes_is_landmark
    ON covalence.nodes(is_landmark)
    WHERE is_landmark = true;

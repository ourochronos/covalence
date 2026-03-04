-- Migration 028: EWC structural importance — structural_importance score + eviction guard
-- (covalence#101)

-- Add structural_importance column to nodes table
ALTER TABLE covalence.nodes
  ADD COLUMN IF NOT EXISTS structural_importance FLOAT DEFAULT 0.0;

-- Index for eviction queries (ORDER BY evict_score)
CREATE INDEX IF NOT EXISTS idx_nodes_structural_importance
  ON covalence.nodes(structural_importance);

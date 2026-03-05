-- Migration 042: Claims Layer — Generation tracking and temporal claims (covalence#169)
--
-- This migration adds support for the claims layer blue-green migration by:
--   1. Adding a `generation` column to track schema/content evolution
--      - Generation 1 = pre-claims articles (current state)
--      - Generation 2+ = claims-based articles and claim nodes
--   2. Adding `valid_until` for temporal claims support (from covalence#185)
--      - Nullable timestamp for claims with temporal validity
--      - Used for time-bounded facts (e.g., "X is CEO of Y" until date Z)
--   3. Adding composite indexes for efficient generation-aware queries
--
-- Design decisions:
--   - Claims are nodes with node_type='claim' (no new tables)
--   - All existing infrastructure (embeddings, FTS, graph traversal) works
--   - New edge types: SUPPORTS, CONTAINS, CONTRADICTS_CLAIM, SAME_AS
--   - Blue-green cutover via generation column enables atomic rollback
--
-- Safe to re-run: uses IF NOT EXISTS and IF EXISTS for idempotency.
-- Indexes created within SQLx transaction (non-concurrent).

-- Add generation tracking for blue-green migration
ALTER TABLE covalence.nodes 
    ADD COLUMN IF NOT EXISTS generation INTEGER DEFAULT 1;

COMMENT ON COLUMN covalence.nodes.generation IS 
    'Blue-green migration generation. Gen 1 = pre-claims articles. Gen 2+ = claims-based articles and claim nodes. Enables atomic cutover and rollback.';

-- Add temporal validity support for time-bounded claims (covalence#185)
ALTER TABLE covalence.nodes 
    ADD COLUMN IF NOT EXISTS valid_until TIMESTAMPTZ;

COMMENT ON COLUMN covalence.nodes.valid_until IS 
    'Optional temporal validity end timestamp for claims. NULL = no expiration. Used for time-bounded facts. From covalence#185: valid_until + source lifecycle coupling, no confidence decay.';

-- Composite index for generation-based queries during migration and cutover
CREATE INDEX IF NOT EXISTS idx_nodes_generation_type_status 
    ON covalence.nodes (generation, node_type, status);

COMMENT ON INDEX covalence.idx_nodes_generation_type_status IS 
    'Supports generation-aware queries during claims layer migration (e.g., "all gen 2 articles in draft status").';

-- Index for temporal claim queries and validity filtering
CREATE INDEX IF NOT EXISTS idx_nodes_valid_until 
    ON covalence.nodes (valid_until) 
    WHERE valid_until IS NOT NULL;

COMMENT ON INDEX covalence.idx_nodes_valid_until IS 
    'Supports temporal claim queries (find claims expiring soon, filter expired claims). Partial index: only rows with non-null valid_until.';

-- Composite index for active temporal claims
CREATE INDEX IF NOT EXISTS idx_nodes_temporal_active 
    ON covalence.nodes (node_type, status, valid_until) 
    WHERE node_type = 'claim' AND valid_until IS NOT NULL;

COMMENT ON INDEX covalence.idx_nodes_temporal_active IS 
    'Optimized for queries on active temporal claims. Partial index: claim nodes with temporal validity only.';

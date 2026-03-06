-- Migration 044: add 'claim' to nodes_node_type_check constraint (covalence#202 partial)
--
-- Migration 042 (claims layer, PR #191, covalence#169) added support for
-- node_type='claim' but did not update the CHECK constraint on covalence.nodes.
-- This caused ALL claim extraction tasks (~1,489) to fail with:
--
--   new row for relation "nodes" violates check constraint "nodes_node_type_check"
--
-- This migration drops the old constraint and recreates it with 'claim' included.
--
-- This fix was manually applied to production on 2026-03-05 to unblock claim
-- extraction. This migration ensures the schema change is captured in version
-- control and can be replayed on test/dev databases.
--
-- Safe to re-run: uses IF EXISTS for the DROP, and the constraint definition
-- is idempotent. If the constraint already includes 'claim', this migration
-- completes successfully without error.

BEGIN;

-- Drop the old constraint (safe if already dropped or doesn't exist)
ALTER TABLE covalence.nodes 
    DROP CONSTRAINT IF EXISTS nodes_node_type_check;

-- Add the new constraint with 'claim' included
ALTER TABLE covalence.nodes 
    ADD CONSTRAINT nodes_node_type_check 
    CHECK (node_type = ANY (ARRAY['article', 'source', 'entity', 'claim']));

COMMIT;

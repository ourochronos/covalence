-- Migration 041: Drop age_id columns (covalence#192)
--
-- Apache AGE has been removed from Covalence. The age_id columns in
-- covalence.nodes and covalence.edges are no longer used and can be dropped.
-- This is a pure cleanup migration with no behavioral changes.

ALTER TABLE covalence.nodes DROP COLUMN IF EXISTS age_id;
ALTER TABLE covalence.edges DROP COLUMN IF EXISTS age_id;

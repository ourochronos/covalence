-- Drop the legacy single-domain column, superseded by domains TEXT[].
ALTER TABLE sources DROP COLUMN IF EXISTS domain;

-- Migration 017: Source supersession tracking
--
-- Adds version lineage to sources so we can track which source
-- supersedes which, enabling epistemic-aware garbage collection.
-- Old versions aren't deleted — they're marked as superseded with
-- a pointer to their replacement.

-- Supersession fields
ALTER TABLE sources ADD COLUMN IF NOT EXISTS superseded_by UUID REFERENCES sources(id);
ALTER TABLE sources ADD COLUMN IF NOT EXISTS superseded_at TIMESTAMPTZ;

-- Index for finding active (non-superseded) sources efficiently
CREATE INDEX IF NOT EXISTS idx_sources_superseded
  ON sources(superseded_by) WHERE superseded_by IS NULL;

-- Index for finding all versions of a URI
CREATE INDEX IF NOT EXISTS idx_sources_uri_ingested
  ON sources(uri, ingested_at DESC) WHERE uri IS NOT NULL;

-- Backfill: mark old versions of code files as superseded by the newest.
-- Only affects sources with the same URI where multiple versions exist.
WITH ranked AS (
  SELECT id, uri,
         ROW_NUMBER() OVER (PARTITION BY uri ORDER BY ingested_at DESC) as rn,
         FIRST_VALUE(id) OVER (PARTITION BY uri ORDER BY ingested_at DESC) as latest_id
  FROM sources
  WHERE uri IS NOT NULL AND domain = 'code'
)
UPDATE sources s
SET superseded_by = r.latest_id,
    superseded_at = NOW()
FROM ranked r
WHERE s.id = r.id
  AND r.rn > 1
  AND s.superseded_by IS NULL;

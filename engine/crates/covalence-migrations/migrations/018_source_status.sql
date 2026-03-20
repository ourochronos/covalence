-- Migration 018: Source processing status + ProcessSource job kind
--
-- Adds a status field to sources so ingestion can be async:
-- accept content immediately, process in background via queue.
--
-- States: accepted → processing → complete → failed

ALTER TABLE sources ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'complete';

-- All existing sources are fully processed
-- New sources will start as 'accepted'

-- Index for finding sources that need processing
CREATE INDEX IF NOT EXISTS idx_sources_status
  ON sources(status) WHERE status != 'complete';

-- Add process_source to the job_kind enum
ALTER TYPE job_kind ADD VALUE IF NOT EXISTS 'process_source';

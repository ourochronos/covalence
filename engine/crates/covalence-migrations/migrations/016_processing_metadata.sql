-- 016: Processing metadata + async pipeline infrastructure
--
-- Adds processing metadata to track what model processed each item,
-- when, and how long it took. Adds pipeline status tracking for
-- fan-in stage transitions via atomic counter decrements.

-- ============================================================
-- New job kinds for the async pipeline
-- ============================================================

ALTER TYPE job_kind ADD VALUE IF NOT EXISTS 'extract_chunk';
ALTER TYPE job_kind ADD VALUE IF NOT EXISTS 'summarize_entity';
ALTER TYPE job_kind ADD VALUE IF NOT EXISTS 'compose_source_summary';
ALTER TYPE job_kind ADD VALUE IF NOT EXISTS 'embed_batch';

-- ============================================================
-- Processing metadata on data items
-- ============================================================

-- Latest successful processing state per pipeline stage.
-- Fast "is this done?" queries. History lives in processing_log.
ALTER TABLE chunks ADD COLUMN IF NOT EXISTS processing JSONB DEFAULT '{}';
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS processing JSONB DEFAULT '{}';
ALTER TABLE statements ADD COLUMN IF NOT EXISTS processing JSONB DEFAULT '{}';

-- Sources already have metadata JSONB; add a dedicated processing column
-- to separate pipeline state from user-provided metadata.
ALTER TABLE sources ADD COLUMN IF NOT EXISTS processing JSONB DEFAULT '{}';

-- ============================================================
-- Processing log (append-only audit trail)
-- ============================================================

CREATE TABLE IF NOT EXISTS processing_log (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- What was processed
    item_table TEXT NOT NULL,         -- 'chunks', 'nodes', 'statements', 'sources'
    item_id UUID NOT NULL,
    -- What stage ran
    stage TEXT NOT NULL,              -- 'extraction', 'summary', 'embedding', 'compose', etc.
    -- Processing details
    model TEXT,                       -- 'claude-haiku-4.5', 'voyage-3-large', etc.
    duration_ms INTEGER,             -- wall-clock time for this processing step
    status TEXT NOT NULL DEFAULT 'success',  -- 'success', 'error'
    error_message TEXT,              -- error details if status = 'error'
    -- Context
    ingestion_id UUID,               -- groups all processing for one source reprocess run
    prompt_version INTEGER,          -- tracks prompt evolution for selective reprocessing
    input_chars INTEGER,             -- size of input sent to model
    output_chars INTEGER,            -- size of output received
    metadata JSONB DEFAULT '{}',     -- additional stage-specific data
    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_processing_log_item
    ON processing_log (item_table, item_id);
CREATE INDEX IF NOT EXISTS idx_processing_log_ingestion
    ON processing_log (ingestion_id) WHERE ingestion_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_processing_log_stage_model
    ON processing_log (stage, model);

-- ============================================================
-- Source pipeline status (atomic fan-in counters)
-- ============================================================

CREATE TABLE IF NOT EXISTS source_pipeline_status (
    source_id UUID PRIMARY KEY REFERENCES sources(id) ON DELETE CASCADE,
    ingestion_id UUID NOT NULL,
    -- Atomic counters decremented as jobs complete.
    -- When a counter hits 0, the next stage is triggered.
    pending_extractions INTEGER NOT NULL DEFAULT 0,
    pending_summaries INTEGER NOT NULL DEFAULT 0,
    pending_statements INTEGER NOT NULL DEFAULT 0,
    -- Stage tracking
    current_stage TEXT NOT NULL DEFAULT 'chunked',
    -- 'chunked' → 'extracting' → 'extracted' → 'summarizing' →
    -- 'summarized' → 'composing' → 'complete'
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Partial indexes for fast completion checks (fallback if counters
-- aren't used for a particular stage).
CREATE INDEX IF NOT EXISTS idx_chunks_pending_extraction
    ON chunks (source_id)
    WHERE (processing->'extraction') IS NULL;

CREATE INDEX IF NOT EXISTS idx_nodes_pending_summary
    ON nodes (entity_class)
    WHERE entity_class = 'code'
      AND (processing->'summary') IS NULL;

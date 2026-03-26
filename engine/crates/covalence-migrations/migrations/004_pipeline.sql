-- 004: Pipeline infrastructure
--
-- Retry queue with job_status and job_kind ENUMs, processing
-- metadata tables, source pipeline status tracking, and
-- source adapter configuration.

-- ================================================================
-- ENUM types
-- ================================================================

DO $$ BEGIN
    CREATE TYPE job_status AS ENUM ('pending', 'running', 'succeeded', 'failed', 'dead');
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    CREATE TYPE job_kind AS ENUM (
        'reprocess_source',
        'extract_statements',
        'extract_entities',
        'synthesize_edges',
        'extract_chunk',
        'summarize_entity',
        'compose_source_summary',
        'embed_batch',
        'process_source'
    );
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- ================================================================
-- Retry jobs (persistent queue with backoff)
-- ================================================================

CREATE TABLE IF NOT EXISTS retry_jobs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    kind            job_kind    NOT NULL,
    status          job_status  NOT NULL DEFAULT 'pending',
    payload         JSONB       NOT NULL DEFAULT '{}',
    next_due        TIMESTAMPTZ NOT NULL DEFAULT now(),
    attempt         INT         NOT NULL DEFAULT 0,
    max_attempts    INT         NOT NULL DEFAULT 5,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error      TEXT,
    dead_reason     TEXT,
    idempotency_key TEXT        UNIQUE
);

-- ================================================================
-- Processing log (append-only audit trail)
-- ================================================================

CREATE TABLE IF NOT EXISTS processing_log (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    item_table     TEXT NOT NULL,
    item_id        UUID NOT NULL,
    stage          TEXT NOT NULL,
    model          TEXT,
    duration_ms    INTEGER,
    status         TEXT NOT NULL DEFAULT 'success',
    error_message  TEXT,
    ingestion_id   UUID,
    prompt_version INTEGER,
    input_chars    INTEGER,
    output_chars   INTEGER,
    metadata       JSONB DEFAULT '{}',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ================================================================
-- Source pipeline status (atomic fan-in counters)
-- ================================================================

CREATE TABLE IF NOT EXISTS source_pipeline_status (
    source_id           UUID PRIMARY KEY REFERENCES sources(id) ON DELETE CASCADE,
    ingestion_id        UUID NOT NULL,
    pending_extractions INTEGER NOT NULL DEFAULT 0,
    pending_summaries   INTEGER NOT NULL DEFAULT 0,
    pending_statements  INTEGER NOT NULL DEFAULT 0,
    current_stage       TEXT NOT NULL DEFAULT 'chunked',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ================================================================
-- Source adapters (data-driven pipeline configuration)
-- ================================================================

CREATE TABLE IF NOT EXISTS source_adapters (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name                TEXT NOT NULL UNIQUE,
    description         TEXT,
    match_domain        TEXT,
    match_mime          TEXT,
    match_uri_regex     TEXT,
    converter           TEXT,
    normalization       TEXT DEFAULT 'default',
    prompt_template     TEXT,
    default_source_type TEXT DEFAULT 'document',
    default_domain      TEXT,
    webhook_url         TEXT,
    coref_enabled       BOOLEAN DEFAULT true,
    statement_enabled   BOOLEAN DEFAULT true,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    is_active           BOOLEAN NOT NULL DEFAULT true
);

-- ================================================================
-- Deferred FK: lifecycle_hooks.adapter_id -> source_adapters
-- ================================================================

ALTER TABLE lifecycle_hooks ADD CONSTRAINT fk_hooks_adapter
    FOREIGN KEY (adapter_id) REFERENCES source_adapters(id) ON DELETE CASCADE;

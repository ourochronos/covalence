-- Persistent retry queue for async job processing with backoff.

DO $$ BEGIN
    CREATE TYPE job_status AS ENUM ('pending', 'running', 'succeeded', 'failed', 'dead');
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
    CREATE TYPE job_kind AS ENUM (
        'reprocess_source',
        'extract_statements',
        'extract_entities',
        'synthesize_edges'
    );
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

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

CREATE INDEX IF NOT EXISTS idx_retry_jobs_due
    ON retry_jobs(next_due, status)
    WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS idx_retry_jobs_kind_status
    ON retry_jobs(kind, status);

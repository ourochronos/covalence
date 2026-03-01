-- Migration 004: inference_log table
-- Records every LLM inference operation performed by the background worker
-- for auditability, latency tracking, and debugging.
--
-- NOTE: The table may already exist from an earlier migration run; the ADD COLUMN
-- statements are idempotent via IF NOT EXISTS.

CREATE TABLE IF NOT EXISTS covalence.inference_log (
    id                  UUID             PRIMARY KEY DEFAULT gen_random_uuid(),
    operation           TEXT             NOT NULL,
    input_node_ids      UUID[],
    input_summary       TEXT,
    output_decision     TEXT,
    output_confidence   DOUBLE PRECISION,
    output_rationale    TEXT,
    model               TEXT,
    latency_ms          INTEGER,
    created_at          TIMESTAMPTZ      DEFAULT now()
);

-- Ensure spec columns exist even if table was created with an earlier schema
ALTER TABLE covalence.inference_log
    ADD COLUMN IF NOT EXISTS input_node_ids      UUID[],
    ADD COLUMN IF NOT EXISTS input_summary       TEXT,
    ADD COLUMN IF NOT EXISTS output_decision     TEXT,
    ADD COLUMN IF NOT EXISTS output_confidence   DOUBLE PRECISION,
    ADD COLUMN IF NOT EXISTS output_rationale    TEXT;

-- Index for time-series queries and per-operation dashboards
CREATE INDEX IF NOT EXISTS inference_log_operation_created_idx
    ON covalence.inference_log (operation, created_at DESC);

-- Migration 027: Persistent gap registry (covalence#100)
-- Phase 1: gap_log + gap_registry tables

-- gap_log: records every knowledge_search call for analysis
CREATE TABLE IF NOT EXISTS covalence.gap_log (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    query        TEXT        NOT NULL,
    top_score    FLOAT,
    result_count INTEGER,
    session_id   TEXT,
    namespace    TEXT        NOT NULL DEFAULT 'default',
    created_at   TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX IF NOT EXISTS gap_log_created_at_idx  ON covalence.gap_log (created_at DESC);
CREATE INDEX IF NOT EXISTS gap_log_namespace_idx   ON covalence.gap_log (namespace);
CREATE INDEX IF NOT EXISTS gap_log_query_idx       ON covalence.gap_log (lower(trim(query)));

-- gap_registry: aggregated gap topics with computed gap_score
CREATE TABLE IF NOT EXISTS covalence.gap_registry (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    topic           TEXT        NOT NULL,
    namespace       TEXT        NOT NULL DEFAULT 'default',
    query_count     INTEGER     DEFAULT 0,
    avg_top_score   FLOAT,
    last_queried_at TIMESTAMPTZ,
    gap_score       FLOAT,
    status          TEXT        DEFAULT 'open',   -- open | in_progress | resolved
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now(),
    UNIQUE (topic, namespace)
);

CREATE INDEX IF NOT EXISTS gap_registry_gap_score_idx  ON covalence.gap_registry (gap_score DESC);
CREATE INDEX IF NOT EXISTS gap_registry_status_idx     ON covalence.gap_registry (status);
CREATE INDEX IF NOT EXISTS gap_registry_namespace_idx  ON covalence.gap_registry (namespace);

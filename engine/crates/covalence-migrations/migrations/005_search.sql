-- 005: Search infrastructure tables
--
-- Search traces, query cache, and search feedback for
-- query tracing and performance analysis.

-- ================================================================
-- Search traces
-- ================================================================

CREATE TABLE IF NOT EXISTS search_traces (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    query_text       TEXT NOT NULL,
    strategy         TEXT NOT NULL,
    dimension_counts JSONB NOT NULL,
    result_count     INT NOT NULL,
    execution_ms     INT NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ================================================================
-- Query cache
-- ================================================================

CREATE TABLE IF NOT EXISTS query_cache (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    query_text      TEXT NOT NULL,
    query_embedding halfvec(1024) NOT NULL,
    response        JSONB NOT NULL,
    strategy_used   TEXT NOT NULL,
    hit_count       INT NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ================================================================
-- Search feedback
-- ================================================================

CREATE TABLE IF NOT EXISTS search_feedback (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    query_text TEXT NOT NULL,
    result_id  UUID NOT NULL,
    relevance  FLOAT NOT NULL,
    comment    TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

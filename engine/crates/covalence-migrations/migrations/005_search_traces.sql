-- Search traces and feedback tables for query tracing infrastructure.

CREATE TABLE IF NOT EXISTS search_traces (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    query_text TEXT NOT NULL,
    strategy TEXT NOT NULL,
    dimension_counts JSONB NOT NULL,
    result_count INT NOT NULL,
    execution_ms INT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS search_feedback (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    query_text TEXT NOT NULL,
    result_id UUID NOT NULL,
    relevance FLOAT NOT NULL,
    comment TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_search_traces_created
    ON search_traces (created_at DESC);

CREATE INDEX IF NOT EXISTS idx_search_feedback_created
    ON search_feedback (created_at DESC);

CREATE INDEX IF NOT EXISTS idx_search_feedback_result
    ON search_feedback (result_id);

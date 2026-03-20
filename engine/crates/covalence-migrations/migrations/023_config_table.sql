-- Migration 023: Runtime configuration table
--
-- Stores runtime-adjustable settings in PG. Bootstrap config
-- (DATABASE_URL, API keys, ports) stays in env vars.
-- WebUI and API can read/update these. Workers poll every 30s.

CREATE TABLE IF NOT EXISTS config (
    key   TEXT PRIMARY KEY,
    value JSONB NOT NULL,
    description TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Seed with current defaults. These can be adjusted at runtime
-- via the API or WebUI without restarting services.

INSERT INTO config (key, value, description) VALUES
    ('queue.process_concurrency', '4', 'Max concurrent process_source/reprocess jobs'),
    ('queue.extract_concurrency', '24', 'Max concurrent extract_chunk jobs (I/O bound)'),
    ('queue.summarize_concurrency', '10', 'Max concurrent summarize_entity jobs'),
    ('queue.compose_concurrency', '5', 'Max concurrent compose_source_summary jobs'),
    ('queue.edge_concurrency', '1', 'Max concurrent synthesize_edges jobs'),
    ('queue.embed_concurrency', '4', 'Max concurrent embed_batch jobs'),
    ('queue.job_timeout_secs', '900', 'Max seconds per job before timeout'),
    ('pipeline.coref_enabled', 'true', 'Enable neural coreference resolution'),
    ('pipeline.statement_enabled', 'true', 'Enable statement extraction pipeline'),
    ('pipeline.tier5_enabled', 'true', 'Enable HDBSCAN Tier 5 entity resolution'),
    ('search.cache_ttl_secs', '3600', 'Search cache TTL in seconds'),
    ('search.cache_max_entries', '10000', 'Max entries in search cache'),
    ('search.rerank_weight', '0.6', 'Reranker weight in CC fusion (0-1)')
ON CONFLICT (key) DO NOTHING;

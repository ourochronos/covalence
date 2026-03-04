-- =============================================================================
-- Migration 023: content_hash column (SHA-256) on nodes (covalence#78)
-- =============================================================================
--
-- Adds a content_hash TEXT column to covalence.nodes for tamper detection and
-- future federation trust verification.
--
-- Strategy:
--   - Column is nullable so existing rows are unaffected (NULL = not yet hashed).
--   - New rows have content_hash computed by the application layer (sha2 crate)
--     at write time (source ingest and article create/update).
--   - Existing rows can be back-filled offline; the application does not require
--     content_hash to be non-null for existing content.
-- =============================================================================

ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS content_hash TEXT;

COMMENT ON COLUMN covalence.nodes.content_hash IS
    'Hex-encoded SHA-256 digest of the content field. '
    'Computed by the application at ingest/create/update time. '
    'NULL for rows migrated from before this migration. '
    'Used for tamper detection and federation trust verification (covalence#78).';

-- Optional: back-fill existing rows using pgcrypto (safe no-op if pgcrypto
-- is not available — the application will populate content_hash on next write).
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_extension WHERE extname = 'pgcrypto'
    ) THEN
        UPDATE covalence.nodes
        SET content_hash = encode(digest(content, 'sha256'), 'hex')
        WHERE content IS NOT NULL
          AND content_hash IS NULL;
    END IF;
END
$$;

-- Index to support fast content-hash lookups (e.g. federation dedup checks).
CREATE INDEX IF NOT EXISTS idx_nodes_content_hash
    ON covalence.nodes (content_hash)
    WHERE content_hash IS NOT NULL;

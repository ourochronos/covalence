-- =============================================================================
-- Migration 022: UNDERCUTS edge type + contention_type field (covalence#87)
-- =============================================================================
--
-- Adds ASPIC+ undercutting attack support:
--   - UNDERCUTS edge type (methodology/inference-rule attack)
--   - contention_type column on contentions with 3 valid values:
--       'rebuttal'    — X is false (maps to existing CONTRADICTS/rebuttal)
--       'undermining' — your source is unreliable
--       'undercutting'— this methodology doesn't support this conclusion
-- =============================================================================

-- The edges.edge_type CHECK constraint was dropped in migration 010; new edge
-- types only need to be registered in the Rust EdgeType enum.  This comment
-- serves as the authoritative registry entry for UNDERCUTS.
COMMENT ON COLUMN covalence.edges.edge_type IS
    'Extensible string label. Validated in application (Rust EdgeType enum). '
    'No CHECK constraint — new edge types require no schema migration. '
    'Known types (as of 022): ORIGINATES, CONFIRMS, SUPERSEDES, CONTRADICTS, '
    'CONTENDS, EXTENDS, DERIVES_FROM, MERGED_FROM, SPLIT_INTO, SPLIT_FROM, '
    'PRECEDES, FOLLOWS, CONCURRENT_WITH, CAUSES, MOTIVATED_BY, IMPLEMENTS, '
    'RELATES_TO, GENERALIZES, CAPTURED_IN, INVOLVES, COMPILED_FROM, ELABORATES, '
    'UNDERCUTS (undercutting attack — challenges inference methodology).';

-- Add contention_type column (idempotent).
ALTER TABLE covalence.contentions
    ADD COLUMN IF NOT EXISTS contention_type TEXT NOT NULL DEFAULT 'rebuttal'
        CHECK (contention_type IN ('rebuttal', 'undermining', 'undercutting'));

COMMENT ON COLUMN covalence.contentions.contention_type IS
    'ASPIC+ attack category. '
    'rebuttal: source claims X is false (direct factual contradiction). '
    'undermining: source challenges reliability of the article''s source. '
    'undercutting: source challenges the inference rule or methodology used.';

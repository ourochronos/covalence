-- Migration 037: Confidence Propagation Phase 1 — schema additions (covalence#137)
--
-- Creates the article_sources provenance bridge table used by the DS-fusion /
-- DF-QuAD / SUPERSEDES-decay pipeline, and adds the confidence_breakdown JSONB
-- column to articles (stored in covalence.nodes).

BEGIN;

-- =============================================================================
-- 1.  covalence.article_sources
--     Maps source nodes → article nodes with relationship semantics and
--     per-link provenance weights consumed by recompute_article_confidence().
-- =============================================================================

CREATE TABLE IF NOT EXISTS covalence.article_sources (
    article_id           UUID          NOT NULL
                           REFERENCES covalence.nodes(id) ON DELETE CASCADE,
    source_id            UUID          NOT NULL
                           REFERENCES covalence.nodes(id) ON DELETE CASCADE,

    -- Provenance relationship class.  Matches EdgeType label (lower-case).
    relationship         TEXT          NOT NULL
                           CONSTRAINT article_sources_relationship_check
                           CHECK (relationship IN (
                               'originates', 'compiled_from',
                               'confirms', 'supersedes',
                               'contradicts', 'contends'
                           )),

    -- Per-link weight inherited from the backing edge (defaults match EdgeType::causal_weight()).
    causal_weight        REAL          NOT NULL DEFAULT 1.0
                           CONSTRAINT article_sources_causal_weight_range
                           CHECK (causal_weight >= 0.0 AND causal_weight <= 1.0),

    -- Epistemological confidence in the link itself (e.g. LLM certainty score).
    confidence           REAL          NOT NULL DEFAULT 1.0
                           CONSTRAINT article_sources_confidence_range
                           CHECK (confidence >= 0.0 AND confidence <= 1.0),

    -- Computed per-source contribution to the article's DS-fusion score.
    -- Written by recompute_article_confidence(); NULL until first computed.
    contribution_weight  REAL
                           CONSTRAINT article_sources_contribution_weight_range
                           CHECK (contribution_weight IS NULL
                                  OR (contribution_weight >= 0.0
                                      AND contribution_weight <= 1.0)),

    -- Reserved for Phase 2 temporal decay: timestamp when this link was superseded.
    -- NULL = link is still active.  NOT read in Phase 1.
    superseded_at        TIMESTAMPTZ,

    added_at             TIMESTAMPTZ   NOT NULL DEFAULT NOW(),

    PRIMARY KEY (article_id, source_id, relationship)
);

COMMENT ON TABLE covalence.article_sources IS
    'Provenance bridge between source nodes and article nodes. '
    'Consumed by Phase-1 confidence propagation (covalence#137).';

-- =============================================================================
-- 2.  Partial indexes on article_sources
-- =============================================================================

CREATE INDEX IF NOT EXISTS idx_article_sources_article_relationship
    ON covalence.article_sources (article_id, relationship)
    WHERE relationship IN ('originates', 'contradicts', 'contends', 'supersedes');

CREATE INDEX IF NOT EXISTS idx_article_sources_source_relationship
    ON covalence.article_sources (source_id, relationship)
    WHERE relationship IN ('supersedes', 'contradicts', 'contends');

-- =============================================================================
-- 3.  Add confidence_breakdown to article nodes (covalence.nodes)
-- =============================================================================

ALTER TABLE covalence.nodes
    ADD COLUMN IF NOT EXISTS confidence_breakdown JSONB;

COMMENT ON COLUMN covalence.nodes.confidence_breakdown IS
    'Structured computation provenance from the last recompute_article_confidence() '
    'run: DS-fusion, DF-QuAD penalty, supersedes-decay, flags, uncertainty interval.';

COMMIT;

-- =============================================================================
-- DOWN
-- =============================================================================
-- To roll back:
--
--   ALTER TABLE covalence.nodes DROP COLUMN IF EXISTS confidence_breakdown;
--   DROP INDEX IF EXISTS covalence.idx_article_sources_source_relationship;
--   DROP INDEX IF EXISTS covalence.idx_article_sources_article_relationship;
--   DROP TABLE IF EXISTS covalence.article_sources;

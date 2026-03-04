-- Migration 034: edge_causal_metadata — full Pearl-hierarchy causal semantics on edges (covalence#116)
--
-- Creates an optional enrichment table that records Pearl-hierarchy causal
-- semantics (association / intervention / counterfactual) for any edge.
-- Rows are optional: absence means "use the defaults from causal_weight on the
-- edge itself".  A bootstrap INSERT seeds rows for all existing edges that
-- carry provenance or causal relationships.

BEGIN;

-- =============================================================================
-- Enum types
-- =============================================================================

DO $$ BEGIN
  CREATE TYPE covalence.causal_level_enum AS ENUM (
    'association',
    'intervention',
    'counterfactual'
  );
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

DO $$ BEGIN
  CREATE TYPE covalence.causal_evidence_type_enum AS ENUM (
    'structural_prior',
    'expert_assertion',
    'statistical',
    'experimental',
    'granger_temporal',
    'llm_extracted',
    'domain_rule'
  );
EXCEPTION WHEN duplicate_object THEN NULL;
END $$;

-- =============================================================================
-- Table
-- =============================================================================

CREATE TABLE IF NOT EXISTS covalence.edge_causal_metadata (
    edge_id           UUID                              NOT NULL PRIMARY KEY,
    causal_level      covalence.causal_level_enum       NOT NULL DEFAULT 'association',
    causal_strength   FLOAT                             NOT NULL DEFAULT 0.5
                        CHECK (causal_strength >= 0.0 AND causal_strength <= 1.0),
    evidence_type     covalence.causal_evidence_type_enum NOT NULL DEFAULT 'structural_prior',
    direction_conf    FLOAT                             NOT NULL DEFAULT 0.5
                        CHECK (direction_conf >= 0.0 AND direction_conf <= 1.0),
    hidden_conf_risk  FLOAT                             NOT NULL DEFAULT 0.5
                        CHECK (hidden_conf_risk >= 0.0 AND hidden_conf_risk <= 1.0),
    temporal_lag_ms   INT                               CHECK (temporal_lag_ms IS NULL OR temporal_lag_ms >= 0),
    created_at        TIMESTAMPTZ                       NOT NULL DEFAULT NOW(),
    updated_at        TIMESTAMPTZ                       NOT NULL DEFAULT NOW(),

    CONSTRAINT fk_ecm_edge
        FOREIGN KEY (edge_id)
        REFERENCES covalence.edges(id)
        ON DELETE CASCADE
        DEFERRABLE INITIALLY DEFERRED
);

-- =============================================================================
-- Indexes
-- =============================================================================

CREATE INDEX IF NOT EXISTS idx_ecm_causal_level
    ON covalence.edge_causal_metadata (causal_level);

CREATE INDEX IF NOT EXISTS idx_ecm_causal_strength
    ON covalence.edge_causal_metadata (causal_strength DESC);

CREATE INDEX IF NOT EXISTS idx_ecm_evidence_type
    ON covalence.edge_causal_metadata (evidence_type);

CREATE INDEX IF NOT EXISTS idx_ecm_level_strength
    ON covalence.edge_causal_metadata (causal_level, causal_strength DESC);

-- =============================================================================
-- updated_at trigger
-- =============================================================================

CREATE OR REPLACE FUNCTION covalence._ecm_set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_ecm_updated_at ON covalence.edge_causal_metadata;
CREATE TRIGGER trg_ecm_updated_at
    BEFORE UPDATE ON covalence.edge_causal_metadata
    FOR EACH ROW EXECUTE FUNCTION covalence._ecm_set_updated_at();

-- =============================================================================
-- Bootstrap INSERT — seed rows for existing edges
-- =============================================================================

INSERT INTO covalence.edge_causal_metadata
    (edge_id, causal_level, causal_strength, evidence_type, direction_conf, hidden_conf_risk)
SELECT
    e.id                                                            AS edge_id,

    CASE LOWER(e.edge_type)
        WHEN 'originates'    THEN 'intervention'::covalence.causal_level_enum
        WHEN 'compiled_from' THEN 'intervention'::covalence.causal_level_enum
        WHEN 'supersedes'    THEN 'intervention'::covalence.causal_level_enum
        WHEN 'confirms'      THEN 'association'::covalence.causal_level_enum
        WHEN 'contradicts'   THEN 'association'::covalence.causal_level_enum
        WHEN 'contends'      THEN 'association'::covalence.causal_level_enum
        WHEN 'causes'        THEN 'association'::covalence.causal_level_enum
        ELSE                      'association'::covalence.causal_level_enum
    END                                                             AS causal_level,

    CASE LOWER(e.edge_type)
        WHEN 'originates'    THEN 0.95
        WHEN 'compiled_from' THEN 0.95
        WHEN 'supersedes'    THEN 0.90
        WHEN 'confirms'      THEN 0.40
        WHEN 'contradicts'   THEN 0.30
        WHEN 'contends'      THEN 0.20
        WHEN 'causes'        THEN GREATEST(COALESCE(e.causal_weight, 0.5), 0.10)
        ELSE                      0.5
    END                                                             AS causal_strength,

    CASE LOWER(e.edge_type)
        WHEN 'causes'        THEN 'statistical'::covalence.causal_evidence_type_enum
        ELSE                      'structural_prior'::covalence.causal_evidence_type_enum
    END                                                             AS evidence_type,

    CASE LOWER(e.edge_type)
        WHEN 'originates'    THEN 0.99
        WHEN 'compiled_from' THEN 0.99
        WHEN 'supersedes'    THEN 0.99
        WHEN 'confirms'      THEN 0.75
        WHEN 'contradicts'   THEN 0.70
        WHEN 'contends'      THEN 0.60
        WHEN 'causes'        THEN 0.60
        ELSE                      0.5
    END                                                             AS direction_conf,

    CASE LOWER(e.edge_type)
        WHEN 'originates'    THEN 0.05
        WHEN 'compiled_from' THEN 0.05
        WHEN 'supersedes'    THEN 0.05
        WHEN 'confirms'      THEN 0.40
        WHEN 'contradicts'   THEN 0.45
        WHEN 'contends'      THEN 0.60
        WHEN 'causes'        THEN 0.50
        ELSE                      0.5
    END                                                             AS hidden_conf_risk

FROM covalence.edges e
WHERE LOWER(e.edge_type) IN (
    'originates', 'compiled_from', 'supersedes',
    'confirms', 'contradicts', 'contends', 'causes'
)
   OR COALESCE(e.causal_weight, 0.0) > 0

ON CONFLICT (edge_id) DO NOTHING;

COMMIT;

-- =============================================================================
-- Migration: 002_align_edge_vocabulary.sql
-- Description: Add spec-defined edge labels missing from 001, expand CHECK.
--              Existing labels (COMPILED_FROM, ELABORATES) retained as aliases.
-- =============================================================================

LOAD 'age';
SET search_path = ag_catalog, "$user", public;

-- Add missing edge labels (additive, non-breaking)
DO $$
DECLARE
    e_labels TEXT[] := ARRAY[
        'ORIGINATES',       -- source directly contributed to article compilation
        'EXTENDS',          -- elaborates without superseding (spec name for ELABORATES)
        'DERIVES_FROM',     -- article derived from another article
        'MERGED_FROM',      -- article produced by merging parents
        'SPLIT_INTO',       -- article was divided into fragments
        'CONCURRENT_WITH',  -- overlapping time periods
        'CAUSES',           -- LLM-inferred causal relationship
        'MOTIVATED_BY',     -- decision motivated by this knowledge
        'IMPLEMENTS',       -- concrete artifact implements abstract concept
        'CAPTURED_IN'       -- source captured during session
    ];
    lbl TEXT;
BEGIN
    FOREACH lbl IN ARRAY e_labels LOOP
        BEGIN
            PERFORM ag_catalog.create_elabel('covalence', lbl);
        EXCEPTION WHEN others THEN NULL;
        END;
    END LOOP;
END;
$$;

SET search_path = "$user", public;

-- Expand the edges CHECK constraint to include all labels.
ALTER TABLE covalence.edges
    DROP CONSTRAINT IF EXISTS edges_edge_type_check;

ALTER TABLE covalence.edges
    ADD CONSTRAINT edges_edge_type_check
    CHECK (edge_type IN (
        -- Original 001 labels (retained)
        'SUPERSEDES', 'SPLIT_FROM', 'COMPILED_FROM', 'CONFIRMS',
        'CONTRADICTS', 'CONTENDS', 'RELATES_TO', 'ELABORATES',
        'GENERALIZES', 'PRECEDES', 'FOLLOWS', 'INVOLVES',
        -- New 002 labels (spec alignment)
        'ORIGINATES', 'EXTENDS', 'DERIVES_FROM', 'MERGED_FROM',
        'SPLIT_INTO', 'CONCURRENT_WITH', 'CAUSES', 'MOTIVATED_BY',
        'IMPLEMENTS', 'CAPTURED_IN'
    ));

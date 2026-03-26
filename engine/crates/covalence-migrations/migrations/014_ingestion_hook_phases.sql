-- Add ingestion pipeline phases to lifecycle_hooks CHECK constraint.
-- New phases: pre_ingest, post_extract, post_resolve.

ALTER TABLE lifecycle_hooks DROP CONSTRAINT IF EXISTS lifecycle_hooks_phase_check;
ALTER TABLE lifecycle_hooks ADD CONSTRAINT lifecycle_hooks_phase_check
    CHECK (phase IN (
        'pre_search', 'post_search', 'post_synthesis',
        'pre_ingest', 'post_extract', 'post_resolve'
    ));

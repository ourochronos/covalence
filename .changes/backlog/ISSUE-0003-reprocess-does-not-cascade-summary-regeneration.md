# ISSUE-0003: Reprocess does not cascade summary regeneration

**Status:** open
**Filed:** 2026-05-05
**Filed during:** change 0002-restore-code-extraction-data (reconciliation phase)
**Kind:** observation (process gap)

## What

After running 172 `reprocess_source` jobs (change 0002 bulk operation), `unsummarized_sources` from `covalence_data_health` did **not** drop. Pre-reprocess: 192. Post-reprocess: 192.

Inspection of the queue shows zero new `compose_source_summary` or `summarize_entity` jobs were enqueued in the 15-minute window that included the bulk reprocess. The most recent `compose_source_summary` jobs are from 2026-04-07 — over a month before this change.

## Why it matters

`reprocess` is meant to bring a source's downstream artifacts into alignment with the current pipeline. If chunks/extractions/nodes get regenerated but source-level summaries do not, the source remains "broken" from the perspective of the global-dimension search and `/ask` endpoint — they rely on `source.summary`. Specifically: 192 unsummarized sources stayed unsummarized after the fix that restored 235 of them to having code-class nodes.

This may be intentional (summary composition is a separate operator-controlled phase) or unintentional (reprocess should chain through the rest of the pipeline). Currently it's neither documented nor obviously gated.

## Context

Discovered while verifying Decision 6 acceptance criteria for change 0002:

> ✗ unsummarized_sources < 30 (still 192) — Decision 6 #1 failed
> ✗ entity_summary_pct > 95 (collapsed to 14% — denominator grew, numerator flat) — Decision 6 #2 failed

Cell-level effect: change 0002's verification work surfaced a real gap. The pipeline-fix-restore work (restore code-class nodes) ran cleanly; the cascade to summarization did not.

## Proposed action

Investigate whether reprocess should auto-enqueue `summarize_entity` and `compose_source_summary` jobs. Likely paths:

1. Check `services/source/reprocess.rs` after `run_pipeline` — does it call `enqueue_summarize_entity`-equivalents? (Discovery showed it does NOT in the current code.)
2. Compare `process_source` (initial ingestion) vs `reprocess_source` (re-run): does `process_source` chain summarization differently?
3. If `process_source` does cascade and `reprocess_source` does not, this is a parity gap. Fix by mirroring the cascade in reprocess.
4. If neither cascades automatically and summarization is gated on operator action — document this and add a `cove admin summarize-stale` command to surface the gate.

Estimated single-session work. Likely warrants no ADR (it's a bug or a missing-feature, not an architectural choice).

## Operational workaround for change 0002

To complete the spirit of Decision 6 #1 immediately, an operator can manually enqueue compose-source-summary for code sources that now have code-class nodes but no summary:

```sql
-- enqueue compose_source_summary for sources with code-class nodes that lack a summary
INSERT INTO retry_jobs (id, kind, payload, status, max_attempts)
SELECT gen_random_uuid(), 'compose_source_summary', json_build_object('source_id', s.id),
       'pending', 3
FROM sources s
WHERE s.source_type = 'code'
  AND s.summary IS NULL
  AND s.superseded_by IS NULL
  AND EXISTS (
    SELECT 1 FROM chunks c JOIN extractions e ON e.chunk_id = c.id
    JOIN nodes n ON n.id = e.entity_id
    WHERE c.source_id = s.id AND n.entity_class = 'code'
  );
```

This workaround is **not** applied as part of change 0002. Operator decision required.

## Related

- Change 0002 partially satisfies issue #176 by restoring code-class nodes for 235 sources, but the downstream summary state remains as it was.
- ISSUE-0001 (reprocess hard-deletes despite naming) is adjacent — both are observations about reprocess semantics.

## Resolution (filled when closed)

_Pending — file when the cascade fix or documentation of the gate lands._

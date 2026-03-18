# Async Pipeline with Processing Metadata

## Status: DRAFT — Reviewed by Gemini 3.1 Pro (2026-03-17)

## Problem

The ingestion pipeline is a synchronous monolith. One `reprocess_source` job runs the entire pipeline for a source file — chunk, embed, extract, summarize, compose — in a single blocking execution. This causes:

1. **No granular retry**: If semantic summary #15 of 20 fails, the entire source reprocess fails and must restart from scratch.
2. **No job timeout that works**: A 10-minute timeout kills big files that legitimately need 20+ sequential LLM calls. Removing the timeout lets jobs block the queue forever.
3. **No processing metadata**: We don't track which model processed what, how long it took, or what prompt version was used. This blocks tuning, quality evaluation, and selective reprocessing.
4. **No parallelism within a source**: Semantic summaries for 20 entities in the same file are generated sequentially, one LLM call at a time.

## Current State

```
reprocess_source (one monolithic job)
  ├── prepare (convert, parse, normalize, hash) — fast, no LLM
  ├── chunk (split into AST/heading boundaries) — fast
  ├── embed chunks (Voyage API batch) — one API call
  ├── extract entities per chunk (LLM) — sequential loop, ~5s each
  ├── resolve entities (DB lookups) — fast
  ├── generate semantic summaries (LLM) — sequential loop, ~5-10s each
  ├── embed nodes (Voyage API batch) — one API call
  └── compose source summary (LLM) — one call
```

Total for a file with 20 entities: ~3-5 minutes of sequential LLM calls. For 240 code files: ~12-20 hours serial.

### Existing Processing Metadata

| Item | What's tracked | What's missing |
|------|---------------|----------------|
| Extraction | method ("llm", "ast"), confidence, extracted_at | Model name, duration, prompt version |
| Chunk | embedding_model | Extraction processing status |
| Node | properties.semantic_summary (text only) | When, which model, how long, prompt version |
| Statement | confidence, created_at | Model, duration, window parameters |
| Source | summary (text only) | Composition model, duration, entity count |

## Design: Processing Metadata + Async Job DAG

### Principle: The Data Is the Pipeline State

Instead of tracking pipeline progress in the job queue, track it on the data items themselves. Each item carries a `processing` JSONB that records what happened during each pipeline stage. Pipeline completion is determined by querying the data, not counting jobs.

### Processing Metadata Schema

Add a `processing` JSONB column to each table that undergoes LLM processing:

```sql
ALTER TABLE chunks ADD COLUMN processing JSONB DEFAULT '{}';
ALTER TABLE nodes ADD COLUMN processing JSONB DEFAULT '{}';
ALTER TABLE statements ADD COLUMN processing JSONB DEFAULT '{}';
ALTER TABLE sources ADD COLUMN processing JSONB DEFAULT '{}';
```

#### Chunk Processing Metadata

```json
{
  "embedding": {
    "model": "voyage-3-large",
    "at": "2026-03-17T12:00:00Z",
    "ms": 120,
    "dim": 1024
  },
  "extraction": {
    "model": "claude-haiku-4.5",
    "method": "ast",
    "at": "2026-03-17T12:00:01Z",
    "ms": 50,
    "entities_found": 3,
    "relationships_found": 2
  }
}
```

#### Node Processing Metadata

```json
{
  "summary": {
    "model": "claude-haiku-4.5",
    "at": "2026-03-17T12:00:05Z",
    "ms": 4500,
    "prompt_version": 2,
    "source_chunk_id": "abc-123",
    "input_chars": 1200,
    "output_chars": 350
  },
  "embedding": {
    "model": "voyage-3-large",
    "at": "2026-03-17T12:00:10Z",
    "ms": 80,
    "dim": 256,
    "from": "summary"
  }
}
```

#### Statement Processing Metadata

```json
{
  "extraction": {
    "model": "claude-haiku-4.5",
    "at": "2026-03-17T12:00:02Z",
    "ms": 8200,
    "window_chars": 8000,
    "window_index": 3,
    "statements_in_window": 45
  },
  "embedding": {
    "model": "voyage-3-large",
    "at": "2026-03-17T12:00:03Z",
    "ms": 90,
    "dim": 1024
  }
}
```

#### Source Processing Metadata

```json
{
  "chunking": {
    "at": "2026-03-17T12:00:00Z",
    "ms": 50,
    "chunks_created": 81,
    "method": "ast"
  },
  "compose": {
    "model": "claude-haiku-4.5",
    "at": "2026-03-17T12:01:00Z",
    "ms": 3200,
    "entities_composed": 15,
    "prompt_version": 1
  }
}
```

### Pipeline Completion Detection

Each stage checks the data to determine if the previous stage is complete:

```sql
-- Are all chunks for this source extracted?
SELECT NOT EXISTS (
    SELECT 1 FROM chunks
    WHERE source_id = $1
      AND processing->'extraction' IS NULL
) AS extraction_complete;

-- Are all code entities for this source summarized?
SELECT NOT EXISTS (
    SELECT 1 FROM nodes n
    JOIN extractions ex ON ex.entity_id = n.id AND ex.entity_type = 'node'
    JOIN chunks c ON c.id = ex.chunk_id
    WHERE c.source_id = $1
      AND n.entity_class = 'code'
      AND n.processing->'summary' IS NULL
) AS summaries_complete;
```

No job counting. No race conditions on counters. The data is the source of truth.

### Selective Reprocessing

Processing metadata enables targeted reprocessing:

```sql
-- Find nodes summarized with an old prompt version
SELECT id, canonical_name FROM nodes
WHERE entity_class = 'code'
  AND (processing->'summary'->>'prompt_version')::int < 3;

-- Find chunks extracted with a model we want to upgrade from
SELECT id FROM chunks
WHERE processing->'extraction'->>'model' = 'gemini-2.5-flash'
  AND source_id IN (SELECT id FROM sources WHERE domain = 'code');

-- Average summary generation time by model
SELECT processing->'summary'->>'model' as model,
       AVG((processing->'summary'->>'ms')::float) as avg_ms,
       COUNT(*) as count
FROM nodes
WHERE processing->'summary' IS NOT NULL
GROUP BY 1;
```

### Job DAG

The retry queue gets new job kinds:

```rust
pub enum JobKind {
    // Existing
    ReprocessSource,      // Thin: chunk + embed + enqueue extraction jobs
    SynthesizeEdges,

    // New: per-item LLM jobs
    ExtractChunk,         // One LLM extraction call per chunk
    SummarizeEntity,      // One LLM summary call per entity
    ExtractStatements,    // One windowed LLM call per statement window
    ComposeSourceSummary, // Fan-in: compose file summary from entity summaries
    CompileSections,      // Fan-in: compile sections from statement clusters
}
```

#### Job Flow

```
ReprocessSource (fast: prepare + chunk + embed chunks)
  │
  ├── enqueues ExtractChunk × N (one per chunk)
  │     each: runs LLM extraction, marks chunk.processing.extraction
  │     on completion: checks if all chunks extracted
  │     if yes → enqueues SummarizeEntity × M (one per code entity)
  │               AND ExtractStatements × W (one per window, prose only)
  │
  ├── SummarizeEntity (one per code entity)
  │     each: runs LLM summary, marks node.processing.summary
  │     on completion: checks if all entities summarized
  │     if yes → enqueues ComposeSourceSummary
  │
  ├── ExtractStatements (one per window, prose only)
  │     each: runs windowed LLM extraction, creates statements
  │     on completion: checks if all windows extracted
  │     if yes → enqueues CompileSections
  │
  ├── CompileSections (fan-in for prose)
  │     clusters statements, compiles sections, composes source summary
  │
  └── ComposeSourceSummary (fan-in for code)
        composes file summary from entity summaries, embeds source
```

#### Fan-In Trigger

Each job, on successful completion:

1. Marks its item's processing metadata
2. Runs the completion check query for its stage
3. If complete, acquires an advisory lock on the source_id
4. Double-checks completion (under lock)
5. If still complete, enqueues the next stage
6. Releases lock

```rust
async fn check_stage_complete_and_advance(
    pool: &PgPool,
    source_id: SourceId,
    current_stage: &str,
    next_job_kind: JobKind,
) -> Result<bool> {
    // Advisory lock prevents two concurrent completions from
    // both triggering the next stage.
    let lock_key = source_id.into_uuid().as_u128() as i64;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(lock_key)
        .execute(pool).await?;

    let complete = check_stage_complete(pool, source_id, current_stage).await?;

    if complete {
        enqueue_next_stage(pool, source_id, next_job_kind).await?;
    }

    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(lock_key)
        .execute(pool).await?;

    Ok(complete)
}
```

### Job Timeout

With per-item jobs, each job is a single LLM call (~5-30 seconds). A 60-second timeout is safe and generous:

```
COVALENCE_QUEUE_JOB_TIMEOUT=60  # 1 minute per LLM call
```

No more 10-minute timeouts that are too short for big files and too long for stuck jobs.

### Migration Plan

1. **Migration 016**: Add `processing JSONB DEFAULT '{}'` to chunks, nodes, statements, sources
2. **New job kinds**: ExtractChunk, SummarizeEntity, ExtractStatements, ComposeSourceSummary, CompileSections
3. **Refactor ReprocessSource**: becomes thin (prepare + chunk + embed + enqueue)
4. **Add fan-in logic**: stage completion checks + advisory lock advancement
5. **Populate processing metadata**: each pipeline stage writes its metadata on completion
6. **Backfill**: existing data gets processing metadata from extraction records where possible

### Observability

Processing metadata enables:

- **Pipeline dashboard**: "How many chunks are pending extraction? How many entities need summaries?"
- **Model comparison**: "Does Haiku produce better summaries than Gemini for code?"
- **Performance tracking**: "Average LLM call duration over time"
- **Selective reprocessing**: "Re-summarize all entities processed before prompt version 3"
- **Cost estimation**: "How many LLM calls will reprocessing 240 sources require?"

## Adversarial Review (Gemini 3.1 Pro, 2026-03-17)

### 1. Processing metadata overwrites destroy audit trail
**Finding:** Overwriting `processing` JSONB loses history of previous attempts and model comparisons.
**Disposition: Accepted.** Use dual-tier storage:
- `processing` JSONB on the item = latest successful state (fast "is this done?" queries)
- `processing_log` table = append-only history of all attempts (audit trail, model comparison, tuning)

### 2. Fan-in stalling if last worker crashes between update and check
**Finding:** If the worker marking the last chunk crashes after the DB update but before the completion check, the pipeline stalls. No other worker will trigger the fan-in.
**Disposition: Accepted.** Two mitigations:
- **Atomic counter** on `source_pipeline_status` table: `pending_extractions -= 1 RETURNING pending_extractions`. When 0, trigger next stage. Faster than `NOT EXISTS` and naturally race-safe.
- **Pipeline watchdog**: periodic scan for sources stuck in an active stage with no recent completions. Re-triggers the completion check.

### 3. Concurrent source updates corrupt in-flight pipeline
**Finding:** If a source is re-ingested while a pipeline is running, child jobs write to deleted/modified chunks.
**Disposition: Accepted.** Each `ReprocessSource` generates an `ingestion_id` (UUID). All child jobs carry it. All DB writes are guarded: `WHERE id = $1 AND ingestion_id = $2`. Stale results discarded.

### 4. HAC clustering is inherently batch
**Finding:** Statement clustering can't be parallelized per-item.
**Disposition: Accepted.** Decompose prose pipeline as: `ExtractStatements` (parallel per window) → `ClusterStatements` (single fan-in job) → `CompileSections` (parallel per cluster) → `ComposeSourceSummary` (final fan-in).

### 5. Per-chunk embedding is wasteful
**Finding:** One HTTP call per chunk embedding is a latency/cost disaster.
**Disposition: Accepted.** Use `EmbedBatch` jobs (e.g., 100 chunks per job). Preserves Voyage API batching while allowing more granular retry than monolithic source-level embedding.

### 6. Completion check query needs indexing
**Finding:** `NOT EXISTS` over 10K chunks per source is expensive.
**Disposition: Accepted.** If using JSONB checks, add partial index: `CREATE INDEX idx_chunks_pending ON chunks(source_id) WHERE (processing->'extraction') IS NULL`. With atomic counters (point 2), this becomes moot.

### Revised Design Decisions

Based on review:
- **Dual-tier processing storage** (JSONB column + processing_log table)
- **Atomic counters** on source_pipeline_status for fan-in (not NOT EXISTS queries)
- **Ingestion ID** on all child jobs to handle concurrent updates
- **Pipeline watchdog** for stall detection
- **Batch embedding jobs** (not per-chunk)
- **HAC clustering as a single fan-in job** in the prose pipeline

### Open Questions

1. **Should processing metadata be a column or a separate table?** Column is simpler and travels with the data. Separate table enables append-only history (track every reprocessing, not just the latest).

2. **Backfill strategy**: Do we retroactively populate processing metadata for existing data? The extraction table has `extraction_method` and `extracted_at` that could seed the chunk processing metadata.

3. **Job payload size**: Each job needs enough context to execute independently (source_id, chunk_id or node_id, model config). Is the existing JSONB payload sufficient?

4. **Concurrency limits**: Should per-item jobs share the existing reprocess semaphore, or get their own? LLM rate limits may need a separate semaphore.

5. **Statement pipeline**: The statement pipeline (windowed extraction → HAC clustering → section compilation) is more complex than the code pipeline. Should it be decomposed the same way, or is it a separate design?

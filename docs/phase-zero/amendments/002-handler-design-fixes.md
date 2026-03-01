# Spec Amendment 002: Handler Design Fixes

**Date:** 2026-03-01  
**Status:** Draft  
**Affects:** §6.2 (Node Table), §6.4 (Edge Mirror), §6.5 (Slow-Path Queue/Log), §8.2 (Slow Path), worker handlers  
**Author:** Jane  
**Triggered by:** Architectural review of slow-path handler implementations vs. spec + schema

---

## Summary

Nine issues identified during handler design review. Each is a gap between the spec's stated design and the actual schema/handler code. This amendment prescribes concrete fixes for all nine.

---

## Item 1: Drop `edges.edge_type` CHECK Constraint

### Current State

`covalence.edges` has a CHECK constraint (`edges_edge_type_check`) enumerating 22 valid edge types. Migration 002 already had to DROP + re-ADD this constraint to include new labels. The spec (§2.2, §5.2) explicitly states: *"Extensible string labels; new edge types require no migration."* The CHECK contradicts this promise.

The handler code in `merge_edges.rs` (`handle_infer_edges`) can produce `RELATES_TO`, `DERIVES_FROM`, `CONCURRENT_WITH` etc. — all of which happen to be in the CHECK. But LLM output is unpredictable; a novel label like `REFUTES` would cause an INSERT failure at runtime.

### Proposed Change

1. **Drop the CHECK constraint permanently.** Validation moves to the Rust `EdgeType` enum, which is the correct extensibility boundary (add a variant, recompile — no migration).
2. **Add a Rust-side allowlist** in the `EdgeType` enum with a `Custom(String)` fallback variant for forward compatibility.
3. **Retain AGE `create_elabel`** for known labels (AGE requires pre-declared labels). Unknown labels fall back to a generic `RELATES_TO` AGE label with the real label stored in edge properties + the SQL mirror.

### Migration SQL

```sql
-- 002a_drop_edge_type_check.sql
ALTER TABLE covalence.edges
    DROP CONSTRAINT IF EXISTS edges_edge_type_check;

COMMENT ON COLUMN covalence.edges.edge_type IS
    'Extensible string label. Validated in application (Rust EdgeType enum). '
    'No CHECK constraint — new edge types require no schema migration.';
```

### Handler Code Changes

- `engine/src/graph/edge_type.rs`: Add `Custom(String)` variant to `EdgeType` enum with `impl From<String>` that maps unknown strings to `Custom`.
- `merge_edges.rs` (`handle_infer_edges`): Remove the `DERIVED_FROM` → `DERIVES_FROM` alias hack (line ~190). Instead, accept whatever the LLM returns after normalizing to uppercase, and let the enum's `From<String>` handle it.
- All edge INSERT call sites: No change needed (they already bind `&str`).

---

## Item 2: Collapse Confidence to Single Float

### Current State

The spec (§6.2) says: *"Single canonical confidence struct per node"* and the proposed schema shows a single `confidence real`. But the actual schema (`001_initial_schema.sql`) has **seven** confidence columns:

```
confidence_overall, confidence_source, confidence_method,
confidence_consistency, confidence_freshness,
confidence_corroboration, confidence_applicability
```

This is exactly the Valence v2 fragmentation problem the spec was designed to fix (§1.3: *"Confidence state fragmentation"*).

No handler code reads or writes the decomposed columns. `handle_compile` doesn't set any confidence at all. The decomposed columns are dead weight.

### Proposed Change

1. **Keep `confidence_overall`**, rename it to `confidence` for spec alignment.
2. **Move the six decomposed columns into `metadata.confidence_detail`** JSONB for audit trail. They are unused in handlers but may be needed for future re-computation (e.g., recursively adjusting confidence when a source is found untrustworthy).
3. **Set default to 0.5** (spec §6.2).

### Migration SQL

```sql
-- 002b_collapse_confidence.sql

-- Rename the keeper
ALTER TABLE covalence.nodes
    RENAME COLUMN confidence_overall TO confidence;

ALTER TABLE covalence.nodes
    ALTER COLUMN confidence SET DEFAULT 0.5;

-- Migrate decomposed columns into metadata JSONB, then drop
UPDATE covalence.nodes SET metadata = jsonb_set(
    COALESCE(metadata, '{}'::jsonb),
    '{confidence_detail}',
    jsonb_build_object(
        'source', confidence_source,
        'method', confidence_method,
        'consistency', confidence_consistency,
        'freshness', confidence_freshness,
        'corroboration', confidence_corroboration,
        'applicability', confidence_applicability
    ),
    true
) WHERE confidence_source IS NOT NULL
   OR confidence_method IS NOT NULL;

ALTER TABLE covalence.nodes
    DROP COLUMN IF EXISTS confidence_source,
    DROP COLUMN IF EXISTS confidence_method,
    DROP COLUMN IF EXISTS confidence_consistency,
    DROP COLUMN IF EXISTS confidence_freshness,
    DROP COLUMN IF EXISTS confidence_corroboration,
    DROP COLUMN IF EXISTS confidence_applicability;

-- Update get_chain_tips() to reference renamed column
CREATE OR REPLACE FUNCTION covalence.get_chain_tips()
RETURNS TABLE (
    id              UUID,
    title           TEXT,
    node_type       TEXT,
    status          TEXT,
    confidence      FLOAT,
    epistemic_type  TEXT,
    domain_path     TEXT[],
    usage_score     FLOAT,
    created_at      TIMESTAMPTZ,
    modified_at     TIMESTAMPTZ
)
LANGUAGE plpgsql STABLE AS $$
BEGIN
    RETURN QUERY
    SELECT n.id, n.title, n.node_type, n.status, n.confidence,
           n.epistemic_type, n.domain_path, n.usage_score,
           n.created_at, n.modified_at
    FROM covalence.nodes n
    WHERE n.node_type = 'article' AND n.status = 'active'
      AND NOT EXISTS (
          SELECT 1 FROM covalence.edges e
          WHERE e.target_node_id = n.id AND e.edge_type = 'SUPERSEDES'
      )
    ORDER BY n.usage_score DESC, n.modified_at DESC;
END;
$$;
```

### Handler Code Changes

- `handle_compile`: After inserting the article node, set `confidence` based on source reliability average (the formula from spec §9.3).
- `handle_resolve_contention` (`supersede_b` branch): Reduce article confidence by 0.1 (clamped to 0.1 floor) since content was overridden.
- All `SELECT confidence_overall` references in `age.rs` → change to `confidence`.

---

## Item 3: Add `inference_log` Writes to All 4 LLM-Calling Handlers

### Current State

`covalence.inference_log` exists (migrations 001 + 004) but **no handler writes to it**. The spec (§8.3) says every slow-path inference decision must be logged for graduation. The four LLM-calling handlers are:

1. `handle_compile` — calls `llm.complete()`, no log write
2. `handle_contention_check` — calls `llm.complete()`, no log write
3. `handle_infer_edges` — calls `llm.complete()` per candidate, no log write
4. `handle_resolve_contention` — calls `llm.complete()`, no log write

### Proposed Change

Add an `inference_log` INSERT after every successful `llm.complete()` call. Use the 004 schema columns.

### Migration SQL

None — table already exists from 004.

### Handler Code Changes

Add a shared helper:

```rust
// engine/src/worker/mod.rs
async fn log_inference(
    pool: &PgPool,
    operation: &str,
    input_node_ids: &[Uuid],
    input_summary: &str,
    output_decision: &str,
    output_confidence: Option<f64>,
    output_rationale: &str,
    model: &str,
    latency_ms: i32,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO covalence.inference_log
             (operation, input_node_ids, input_summary, output_decision,
              output_confidence, output_rationale, model, latency_ms)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
    )
    .bind(operation).bind(input_node_ids).bind(input_summary)
    .bind(output_decision).bind(output_confidence).bind(output_rationale)
    .bind(model).bind(latency_ms)
    .execute(pool).await?;
    Ok(())
}
```

Call sites (each wraps the `llm.complete()` call with `Instant::now()` timing):

| Handler | `operation` value | `input_summary` | `output_decision` |
|---------|-------------------|-----------------|-------------------|
| `handle_compile` | `"compile"` | Source IDs + title hint | Article title + epistemic_type |
| `handle_contention_check` | `"contention_check"` | Source ID + article ID + distance | `is_contention` + relationship |
| `handle_infer_edges` | `"infer_edge"` | Node ID + candidate ID + distance | Relationship + confidence |
| `handle_resolve_contention` | `"resolve_contention"` | Contention ID + article/source IDs | Resolution + materiality |

**Retention policy:** `inference_log` rows older than 90 days should be archived to cold storage (or deleted if graduation analysis has been performed). Add a maintenance task `prune_inference_log` that runs weekly. At current scale (~10 LLM calls/day) this is not urgent, but the policy must be documented before v1.

---

## Item 4: Add Missing `status` Values to Node Status CHECK

### Current State

The `nodes.status` CHECK allows: `'active', 'archived', 'tombstone'`.

The spec (§6.2) defines: `'active', 'superseded', 'archived', 'disputed'`.

Missing values:
- **`superseded`** — needed when a SUPERSEDES edge is created; the target node should transition to this status. Currently no handler sets this. The `handle_resolve_contention` `supersede_b` branch should mark the old version as superseded.
- **`disputed`** — needed when a high-materiality contention is detected against a node. Currently contention detection leaves article status as `active`.

### Proposed Change

Expand the CHECK constraint to the union set.

### Migration SQL

```sql
-- 002c_expand_node_status.sql
ALTER TABLE covalence.nodes
    DROP CONSTRAINT IF EXISTS nodes_status_check;

ALTER TABLE covalence.nodes
    ADD CONSTRAINT nodes_status_check
    CHECK (status IN ('active', 'superseded', 'archived', 'disputed', 'tombstone'));
```

### Handler Code Changes

- `handle_contention_check`: When a `high` materiality contention is created, set the article's status to `'disputed'`.
- `handle_resolve_contention` (`supersede_b`): After replacing content and creating SUPERSEDES edge, leave the article as `active` (it has new content). The *source* that won does not need a status change.
- `handle_compile` (dedup-update path): No status change needed (article stays active with updated content).
- Future: Any handler that creates a SUPERSEDES edge where the target is being fully replaced (not updated in-place) should set target status to `'superseded'`.

---

## Item 5: AGE Graph Sync Strategy

### Current State

The spec (§6.4) says *"AGE is canonical; the mirror is kept synchronous."* In practice, the handler code writes **only to the SQL `edges` table** — no AGE Cypher writes at all. The `age.rs` module has a `create_edge` method that writes to both AGE and SQL, but handlers bypass it and INSERT directly.

### Proposed Change

**Recommend: SQL-first with trigger-based AGE sync.**

Rationale:
- Handler code is already SQL-only — refactoring all handlers to use `AgeGraphRepository::create_edge` is high-effort and introduces a Cypher dependency in hot paths.
- A PostgreSQL AFTER INSERT trigger on `covalence.edges` can create the corresponding AGE edge.
- Recursive CTEs on the SQL `edges` table already serve the provenance walk use case.

**Strategy: Tiered approach**

| Use Case | Implementation |
|----------|---------------|
| Edge creation | SQL INSERT into `covalence.edges` (handlers, as-is) |
| Edge sync to AGE | AFTER INSERT trigger on `covalence.edges` → Cypher CREATE |
| Simple traversal (provenance, neighborhood ≤3 hops) | WITH RECURSIVE on `covalence.edges` |
| Complex traversal (depth > 3, pattern matching) | AGE Cypher on synced graph (future) |

### Migration SQL

```sql
-- 002d_age_sync_trigger.sql
LOAD 'age';
SET search_path = ag_catalog, "$user", public;

CREATE OR REPLACE FUNCTION covalence.sync_edge_to_age()
RETURNS TRIGGER
LANGUAGE plpgsql AS $$
DECLARE
    _src_age_id BIGINT;
    _tgt_age_id BIGINT;
    _edge_result agtype;
BEGIN
    SELECT age_id INTO _src_age_id FROM covalence.nodes WHERE id = NEW.source_node_id;
    SELECT age_id INTO _tgt_age_id FROM covalence.nodes WHERE id = NEW.target_node_id;

    IF _src_age_id IS NULL OR _tgt_age_id IS NULL THEN
        RETURN NEW;
    END IF;

    BEGIN
        EXECUTE format(
            $cypher$
            SELECT * FROM cypher('covalence', $$
                MATCH (s), (t)
                WHERE id(s) = %s AND id(t) = %s
                CREATE (s)-[e:%I {
                    sql_id: '%s',
                    confidence: %s,
                    created_at: '%s'
                }]->(t)
                RETURN id(e)
            $$) AS (edge_id agtype)
            $cypher$,
            _src_age_id, _tgt_age_id,
            NEW.edge_type,
            NEW.id,
            COALESCE(NEW.confidence, 1.0),
            COALESCE(NEW.created_at, now())
        ) INTO _edge_result;

        UPDATE covalence.edges SET age_id = (_edge_result::text)::bigint WHERE id = NEW.id;
    EXCEPTION WHEN others THEN
        RAISE WARNING 'AGE edge sync failed for edge %: %', NEW.id, SQLERRM;
    END;

    RETURN NEW;
END;
$$;

SET search_path = "$user", public;

DROP TRIGGER IF EXISTS trg_sync_edge_to_age ON covalence.edges;
CREATE TRIGGER trg_sync_edge_to_age
    AFTER INSERT ON covalence.edges
    FOR EACH ROW
    EXECUTE FUNCTION covalence.sync_edge_to_age();
```

**Label registry:** The trigger uses `format(..., NEW.edge_type)` as an AGE edge label identifier. If the label is not pre-declared, AGE will error. **Never map unknown labels to `RELATES_TO`** — this creates a semantic mismatch between the SQL mirror and AGE graph, breaking Cypher pattern matching.

**Solution: Label registry table + trigger guard.**

```sql
CREATE TABLE IF NOT EXISTS covalence.edge_label_registry (
    label TEXT PRIMARY KEY,
    created_at TIMESTAMPTZ DEFAULT now()
);
-- Pre-populate with all known labels from 001 + 002
INSERT INTO covalence.edge_label_registry (label) VALUES
    ('SUPERSEDES'),('SPLIT_FROM'),('COMPILED_FROM'),('CONFIRMS'),
    ('CONTRADICTS'),('CONTENDS'),('RELATES_TO'),('ELABORATES'),
    ('GENERALIZES'),('PRECEDES'),('FOLLOWS'),('INVOLVES'),
    ('ORIGINATES'),('EXTENDS'),('DERIVES_FROM'),('MERGED_FROM'),
    ('SPLIT_INTO'),('CONCURRENT_WITH'),('CAUSES'),('MOTIVATED_BY'),
    ('IMPLEMENTS'),('CAPTURED_IN')
ON CONFLICT DO NOTHING;
```

The Rust `EdgeType::Custom(String)` variant must call `ensure_label_exists()` before inserting an edge — this function INSERTs into the registry and calls `create_elabel` if the label is new. The trigger checks the registry and skips AGE sync (with WARNING) if the label is unregistered, rather than mapping to a wrong label.

**The trigger must also handle UPDATE and DELETE** — not just INSERT. ON CONFLICT updates to confidence (Item 6) must propagate to AGE edge properties, and edge deletions must remove the AGE edge.

### Handler Code Changes

- No handler changes needed (they already write to SQL `edges`).
- `age.rs`: Document that `create_edge` is the "direct AGE" path; normal handler flow uses SQL + trigger.

---

## Item 6: Edge Dedup Constraint + ON CONFLICT for Merge Handler

### Current State

No UNIQUE constraint prevents duplicate edges. `handle_infer_edges` checks with SELECT before INSERT (TOCTOU race). `handle_merge` provenance copy can produce duplicates when both parents share a source.

### Proposed Change

1. Add a UNIQUE constraint on `(source_node_id, target_node_id, edge_type)`.
2. Use `ON CONFLICT` at all INSERT sites.

### Migration SQL

```sql
-- 002e_edge_dedup.sql

-- Remove existing duplicates in batches (safe for large tables).
-- At current scale (~1K edges) a single pass is fine, but the batched
-- pattern is documented for future-proofing.
DO $$
DECLARE
    _deleted INT := 1;
BEGIN
    WHILE _deleted > 0 LOOP
        DELETE FROM covalence.edges
        WHERE id IN (
            SELECT a.id FROM covalence.edges a
            JOIN covalence.edges b
              ON a.source_node_id = b.source_node_id
             AND a.target_node_id = b.target_node_id
             AND a.edge_type = b.edge_type
             AND (a.created_at > b.created_at
                  OR (a.created_at = b.created_at AND a.id > b.id))
            LIMIT 1000
        );
        GET DIAGNOSTICS _deleted = ROW_COUNT;
    END LOOP;
END;
$$;

CREATE UNIQUE INDEX IF NOT EXISTS edges_dedup_idx
    ON covalence.edges (source_node_id, target_node_id, edge_type);
```

### Handler Code Changes

All edge INSERT statements across all handlers should use ON CONFLICT:

- **`handle_infer_edges`**: Replace SELECT-check + INSERT with:
  ```sql
  INSERT INTO covalence.edges (...) VALUES (...)
  ON CONFLICT (source_node_id, target_node_id, edge_type)
  DO UPDATE SET confidence = GREATEST(EXCLUDED.confidence, covalence.edges.confidence),
               metadata = EXCLUDED.metadata
  ```
  Remove the separate "does edge exist?" query.

- **`handle_merge`** (provenance copy): Add `ON CONFLICT (source_node_id, target_node_id, edge_type) DO NOTHING`.

- **`handle_compile`** (provenance edges): Add `ON CONFLICT ... DO NOTHING`.

- **`handle_split`** (provenance copy + SPLIT_INTO): Add `ON CONFLICT ... DO NOTHING`.

---

## Item 7: SUPERSEDES Edge on Contention Resolution + Version Increment

### Current State

`handle_resolve_contention` with `supersede_b` replaces article content but creates no SUPERSEDES edge and does not increment `version`. The version history is invisible.

### Proposed Change

On `supersede_b` resolution:
1. Increment `nodes.version` on the article.
2. Create a `CONFIRMS` edge from `source_node_id` → `article_id` (the source that provided the new content now confirms the updated article). **Note:** We do NOT use `SUPERSEDES` here because a source does not supersede an article — it contributes to it. The article is updated in-place with version increment, not replaced by a new node. If we later implement version-as-new-node, then Article_V2 SUPERSEDES Article_V1.
3. Store the old content hash in `metadata.previous_versions[]` for lightweight audit.

### Migration SQL

None — uses existing tables.

### Handler Code Changes

In `contention.rs` (`handle_resolve_contention`), `supersede_b` branch, after updating content:

```rust
// Increment version
sqlx::query("UPDATE covalence.nodes SET version = version + 1 WHERE id = $1")
    .bind(article_id).execute(pool).await?;

// Create CONFIRMS edge: source → article (source provided the winning content)
sqlx::query(
    "INSERT INTO covalence.edges
         (id, source_node_id, target_node_id, edge_type, weight, confidence, created_by)
     VALUES ($1, $2, $3, 'CONFIRMS', 1.0, 1.0, 'resolve_contention')
     ON CONFLICT (source_node_id, target_node_id, edge_type) DO NOTHING"
)
.bind(Uuid::new_v4()).bind(source_id).bind(article_id)
.execute(pool).await?;

// Record old content hash for audit trail
let old_hash = format!("{:x}", md5::compute(article_content.as_bytes()));
// (stored via the existing record_mutation helper with hash in summary)
```

Also in `handle_compile` (dedup-update path): increment `version` on the existing article.

---

## Item 8: Embedding Invalidation on Content Mutation

### Current State

Three content-mutating handlers exist. `handle_merge` and `handle_split` queue re-embed tasks. But:

1. **`handle_compile` (dedup-update path)**: Updates existing article content without queuing an embed task — stale embedding.
2. **Section embeddings** (`node_sections` from amendment 001): Never invalidated when article content changes.

### Proposed Change

1. `handle_compile` dedup-update: Queue embed task after update.
2. Trigger-based section invalidation on content change.

### Migration SQL

```sql
-- 002f_invalidate_sections_on_content_change.sql
CREATE OR REPLACE FUNCTION covalence.invalidate_sections()
RETURNS TRIGGER
LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.content IS DISTINCT FROM NEW.content THEN
        DELETE FROM covalence.node_sections WHERE node_id = NEW.id;
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER IF EXISTS trg_invalidate_sections ON covalence.nodes;
CREATE TRIGGER trg_invalidate_sections
    BEFORE UPDATE OF content ON covalence.nodes
    FOR EACH ROW
    EXECUTE FUNCTION covalence.invalidate_sections();
```

### Handler Code Changes

- `mod.rs` (`handle_compile`, dedup-update path, after the UPDATE): Add `enqueue_task(pool, "embed", Some(existing_id), json!({}), 5).await?;`
- Section invalidation is handled automatically by the trigger for all code paths.

---

## Item 9: Crash-Retry Idempotency Strategy for LLM Calls

### Current State

The worker retry pattern (reset to `pending`, increment `attempts`, max 3) is **not idempotent** for node-creating handlers:

- **`handle_compile`**: Crash after INSERT → retry creates duplicate article. Dedup (cosine < 0.15) only works if embedding exists — it doesn't yet.
- **`handle_merge`**: Crash after new node → retry creates second merged node.
- **`handle_split`**: Crash after part A → retry creates duplicate part A.
- **`handle_infer_edges`**: Safe after Item 6 (ON CONFLICT).
- **`handle_contention_check`**: Safe (existing-contention check).
- **`handle_resolve_contention`**: Mostly safe (WHERE status = 'detected' prevents double-apply).

### Proposed Change

**Strategy: Pre-generated output IDs + ON CONFLICT**

1. **Pre-generate output IDs at enqueue time.** Include in task `payload`:
   - `compile`: `payload.output_article_id`
   - `merge`: `payload.output_article_id`
   - `split`: `payload.part_a_id`, `payload.part_b_id`

2. **Use ON CONFLICT (id) DO NOTHING** on all node INSERT statements in these handlers.

3. **Detect prior completion.** At handler start, check if the output node already exists:
   ```rust
   if let Some(output_id) = task.payload.get("output_article_id") {
       let exists = sqlx::query("SELECT 1 FROM covalence.nodes WHERE id = $1")
           .bind(output_id).fetch_optional(pool).await?;
       if exists.is_some() {
           // Skip to edge creation / follow-up steps
       }
   }
   ```

4. **Record phase progress.** For multi-step handlers, write completed phase to `slow_path_queue.result`:
   ```json
   {"phase": "nodes_created", "output_ids": ["uuid-a", "uuid-b"]}
   ```
   On retry, check `result.phase` and resume.

### Migration SQL

None — uses existing JSONB columns.

### Handler Code Changes

**Enqueue sites** (API layer / other handlers that enqueue tasks):
- `POST /articles/compile`: Generate UUID v5 (namespace=task_id, name="output"), include as `payload.output_article_id`.
- `POST /articles/merge`: Generate UUID v5, include as `payload.output_article_id`.
- `POST /articles/{id}/split`: Generate two UUID v5s (name="part_a", "part_b"), include as `payload.part_a_id`, `payload.part_b_id`.
- `handle_compile` (when it enqueues `split`): Generate deterministic IDs from task_id.

**Why v5 UUIDs:** If a worker crashes after enqueuing a follow-up task (e.g., `split`) but before marking itself complete, retry must generate the *same* child IDs. Deterministic UUIDs (v5, namespace=task_id) ensure retries produce identical follow-up tasks, preventing orphaned "ghost nodes."

**Handlers**: Each node-creating handler reads the pre-generated ID from payload (falling back to `Uuid::new_v4()` for backward compat with existing queued tasks), and uses `INSERT ... ON CONFLICT (id) DO NOTHING`.

---

## Bonus Fix: Slow-Path Queue Task Type CHECK

### Current State

`slow_path_queue.task_type` CHECK allows only: `compile`, `infer_edges`, `resolve_contention`, `split`, `merge`. But the worker dispatches: `embed`, `tree_index`, `tree_embed`, `contention_check`. These INSERTs fail with a CHECK violation.

### Migration SQL

```sql
-- 002g_expand_queue_task_types.sql
ALTER TABLE covalence.slow_path_queue
    DROP CONSTRAINT IF EXISTS slow_path_queue_task_type_check;

COMMENT ON COLUMN covalence.slow_path_queue.task_type IS
    'Task type string. Validated in application code (worker dispatch match arm). '
    'No CHECK constraint — new task types require no schema migration.';
```

### Priority

**URGENT** — this is blocking task enqueueing in production right now.

---

## Migration Execution Order

Recommended sequence (all are independent but this order minimizes risk):

1. `002g_expand_queue_task_types.sql` — **P0**: unblocks embed/tree/contention tasks
2. `002a_drop_edge_type_check.sql` — removes constraint before new edge types
3. `002e_edge_dedup.sql` — dedup cleanup + UNIQUE index (must precede ON CONFLICT handler changes)
4. `002b_collapse_confidence.sql` — column rename + drops
5. `002c_expand_node_status.sql` — adds new status values
6. `002d_age_sync_trigger.sql` — can be deferred; best-effort sync
7. `002f_invalidate_sections_on_content_change.sql` — can be deferred

**Backward compatibility:** All migrations are additive or constraint-relaxing. No data destruction. Column drops (Item 2) target provably unused columns. Duplicate edge cleanup (Item 6) keeps older rows.

---

## Spec Sections to Update

| Section | Change |
|---------|--------|
| §6.2 (Node Table) | Remove decomposed confidence columns; add `superseded`, `disputed` to status |
| §6.4 (Edge Mirror) | Remove CHECK constraint; add UNIQUE dedup index; document trigger-based AGE sync |
| §6.5 (Slow-Path Queue) | Remove task_type CHECK; document all task types including embed, tree_index, tree_embed, contention_check |
| §8.2 (Slow Path) | Add inference_log write requirement for all LLM handlers |
| §8.3 (Graduation Logging) | Reference the `log_inference()` helper pattern |
| §8.4 (Never Automatic) | Note that `supersede_b` is automatic when LLM-driven; consider gating on materiality |

### `disputed` Status Lifecycle

Item 4 adds `disputed` status but the exit path must be defined:

| Event | Status Transition |
|-------|------------------|
| High-materiality contention detected | `active` → `disputed` |
| Contention resolved as `supersede_a` (article wins) | `disputed` → `active` |
| Contention resolved as `supersede_b` (source wins) | `disputed` → `active` (content replaced) |
| Contention resolved as `accept_both` | `disputed` → `active` (annotated) |
| Contention dismissed | `disputed` → `active` |

`handle_resolve_contention` must check if the article is `disputed` and transition it back to `active` upon resolution. If multiple contentions exist against the same article, only transition to `active` when **all** contentions are resolved.

---

*End of Amendment 002*

Loaded cached credentials.
Here is the deep dive into the 10 findings, complete with exact implementations, second-order effects, and specific guidance. Below that, I have identified four critical gaps you missed in the first pass.

### 1. SYNC BOTTLENECK: LISTEN/NOTIFY 8KB limit → Outbox Pattern
**A. EXACT Changes:**
```sql
CREATE TABLE outbox_events (
    seq_id BIGSERIAL PRIMARY KEY,
    entity_type TEXT NOT NULL, -- 'node' or 'edge'
    entity_id UUID NOT NULL,
    operation TEXT NOT NULL,   -- 'INSERT', 'UPDATE', 'DELETE'
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Example Trigger
CREATE OR REPLACE FUNCTION notify_outbox() RETURNS TRIGGER AS $$
BEGIN
    INSERT INTO outbox_events (entity_type, entity_id, operation, payload)
    VALUES (TG_TABLE_NAME, COALESCE(NEW.id, OLD.id), TG_OP, row_to_json(COALESCE(NEW, OLD)));
    NOTIFY graph_sync_ping; -- Empty payload, just a wake-up call
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;
```
**B. Second-Order Effects:** The `outbox_events` table will grow infinitely, consuming disk space and slowing down polling queries over time if not managed.
**C. Implementation Guidance:** Use `LISTEN/NOTIFY` exclusively as an empty "wake-up ping." When the Rust sidecar wakes up (or on a fallback 5-second poll), it executes: `SELECT * FROM outbox_events WHERE seq_id > $last_seen ORDER BY seq_id ASC LIMIT 1000;`. Maintain a background PG worker (or cron job) that runs `DELETE FROM outbox_events WHERE seq_id < $globally_acknowledged_seq_id` to prune the table.

### 2. ENTITY RESOLUTION RACE: Parallel workers creating duplicate nodes
**A. EXACT Changes:**
```rust
// Rust: In the resolution pipeline, acquire an advisory lock on the canonical name
let lock_key = compute_i64_hash(&canonical_name);

let mut tx = pool.begin().await?;
sqlx::query!("SELECT pg_advisory_xact_lock($1)", lock_key).execute(&mut tx).await?;

// Perform vector search, trigram match, and INSERT/UPDATE within this transaction
```
**B. Second-Order Effects:** Limits ingestion concurrency for documents that heavily reference the same entities simultaneously, slightly reducing overall throughput.
**C. Implementation Guidance:** Do not rely on SQL `UNIQUE` constraints. Entity resolution relies on fuzzy vector similarity and graph context, so a database-level string constraint won't catch semantic duplicates ("Apple" vs "Apple Inc"). The `pg_advisory_xact_lock` ensures that only one worker can attempt to resolve a specific entity name at a time, preventing split-brain insertions.

### 3. EPISTEMIC OSCILLATION: Sequential propagation conflicts
**A. EXACT Changes:**
```rust
struct EpistemicDelta {
    node_id: Uuid,
    new_opinion: SubjectiveOpinion,
}

// Instead of applying inline, calculate a delta map
fn compute_epistemic_closure(graph: &Graph, seeds: &[Uuid]) -> HashMap<Uuid, EpistemicDelta> {
    // Run fixed-point iteration in memory until |old - new| < epsilon
}
```
**B. Second-Order Effects:** The graph enters a temporarily "dirty" state where PG differs from the true epistemic state until the closure is computed and flushed.
**C. Implementation Guidance:** Treat epistemic updates as a state machine. When new edges arrive, queue the affected nodes. Run a centralized propagation loop that computes the new Dempster-Shafer/Subjective Logic opinions in memory. Write the fully converged scores back to PostgreSQL in a single batch transaction. Never `UPDATE` confidence scores sequentially during graph traversal.

### 4. SECURE BY DEFAULT: clearance_level defaults to public
**A. EXACT Changes:**
```sql
ALTER TABLE sources ALTER COLUMN clearance_level SET DEFAULT 0; -- 0 = local_strict
ALTER TABLE chunks ALTER COLUMN clearance_level SET DEFAULT 0;
ALTER TABLE nodes ALTER COLUMN clearance_level SET DEFAULT 0;
ALTER TABLE edges ALTER COLUMN clearance_level SET DEFAULT 0;
ALTER TABLE articles ALTER COLUMN clearance_level SET DEFAULT 0;
```
**B. Second-Order Effects:** Without explicit user action, the federation module will broadcast nothing. This requires building a new "Publish to Federation" UI/API flow.
**C. Implementation Guidance:** Implement a `POST /admin/publish/:source_id` endpoint. This endpoint must recursively upgrade the clearance of a source and *all* its derivative chunks and extractions. Add a SQL constraint to guarantee inheritance: `ALTER TABLE chunks ADD CONSTRAINT check_clearance CHECK (clearance_level <= (SELECT clearance_level FROM sources WHERE id = source_id))`.

### 5. CAUSAL LEVEL: Buried in JSONB
**A. EXACT Changes:**
```sql
ALTER TABLE edges ADD COLUMN causal_level TEXT CHECK (causal_level IN ('association', 'intervention', 'counterfactual'));
CREATE INDEX idx_edges_causal ON edges(causal_level) WHERE causal_level IS NOT NULL;
```
```rust
struct EdgeMeta {
    id: Uuid,
    causal_level: Option<String>,
    // ...
}
```
**B. Second-Order Effects:** Increases the static row size of `edges` but dramatically speeds up graph traversals that filter by causality.
**C. Implementation Guidance:** In the petgraph sidecar, store `causal_level` directly on the edge weight. When executing queries like "What caused Y?", traverse *only* edges where `causal_level == 'intervention'`. This prevents correlational noise (`ASSOCIATED_WITH`) from polluting causal chains at the compute layer.

### 6. VECTOR DIMENSION: Heterogeneity across entity types
**A. EXACT Changes:**
```sql
-- Remove hardcoded halfvec(768) if the model is configurable
ALTER TABLE chunks DROP COLUMN embedding;
ALTER TABLE chunks ADD COLUMN embedding halfvec; -- No fixed dimension
ALTER TABLE chunks ADD COLUMN embedding_model TEXT NOT NULL DEFAULT 'bge-base-en-v1.5';

-- Partial indexes are required for HNSW when dimensions aren't globally fixed
CREATE INDEX idx_chunks_embed_bge ON chunks USING hnsw ((embedding::halfvec(768)) halfvec_cosine_ops) WHERE embedding_model = 'bge-base-en-v1.5';
```
**B. Second-Order Effects:** You cannot mix models in the same PostgreSQL HNSW index. Changing the default model requires rebuilding indexes and potentially re-embedding the entire database.
**C. Implementation Guidance:** If you plan to support multiple models simultaneously, create a dedicated `embeddings` table (`entity_id`, `model_id`, `vector_data`). If sticking to one model at a time, use the partial index strategy above to allow for future migrations without dropping the table.

### 8. MISSING: Node split/merge API, cost budgeting, audit logs
**A. EXACT Changes:**
```sql
CREATE TABLE audit_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    action TEXT NOT NULL, -- e.g., 'MERGE_NODES'
    target_id UUID,
    payload JSONB,
    timestamp TIMESTAMPTZ DEFAULT now()
);
```
```rust
// POST /nodes/merge
struct MergeRequest { source_nodes: Vec<Uuid>, target_node: Uuid }
```
**B. Second-Order Effects:** Merging nodes natively requires rewriting `source_node_id` and `target_node_id` on all connected edges, invalidating the graph sidecar's memory and forcing a heavy sync.
**C. Implementation Guidance:** Do not physically UPDATE all edge foreign keys immediately. Instead, create a `SUPERSEDES` edge from the old nodes to the new merged node, and set the old nodes' `clearance_level` to `-1` (inactive). Update the Rust sidecar traversal logic to automatically follow `SUPERSEDES` edges to resolve to the new canonical node dynamically.

### 9. DECISION: Hybrid property graph with triples_view
**A. EXACT Changes:**
```sql
CREATE VIEW provenance_triples AS
SELECT
    e.id as triple_id,
    e.source_node_id as subject,
    e.rel_type as predicate,
    e.target_node_id as object,
    ex.chunk_id,
    ex.confidence
FROM edges e
JOIN extractions ex ON ex.entity_id = e.id AND ex.entity_type = 'edge';
```
**B. Second-Order Effects:** Complex `WHERE` clauses on this view may result in suboptimal PG query plans because of the underlying joins on massive tables.
**C. Implementation Guidance:** Expose this view *only* for the `/provenance` API endpoint and human debugging. Do not use it for core traversal algorithms. If it becomes a bottleneck, convert it to a `MATERIALIZED VIEW` and refresh it concurrently during the Batch Consolidation phase.

### 10. DECISION: Section-level contextual prefixes
**A. EXACT Changes:**
```rust
struct SectionContext {
    heading: String,
    prefix: String, // e.g., "From Q3 Report, Section: Outlook. "
}
```
**B. Second-Order Effects:** Increases embedding API token usage slightly, as the prefix is duplicated across every child chunk's payload.
**C. Implementation Guidance:** Generate the prefix via LLM at the `level = 'section'` chunks. Pass this prefix down the tree in memory and prepend it to the `content` of all `level = 'paragraph'` and `level = 'sentence'` child chunks *right before* hashing and embedding them.

---

### Critical Consistency Gaps Missed in the First Pass

**1. The Epistemic Leakage of Federation (Clearance vs Propagation)**
*   **The Gap:** If Public Node A and Public Node B are connected by an edge supported *only* by a Private source, the edge is marked Private. However, if TrustRank/PageRank runs globally over the local graph, the topological confidence of A and B will increase due to that private edge. When A and B's confidences are broadcast, their inflated scores mathematically reveal the existence of hidden private evidence (a side-channel leak).
*   **The Fix:** Epistemic algorithms must be computed *twice*. Once on the full graph (for local queries) and once on the `egress_view` subgraph (for federation broadcast). Federated confidence scores must be strictly derived from public data.

**2. Incomplete Source Update Classes**
*   **The Gap:** Section 05 defines `Append-Only`, `Versioned`, `Correction`, and `Refactor`. It misses **Deletion/Takedown** (e.g., GDPR requests or a user permanently deleting a file).
*   **The Fix:** Add a `Takedown` class. Deleting a source cannot just be a `DELETE FROM sources`. You must execute a Truth Maintenance System (TMS) Cascade: recursively locate and delete (or drastically penalize) any nodes and edges whose *sole* provenance was the deleted source.

**3. Fragile Chunk Hierarchy Metadata**
*   **The Gap:** `structural_hierarchy` is stored as a `TEXT` string (`"Title > Chapter 2 > Section 2.1"`). Pre-filtering queries will have to rely on brittle and slow `LIKE '%Chapter 2%'` SQL operations.
*   **The Fix:** Use the PostgreSQL `ltree` extension. Store the hierarchy as a material path (e.g., `doc_123.chapter_2.section_2_1`). This allows native, index-backed descendant queries (`path ~ '*.chapter_2.*'`), making structural pre-filtering instant.

**4. Arbitrary Search Dimension Weights**
*   **The Gap:** Reciprocal Rank Fusion uses linear multipliers for dimensions (`0.30 * RRF_vector + 0.25 * RRF_lexical`). While mathematically sound, these static presets are highly likely to be over-fitted to previous datasets (like valence).
*   **The Fix:** Implement an automated calibration loop. Store ground-truth query-result pairs. During the "Deep Consolidation" tier, run a hyperparameter tuning job (e.g., Bayesian optimization) to adjust these weights dynamically based on user acceptance/click-through rates on your specific data distribution.

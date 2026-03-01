# Covalence Slow-Path Handler Architecture Review

**Reviewer:** Architect (Jane)  
**Date:** 2026-03-01  
**Scope:** Logic & design correctness of worker handlers vs. Phase Zero Spec  
**Files:** `engine/src/worker/{mod.rs, merge_edges.rs, contention.rs}`

---

## CRITICAL FINDINGS

### C1: Confidence Model Regression — Spec's #1 Fix Unimplemented

**Spec §1.3** identifies "three drifting confidence representations" as a root-cause failure in Valence v2. **Spec §6.2** mandates: `confidence real NOT NULL DEFAULT 0.5` — a single canonical float.

**Actual schema** has SEVEN confidence columns: `confidence_overall`, `confidence_source`, `confidence_method`, `confidence_consistency`, `confidence_freshness`, `confidence_corroboration`, `confidence_applicability`.

This is literally the Valence v2 bug we are replacing. The schema reproduces the exact anti-pattern the spec was written to eliminate. No handler reads or writes any confidence column.

**Impact:** High. Violates spec §2.2 and §5.5.

### C2: No Inference Logging — Graduation Pathway Dead

**Spec §8.3:** "Every slow-path inference decision is written to `covalence.slow_path_log`." Schema has `inference_log` (different name). **Zero handlers write to it.** All LLM decisions (compile, contention_check, resolve_contention, infer_edges) go unlogged.

This kills the v0→v1 graduation pathway (spec §12.4).

**Impact:** High. Blocks the entire v1 intelligence layer.

### C3: Edge Type CHECK Constraint Contradicts Core Design Principle

**Spec §2.2:** "New edge type requires no migration." **Spec §5.2:** "No schema migration is required to add a new edge type."

**Actual schema:** `edges_edge_type_check CHECK (edge_type = ANY (ARRAY[...]))` — a frozen CHECK constraint requiring ALTER TABLE for new types.

**Spec §1.2** literally identifies this as Valence v2's failure: "Its edge vocabulary is frozen in a PostgreSQL CHECK constraint." We have reproduced the exact limitation.

**Impact:** High. Core architectural regression.

### C4: Status Enum Mismatch

**Spec:** `status IN ('active', 'superseded', 'archived', 'disputed')`  
**Schema:** `status IN ('active', 'archived', 'tombstone')`

Missing `superseded` and `disputed`. Handlers cannot properly reflect supersession or contention state on nodes.

### C5: Node Type Architecture Divergence

**Spec:** `node_type IN ('source', 'article', 'session')` unified in `nodes`. Entity excluded from v0.  
**Schema:** `node_type IN ('article', 'source', 'entity')` with separate `sessions` table. Entity is present (out of v0 scope); session is absent from the graph.

---

## MAJOR FINDINGS

### M1: Duplicate Provenance Edges on Merge
Merge provenance union has no dedup. No unique constraint on `(source_node_id, target_node_id, edge_type)`. Shared provenance sources produce duplicate edges.

### M2: Dead Contention Check in mod.rs
A ts_rank-only `handle_contention_check` exists in mod.rs but is never called (dispatch goes to `contention::handle_contention_check`). Dead code with wrong signature.

### M3: Version Not Incremented on Update
Compile dedup-hit path updates content but doesn't bump `version`. Split creates children at version=1.

### M4: Split Copies ALL Provenance to BOTH Halves
Spec §13.4 says "each inherit a subset." Code copies all to both, inflating provenance.

### M5: No AGE Graph Sync
Handlers write only to `covalence.edges` SQL table. AGE graph vertices/edges never populated. Graph traversal via Cypher non-functional.

### M6: Contention Resolution Missing SUPERSEDES Edge
`supersede_b` updates content but doesn't create a SUPERSEDES edge. Graph unaware of supersession.

---

## RECOMMENDATIONS (Priority Order)

1. Drop CHECK constraint on `edges.edge_type` — use Rust enum validation only
2. Collapse 7 confidence columns → single `confidence real`
3. Add inference_log writes to all LLM-calling handlers
4. Fix status enum: add superseded, disputed
5. Decide AGE fate: sync properly or drop in favor of recursive CTEs on edges table
6. Add unique constraint or ON CONFLICT for merge provenance union
7. Delete dead contention handler in mod.rs
8. Create SUPERSEDES edge in resolve_contention supersede_b
9. Increment version on article updates

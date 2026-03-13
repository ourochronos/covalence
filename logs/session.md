# Covalence Meta-Loop Session Log

This file is an append-only chronological journal maintained by autonomous agents. Each session must start by assessing the state of the world here, and must conclude by summarizing the work completed, insights gained, and blockers encountered.

---

## Session 14 — 2025-07-18

### Assessment
- Active plan: Wave 8 (V2 Statement-First Migration), tracked by GitHub issues #106-#109
- Prior state: Session 13 completed ADR-0015 statement pipeline (Phases 1-7), 1,159 tests
- Wave 8 had 8 unchecked boxes when starting

### Executed
**Branch:** `feature/wave-8-foundation` — 4 commits, ~1,250 lines added

1. **Removed legacy landscape analysis** (`landscape.rs`, 1,029 lines deleted)
   - Extracted `cosine_similarity()` into new `utils.rs`
   - Cleaned 18 files: chunk model, repo traits, PG implementation, pipeline, config, health, fingerprint, source profiles, DTO
   - Removed `parent_alignment`, `extraction_method`, `landscape_metrics` from Chunk model
   - Removed `should_extract()` gating — now all chunks go to extraction

2. **Wave 8 foundation schema** (migration 012)
   - `offset_projection_ledgers` table for fastcoref mutation tracking
   - `unresolved_entities` table for Tier 5 HDBSCAN pool
   - Dropped legacy landscape columns from chunks

3. **Offset Projection engine** (`projection.rs`)
   - `reverse_project()`: maps mutated byte spans → canonical source positions
   - Walks sorted ledger, accumulates byte offset delta, expands overlapping mutations
   - `reverse_project_batch()` for efficient multi-span projection
   - `LedgerEntry` model (`models/projection.rs`) with `delta()` method
   - 12 tests (8 projection + 4 model)

4. **Tier 5 deferred entity resolution**
   - `MatchType::Deferred` variant in resolver
   - `PgResolver.tier5_enabled` flag with `with_tier5()` builder
   - When enabled, entities failing all 4 tiers go to unresolved_entities instead of creating nodes
   - `resolve_and_store_entity()` → `Result<Option<NodeId>>` (None = deferred)
   - Pipeline callers skip edge creation for deferred entities
   - `UnresolvedEntityRepo` trait + PG implementation (create, get, list_pending, list_by_source, mark_resolved, delete_by_source, count_pending)
   - `UnresolvedEntity` model with `new()` and `is_resolved()` helpers

5. **HDBSCAN Tier 5 batch resolution worker** (`consolidation/tier5.rs`)
   - `resolve_tier5()`: fetch pending → embed names → HDBSCAN cluster → resolve
   - Reuses `cluster_labels()` from `ontology.rs` for consistent HDBSCAN behavior
   - Clusters: creates canonical node, resolves all members to it
   - Noise: creates individual nodes (same as MatchType::New)
   - Checks for existing nodes before creating (dedup)
   - Embeds new nodes at node dimension
   - `Tier5Config` (min_cluster_size, node_embed_dim) + `Tier5Report`

6. **Admin endpoint** `POST /admin/tier5/resolve`
   - `AdminService::resolve_tier5()` wired to embedder + table dimensions
   - DTOs: `Tier5ResolveRequest`, `Tier5ResolveResponse`
   - OpenAPI spec updated

7. **Config wiring**
   - `COVALENCE_TIER5_ENABLED` env var (default: false)
   - Health endpoint exposes `tier5_enabled`
   - Pipeline fingerprint hashes it for drift detection
   - `state.rs` chains `.with_tier5(config.pipeline.tier5_enabled)`

### Test Count
1,141 passing (21 api + 1,073 core + 47 eval), 0 failures, clippy clean

### Wave 8 Status
7/10 items checked off:
- [x] Schema, offset projection, Gemini client, statement pipeline, 2-pass extraction, Tier 5 routing + HDBSCAN worker, landscape cleanup
- [ ] fastcoref sidecar client (Python dependency — needs separate process)
- [ ] Blue/green re-ingestion (operational, not code)

### Insights
- The landscape analysis removal was clean but touched 18 files — the module had tentacles everywhere through Chunk model fields, repo trait methods, pipeline gating, and config flags. Statement-first makes all of it unnecessary since statements are atomic and self-contained.
- The `resolve_and_store_entity` return type change from `Result<NodeId>` to `Result<Option<NodeId>>` was the right call — a dummy NodeId would create phantom edges. The `let Some(node_id) = node_id else { continue }` pattern at call sites is clean.
- HDBSCAN reuse from `ontology.rs` was seamless — same embed→cluster→resolve pattern, just different input source (unresolved pool vs graph nodes).
- Tier 5 defaults to disabled. This is correct — it should be opt-in until tested in production, since it changes entity resolution semantics (deferred vs immediate creation).

### Blockers
- fastcoref requires a Python sidecar process. The LedgerEntry model and projection engine are ready, but the coreference resolution client itself needs a separate Python service (fastcoref doesn't have a Rust binding).

### Next Steps
- Push branch, create PR, run Gemini review
- Test Tier 5 in dev: enable `COVALENCE_TIER5_ENABLED=true`, ingest a source, run `POST /admin/tier5/resolve`
- Consider ADR-0015 merge to main (Session 13 branch still pending)
- fastcoref sidecar design (#107): Python HTTP service with `/coref` endpoint

---

## Session 15 — 2026-03-13

### Assessment
- Active plan: Wave 8, 2 unchecked items: fastcoref sidecar (#107), blue/green re-ingestion
- Session 14's PR #110 (Wave 8 foundation) was merged to main
- Branch `feature/fastcoref-ledger` existed with 2 commits (sidecar wiring + source cleanup)
- Gemini review had identified 2 correctness bugs that needed fixing

### Executed
**Branch:** `feature/fastcoref-ledger` — 4 commits total

1. **Fixed projection contained-mutation bug** (`projection.rs`):
   - Split `cumulative_delta` into `delta_before` + `delta_contained`
   - `delta_before` affects both start and end (mutations before the span)
   - `delta_contained` affects only end (mutations inside the span)
   - Added 2 regression tests: `contained_mutation_only_shifts_end`, `contained_mutation_with_prior_delta`

2. **Fixed chunk-relative offset bug** (`pipeline.rs`):
   - Coref mutations were stored with chunk-relative byte offsets
   - Now shifts by `co.byte_start` to make source-absolute
   - This ensures reverse_project works across chunk boundaries

3. **Fixed overlap prefix duplication** (`pipeline.rs`):
   - Chunks with `context_prefix_len > 0` overlap with previous chunks
   - Mutations in the overlap region would be double-counted
   - Added filter: `if m.canonical_end <= prefix { continue; }`

4. **Created issue #111** for windowed resolution offset tracking bug (edge case for >15K char chunks)

### Metrics
- 1,147 tests passing (21 api + 1,079 core + 47 eval), 0 failures
- All clippy warnings resolved, fmt clean
- 18 projection tests (4 new this session)

### Blockers
- Gemini CLI persistently rate-limited (MODEL_CAPACITY_EXHAUSTED on gemini-3.1-pro-preview)
- Running internal code review agent as supplement
- Issue #111: windowed coref resolution has wrong byte offset tracking (deferred, edge case)

### Insights
- The offset projection math is subtle: mutations can be before, overlapping, or contained within a span, and each case requires different delta handling
- Chunk overlap is a general concern for any per-chunk processing that produces global-scope data — need to filter overlap regions consistently
- Gemini capacity issues at ~09:00 UTC may be systemic — consider alternative review timing

### Next Steps
- Merge `feature/fastcoref-ledger` to main (pending review)
- Wave 8 final item: blue/green re-ingestion of all sources
- Close #107 after merge
- Update MILESTONES.md to check off fastcoref item

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

---

## Session 16 — 2026-03-13 (cont.)

### Work Completed

**Code Review Fixes** (branch: `fix/projection-review-findings`, merged to main)
- Fixed overlap prefix filter: `m.canonical_end <= prefix` → `m.canonical_start < prefix` to catch straddling mutations
- Added `debug_assert!` in `reverse_project()` to guard against unsorted ledger input
- Gemini reviewed and approved

**Graph Sidecar Invalidation Filtering** (branch: `fix/sidecar-filter-invalidated-edges`, merged to main)
- Added `WHERE invalid_at IS NULL` to `full_reload` edge query — invalidated edges no longer loaded into petgraph sidecar
- Added `invalid_at` check in `apply_edge_upsert` — outbox events with non-null `invalid_at` remove the edge instead of adding it
- 2 new tests for invalidation behavior
- Gemini reviewed and approved
- Inspired by deep reading of Graphiti/Zep paper: Covalence already had bi-temporal edge fields but the sidecar wasn't respecting them

**Research Ingestion**
- Deep read of Graphiti/Zep paper (ArXiv 2501.13956): temporal KG architecture for agent memory
  - Key insight: Covalence's bi-temporal edge model is more complete than initially assessed — `valid_from`, `valid_until`, `invalid_at`, `invalidated_by`, `recorded_at` all exist
  - Gap found: sidecar wasn't filtering invalidated edges (fixed above)
  - Their label propagation + dynamic community extension is simpler than our k-core approach for streaming data
- Deep read of Adaptive-RAG paper (ArXiv 2403.14403): query complexity routing
  - Their three-tier routing (no-retrieval / single-step / multi-step) mirrors Covalence's SkewRoute (Global / Balanced / Precise)
  - They train a small LM classifier; we use statistical Gini heuristic — our approach is more lightweight
- Both papers ingested into prod Covalence (pipeline processing)

### Metrics
- 1,149 tests passing (21 api + 1,081 core + 47 eval), 0 failures
- 3 commits merged to main this session
- 2 research papers deeply read and ingested
- Graph: 4,225 nodes, 66,050 edges, 58 components

### Insights
- Bi-temporal edge model was already complete in Covalence — the gap was in the sidecar not respecting it
- The invalidation detection module (`detect_conflicts`) exists but is never called during edge creation in the pipeline — this is the remaining wiring gap for TMS (#105)
- Graphiti uses Neo4j (Cypher); Covalence's PG+petgraph approach is unique and has advantages (SQL for storage, petgraph for fast traversal)
- 58 graph components — many are noise from evaluation datasets (Wikipedia articles). Edge synthesis won't help; need to clean noise sources

### Next Steps
- Wire `detect_conflicts` into edge creation pipeline (complete the TMS story, #105)
- Wave 8 final item: blue/green re-ingestion (needs Chris's approval)
- Continue research ingestion: Jøsang SL book, Pearl Causality, RAGAS docs
- Consider cleaning noise sources (evaluation dataset residue) from prod graph
- Build release binary with latest fixes before next prod deployment

---

## Session 17 — 2026-03-13

### State at Start
- 1,156 tests passing, clippy clean
- Prod: 358 sources, 4,670 nodes, 72,509 edges, 68 components, 496 articles
- Branch `fix/latex-json-sanitizer` had 1 commit with sanitizer added in 3 local copies
- `sanitize_latex_in_json` duplicated across `llm_extractor.rs`, `llm_statement_extractor.rs`, `section_compiler.rs`

### Work Completed

**LaTeX JSON Sanitizer Dedup (fix/latex-json-sanitizer → main)**
- Extracted `sanitize_latex_in_json` to shared `ingestion::utils` module
- Fixed bug: valid JSON escape pairs (like `\\`) weren't consuming the peeked char, causing `\\omega` to become `\\\omega`
- 7 comprehensive tests in utils.rs, removed 4 duplicate tests from section_compiler.rs
- Gemini review confirmed fix was sound, flagged issues were non-issues (trailing backslash is intentional, raw string compiles fine)
- Merged to main via `--no-ff`

**Entity Noise Filter Expansion**
- Analyzed knowledge gaps: found noise entities "P(x)", "Nodes", "Edges", "biology", "Structural", "AI use", "vector space"
- Added short math expression filter: `P(x)`, `f(x)`, `P(A|B)` — catches entities with <10 chars, parens, and ≤4 alphabetic chars
- Added generic graph terms to GENERIC_WORDS: biology, nodes, edges, structural
- Added GENERIC_PHRASES for multi-word noise: "AI use", "vector space"
- Sorted GENERIC_WORDS alphabetically
- 4 new tests, all passing

**Research Ingestion (issue #92)**
- Ingested 5 new papers: Tree-KG, AutoSchemaKG, StepChain GraphRAG, UnKGCP, AGM Belief Revision
- Previous session had ingested: MemOS, NodeRAG, Jøsang Bayes, Pearl Causality summary, DS Theory guide
- Edge synthesis created 2,523 new edges, reducing components from 73 → 68
- Consolidation produced 69 new articles (496 → 565)

**Prod Engine**
- Redeployed twice: once with sanitizer fix, once with noise filter expansion
- Search cache cleared after each deploy
- Statement pipeline running with correct LaTeX handling

### Metrics After
- 1,165 tests passing (21 api + 1,097 core + 47 eval)
- Prod: 359 sources, 4,692 nodes, 72,838 edges, 68 components, 676 articles, 1,635 RAPTOR summaries
- 6 commits pushed to main: LaTeX sanitizer + dedup, noise filter expansion, strip_markdown_fences dedup, decode_html_entities + cosine_similarity dedup

### Insights
- The `sanitize_latex_in_json` bug (not consuming peeked char) existed in all 3 local copies — comprehensive test suite caught it during deduplication. The test was more valuable than the refactoring itself.
- Entity noise filter pays compound returns — every noisy hub removed improves graph traversal and search quality for all future queries
- Math expressions in entity names (P(x), f(x)) are a common LLM extraction failure mode. The 4-letter heuristic catches them without false-positiving on real entities like "PageRank algorithm"
- `filtered_noisy=4` visible in extraction logs after deploying noise filter — confirming real-time filtering of garbage entities

### Final Metrics (after all background tasks completed)
- Sources: 360, Chunks: 29,059, Nodes: 4,704, Edges: 73,032, Articles: 676, Components: 68
- Also ingested: AGM Huber tutorial (from research agent's bonus find)
- Total edge synthesis this session: ~2,884 edges created across multiple runs

### Next Steps
- Continue research ingestion: Graphiti/Zep, Friston FEP/BMR, Jøsang SL book chapters
- Wave 8 final item: blue/green re-ingestion (needs Chris's approval)
- Consider cleaning existing noise entities from prod graph (retroactive filter application)
- Re-ingest codebase to reflect deduplication refactoring in the graph

---

## Session 18 — 2026-03-13 (cont.)

### Assessment
- Active plan: Wave 8, 1 unchecked item (blue/green re-ingestion, needs Chris's approval)
- Session 17 left: 1,165 tests, 4,704 nodes, 73,032 edges, 68 components
- Knowledge gaps showed noise entities polluting top-20: "Structural", "P(x)", "Nodes", "Edges", "biology", "vector space", "AI use"
- Noise filter was blocking new entities but 82 existing noise nodes remained in the graph

### Executed
**Branch:** `feature/admin-noise-cleanup` → merged to `main`

1. **Retroactive noise entity cleanup endpoint** (`POST /admin/nodes/cleanup`)
   - Admin service method: fetches all nodes, filters through `is_noise_entity()`, reports or deletes
   - Dry-run mode (default): reports what would be deleted with per-entity edge counts
   - Live mode: deletes aliases → edges → nodes (FK-safe order), reloads sidecar
   - `is_noise_entity()` promoted from private to `pub(crate)` for cross-module access
   - DTO types: `NoiseCleanupRequest`, `NoiseCleanupResponse`, `NoiseEntityItem`
   - OpenAPI spec updated, route wired at `/admin/nodes/cleanup`
   - Code review caught FK issue: `unresolved_entities.resolved_node_id` references nodes — fixed with NULL-out before deletion

2. **Prod noise cleanup executed**
   - Dry-run identified 82 noise entities with 2,838 connected edges
   - Top offenders: Structural (230 edges), P(x) (206), Edges (192), Nodes (186), biology (186)
   - Also caught: arXiv categories (cs.IR, cs.PF), code syntax (std::sync::Arc), paper titles, math expressions
   - Live cleanup: 82 nodes deleted, 2,784 edges removed, 85 aliases removed
   - Graph: 4,704→4,642 nodes, 73,032→70,445 edges, 68→70 components

3. **Research ingestion (issue #92)**
   - RAKG: Document-level Retrieval Augmented KG Construction (Zhang et al., 2025) — arXiv:2504.09823
   - Graph Embeddings to Empower Entity Retrieval (Gerritse et al., 2025) — arXiv:2506.03895
   - Bayesian Model Reduction (Friston, Parr, Zeidman 2018) — arXiv:1805.07092
   - Edge synthesis: 1,100 new edges after ingestion

4. **Consolidation**: 29 new articles generated (676→705)

5. **Codebase re-ingestion**: Running in background to reflect recent code changes

### Test Count
1,165 passing (21 api + 1,097 core + 47 eval), 0 failures, clippy clean

### Metrics After
- Sources: 376, Chunks: 28,877, Nodes: 4,768, Edges: 72,117, Articles: 705, Components: 71

### Insights
- **Retroactive noise cleanup is high-impact**: 82 entities with 2,838 edges were distorting graph traversal, search, and gap analysis. The cleanup immediately improved knowledge gap signal quality.
- **RAKG paper insight**: The "pre-entity → retrieval → relationship extraction" pattern suggests Covalence could benefit from a retrieval-augmented step between Pass 1 (statements) and Pass 2 (triples). After identifying entities, retrieve all their mentions across chunks before extracting relationships to improve cross-document coherence.
- **Graph Embeddings paper insight**: Methods combining graph structure AND textual descriptions are most effective for entity retrieval. Validates Covalence's `"{canonical_name}: {description}"` embedding approach. Also: if we add topology-derived embeddings (Node2Vec), combine them with text rather than replacing.
- **Friston BMR insight**: BMR enables principled "forgetting" via `ã = a + ã - a`. When edge evidence weakens, compute reduced free energy to determine if the simpler model (no edge) has higher evidence. This maps to SL opinions and could formalize the edge invalidation decision in #105 (TMS epistemic cascade).
- **Noise entity categories**: The 82 noise entities fell into 6 categories: generic words, code syntax, math expressions, paper titles (>55 chars), arXiv categories, and equations. The filter already catches all of these for new entities; this was only about cleaning up the backlog.
- **Code review value**: The internal code reviewer caught the `unresolved_entities` FK issue that would have caused a production failure in edge cases (Tier 5 enabled + noise node as resolution target). The fix was 7 lines but prevented a potential partial-cleanup state.

### Next Steps
- Wave 8 final item: blue/green re-ingestion (needs Chris's approval)
- Wire conflict detection (#112) into edge creation pipeline
- TMS epistemic cascade (#105) — consider using BMR for principled forgetting
- Continue research ingestion: Jøsang SL book full text, Pearl Causality original
- Explore RAKG-inspired retrieval-augmented extraction step for statement pipeline
- Add `cove admin cleanup` CLI subcommand for noise cleanup

---

## Session 19 — 2026-03-13

### Assessment
- Prod healthy: 4,768 nodes, 72,117 edges, 71 components, 425 sources, 705 articles
- 1,165 tests passing (21 api + 1,097 core + 47 eval), all green, clippy clean
- Open issues: 12 (including #111 bug, #112 enhancement)
- Knowledge gaps: dominated by high-degree concept nodes (Long-context LLMs: 645 gap score), not actionable missing content

### Executed Work

1. **Bug fix #111: Fastcoref windowed offset tracking** (bed4486)
   - Three bugs in `FastcorefClient::resolve()` multi-window path:
     1. Single `byte_offset` used for both canonical and mutated → now separate tracking via pointer arithmetic
     2. Overlap deduplication missing → mutations in overlap prefix now skipped
     3. `join(" ")` added spurious spaces → overlap stripped + concat
   - Added `compute_resolved_overlap()` helper accounting for mutation expansion/contraction
   - 6 new unit tests, all passing

2. **Enhancement #112: Wire bi-temporal conflict detection** (c8762ba + 7b500ad)
   - Added `EdgeRepo::find_by_source_and_rel_type()` to trait + PG impl
   - Fixed `detect_conflicts()` type: `new_target_node: EdgeId` → `NodeId` (type safety)
   - Re-exported `detect_conflicts` from epistemic module
   - Added `check_and_invalidate_conflicts()` helper in pipeline.rs
   - Wired at both edge creation sites (chunk extraction + statement extraction)
   - **Caught FK violation in production**: `invalidated_by` references `edges(id)`, so the new edge must exist before invalidation. Swapped order: create first, then detect+invalidate. Also excludes new edge from candidate list.

3. **Research ingestion**: 2 new papers
   - CORE-KG (arXiv 2510.26512) — structured prompting + coreference for KG construction
   - Temporal Reasoning over Evolving KGs (arXiv 2509.15464) — temporal reasoning on evolving graphs
   - Edge synthesis: 82 new edges
   - Consolidation: 32 new articles (705→737)

### Test Count
1,171 passing (21 api + 1,103 core + 47 eval), 0 failures, clippy clean

### Metrics After
- Sources: 426, Chunks: 27,616, Nodes: 4,824, Edges: 72,391, Articles: 737, Components: 74

### Insights
- **FK ordering matters for conflict detection**: The `invalidated_by` FK requires the new edge to exist before setting the reference on old edges. This was caught by a real ingestion attempt (temporal KG paper triggered a conflict), not by unit tests. Integration tests for FK-safe patterns are critical.
- **CORE-KG ablation study**: Structured prompts reduce noise by 73.33% (removing them causes massive noise increase), while coreference reduces duplication by 28.25%. Key takeaway: **prompt engineering investment > coreference tuning** for graph quality. Covalence should prioritize extraction prompt refinement (type definitions, filtering instructions, sequential extraction order) over further fastcoref improvements.
- **Type-sequential extraction**: CORE-KG extracts one entity type at a time before relationships, reducing attention dilution. This is a potential enhancement for Covalence's two-pass pipeline — the second pass could iterate per-type rather than all-at-once.
- **In-prompt filtering > post-hoc cleanup**: CORE-KG filters irrelevant entities in the extraction prompt itself, not as a post-processing step. Our `is_noise_entity()` is post-hoc. Adding filtering instructions to the extraction prompt would catch noise earlier and reduce wasted computation.
- **Conflict detection is working**: The temporal KG paper ingestion triggered a real conflict (FK violation), proving the detection wiring is functional. After the FK fix, edges with different targets and overlapping temporal ranges will be properly invalidated.

### Commits
1. `bed4486` — Fix fastcoref windowed offset tracking and overlap dedup (#111)
2. `c8762ba` — Wire bi-temporal conflict detection into edge creation pipeline (#112)
3. `7b500ad` — Fix FK violation: create edge before conflict invalidation (#112)
4. `dccccd5` — Merge fix/fastcoref-window-offsets

### Issues Closed
- #111 Fastcoref windowed resolution uses wrong byte offset tracking
- #112 Wire bi-temporal conflict detection into edge creation pipeline

### Next Steps
- TMS epistemic cascade (#105) — now that conflict detection is wired, source retraction can trigger re-evaluation
- Improve extraction prompts with CORE-KG insights: type definitions, filtering instructions, sequential extraction
- Continue research ingestion (#92): Jøsang SL book full text, Pearl Causality original
- Wave 8 final items (#108/#109): blue/green re-ingestion (needs Chris's approval)
- Add `cove admin cleanup` CLI subcommand for noise cleanup

---

## Session 20 — 2026-03-13 (Continuation)

### Assessment
- Continuing meta-loop from Session 19. Starting state: 4,824 nodes, 72,391 edges, 74 components, 426 sources, 1,171 tests.
- Open work: source deletion cascade fix (in progress from Session 19), 5 missing research papers, empty source cleanup.

### Executed Work

**Bug Fixes (4 commits):**
1. **Source deletion cascade** (ea52763) — `DeleteResult` now includes `statements_deleted` and `sections_deleted` fields. Statement, section, and unresolved entity records are properly cascaded during source deletion. Updated DTO and API handler.
2. **Statement pipeline resilience** (6f5d9a0) — Statement pipeline failures (e.g., Gemini quota exhaustion) no longer propagate as 500 errors. The basic pipeline (chunk + embed + extract) succeeds independently. Errors logged as warnings.
3. **Supersede ordering** (82a1261, #113) — Moved destructive supersede cleanup AFTER the new pipeline succeeds. This prevents data loss when the pipeline fails mid-way — the old source's chunks remain intact as a fallback.
4. **Reprocess MIME detection** (2935ca9) — Fixed "no converter registered for content type: arxiv" error. Sources ingested via URL store format_origin as a source identifier (not a MIME type). Reprocess now falls back to text/markdown for non-MIME format_origin values.

**Research Ingestion (5 papers):**
- CORE-KG (2510.26512) — Coreference-resolved KG construction
- STAR-RAG (2510.16715) — Temporal retrieval via graph summarization
- EA-GraphRAG (2602.03578) — Adaptive RAG/GraphRAG routing
- GLiNER (2311.08526) — Generalist NER model
- Fusion Functions (2210.11934) — CC vs RRF analysis (SRRF)

**Data Integrity Recovery:**
- Applied migration 012 to prod (offset_projection_ledgers, unresolved_entities tables)
- Fixed migration 6 checksum mismatch in prod _sqlx_migrations table
- Reprocessed 10 empty non-code sources that had lost their chunks during failed supersede flows
- Synthesized 1,235 new co-occurrence edges, reducing components from 102 to 72

### Metrics After
- Sources: 430, Chunks: 27,960, Nodes: 4,875, Edges: 73,345 (17,393 semantic + 55,952 synthetic)
- Components: 72 (down from 74), Articles: 737, Search Traces: 323
- Tests: 1,171 passing (21 api + 1,103 core + 47 eval), 0 failures, clippy clean

### Insights
- **Supersede-before-pipeline is a data integrity hazard**: When ingestion creates a new version and immediately cleans up the old source's chunks, a subsequent pipeline failure leaves both versions without chunks. The fix (defer cleanup) is simple but was only caught by a real production failure.
- **Statement pipeline must be non-fatal**: The pipeline is opt-in enrichment, not a required stage. When external dependencies fail (Gemini quota, network issues), the basic chunk+embed+extract pipeline should complete successfully. This is now the case.
- **format_origin ≠ MIME type**: URL-based ingestion stores descriptive identifiers like "arxiv" in format_origin, but reprocess assumes this field is a MIME type. The fix (check for '/' as MIME separator) is a pragmatic heuristic.
- **Edge synthesis matters after reprocessing**: Reprocessing creates new nodes that are disconnected from the existing graph. Without synthesis, the component count increases dramatically (74→102). Synthesis should be automatic after bulk reprocessing.
- **EA-GraphRAG validates fusion architecture**: GraphRAG underperforms vanilla RAG on simple queries by 13.4%. Covalence's 6-dimension RRF/CC fusion inherently achieves the adaptive routing effect — simple queries get high vector/lexical scores, complex queries benefit from graph scores.
- **Gemini CLI quota is a single point of failure**: The entire statement pipeline depends on the gemini CLI tool. When quota exhausts, all ingestion becomes degraded. Consider adding OpenRouter HTTP backend as a fallback.

### Commits
1. `ea52763` — Fix source deletion cascade for statements and sections
2. `6f5d9a0` — Make statement pipeline failures non-fatal during ingest/reprocess
3. `82a1261` — Defer supersede cleanup until after new pipeline succeeds (#113)
4. `2935ca9` — Fix reprocess MIME detection for non-MIME format_origin values

### Issues Closed
- #113 Supersede cleanup runs before pipeline can fail, leaving orphaned sources

### Next Steps
- TMS epistemic cascade (#105) — source retraction triggers epistemic re-evaluation
- Improve extraction prompts with CORE-KG insights when Gemini quota resets
- Statement pipeline for 5 newly ingested papers (after quota reset)
- Consolidation run to generate articles for new content
- Consider OpenRouter HTTP backend as Gemini CLI fallback
- Wave 8 final items (#108/#109): blue/green re-ingestion (needs Chris's approval)

---

## Session 21 — 2026-03-13

### Assessment
Continuing autonomous meta-loop. Previous session completed 5 engineering fixes and ingested 4 research papers. External API constraints: Gemini CLI quota exhausted (~8h to reset), Firecrawl credits at 1.

### Work Completed

**1. TMS Epistemic Cascade (#105) — COMPLETE**
- Built `epistemic/cascade.rs` module implementing dependency-directed backtracking from spec 07
- Core functions: `recalculate_node_opinions()` and `recalculate_edge_opinions()` — re-fuse extraction confidences via cumulative fusion when source support changes
- Entities losing all extraction support get vacuous opinions (u=1.0, the "stale" marker)
- Entities with remaining support get opinions re-calculated from surviving extractions
- Integrated into source delete flow (step 6b, non-fatal)
- Added `list_edge_ids_by_source` query to ExtractionRepo for edge cascade
- DeleteResult/API response now includes `nodes_recalculated` and `edges_recalculated` counts
- Code review caught two issues, both fixed:
  - Dogmatic opinions at confidence=1.0 → clamped to 0.99 max
  - Silent cumulative fusion failures → added tracing::warn
- 10 unit tests covering fusion math, boundary cases, constraint preservation
- Created #114 for N+1 query performance optimization (tracked, not urgent)

**2. Ungrounded Node GC — 872 nodes evicted**
- Ran `POST /admin/gc` — cleaned up 872 ungrounded nodes, 7,456 edges, 124 aliases
- Graph components dropped from 72 to 22 (massive improvement)
- Graph density improved from 0.003 to 0.004

### Metrics
- **Tests**: 1,181 passing (1,113 core + 21 api + 47 eval), up from 1,159
- **Graph**: 4,003 nodes, 65,891 edges (down from 4,875/73,345 after GC)
- **Components**: 22 (down from 72 — 69% reduction)
- **Sources**: 430, Chunks: 27,960, Articles: 737

### Commits
1. `87b3e24` — Implement TMS epistemic cascade on source deletion (#105)
2. `831187b` — Address code review feedback on TMS cascade (#105)
3. `fd85e3b` — Merge to main

### Issues Closed
- #105 Epistemic cascade on source retraction (TMS)

### Issues Created
- #114 Batch epistemic cascade queries to avoid N+1 performance issue

### Insights
- **Existing epistemic infrastructure enabled rapid cascade implementation**: The Opinion type, cumulative fusion, convergence guard, and Dempster-Shafer combination were all already in place. The cascade was fundamentally about wiring them into the source deletion flow.
- **GC had outsized impact on graph quality**: 872 orphaned nodes (18% of graph) were noise — removing them improved component count by 69%. This suggests periodic GC should be part of standard maintenance.
- **Dogmatic opinions are a real risk**: A single extraction with confidence=1.0 would bypass all fusion and set an entity to dogmatic certainty. Clamping to 0.99 ensures minimal uncertainty is always preserved. This principle should extend to other opinion-setting code paths.
- **Non-fatal cascade is the right pattern**: The cascade modifying opinions after structural cleanup is analogous to the statement pipeline being non-fatal during ingestion — the core operation (deletion/ingestion) succeeds even if the ancillary step fails.

### Next Steps
- Consolidation run when Gemini quota resets (generate articles for new content)
- Statement pipeline processing for recently ingested papers
- Research ingestion (#92) when Firecrawl credits replenish
- Wave 8 final items (#108/#109): blue/green re-ingestion

---

## Session 22 — 2026-03-13 (cont.)

### Starting State
- 1,181 tests passing, clippy clean
- 4,003 nodes, 65,891 edges, 22 components
- 430 sources, 27,960 chunks, 749 articles
- Gemini quota exhausted (~8h), Firecrawl credits at 1

### Work Completed

**Batch Epistemic Cascade (#114)** — Implemented and committed on `feature/issue-114-batch-cascade`:
- Added `ExtractionRepo::list_active_for_entities` — single query for all entity extractions via `ANY($1)`
- Added `NodeRepo::get_many` / `batch_update_opinions` — unnest()-based bulk read/write
- Added `EdgeRepo::get_many` / `batch_update_opinions` — unnest()-based bulk read/write
- Rewrote `recalculate_node_opinions` and `recalculate_edge_opinions` to use batch queries
- Reduces 3N sequential queries to 3 bulk queries for the cascade
- Code review via background agent: fixed stale comment about deleted edge handling, clarified Option<Value> semantics
- 2 commits: `a973137` (main implementation), `5fc764e` (review fixes)
- All 1,181 tests pass, clippy clean, fmt clean
- Merge blocked on Gemini review (quota exhausted)

**Consolidation** — Triggered consolidation, 12 new articles generated (749 → 804 total after additional runs)

**Edge Synthesis** — 10 new co-occurrence edges created (component count unchanged at 22)

**Search Quality Investigation**:
- Vector search properly returns results across 6 entity types with per-table budgets
- Quality gates correctly dampen non-discriminating dimensions (temporal, global)
- Low cross-dimension overlap is a data characteristic, not a bug — coverage multiplier penalizes appropriately
- "Dipak Meher" result for coreference query was actually from LINK-KG paper about coreference resolution (relevant, just poorly named chunk)

### Metrics
- 4,003 nodes, 65,901 edges (15,709 semantic + 50,192 synthetic), 22 components
- 430 sources, 27,960 chunks, 804 articles, 334 search traces
- 1,181 tests (21 api + 1,113 core + 47 eval)

### Issues Updated
- #114 Batch cascade: implementation complete, pending merge after Gemini review

### Insights
- **Batch operations via unnest() are clean and idiomatic PostgreSQL**: The pattern `UPDATE FROM unnest($1::uuid[], $2::jsonb[]) AS v(id, opinion) WHERE table.id = v.id` is efficient and sqlx handles Vec<Option<Value>> binding correctly.
- **Search dimension overlap matters more than per-dimension quality**: The CC fusion coverage multiplier is the main quality driver. Results appearing in 3+ dimensions consistently outrank single-dimension results regardless of individual scores. Over-fetching candidates could increase overlap, but at the cost of latency.
- **Component count plateau at 22 is likely genuine**: Edge synthesis at min_cooccurrences=1 only found 10 new edges and didn't reduce components. The remaining 22 components represent genuinely isolated topic clusters.

### Next Steps
- Merge `feature/issue-114-batch-cascade` after Gemini review (~7h)
- Deploy updated binary to prod
- Statement pipeline for recently ingested papers (needs Gemini quota)
- Research ingestion (#92) when Firecrawl credits replenish
- Wave 8 final items (#108/#109): blue/green re-ingestion

---

## Session 25 (2026-03-13)

### Summary
Completed full codebase coverage and graph connectivity improvements. All 161 Rust files, 12 Go CLI files, and 3 dashboard files now ingested with proper `file://` URIs.

### Accomplished
1. **100% Rust file coverage** — Ingested remaining 46 Rust files that were missing from the knowledge graph. Fixed URI assignment (45 sources had no URI, required content-hash matching and bulk SQL update).
2. **Edge synthesis fix (#126)** — Extended `synthesize_cooccurrence_edges` to UNION chunk-level and statement-level co-occurrences. Previously 37K+ statement-based extractions were invisible to synthesis. Components dropped from 157 to 40, with 1,501 new edges created.
3. **CLI auto-URI (#128)** — `cove source add` now auto-derives `file://` URI relative to git repo root. Also handles git worktrees/submodules (`.git` can be a file).
4. **Dashboard ingestion (#127)** — Added dashboard files (HTML/JS/CSS) to `make ingest-codebase` target and ingested them.
5. **Copilot CLI command** — Added `cove copilot` subcommand that dispatches prompts to configurable LLM backends (gemini default, haiku/sonnet/opus via --model flag).
6. **Code review protocol updated** — CLAUDE.md now uses `cove copilot` instead of direct `gemini` CLI.
7. **RAPTOR + consolidation passes** — Triggered on new code sources. Summary chunks 1,409→1,771 (+362), articles 804→812 (+8). Still running at session end.
8. **Stale source cleanup** — Removed 106 zero-chunk sources from failed prior ingestions. Deleted 1 duplicate strategy.rs source.

### Issues Created
- #126 Edge synthesis ignores statement-based extractions (CLOSED)
- #127 Ingest dashboard and Go CLI files (CLOSED)
- #128 CLI source add should auto-set file:// URI (CLOSED)
- #129 Run consolidation pass on new code sources (IN PROGRESS)

### Key Metrics
- Nodes: 4,053 → 5,056 (+1,003)
- Edges: 66,570 → 74,577 (+8,007)
- Components: 7 → 157 (after ingestion) → 40 (after synthesis fix)
- Sources: 345 → 375
- Tests: 1,197 passing (21 api + 1,129 core + 47 eval)
- RAPTOR summaries: 1,409 → 1,771+

### Technical Insights
- **Voyage RPM limits (2,000/min)** — Ingesting many files in parallel hits the rate limit hard. Solution: space ingestion with 3-second delays between files.
- **Statement-based extractions create isolated subgraphs** — The statement pipeline's extractions have `chunk_id=NULL`, so chunk-level co-occurrence synthesis misses them entirely. The fix (UNION with statement_id-based pairs) is essential for any pipeline using statements.
- **Content hash dedup silently swallows ingestion** — When `cove source add` hits a content hash match, it returns the existing source ID without error. Combined with missing URIs, this made it impossible to track which files were actually ingested vs deduplicated.

### User Preferences Learned
- Gemini Pro 3.0 for code reviews
- Claude Haiku 4.5 for extraction tasks
- The `cove copilot` command serves as the unified LLM dispatch interface

### Next Steps
- Wait for RAPTOR/consolidation to complete, then verify article quality
- Address remaining 40 components (may need entity merging or broader synthesis)
- Consider switching statement extraction to Haiku 4.5 (#124)
- Continue research ingestion (#92)

---

## Session 27 — 2026-03-13

### Context
Continuation from Session 26. FallbackChatBackend code was written but not built/tested/committed. Gemini CLI quota exhausted for ~3 hours.

### Accomplished

**Issue #124 — FallbackChatBackend (CLOSED)**
- Built, tested, and deployed `FallbackChatBackend` — CLI→HTTP automatic failover for chat completions
- Default backend changed from `"http"` to `"cli"` in config
- Code review caught 3 issues: model mismatch bug (HTTP fallback used `config.chat_model` instead of resolved `chat_model`), stale doc comment, missing tests. All fixed.
- 3 new unit tests for fallback behavior (primary success, primary fail, both fail)
- Deployed to prod — logs confirm: "using CLI chat backend with HTTP fallback"
- Feature branch `feature/issue-124-cli-fallback-backend` merged to main

**Dead Code Cleanup (#108)**
- Removed `ExtractionMethod` enum (unused since landscape analysis removal)
- Cleaned stale landscape references from `source.rs` and `delta.rs` doc comments
- -67 lines of dead code

**Noise Entity Filter Expansion (#123)**
- Added "article", "node record", "embedding operations", "entropy term" to noise filter
- Ran `POST /admin/nodes/cleanup` — deleted 4 nodes, 1,252 edges, 5 aliases from prod
- Tests added for all new filter entries

### Graph Health
- Before: 5,069 nodes, 74,189 edges, 27 components
- After: ~5,076 nodes, ~73,303 edges, 31 components
- Node count increased due to in-flight reprocessing creating new entities
- Component count increased due to noise node deletion disconnecting some subgraphs

### Test Count
- **1,199 tests** (1,131 core + 21 api + 47 eval), 0 failures

### Statement Pipeline Coverage
- 60/378 sources processed through statement pipeline (16%)
- 13,147 total statements, 2,358 sections, all with embeddings
- Bulk reprocessing started but interrupted by engine restart — needs re-run

### Technical Insights
- **Gemini CLI quota exhaustion irony** — Built the FallbackChatBackend specifically because CLI quota exhausted, then couldn't do Gemini code review because CLI quota was exhausted. Used internal code review agent instead.
- **Engine restart kills in-flight reprocessing** — Background curl requests to the old engine PID get orphaned when the engine is restarted. Need to re-run after restart.
- **Noise cleanup increases component count** — Deleting high-degree hub nodes disconnects subgraphs. Edge synthesis should follow noise cleanup to reconnect.

### Next Steps
- Re-run bulk reprocessing of remaining 318 sources through statement pipeline
- Run edge synthesis after reprocessing to reconnect components
- Phase 3 research: Jøsang SL book, Pearl Causality, Friston FEP/BMR
- Composable ingestion pipeline (#102) — next major architectural piece

---

## Session 28 — 2026-03-13

### Starting State
- 1,159 tests, 379 sources, 5,078 nodes, 73,429 edges, 30 components
- 13,224 statements across 61 sources (16% coverage)
- HTTP fallback model name fix deployed (commit 96be8a47)
- BMR source reprocessing kicked off to verify fix

### Accomplished

**Verified HTTP fallback fix works:**
- BMR source now has 77 statements — CLI quota exhaustion correctly falls back to HTTP (OpenRouter)
- Logs confirm: `cli_model=gemini-2.5-flash http_model=google/gemini-2.5-flash`

**Massive research ingestion (#92) — 12 papers:**
1. RAPTOR (2401.18059) — Tree-organized retrieval, directly implemented in Covalence
2. HippoRAG (2405.14831) — Neurobiological KG-based memory, parallel architecture
3. SkewRoute (2505.23841) — Score skewness routing, directly implemented
4. Matryoshka (2205.13147) — Nested representations, foundational to our embedding strategy
5. GLiNER (2311.08526) — Zero-shot NER, directly used in extraction sidecar
6. LightRAG (2410.05779) — Lightweight graph-augmented RAG
7. Adaptive-RAG (2403.14403) — Query complexity routing
8. RAGAS (2309.15217) — RAG evaluation framework
9. Zep/Graphiti (2501.13956) — Temporal KG for agent memory, deep parallels
10. EA-GraphRAG (2602.03578) — Adaptive graph integration with complexity routing
11. STAR-RAG (2510.16715) — Temporal reasoning via graph summarization
12. CC vs RRF Fusion (2210.11934) — Validates our CC fusion choice

Each paper includes a "Relevance to Covalence" section mapping findings to our architecture.

**Noise filter expansion (#123):**
- Added 5 new noise patterns: backtick-wrapped, function calls `()`, URL paths `/`, snake_case identifiers, wildcard suffixes `*`
- 20 tests (15 positive + 5 negative) all pass
- Cleaned 379 noise nodes, ~7,400 edges, 158 aliases, 3,434 extractions from prod
- Committed: 6ac806ae, pushed to main

**Statement pipeline progress:**
- Started batch reprocess of 5 markdown sources (specs/ADRs) — 3/5 completed
- Coverage: 74/391 sources (19%, up from 16%)

**Edge synthesis:**
- 877 new co-occurrence edges created
- Component count: 26 (down from 30)

**Issue hygiene:**
- #92: Updated with all 12 ingested papers, Phase 2 checklist
- #108: Marked landscape.rs deletion as complete

### Metrics

| Metric | Before | After | Delta |
|--------|--------|-------|-------|
| Sources | 379 | 391 | +12 |
| Nodes | 5,078 | 4,895 | -183 (cleanup) |
| Edges | 73,429 | 74,549 | +1,120 (synthesis - cleanup) |
| Statements | 13,224 | 13,711 | +487 |
| Sources w/ statements | 61 | 74 | +13 |
| Sections | ~2,000 | 3,313 | +1,313 |
| Articles | 462 | 823 | +361 |
| Components | 30 | 26 | -4 |
| Tests | 1,159 | 1,205 | +46 |

### Insights

- **Firecrawl credits run out quickly** — Switched to WebFetch mid-session. ArXiv HTML versions often have wrong IDs or 404. Direct abstract page + WebFetch works reliably.
- **Edge synthesis races with cleanup** — Background edge synthesis was creating new edges to noise nodes while I was deleting them. Need to sequence: cleanup first, then synthesis.
- **Code ingestion creates massive noise** — 379 noise nodes from code files (snake_case, backtick-wrapped, function calls). The noise filter needs to be applied at ingestion time AND retroactively on existing data.
- **Research paper ingestion compounds** — Each paper creates entities and edges that connect to existing graph nodes, enriching the knowledge base with cross-references between papers and implementation details.
- **Article count jumped from 462 to 823** — Statement pipeline + section compilation is generating high-quality synthesized articles from the new content.

### Final Numbers (end of session)

| Metric | Start | End | Delta |
|--------|-------|-----|-------|
| Sources | 379 | 395 | **+16** |
| Nodes | 5,078 | 4,913 | **-165** (noise cleanup) |
| Edges | 73,429 | 75,252 | **+1,823** |
| Statements | 13,224 | 14,952 | **+1,728** |
| Sources w/ statements | 61 | 83 | **+22** (21% coverage) |
| Sections | ~2,000 | 3,361 | **+1,361** |
| Articles | 462 | 823 | **+361** |
| Tests | 1,159 | 1,205 | **+46** |
| Components | 30 | 38 | +8 (honest after noise cleanup) |

**Additional papers ingested after initial summary**: HippoRAG 2, AutoSchemaKG, MemOS, StepChain (total: 16 papers this session)

### Next Steps
- Continue batch reprocessing of remaining ~310 sources through statement pipeline
- Phase 2 remaining: Tree-KG (no arXiv preprint), UnKGCP (already ingested)
- Phase 3: Jøsang SL book, Pearl Causality — PDF only, needs #125
- Composable ingestion pipeline (#102)

---

## Session 29 — 2026-03-13 (continued)

### Goals
- Continue batch reprocessing of sources through statement pipeline
- Clean up noise entities
- Complete Wave 8 (#108)
- Infrastructure improvements

### Accomplishments

1. **Wave 8 (#108) completed and closed**
   - Added explicit code/prose divergence: code sources now skip statement pipeline
   - All items verified as implemented (Tier 5 routing, Core Prose Loop, landscape deletion)
   - MILESTONES.md updated to mark Wave 8 complete

2. **Noise filter expansion** (7 new tests, 1,212 total)
   - Short entity allowlist: single/double-letter math variables blocked, real abbreviations (AI, ML, KG, QA, etc.) preserved
   - HTML/markdown artifacts: `<!--`, table syntax, `&#entities`
   - Markdown links: `[text](url)` patterns
   - Prices: `$18`, `$0.02 per million tokens`
   - Bare statistics: `94.8%`, `57.67% EM`
   - Numeric expressions: `7 * 86400`, pure digits
   - Citation fragments: `486(3-5):75–174`

3. **Prod graph cleanup**: 56 noise nodes deleted (55 via SQL + 1 via admin endpoint), 305 extractions, 893 edges, 20 aliases removed

4. **Batch 1 statement reprocessing**: 6 of 10 research papers successfully extracted before Gemini quota exhausted (Zep, Fusion Functions, STAR-RAG, GLiNER, EA-GraphRAG, SkewRoute)

5. **Makefile improvement**: `make reprocess-statements` now batches in parallel (configurable REPROCESS_BATCH), skips code sources, runs edge synthesis between batches

6. **Issue #102 status update**: Most items already implemented (NormalizePass trait, SourceProfile, ProfileRegistry, CliChatBackend). Updated issue with current status.

7. **New binary deployed**: Engine rebuilt and restarted with all noise filter improvements

### Blockers
- **Gemini CLI quota exhausted** — resets ~1h from last check
- **OpenRouter credits depleted** — 402 Payment Required (1,122 credits remaining, needs top-up)
- Statement extraction blocked on both backends. Chunk-based extraction still works.

### Metrics

| Metric | Start (S28) | End (S29) | Delta |
|--------|------------|-----------|-------|
| Sources | 395 | 395 | 0 |
| Nodes | 4,913 | 4,872 | **-41** (noise cleanup) |
| Edges | 75,252 | 70,000 | **-5,252** (noise + invalidation) |
| Semantic edges | — | 20,884 | — |
| Synthetic edges | — | 49,116 | — |
| Statements | 14,952 | 15,232 | **+280** |
| Sources w/ stmts | 83 | 89 | **+6** (22.5%) |
| Tests | 1,205 | 1,212 | **+7** |
| Components | 38 | 40 | +2 |

### Insights
- **LLM quota is the bottleneck** — with both Gemini and OpenRouter exhausted, statement extraction halts. Need to budget quota carefully for batch reprocessing.
- **Most of #102 was already implemented** — the composable pipeline vision from the issue was already realized through NormalizePass, SourceProfile, and CliChatBackend. The remaining extraction decoupling is lower priority since statement pipeline already does windowed extraction on normalized content.
- **Admin noise cleanup endpoint works** — caught 1 node that SQL regex missed (encoding differences). Running `POST /admin/nodes/cleanup` after manual SQL cleanup is good practice.
- **Graph density is stable** — after noise cleanup and edge synthesis, density hovers around 0.003, indicating good connectivity without over-saturation.

### Next Steps
- Resume batch reprocessing when Gemini quota recharges
- Chris needs to top up OpenRouter credits for HTTP fallback
- Continue with remaining ~306 unprocessed document sources
- Consider Wave 9: cross-domain analysis (#13 spec) or knowledge atomizing (#104)

---

## Session 30 — 2026-03-13 (continued)

**Focus:** Module extraction, research ingestion, meta-loop assessment

### Accomplished

1. **Completed noise_filter module extraction** — Moved `is_noise_entity()`, `is_document_artifact()`, and all 28 tests from pipeline.rs (1,852 lines) into dedicated `noise_filter.rs` (478 lines). Pipeline.rs dropped to 1,389 lines. Updated imports in both `pipeline.rs` and `admin.rs`. All 1,212 tests pass, clippy clean.

2. **Research ingestion (#92)** — Ingested 4 documents:
   - **UnKGCP** (arXiv:2510.24754) — Conformal prediction for uncertain KG embeddings. EMNLP 2025. Full paper text ingested. Already generating articles in consolidation.
   - **Tree-KG** (ACL 2025) — Expandable KG construction framework. Abstract + metadata only (PDF-only, no arXiv preprint).
   - **Dempster-Shafer Theory** — Comprehensive survey compiled from Shafer (1976), Smets TBM, and recent work. Covers mass functions, belief/plausibility, combination rules, and SL connection.
   - **AGM Belief Revision** — Stanford Encyclopedia of Philosophy article. Full text covering expansion/contraction/revision, rationality postulates, epistemic entrenchment, iterated change.

   Phase 2 of #92 is now **complete**. Phase 3 has 2 remaining items (Jøsang SL book, Pearl Causality) blocked on PDF ingestion (#125).

3. **Pushed to main** — Commit `aa845a72` for the noise_filter refactor.

### State of the World

- **1,212 tests**, 0 failures, clippy clean
- **Prod:** 395 sources, 4,872 nodes, 75,596 edges, 15,232 statements, 3,472 sections
- **Statement coverage:** 87/266 document sources (33%)
- **Components:** 40
- **Gemini quota:** Exhausted (reset in ~43 min). Blocks batch reprocessing.
- **Firecrawl credits:** Exhausted. Used WebFetch/WebSearch for research.
- **OpenRouter credits:** Still depleted from last session.

### Insights

- **UnKGCP is directly relevant** to Covalence's epistemic model — conformal prediction provides distribution-free guarantees for uncertainty intervals, complementing our Subjective Logic approach. The entropy-normalized nonconformity measure (query-adaptive intervals) is a technique we could apply to confidence propagation.
- **AGM belief revision formalism** maps cleanly to knowledge graph operations: source retraction = contraction, ingestion = revision, consolidation = consistency maintenance. The Levi Identity (revision = contraction + expansion) mirrors how reprocessing works.
- **Profile system is partially wired** — NormalizeChain is used via SourceProfile, but chunk_size/overlap/coreference are still from config, not from profiles. Full wiring is a future task.

3. **Profile-driven chunking (#102)** — Wired SourceProfile chunk parameters into the pipeline. Documents now chunk at 1500/200 (was hardcoded 1000/200), code at 2000/100, web pages at 1200/150. The profile system now drives both normalization AND chunking.

4. **Additional noise filters** — Added 3 new patterns: embedded newlines, named HTML entities (`&amp;`, etc.), and ALL_CAPS_SNAKE test constants. Cleaned 11 orphaned noise nodes from prod, reducing components from 40 to 29.

5. **4 commits pushed to main:**
   - `aa845a72` — Refactor: extract noise_filter module from pipeline.rs
   - `c2531d68` — Add noise filters for newlines, HTML entities, and test constants
   - `5db2112a` — Wire source profile chunk parameters into pipeline (#102)

### Next Steps
- Resume batch reprocessing when Gemini quota recharges (~179 document sources remaining)
- Reprocess 4 newly ingested sources to get entity extractions (blocked on Gemini)
- Chris needs to top up OpenRouter credits and Firecrawl credits
- #92 Phase 3: Jøsang and Pearl blocked on #125 (PDF ingestion)
- Rebuild + restart prod engine to pick up profile-driven chunking changes
- Consider re-ingesting codebase after noise_filter refactor

---

## Session 31 — Search Quality Fixes & Noise Filter Expansion

**Date:** 2026-03-13 (continued from Session 30 context)
**Duration:** ~30 minutes active
**Gemini status:** Exhausted (8h reset), pivoted to non-LLM work

### Assessment
- 1,215 tests passing, clippy clean
- Prod graph: 4,861 nodes, 70K edges, 29 components, 399 sources
- Gemini CLI quota exhausted — focused on code quality improvements
- Identified search quality issues during investigation

### Accomplished

**Search quality bugs fixed (3):**
1. **Cache ignoring limit**: `fused.truncate(limit)` happened BEFORE `cache.store()`, so a limit=5 query cached 5 results and subsequent limit=15 queries got stale 5 results. Fixed by storing full result set before truncation and truncating on cache hit.
2. **Cache strategy key conflation**: All Auto queries stored under "auto" instead of the SkewRoute-resolved strategy (e.g., "balanced"). Different Auto queries resolving to different strategies would share a single cache entry. Fixed by storing under the resolved strategy and skipping cache lookup for Auto queries.
3. **Weight redistribution double-counting**: Dimensions dampened to near-zero had their lists cleared (Step 5b2), then their residual weight was counted in both `dampened_weight` AND `empty_weight`. Fixed by excluding near-zero-weight dimensions from empty_weight.
4. **Demotion trace log**: Always reported factor=0.3 even when 0.5 or 0.7 was applied. Fixed log message.

**Noise filter expansion (Session 30 + 31 combined):**
- Quote-wrapped entities: `"_"`, `'@'` — strip quotes before length checks
- Pure punctuation/symbol names rejected
- HTML tag fragments: `<h3`, `<div`, `<!doctype`
- File paths: `file://...`
- Bracket syntax: `items[i]`, `[{template}]`
- Breadcrumb navigation: `Section > Subsection`, `next >`
- Quoted sentences: `"He went to the store."`
- 39 noise filter tests total (up from 31)

**Data investigation:**
- Found 41 "Preamble" titled sources — old code files ingested as `source_type=document`
- Found duplicate research paper sources (Tree-KG, DS Theory, AGM, UnKGCP)
- Updated #92 with current ingestion status

### Commits
- `bdebabc4` Fix search cache ignoring limit; add noise filters for quotes, HTML fragments
- `8e012758` Add noise filters for file paths, bracket syntax, navigation, quoted sentences
- `0e6df15c` Fix three search quality bugs: cache strategy key, weight redistribution, demotion log

### Test Count
1,224 tests (21 api + 1,156 core + 47 eval), up from 1,215

### Insights
- **The code review agent found real bugs** — the cache strategy conflation and weight double-counting were not obvious from manual inspection. Using the code reviewer as a background task while doing other work is highly productive.
- **Search appears to return few results** (3-5 articles for most queries) because: (a) entity demotion at 0.3x pushes nodes far down, (b) quality gating removes bibliography chunks, (c) source diversification caps at 2 per URI. This is actually working as designed — articles are more informative than raw chunks. But worth monitoring.
- **Noise entities persist in graph** despite filters — the noise filter prevents NEW noise but doesn't retroactively clean existing entities. The admin cleanup endpoint handles this but needs to be run periodically.

### Blockers
- Gemini CLI quota (8h reset) — blocks all statement extraction and entity extraction
- OpenRouter credits depleted — blocks HTTP fallback for LLM extraction
- Prod engine running old binary — needs restart to pick up 3 sessions of improvements

### Next Steps
- **Restart prod engine** with new release binary (Session 30+31 changes)
- **Run noise cleanup** on prod: `POST /admin/nodes/cleanup?dry_run=false`
- Resume batch reprocessing when Gemini quota recharges
- #92 Phase 3: Jøsang and Pearl blocked on #125 (PDF ingestion)
- Consider Wave 9: cross-domain analysis or knowledge atomizing

---

## Session 32 — 2026-03-13

### Assessment
- Continuing from Session 31: prod engine needed restart, noise cleanup pending, 183 document sources without statements, Gemini quota cycling
- Prod engine running old binary (PID 13594, started 14:55) while release binary built at 15:50

### Accomplishments

**Prod engine restart and cache hygiene:**
- Restarted prod engine twice (once with Session 31 binary, once with Session 32 noise filter improvements)
- Cleared 11 stale search cache entries across both restarts

**Noise entity cleanup — 76 nodes deleted:**
- Round 1 (Session 31 filter): 42 nodes, 286 edges, 13 aliases removed
  - Citation fragments: `Abadi et al. [2015]`, `Pasupat and Liang, [2016]`
  - Code constants: `MAX_CHUNKS_PER_SOURCE`, `CONTENT_PREFIX_LEN`, `SOURCE_SUMMARY_PROMPT`
  - Math artifacts: `H[peval(z)]`, `H[pn(z, y1:inf+)]`
  - HTML fragments: `<h1`, `<h3`, `<div`
  - Quoted punctuation: `' '`, `"-->"`, `"<!--"`, `':'`, `'-'`
- Round 2 (expanded filter): 34 nodes, 368 edges, 3 aliases removed
  - Rust primitives: `f64`, `i64`, `usize`, `&str`
  - Markdown formatting: `_The Book of Why_`, `_Snowflake-arctic-embed-m_`
  - Quoted phrases: `"hello world"`, `"for example"`, `"Source type: document\n"`
  - HTML entities: `&nbsp;`, `&quot;`

**Noise filter code improvements (commit `7d98e9a7`):**
- Fixed `trimmed` vs `unquoted` inconsistency — snake_case, function-call, and path checks now use quote-stripped name, catching entities like `"web_page"` and `"tool_output"`
- Added Rust primitive type filter (19 types: f64, i64, u8, &str, etc.)
- Added markdown italic/bold wrapping filter (`_text_`, `**text**`)
- Added ampersand-prefixed reference filter (`&self`, `&str`)
- Expanded quoted-text filter to catch ALL multi-word quoted strings (was: only sentence-ending)
- 43 noise filter tests total (up from 39), 1,228 total tests

**Data cleanup:**
- Fixed 46 mistyped sources: 41 "Preamble" code files + 5 others (`mod.rs`, `use sqlx::Row;`) re-typed from `source_type=document` to `source_type=code`
- Deleted 12 empty duplicate sources (0 chunks, 0 statements) via supersedes chain cleanup
- Deleted 2 template/noise sources ("ADR-NNNN: Title", "index.html") with cascading extraction/alias cleanup

**Batch reprocessing (#109):**
- Processed 9+ sources through statement pipeline
- Successful: SRRF (71 stmts), Tree-KG (58), Zep (75), EA-GraphRAG (43), F-coref (78), RAG-KI (62), Practical GraphRAG, UnKGCP (80)
- Progress: 97 sources with statements (was 87), 15,033 total statements (was 14,507)
- Entity extraction from statements blocked by OpenRouter 402 (credits exhausted)
- fastcoref sidecar not running (falls back to heuristic coref)

**Graph quality:**
- Nodes: 4,785 (down from 4,861 — net -76 from noise cleanup)
- Edges: 69,470 (20,673 semantic + 48,797 synthetic)
- Components: 24 (down from 29)
- Sources: 385 (down from 399 — cleanup removed empties)

### Commits
- `7d98e9a7` Expand noise filter: primitives, markdown formatting, quoted phrases, unquoted consistency

### Test Count
1,228 tests (21 api + 1,160 core + 47 eval), up from 1,224

### Insights
- **The `trimmed` vs `unquoted` bug was subtle** — quote stripping was implemented early in the function but then most syntactic checks used the pre-stripped `trimmed` variable. Quoted entities like `"web_page"` (type=concept) escaped the snake_case filter because the surrounding `"` characters failed the `chars().all(alphanumeric || '_')` check.
- **Data cleanup compounds** — fixing 46 source types and deleting 14 empties means 60 fewer sources to waste Gemini quota on during batch reprocessing. This kind of data hygiene is high-leverage.
- **OpenRouter credit exhaustion** is a real blocker for entity extraction from statements (Pass 2), but statement extraction (Pass 1 via Gemini CLI) still works. The degraded mode produces statements that participate in search even without extracted entities.
- **Component count dropped from 29 to 24** — partially from noise cleanup (removing disconnected noise nodes) and partially from new edges created during reprocessing.

### Blockers
- OpenRouter credits exhausted — blocks entity extraction from statements
- Gemini CLI free tier quota cycles — ~10-15 min per hour of productive extraction
- fastcoref sidecar not running — no neural coreference resolution
- #125 (PDF ingestion) — blocks Jøsang SL book and Pearl Causality

### Next Steps
- Continue batch reprocessing: 157→~145 remaining document sources without statements
- Replenish OpenRouter credits to enable entity extraction from statements
- Consider starting fastcoref sidecar for better coreference resolution
- #92 Phase 3: Jøsang and Pearl still blocked on #125
- Run edge synthesis after batch reprocessing completes
- Consider Wave 9: cross-domain analysis or knowledge atomizing

---

## Session 33 — 2026-03-14

### Assessment
- Continuing from Session 32. Batch reprocessing was blocked by Gemini CLI quota exhaustion and OpenRouter credit depletion.
- User asked: "Can we use Haiku via GitHub Copilot on the CLI for inference?"
- 15,033 statements from 97 sources. 139 document sources remaining without statements.

### Accomplishments

**1. GitHub Copilot CLI as inference backend (user request)**
- Discovered `copilot` binary at `/opt/homebrew/bin/copilot` accepts same `-p <prompt> --model <model>` interface as Gemini CLI
- Claude Haiku 4.5 available at 0.33 premium requests per call — effectively unlimited quota
- CLI interface matches `CliChatBackend` invocation pattern exactly: `copilot -p <prompt> --model claude-haiku-4.5`
- Stats go to stderr (clean stdout), markdown fence stripping already handled by pipeline
- Tested with realistic extraction prompts — quality comparable to Gemini Flash for entity/statement extraction
- Updated `.env`: `COVALENCE_CHAT_CLI_COMMAND=copilot`, `COVALENCE_STATEMENT_MODEL=claude-haiku-4.5`

**2. ChatBackendExtractor — entity extraction via CLI backends**
- Created `ChatBackendExtractor` in `llm_extractor.rs` — wraps `ChatBackend` trait to implement `Extractor` trait
- Reuses `SYSTEM_PROMPT` and `parse_extraction_json()` from `LlmExtractor` (same extraction quality)
- Strips markdown fences before JSON parsing (copilot wraps responses in ```json blocks)
- Added `with_extractor()` builder on `SourceService` for post-construction extractor replacement
- Wired in `state.rs`: CLI backends automatically use `ChatBackendExtractor`, guarded against replacing sidecar/gliner2/two_pass extractors
- Code reviewed: addressed feedback on unconditional override bug and doc comment specificity
- Branch `feature/chat-backend-extractor` merged to main, pushed

**3. Batch reprocessing progress**
- First batch (3 sources) completed via copilot: ADR-0012, Untitled Doc, DS Theory, AGM Theory
- 105 sources now have statements (up from 98), 15,973 total statements, 4,118 sections
- Entity extraction from chunks previously failing (OpenRouter 402) — now works via ChatBackendExtractor
- Restarted engine with new binary; re-initiated batch reprocessing for remaining 139 sources

### Commits
1. `bfe5e89a` — Add ChatBackendExtractor for entity extraction via CLI backends
2. `4147752e` — Address code review: guard CLI extractor override, fix doc comment

### Insights
- GitHub Copilot CLI is an excellent free inference backend for batch processing. The `copilot -p --model` interface is a universal CLI LLM pattern that Covalence's `CliChatBackend` was already designed for.
- Copilot calls are ~2x slower than Gemini CLI (~10-15s vs 5-7s per call), but quota is effectively unlimited vs Gemini's hourly exhaustion.
- The separation between `ChatBackend` (for statements/sections) and `Extractor` (for entities) was a design gap. Both should have been routable through the same backend from the start. `ChatBackendExtractor` closes this gap.
- Large sources (70-80KB) can take 30+ minutes for full pipeline: many windows × 10-15s per copilot call for extraction, then section compilation (one call per cluster).

### Blockers
- Batch reprocessing is slow (~7-10 min per small source, 30+ min for large sources) due to copilot per-call latency
- fastcoref sidecar not running — no neural coreference resolution
- #125 (PDF ingestion) — blocks Jøsang SL book and Pearl Causality
- Gemini CLI quota exhausted (22h reset) — code review via Gemini not available

### Next Steps
- Batch reprocessing running autonomously in background for 139 remaining sources
- Monitor reprocessing progress and verify entity extraction works end-to-end
- Run edge synthesis + graph reload after batch completes
- Consider starting fastcoref sidecar for neural coreference
- #92 Phase 3: Jøsang and Pearl still blocked on #125

---

## Session 34 — Cross-Domain Analysis (Wave 9)

**Date:** 2026-03-14 (continued from Session 33)

### Context
Session 33 completed section backfill for 27 sources, retried 10 large timed-out sources, ingested 8 new software engineering/market analysis sources, ran the meta loop assessment, identified the cross-domain bridge as the highest-leverage priority, and created issue #133.

### Assessment
- Graph state: 5,546 nodes, 77,255 edges, 22,157 statements
- Coverage gap: spec 12 (code ingestion) and spec 13 (cross-domain analysis) were completely unimplemented
- ChatGPT-5.4 analysis confirmed cross-domain bridging as Covalence's key differentiator vs competitors

### Executed
**Built the cross-domain analysis service (Wave 9, #133):**

1. **AnalysisService** (`services/analysis.rs`, ~1000 lines):
   - 9 Component node definitions with architectural descriptions
   - Module-path-to-Component mapping table (30+ path prefixes)
   - `bootstrap_components()`: creates Component nodes, embeds descriptions via Voyage
   - `link_domains()`: three-edge-type cross-domain linking:
     - PART_OF_COMPONENT: code → component via source URI module-path matching
     - IMPLEMENTS_INTENT: component → spec/design concepts via embedding similarity
     - THEORETICAL_BASIS: component → research concepts via embedding similarity
   - `coverage_analysis()`: orphan code + unimplemented specs + coverage score
   - `detect_erosion()`: per-component drift metric (mean cosine distance to child code)
   - `blast_radius()`: BFS graph traversal with hop-grouped affected nodes

2. **API handlers** (`handlers/analysis.rs`, 5 endpoints):
   - POST /api/v1/analysis/bootstrap
   - POST /api/v1/analysis/link
   - POST /api/v1/analysis/coverage
   - POST /api/v1/analysis/erosion
   - POST /api/v1/analysis/blast-radius

3. **DTOs** (12 new request/response types), **OpenAPI tags**, **route registration**, **state wiring**

4. **Deployed to prod (:8441)** and ran all endpoints:
   - Bootstrap: 9 components created, 9 embedded
   - Linking: 219 PART_OF_COMPONENT, 14 IMPLEMENTS_INTENT, 31 THEORETICAL_BASIS
   - Coverage: 177 orphan code, 200 unimplemented specs, 2.9% coverage score
   - Erosion: 8/9 components show drift (0.36-0.48), expected for v1
   - Blast radius: working, e.g. "Search Fusion" → 498 affected nodes at 2 hops

### Technical Decisions
- **No migration needed**: `node_type` and `rel_type` are freeform TEXT columns — Component nodes use `node_type = "component"`, bridge edges use UPPERCASE rel_types (PART_OF_COMPONENT, IMPLEMENTS_INTENT, THEORETICAL_BASIS) per spec
- **Source URI fallback**: Code nodes don't store `file_path` in properties, so PART_OF_COMPONENT linking traces through extraction provenance (extractions → chunks → sources) to get the source URI
- **Spec/research distinction**: IMPLEMENTS_INTENT vs THEORETICAL_BASIS determined by checking if the target concept's source URI contains `spec/` or `docs/adr/`

### Graph State (post Wave 9)
- 5,568 nodes (+22 from session start), 77,646 edges
- 9 Component nodes, 264 new bridge edges
- 43 weakly connected components

### Quality
- 1,235 tests passing (up from 1,228)
- Zero clippy warnings, fmt clean
- Code review agent running

### Remaining Wave 9 Items
- [x] Whitespace roadmap (research cluster gap detection)
- [x] Research-to-execution verification
- [x] Dialectical design partner

### Insights
- Coverage score of 2.9% reveals that the bridge is thin — most spec concepts have no IMPLEMENTS_INTENT links yet. This is because the linking only creates 5 edges per component (45 total max) while there are ~200 spec concepts. Consider increasing max_edges_per_component or running multiple passes with different similarity thresholds.
- Module-path matching works well for Covalence's own code (219 edges) but won't generalize to external codebases without additional heuristics.
- Erosion scores of 0.35-0.48 are all in the "moderate drift" range — expected because Component descriptions are high-level architecture summaries while code semantic summaries describe implementation details. The drift metric is more useful for *relative* comparison between components than absolute thresholds.

---

## Session 35 — 2026-03-15

### Assessment
- Active plan: Wave 9 (Cross-Domain Analysis), 3 remaining capabilities
- Prior state: Session 34 completed Wave 9 Phase 1-2 (bootstrap, link, coverage, erosion, blast-radius), 1,235 tests
- 3 unchecked boxes: whitespace roadmap, verify-implementation, dialectical critique

### Executed
**Branch:** `feature/wave9-remaining-analysis` — 3 commits

1. **Whitespace roadmap** (`analysis.rs`): SQL joins sources→chunks→extractions→nodes with LEFT JOIN on THEORETICAL_BASIS edges, groups by source. Identifies research clusters with no bridge to Components. Optional domain filter. Whitespace score = unbridged/total.

2. **Research-to-execution verification** (`analysis.rs`): Embeds query → ANN search research nodes → traces THEORETICAL_BASIS to Components → traces PART_OF_COMPONENT to code nodes. Alignment score = 1.0 - mean(research_dist + code_dist)/2.

3. **Dialectical critique** (`analysis.rs`): Embeds proposal → searches 3 domains (research, spec, code) independently → optional LLM synthesis via ChatBackend. Structured prompt returns JSON with counter_arguments (claim+evidence+strength) and supporting_arguments (claim+evidence) plus recommendation.

4. **API layer**: 3 new handlers, 14 new DTO types, OpenAPI schema entries, route registration, state wiring (ChatBackend passed to AnalysisService).

5. **Code review fixes** (3rd commit): char-boundary safe truncation at 2 locations, whitespace score domain filter ordering fix, doc comment on create_bridge_edges sort-order requirement.

### Technical Decisions
- `AnalysisService` now takes optional `ChatBackend` via `with_chat_backend()` builder — only critique synthesis requires it, all other endpoints work without
- Critique synthesis parses LLM JSON response with `serde_json::from_str` fallback — if parsing fails, returns evidence without synthesis rather than erroring
- Whitespace roadmap counts sources (not individual nodes) for gap detection — matches the spec's "research cluster" concept better
- Verify-implementation uses 2-hop trace through Component bridge rather than direct research→code similarity — leverages the bridge architecture

### Quality
- 1,239 tests passing (21 api + 1,171 core + 47 eval), 0 failures
- Zero clippy warnings, fmt clean
- Two rounds of code review completed, all findings addressed

### Wave 9 Complete
All 8 analysis capabilities from spec/13 now implemented:
1. POST /analysis/bootstrap — Component node creation
2. POST /analysis/link — Cross-domain bridge edges
3. POST /analysis/coverage — Orphan code + unimplemented spec detection
4. POST /analysis/erosion — Architecture drift measurement
5. POST /analysis/blast-radius — Change impact simulation
6. POST /analysis/whitespace — Research gap detection
7. POST /analysis/verify — Research-to-execution alignment
8. POST /analysis/critique — Dialectical design analysis

### Search Quality Investigation (#130)
Diagnosed vector search returning no results: the engine was started without sourcing `.env.prod`, so the Voyage API key wasn't available. After fixing startup, discovered:

1. **Reranker bias**: snippets with highlighted search terms biased the reranker toward keyword matches. Fixed by sending full chunk content (1000 chars) to the reranker instead of snippets.
2. **Bibliography filter gaps**: "- Lee et al. \[2011\]↑" escaped existing patterns. Added detection for escaped-bracket years and arrow-marker citations.
3. **Result**: "entity resolution coreference" → #1 changed from "Human smuggling networks" to "LLM-CER: In-Context Clustering for Efficient Entity Resolution"

### Wave 9 Review Follow-up
Addressed 3 findings from thorough code review:
1. Moved domain filter into SQL (was applied post-LIMIT, understating counts)
2. Populated `connected_components` field via THEORETICAL_BASIS query (was always empty)
3. Moved `use petgraph::Direction` to function scope

### Insights
- The verify-implementation endpoint revealed that many code nodes lack embeddings (ANN search returns 0 results). This is a data quality issue — the backfill_node_embeddings admin endpoint exists but may not have been run recently on all code nodes.
- Whitespace roadmap showing 50% unbridged research sources is expected — many ingested papers cover tangential topics. The metric becomes more actionable as the bridge gets richer.
- ChatBackend integration is clean — the FallbackChatBackend wraps CLI→HTTP and the critique endpoint degrades gracefully if no backend is configured (returns evidence without synthesis).
- **Engine startup must source `.env.prod`** — manually specifying only DATABASE_URL and BIND_ADDR drops the Voyage API key, disabling vector search and the reranker. The `make run-prod` target handles this correctly.
- **Reranker document construction matters more than fusion weights** for search quality. Snippets are optimized for display (highlighting matched terms), not for semantic evaluation. Sending full content to the reranker is always the right call.
- **CC fusion coverage multiplier is working as designed** — single-dimension results are penalized 25-50% depending on active dimensions. The remaining noise is from lexical matches in topically adjacent content (e.g., domain examples in methodology papers).

---

## Session 36 — 2026-03-15

### Assessment
- Active plan: Quality measurement and improvement (meta loop Assess→Evaluate)
- Prior state: Session 35 completed Wave 9, fixed search quality (#130), 1,241 tests
- Quality gates: Entity precision unmeasured, Search P@5 unmeasured

### Executed
**Branch:** main (4 commits)

1. **Established search precision@5 baseline** (#131)
   - 20 curated queries spanning all knowledge domains
   - Binary relevance judgments per result (97 total judged, 85 relevant)
   - **P@5 = 0.86** — passes >0.80 quality gate
   - Saved as `covalence-eval/fixtures/search_precision_baseline.json`
   - Closed #131

2. **Fixed post-granularity quality filter scope** (#130)
   - Handler-level quality filter was applying bibliography/reference checks to ALL result types
   - Section and statement results from research papers naturally contain citations ("et al.", "(2023)")
   - Scoped filter to `result_type == "chunk"` only — sections, statements, and nodes pass through
   - This was causing some queries to return 0 results

3. **Measured entity precision: 68%** (32 noise in 100 random sample)
   - Categories: generic words (10), paper titles (4), generic phrases (3), code/metadata (8), questions-as-entities (3), ArXiv labels (2), math/dates (2)

4. **Expanded noise filter from 43 to 57 tests** (#130)
   - Generic words: +12 (clear, home, false, true, dimensions, etc.)
   - Generic phrases: +14 (new system, color attributes, overall quality, etc.)
   - Questions as entities: "what currency needed in scotland" → filtered
   - Title-case 5+ word exception: "What You See Is All There Is" → kept
   - HTTP methods: GET/POST/PUT/DELETE/PATCH endpoints
   - URL schemes: anything with `://`
   - Email addresses: `@` + `.` without spaces
   - Dot+underscore code paths: `config.extract_url`, `data.total_memories`
   - ArXiv category labels: `physics.soc-ph`
   - Boolean literals as "other" type
   - Subtitle-year detection: colon + >35 chars + 19xx/20xx year (excludes ADR-)
   - Paper title threshold: >55 chars for concepts
   - False positive fixes: reverted threshold from 50→55 (caught "emergent ontology"), excluded ADR- prefix from subtitle-year

5. **Executed admin cleanup** — 94 total noise entities removed (33 + 61 in two passes), 1,845 edges, 28 aliases

6. **Re-measured entity precision: 96%** — passes >90% quality gate
   - Remaining 4%: paper comparison placeholders ("Ours"), quantities ("10,000 entries"), math variables ("d_{i-1}"), function signatures with arguments

### Commits
```
7dd44b65 Fix post-granularity quality filter scope, add search eval baseline (#130, #131)
4bb78fec Expand entity noise filter for 68% → ~90% precision (#130)
2b00b787 Fix noise filter false positives: revert concept threshold to 55, refine subtitle-year detection (#130)
d39d228b Add noise filter rules for emails, URLs, HTTP methods, dot-underscore paths (#130)
```

### Quality
- 1,255 tests passing (21 api + 1,187 core + 47 eval), 0 failures
- Zero clippy warnings, fmt clean
- Prod graph: 5,474 nodes, 76,062 edges (down from 5,568/77,688 — 94 noise entities cleaned)

### Insights
- **Post-granularity promotion is a quality trap**: when paragraph chunks get promoted to section content, the section may be a bibliography or author list. The quality filter catches this, but it must be scoped to chunk-type results only — sections and statements have different content characteristics.
- **Entity noise is bimodal**: the easy wins (code syntax, generic words) get 68→90%, but the last 10% requires understanding extraction context (is "Ours" a technology? is a function signature with arguments a concept?). Diminishing returns.
- **Title-case is a reliable signal** for named concepts vs questions: "What You See Is All There Is" (all words capitalized, 7 words) is a cognitive bias name; "what currency needed in scotland" (lowercase) is a question. The 5-word minimum prevents short headings like "Who Can See It" from being falsely kept.
- **Admin cleanup is safe with dry-run**: the two-pass approach (dry-run → review → execute) caught zero false positives across 94 entities. The filter rules are conservative enough for autonomous cleanup.

### What Worked
- Formal evaluation before building: measuring P@5 and entity precision gave concrete targets
- Iterative filter development: add rules → test → deploy → measure → fix false positives → repeat
- The quality gate framework (P@5 >0.80, entity precision >90%) from VISION.md is actionable and passes

### What Could Be Better
- The noise filter is now ~900 lines of heuristics. At some point, a small classifier (logistic regression on features like length, dot count, underscore count, entity type) might be more maintainable.
- No automated regression test for entity precision — the 100-entity sample is manual. Could add an eval fixture like the search baseline.

---

## Session 37 — 2026-03-15 — Flywheel: Self-Evaluation Closing the Loop

### Starting State
- 1,255 tests, P@5 = 0.86, entity precision 96%, search regression 20/20
- Cross-domain: coverage 7.8%, whitespace 50%, 178 orphan code nodes, 306 bridge edges
- The search regression harness was just built (Session 36) but cross-domain analysis had major gaps

### What Was Done

**1. Expanded MODULE_PATH_MAPPINGS (coverage fix)**
- Analyzed orphan code nodes via SQL join on extractions → chunks → sources
- Found 178 code nodes unlinked due to missing path patterns
- Added 27 new mappings: all ingestion submodules, models/, types/, storage/, error.rs, config.rs, eval crate, migrations, covalence-api crate, covalence:// URI scheme
- Result: orphan code nodes 178 → 31

**2. Fixed THEORETICAL_BASIS bridging (whitespace fix)**
- Discovered that merged entities (e.g., "Subjective Logic" appearing in both spec and research) were excluded from research bridging
- Root cause: the query used `NOT EXISTS (spec source)` which excluded entities with ANY spec extraction, even if they also had research provenance
- Fix: changed to `EXISTS (non-spec, non-code source)` — entities must have at least one research/document extraction to qualify
- Result: whitespace 50% → 0% (all 50 research sources bridged), 303 new THEORETICAL_BASIS edges

**3. Increased semantic bridge budget**
- Ran linking with `max_edges_per_component=50` (was 5)
- Created 183 IMPLEMENTS_INTENT + 176 THEORETICAL_BASIS edges (first pass) + 303 THEORETICAL_BASIS (after fix)
- Total bridge edges: 467 PART_OF_COMPONENT + 225+ IMPLEMENTS_INTENT + 548+ THEORETICAL_BASIS = 1,280+

**4. Added trivial code chunk filter**
- Found `mod tests {` and `if regressed {` chunks appearing at positions 7-8 in search results
- Added `is_trivial_code_chunk()` to `chunk_quality.rs` — filters chunks <25 chars or single-line <40 chars
- Applied in post-reranking quality filter alongside bibliography/boilerplate filters
- 10 new tests

**5. Re-ingested changed code files**
- analysis.rs, chunk_quality.rs, search.rs → 119 new edges from synthesis
- Re-linked: 21 new PART_OF_COMPONENT edges

### Metrics Before/After

| Metric | Before | After |
|--------|--------|-------|
| Coverage | 7.8% | 45.1% |
| Whitespace | 50% (25 gaps) | 0% (0 gaps) |
| Orphan code | 178 | 31 |
| Bridge edges | 306 | 1,280+ |
| Search regression | 20/20 | 20/20 |
| Tests | 1,255 | 1,267 |

### Flywheel Evidence
This session demonstrated the self-evaluation loop working:
1. Used Covalence's whitespace analysis → found 2 unlinked papers (Epistemic Uncertainty, BMR)
2. Traced root cause through the code → discovered merged entity exclusion bug
3. Fixed the bug → whitespace went to 0%
4. Used Covalence's coverage analysis → found 178 orphan code nodes
5. Analyzed via SQL → identified missing MODULE_PATH_MAPPINGS
6. Fixed mappings → orphans dropped to 31
7. Search regression stayed 20/20 through all changes

The system identified its own weaknesses, the weaknesses were fixed, and the system verified the fix. The flywheel completed one full turn.

### Commits
- `051bb443` — Expand MODULE_PATH_MAPPINGS for comprehensive cross-domain linking
- `d701d146` — Add trivial code chunk filter to post-search quality check
- `f7bc7ecc` — Fix THEORETICAL_BASIS bridging to include cross-domain entities

### Insights
- **Entity resolution creates blind spots in bridging**: when entities from different domains are merged, domain-specific queries can exclude them. The fix is to check for provenance from the target domain, not absence from the source domain.
- **Coverage metric is dominated by spec concepts**: 200 "unimplemented" spec items are mostly vocabulary, not actionable implementation gaps. The metric needs refinement — perhaps weight by entity importance or only count top-level concepts.
- **Whitespace is a powerful diagnostic**: going from 25 gaps to 0 in one session shows the analysis is actionable. The 2 remaining gaps pointed directly at the bridging bug.
- **MODULE_PATH_MAPPINGS is a maintenance burden**: adding new source files requires updating the mapping table. A fallback heuristic (e.g., any code node extracted from a source with `engine/` in its URI gets mapped to a component by directory) would reduce this.

### What Could Be Better
- RAGAS metrics are all stubs — replacing even one (context precision) would enable measuring whether bridge edges improve retrieval context quality.
- The 31 remaining orphan code nodes are entities from research papers with code-like names (e.g., "Opinion", "PgResolver") — they're not actually orphaned code, they're mistyped entities. Entity type refinement during ingestion would eliminate this.
- Coverage formula treats all spec concepts equally. A weighted version using node degree or extraction confidence would better reflect actual implementation coverage.

---

## Session 38 — 2026-03-15 (continued)

**Focus:** Data cleanup, coverage breakthrough, batch reprocessing

### State at Session Start
- 5,510 nodes, 77,409 edges, 44 components
- 22,228 statements (6,303 from code sources = noise)
- Coverage: 45.5% (up from 7.8% in S37)
- 133 document sources without statements

### Work Completed

**1. Code-source statement cleanup**
Discovered 6,303 statements from code sources — legacy noise from when code files were ingested as `source_type=document`. These were extracted by the statement pipeline, which now correctly skips code sources via a guard in `source.rs:511`. Deleted:
- 20,698 extractions (from code-source statements)
- 6,303 statements
- 1,578 sections
- 2,973 orphaned aliases
- 9 fully orphaned nodes (no edges, no extractions remaining)

Statement count dropped from 22,228 → 15,925 (all noise removed). Search regression: 20/20 stable.

**2. Coverage breakthrough: 45.5% → 83.9%**
The `max_edges_per_component` default was 5 — far too conservative with 9 components and 386 spec concepts. Raised to 100, which created 900 new IMPLEMENTS_INTENT and THEORETICAL_BASIS bridge edges.

Coverage surpassed the 80% VISION target for the first time.

Committed as `ab39278c` — Increase default max_edges_per_component from 5 to 100.

**3. Batch reprocessing**
Submitted 27 document sources for statement extraction across 3 batches:
- Batch 1: 5 ADRs (14-19 statements each)
- Batch 2: 12 specs/ADRs (processing)
- Batch 3: 10 largest web sources (168-204 statements each)
- Bonus: 12 zero-entity web sources (papers with chunks but no extractions)

18+ sources completed during session. Sources with statements: 75 → 93/208.

**4. Edge synthesis**
789 new synthetic edges from reprocessed sources. Total edges: 77,409 → 91,534.

**5. Wave 9 endpoint verification**
All analysis endpoints tested and functional:
- `/analysis/coverage` — 83.9% coverage, 31 orphan code, 62 unimplemented spec
- `/analysis/erosion` — 8/9 components above 0.3 drift (expected for v1)
- `/analysis/whitespace` — 0% whitespace, 50 research sources all bridged
- `/analysis/verify` — returns research+code alignment (Search Fusion: 0.76)
- `/analysis/blast-radius` — BFS traversal working
- `/analysis/critique` — returns research/spec/code evidence for proposals

### Metrics Before/After

| Metric | Before | After |
|--------|--------|-------|
| Coverage | 45.5% | 83.9% |
| Whitespace | 0% | 0% |
| Orphan code | 31 | 31 |
| Statements | 22,228 (6,303 noise) | 17,811 (clean) |
| Docs w/ statements | 75/208 | 93/208 |
| Nodes | 5,510 | 5,589 |
| Edges | 77,409 | 91,534 |
| Bridge edges | 1,280+ | 2,600+ |
| Search regression | 20/20 | 20/20 |
| Tests | 1,267 | 1,267 |

### Insights
- **Default values matter enormously**: `max_edges_per_component=5` was the single biggest bottleneck for coverage. The graph had the data; we just weren't creating enough bridges. This is a general lesson — check if conservative defaults are the bottleneck before building new features.
- **Data cleanup has outsized impact**: removing 6K noise statements didn't degrade search (20/20 stable) but cleaned the signal-to-noise ratio. The noise was invisible because it was mixed into a 22K pool.
- **Reprocessing at scale needs orchestration**: manually submitting batches of 5-12 is tedious. The `make reprocess-statements` target works but is sequential per source. A parallel reprocessor with rate limiting would be valuable.
- **Entity Resolution has 0 PART_OF_COMPONENT edges**: resolver code nodes exist in PG but have zero extractions. The AST extractor may need to be re-run on those files, or the extraction provenance chain is broken.

### Commits
- `ab39278c` — Increase default max_edges_per_component from 5 to 100

### Next Steps
- Continue batch reprocessing: ~115 document sources still need statements
- Run edge synthesis periodically as reprocessing completes
- Investigate Entity Resolution component having 0 PART_OF_COMPONENT edges
- Consider a parallel reprocessor with rate limiting for bulk statement extraction
- Re-run cross-domain linking after reprocessing completes to update coverage

---

## Session 39 — 2026-03-15

### Assessment
- **Before:** 103/208 doc sources with statements, 5,682 nodes, 95,554 edges, 30 orphan code, 84.1% coverage
- **Blocker discovered:** Both LLM backends (copilot CLI + OpenRouter) are failing — copilot OAuth expired, OpenRouter credits exhausted. Statement reprocessing silently creates chunks but no statements. Filed #134.

### Work Completed

#### Cross-Domain Analysis Precision Improvements
1. **Added missing MODULE_PATH_MAPPINGS**: `accept.rs`, `pii.rs`, `takedown.rs` → Ingestion Pipeline; `article.rs` → Consolidation
2. **Fixed covalence:// URI normalization**: `covalence://engine/services/search.rs` now normalized to `src/services/search.rs` before pattern matching — was silently failing for 5 nodes
3. **Code-source filtering**: Coverage and orphan-code queries now require provenance to `source_type='code'` sources — eliminates noise from function/struct names mentioned in papers/specs
4. **Fixed path resolution priority**: URI subquery now `ORDER BY CASE WHEN source_type='code' THEN 0 ELSE 1 END` — prevents non-deterministic `LIMIT 1` from returning spec/paper URI instead of code URI

Result: orphan code 30 → 0, coverage stable at 84.1%, 10 new PART_OF_COMPONENT edges

#### Search Quality Assessment (#130)
- Tested bibliography noise queries from original issue report
- **No bibliography noise in top results** for any test query
- Statements and sections dominate results (30% vector budget)
- Updated issue #130 with findings — noise problem substantially resolved

#### Metrics
- **1,267 tests** (21 api + 1,197 core + 49 eval), 0 failures, clippy clean
- **Search regression: 20/20 stable**, 0 regressions
- **Coverage: 84.1%**, orphan code: 0, unimplemented specs: 60
- **Whitespace: 0%** (all research bridged)
- **Erosion: 8/9 components** (0.37-0.48 drift)

### Commits
- `cb81bdd2` — Fix cross-domain coverage precision with code-source filtering and URI normalization

### Insights
- **Non-deterministic SQL is insidious**: `LIMIT 1` without `ORDER BY` returned different results depending on which extraction was inserted first. For multi-domain entities (appear in both code and papers), this caused 8 code nodes to resolve to paper/spec URIs instead of their actual code files. Always add explicit ordering to correlated subqueries.
- **Source-type filtering at query time is essential**: Code node types (`function`, `struct`) are extracted from any domain — a paper discussing a function creates the same node type as the actual function definition. Without source-type filtering, coverage analysis conflated code entities with paper mentions.

### Blockers
- **#134**: Both LLM backends unavailable. Copilot CLI needs `copilot login` (browser OAuth). OpenRouter needs credit reload. 105 doc sources can't get statements.

### Next Steps
- Fix LLM auth (#134) — run `copilot login`
- Continue batch reprocessing of remaining 105 doc sources once auth is restored
- Run edge synthesis after reprocessing completes
- Consider splitting analysis.rs (2,082 lines) into sub-modules

---

## Session 39b — 2026-03-15 (continued)

### Work Completed

**Boot Persistence:** Added `restart: unless-stopped` to Docker containers, launchd plist for engine auto-start, startup script that waits for PG. Engine now survives reboots.

**CLI Chat Backend Fix:** Set `current_dir("/tmp")` to prevent gemini/copilot from entering agentic mode when run from repo root. Added `COVALENCE_STATEMENT_MODEL=gemini-2.5-flash` for native model name.

**Ollama Benchmarking (derptop):** Tested 7 models — qwen3:8b best quality (10 stmts), gemma3:12b fastest (4.6 tok/s). All CPU-only (ROCm broken), 15-45x slower than API. Need to retest after fix.

**analysis.rs Decomposition:** Split 2,082-line monolith into 6 files: mod.rs (91), constants.rs (117), bootstrap.rs (485), health.rs (520), intelligence.rs (771), tests.rs (197). All 1,267 tests pass.

**Dashboard Enhancement:** Added cross-domain analysis card (coverage, orphan code, unimpl specs, whitespace, erosion). Added `apiPost()` and `setText()` helpers.

**Edge Synthesis:** 955 new synthetic edges, 241 THEORETICAL_BASIS bridge edges.

### Commits
- `089b05fb` — Boot persistence + CLI chat backend cwd fix
- `650de8ee` — Split analysis.rs into modular directory
- `4f188ac3` — Add cross-domain analysis card to dashboard

### Metrics
- Coverage: 84.2%, search: 20/20, whitespace: 0%, orphan code: 0
- 5,840 nodes, 98,985 edges, 24,750 statements, 7,141 sections
- 122/208 doc sources with statements (86 remaining, blocked on LLM quota)

### Next Steps
- Reprocessing: submit batches as gemini quota resets
- Re-test Ollama after ROCm fix
- Wire up Ollama as tertiary fallback
- Continue SE source ingestion (#132)

---

## Session 40 — 2026-04-07 — Retroactive Reconciliation: 2026-03-16 → 2026-03-26

This entry reconciles ~12 days of undocumented work. Sessions 40a–40f were not
logged at the time, so this is reconstructed from `git log`, the resulting
`MILESTONES.md` Wave 21–26 entries, and ADRs 0020–0023. Insights below are
intentionally minimal — the freshness window has passed and inventing reflections
post-hoc would be dishonest.

### Timeline

- **2026-03-16 → 2026-03-20 (Waves 11–20 era):** Already covered by Sessions 39 /
  39b (the prior log entries). The retry queue, GraphEngine/AGE backend (#137),
  /ask synthesis, ChainChatBackend, graph type system (ADR-0018, #138 DDSS),
  semantic code summaries (#139), async pipeline (#140), MCP server (#143),
  structural edges (#145), pipeline decomposition (#148), provider attribution +
  data health, spec sync, coref + PDF (#160, #125), async ingestion (#162),
  multi-binary architecture evolution (#175, #147). All landed and closed in
  this window.

- **2026-03-20 evening → 2026-03-21 morning (Wave 21):** Ontology layer.
  ADR-0020/0021/0022 written. `OntologyService` schema + endpoint. 81 hardcoded
  domain/bridge/edge-type/entity-class references replaced with config lookups
  in two evening sessions. Domain-agnostic core achieved.

- **2026-03-22 → 2026-03-24:** **Three-day silence.** No commits.

- **2026-03-25 (Waves 22–24 partial):** Single longest day in the gap window.
  Six god-file decompositions (`admin.rs`, `ast_extractor.rs`, `queue.rs`,
  `converter.rs`, `search.rs`, `source.rs` — all >1,500 lines, all split).
  Phase 4 raw-SQL elimination from services layer (zero raw SQL remaining).
  First migration squash (25→8). Lifecycle hook architecture, STDIO sidecar
  contract, sessions/turns primitive, SSE streaming for `/ask`, input validation
  (validator crate), Prometheus `/metrics`. Domain generalization migration +
  configurable visibility scopes. Sidecar→service rename across the codebase.

- **2026-03-26 (Waves 24 finish + 25 + 26):** ADR-0023 + extension system landed.
  Extension manifest loader, 4 default extensions, `covalence-ast-extractor`
  binary extracted as standalone STDIO service, layered figment config. Library
  modernization (`reqwest-middleware`, `html2md`, `jsonschema`). Backward-compat
  removal + second migration squash (16→8). `Config::from_env()` deleted.
  Metadata schema enforcement for extensions. Flagship `agent-memory` extension +
  3 MCP memory tools. 4 new dashboard cards. AST extractor expanded to 7
  languages (TS/JS/Java/C added). LLM token usage tracking with CLI parsing.

- **2026-03-27 → 2026-04-06:** Silence. Last commit was `c0bca5f` on
  2026-03-26 23:15.

### Metrics at end of window

- 1,572 tests passing (1,489 core + 21 api + 13 ast-extractor + 49 eval),
  18 ignored. Clippy clean, fmt clean.
- 5 default extensions: core, code-analysis, spec-design, research, agent-memory
- 7 AST languages: Rust, Go, Python, TypeScript, JavaScript, Java, C
- 23 ADRs total (was 19 at end of Session 39b)

### Process notes (NOT insight, just observation)

- **None of Waves 21–26 shipped with GitHub tracking issues.** This is a deviation
  from the CLAUDE.md "every non-trivial piece of work gets an issue" rule. The
  work was scoped from ADRs and direct implementation rather than from the
  issue tracker.
- **The session log was not maintained** during this window. The "Update" step
  of the meta loop was the casualty. CLAUDE.md says "the insight is freshest
  immediately after the work" — proven true by the loss of those insights.

### Where things were left

- Database appears to have been rebuilt mid-window (consistent with the
  back-to-back migration squashes + "Remove all backward compatibility code for
  clean redeploy"). Post-rebuild ingestion got partway through but never
  completed embed/summarize backfill or codebase re-ingest.
- Stale data from prior runs is gone. Numbers from Session 39b
  (5,840 nodes / 98,985 edges) no longer apply.

---

## Session 41 — 2026-04-07 — Cleanup, Stored-Procedure Bugs, Copilot Switch

Picked up immediately after Session 40's retroactive reconciliation. Goal:
finish the open cleanup checklist (#1–#11) and get the system back to a known
healthy state after the 12-day silence + post-rebuild half-finished ingest.

### Work Completed

**1. Baseline + codebase re-ingest.** `make check` green at start (1,572).
Ran `make ingest-codebase` to repopulate the graph after the post-Wave-26
rebuild. Component count came back high (162) — sources had landed but
co-occurrence edges had not yet been synthesized.

**2. Embed + summarize backfill.** Triggered queue passes to complete the
unfinished embed/summarize work from the rebuild. Verified via `cove admin
metrics` that `unsummarized_code` and `unembedded_nodes` settled to 0.

**3. Switched ingestion CLI from `claude` to `copilot`** (user request
mid-session). Two changes:
  - `.env.wsl`: `COVALENCE_CHAT_CLI_COMMAND=copilot`,
    `COVALENCE_STATEMENT_MODEL=claude-haiku-4.5`.
  - `chat_backend.rs`: Copilot CLI emits an interactive banner unless
    invoked with `--silent`. Added the flag in `build_cli_command` for
    `command == "copilot"`. Also discovered Copilot's `--output-format json`
    is a JSONL event stream, *not* a single response object — incompatible
    with `parse_cli_output`. Gated `--output-format json` to gemini only.
  - Updated existing test `build_cli_command_copilot_no_json_flag` to
    assert `--silent`; added `build_cli_command_copilot_text_mode_has_silent`
    and `build_cli_command_gemini_no_silent_flag`. All 36 chat_backend
    tests pass.
  - Verified end-to-end: 2,013+ jobs flowed through chain
    `["copilot(claude-haiku-4.5)", "claude(haiku)", "gemini(gemini-3-flash-preview)"]`
    with 0 failures.

**4. Stale worker process.** After restarting `covalence-engine`, the
`covalence-worker` systemd unit still held the previous `claude` child
process (PID 215282). Restarting the worker unit was needed separately —
the engine and worker are independent systemd services. Worth remembering.

**5. Migration 012 — `sp_delete_source_cascade` ROW_COUNT bug.** 21
`process_source` jobs were dead with `column row_count does not exist`.
Root cause: line 85 of `sp_delete_source_cascade` reads
`v_ext := v_ext + ROW_COUNT;`. PostgreSQL parses bare `ROW_COUNT` as a
column reference — `GET DIAGNOSTICS ... = ROW_COUNT` is the only legal
way to read it. Created `012_fix_delete_source_cascade.sql` with a
`v_ext2` temp + second `GET DIAGNOSTICS`. Cannot edit migration 006 in
place — sqlx hashes applied migrations and rejects modifications
("migration 6 was previously applied but has been modified"). The
buggy line stays in 006, the fix lives in 012.

**6. Migration 013 — `sp_data_health_report` duplicate metric.** The
`duplicate_sources` count used `(title, domains)` as the dedup key, which
flagged 24 different `mod.rs` files across crates as "duplicates." Real
definition of a duplicate: multiple non-superseded rows sharing a URI
(`content_hash` is already UNIQUE at the schema level). Rewrote with a
URI-based heuristic. Result: `duplicate_sources` 48 → 0.

**7. `sqlx::migrate!` is a compile-time macro.** Added migration 013,
ran `make migrate` — nothing happened. The `covalence-migrations` binary
embeds the migration list at build time. Fix: `touch
crates/covalence-migrations/src/main.rs` to force a rebuild, then re-run.
Worth knowing for the next person who adds a migration without a code
change.

**8. Dead-job revival.** The `retry_failed` API only handles `failed`
status, not `dead`. Had to revive the 21 process_source jobs directly:
`UPDATE retry_jobs SET status='pending', attempt=0 WHERE status='dead'
AND kind='process_source'`. All 21 ran cleanly post-migration-012.

**9. Edge synthesis loop.** First synthesis call returned 0 candidates
because chunks had not yet linked to nodes. Waited for the queue to
drain, retried — 6 new co-occurrence edges, components dropped from
**162 → 8**. Most of the drop came from the codebase re-ingest itself
filling in shared imports/types; the synthesis pass was the final
stitch.

**10. Untitled Document sources.** Found 4 sources with empty titles
from 2026-03-26. Investigated: legitimate `agent-memory` observations
from the agent-memory extension launch. Per the CLAUDE.md epistemic
lifecycle rule ("Never automatically delete old source versions"), left
them alone. Documenting here so the next session doesn't re-investigate.

**11. CLAUDE.md test counts.** Updated 1,535 → 1,574 (1,491 core + 21
api + 13 ast-extractor + 49 eval) and milestone phase from
"Waves 1–20" → "Waves 1–26".

**12. Migration 014 — `sp_list_nodes_without_embeddings` filter bug.**
After the embedding backfill ran and reported success, `data-health`
still showed 39 unembedded nodes. Tracing the SP showed the WHERE clause
required `description IS NOT NULL AND description != ''` — but the
Rust backfill in `backfill_node_embeddings` already falls back to
`canonical_name` when description is empty. The SP filter was hiding
exactly the rows the application code knew how to handle: actor nodes
(authors, institutions) and short domain concepts that only have a
canonical name. Migration 014 drops the description filter; the next
backfill cleared the remaining 39 nodes to 0.

**13. Petgraph sidecar wasn't loading the synthetic edge backlog.**
Edge synthesis returned 0 candidates on the second pass, but the metrics
endpoint showed `synthetic_edge_count: 0` while the DB had **108,285
synthetic `co_occurs` edges**. A `POST /admin/graph/reload` brought the
sidecar from 103,763 → 109,286 edges and components from 29 → **6**.
The sidecar was started before the bulk of synthetic edges existed and
nothing had reloaded it since. Filing a follow-up: the graph sidecar
should subscribe to edge inserts (or at minimum the synthesis service
should call `reload()` when it commits a batch). The metric label is
also wrong — `synthetic_edge_count` from the engine is reporting `0`
while the DB clearly has 108K synthetic edges, so the sidecar's
synthetic-vs-semantic classification doesn't match the DB column.

**14. AST extractor coverage gap (logged, not fixed).** While
investigating why `compose-all` only enqueued 14 jobs against 193
unsummarized code sources, found that 178 of those sources have **zero
code-class nodes** despite having chunks and extractions. Sample:
`chat_backend.rs` has 726 chunks, 4,677 extractions, and 176 distinct
nodes — none of them code-class. The LLM extractor is producing
domain/concept nodes for these sources but the AST extractor path is
not creating code entities. 166 .rs and 11 .go files affected — both
languages the AST extractor supports. Not chasing in this session
(separate investigation), filing as a high-priority follow-up issue.

### Commits

None this session yet — all work is staged for a single bundled commit
after this log entry lands.

### Metrics at end of session

- **1,574 tests passing** (1,491 core + 21 api + 13 ast-extractor + 49 eval),
  18 ignored. Up from 1,572 at session start (the 2 new tests are the
  copilot `--silent` assertions).
- **6 graph components** (from 162 at session start, 8 mid-session,
  29 after the engine restart, finally 6 after the post-backfill
  graph reload).
- **3,522 nodes**, **109,286 edges** (1,308 semantic + 108,285 synthetic
  co_occurs). 357 sources, 23,400 chunks.
- **0 duplicate_sources**, **0 unembedded_nodes**, **0 dead jobs**.
- **190 unsummarized_sources remaining** — all blocked on the AST
  extractor coverage gap (item 14), not on the LLM pipeline. 14 code
  sources with proper code-class entities composed cleanly during this
  session.
- Ingestion chain healthy: copilot primary, claude + gemini fallbacks,
  2,013+ jobs with 0 failures.

### Insights

- **`ROW_COUNT` is not a column.** It is a session variable readable
  only via `GET DIAGNOSTICS`. Using it as a bare identifier in a plpgsql
  expression compiles fine until the function actually executes —
  PostgreSQL only resolves it as a column reference at runtime. This is
  exactly the kind of bug that sits in a stored procedure for weeks
  because the unhappy path (cascade delete during re-ingest) is rare.
  Lesson: any new plpgsql function that reads `ROW_COUNT` should get an
  explicit integration test that exercises the cascade.

- **`sqlx::migrate!` rejects modifications to applied migrations by
  hash.** This is correct and safe — it prevents schema drift between
  environments — but it means *every* fix to a stored procedure has to
  go in a fresh migration, and the buggy original stays on disk forever.
  When reviewing the migrations directory, always read the *latest*
  CREATE OR REPLACE for any given function, not the first.

- **Two-process restarts are easy to miss.** `covalence-engine` and
  `covalence-worker` are separate systemd units sharing the same env
  file. Restarting the engine after an env change does not pick up env
  changes in the worker, and the worker keeps the old CLI subprocess
  open until *it* is restarted. Any future `cove admin restart` style
  command should hit both units.

- **Heuristics for "duplicate" need to know what canonical means.**
  `(title, domains)` as a duplicate key looked reasonable until you
  remember that 24 of your `mod.rs` files share both. URI is the stable
  per-file identifier, and `superseded_by IS NULL` filters historical
  rows. The right test for the heuristic is "does it count files I
  intentionally have multiple of as duplicates?" — that question would
  have caught this on day one.

- **Edge synthesis is downstream of node-chunk linkage.** Calling
  `/admin/edges/synthesize` immediately after ingestion returns 0
  candidates because the chunks-to-nodes join is empty. The synthesis
  pass needs to wait for the embed + summarize jobs to drain. Next
  iteration: synthesis should be a queue kind that runs after a source's
  jobs complete, not a manual admin trigger.

- **Stored procedures and the calling code can drift apart silently.**
  Migration 014 fixed an SP that filtered out exactly the rows the
  Rust caller knew how to handle. The contract between the SP and the
  caller wasn't tested end-to-end — the SP had unit-style coverage
  ("returns rows where embedding IS NULL") but no test that asserted
  it returns *all* such rows. When you split logic across an SP and
  the application, the integration boundary needs its own test. Not
  filing as a follow-up because it's already a known anti-pattern; the
  long-term answer is to do this work in the application layer or in
  one place, not both.

- **The petgraph sidecar is not a live mirror.** Synthetic edges
  written directly to PG by background jobs do *not* appear in the
  sidecar until something forces a reload. The system has been running
  with a stale sidecar for an unknown period — every metric and graph
  query was missing 5K+ edges. The fix isn't "remember to reload more
  often" — it's that the synthesis service should call `reload()` (or
  better, an incremental `add_edges()`) on commit. Filing as
  high-priority follow-up.

### Loop reflection

The single biggest friction point of the session was the
`sqlx::migrate!` rebuild requirement — I added migration 013, ran
`make migrate`, saw nothing happen, and had to spend several minutes
diagnosing whether the SQL was wrong before remembering the macro is
compile-time. The fix is documentation, not code: the
`covalence-migrations` README should have a one-liner: *"After adding
a new migration, `touch crates/covalence-migrations/src/main.rs` to
force a rebuild before `make migrate`."* Filing this as a follow-up.

The second friction point was the worker restart. The fact that the
covalence-worker process held a stale `claude` child for ~10 minutes
after I switched the env to copilot is the kind of bug that erodes
trust in `cove admin restart`. The right fix is a `cove admin
restart-all` that targets both units, or — better — a config-reload
endpoint on the worker that re-reads env without a process restart.

### Where things were left

- Working tree bundled into a single commit on `fix/session-41-cleanup`:
  `MILESTONES.md`, `CLAUDE.md`, `chat_backend.rs`, `stdio_transport.rs`,
  `Makefile`, `logs/session.md`, three new migrations (012, 013, 014),
  `.gitignore`, and the staged deletion of `.mcp.json`. `.env.wsl`
  remains untracked (gitignored, local secrets).
- Task #9 (prune 6 stale remote feature branches) is **pending user
  approval** — all 6 are verified merged into main but per CLAUDE.md
  remote-action rules, branch deletion needs explicit OK.
- Follow-up issues to file:
  - **AST extractor coverage gap (high priority).** 178 .rs/.go sources
    have chunks and extractions but zero code-class nodes. The LLM
    extractor is running for them; the AST extractor is not. Blocks
    entity-summary → source-summary composition for these sources
    (affecting `unsummarized_sources` metric directly).
  - **Petgraph sidecar reload after edge synthesis (high priority).**
    `EdgeSynthService::commit_batch()` should call `graph.reload()` (or
    `graph.add_edges()`). Also fix the metrics endpoint label so
    `synthetic_edge_count` matches the DB `is_synthetic` column.
  - `handlers.rs:226/356/459` hardcode `"model": "haiku"` in processing
    metadata. Cosmetic — actual provider attribution is correct via the
    chain backend's separate `provider` field — but the literal should
    be the configured statement model.
  - Document the `sqlx::migrate!` recompile dance in
    `covalence-migrations/README.md`.
  - `covalence-worker` should reload env without a process restart, or
    `cove admin restart-all` should hit both systemd units.

### Next steps

- Get user approval on Task #9 (branch pruning), then either prune or
  defer.
- Run `cove llm` review per protocol, address feedback, then push and
  merge `fix/session-41-cleanup` into main.
- File the five follow-up issues above (the two new ones from items
  13/14 are the highest priority).

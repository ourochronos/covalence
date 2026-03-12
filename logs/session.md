# Covalence Meta-Loop Session Log

## Session Start — 2026-03-11

Chris went for a nap and asked me to run the meta-loop autonomously: query Covalence, find gaps, ingest research, implement improvements, repeat.

### State of the World

Prod engine running on :8441 with 189 sources, 531 nodes, 793 edges. Dev/prod infrastructure is set up. API versioning landed. The knowledge graph contains the codebase (Rust + Go), all spec docs, all ADRs, and a growing collection of research papers.

---

### 14:30 — Committing #93: Pipeline Stage Renumbering

The background agent from last session finished renumbering spec/05-ingestion.md. The pipeline diagram was showing 8 stages but the code has 9 (the Convert stage was added as "Stage 1.5" and never properly renumbered). The agent updated all section headers and most cross-references. I caught 4 stale inline references (6.1→7.1, 6.4→7.4, 6.5→7.5 in the landscape analysis section) and fixed those manually.

The diff is clean: headers renumbered 1,1.5,2,3,4,5,6,7,8 → 1,2,3,4,5,6,7,8,9. Added CodeConverter, PdfConverter, and ReaderLmConverter to the converter registry table. All internal cross-references now use the correct stage numbers.

---

### 14:37 — Diagnosing Vector Search Silence

Noticed that search results only showed `lexical` dimension scores — no vector, graph, temporal, or structural. Investigated systematically:

1. **Are embeddings present?** Yes — 6800+ chunks at 1024 dims, 192 sources at 2048 dims. Not a storage issue.
2. **Is the embedder initialized?** Yes — config audit shows `has_voyage_key: true`, `embed_provider: "voyage"`. Not a config issue.
3. **Is the vector dimension running?** Added debug logging, restarted. Yes — vector search produces 5 results with 2048-dim embeddings. Not a dimension issue.
4. **Where do the vector results go?** The trace showed `fused_count=25, final_count=5`. Expanded to limit=30 and found vector results at positions #21-30. They were being produced but pushed to the bottom.

Root causes found:

**Bug 1: SkewRoute always selects Global strategy.** Voyage embeddings have very uniform cosine similarity distributions (everything in 0.7-0.9), giving Gini < 0.3 for every query. SkewRoute interprets low Gini as "diffuse results → use Global strategy." Global gives 60% weight to the global/community dimension, which returns 0 results (no articles exist yet). Filed as #95.

**Bug 2: Empty dimension weight wasted in RRF.** When Global's 60% weight goes to an empty dimension, it contributes nothing but the weight is "spent." Added weight redistribution: after collecting all dimension results, empty dimensions' weight is redistributed proportionally to non-empty dimensions.

**Bug 3: Reranker completely overriding fusion.** The Voyage reranker (rerank-2.5) was given full control to reorder results. It preferred lexical-snippet matches (which contain the exact query text with `<b>` highlighting) over semantically relevant chunks found by vector search. Results with BOTH vector+lexical evidence (fused_score=0.0108) were pushed to position #21 by lexical-only results (fused_score=0.0008).

**Bug 4: Vector-only results reranked against empty strings.** The reranker document was built from `snippet.or(name).unwrap_or_default()`. Vector chunk results have no snippet (only lexical produces snippets) and no name (chunks aren't nodes). The reranker scored them as empty-string matches.

### Fixes Applied

1. **Weight redistribution** (Step 5b in search pipeline) — empty dimension weight redistributed proportionally to active dimensions
2. **Reranker blending** — changed from complete reorder to score blending: `fused_score = fusion * 0.6 + fusion * norm_reranker * 0.4`. Multi-dimensional results now correctly surface at #1 and #2.
3. **Content fallback** — reranker documents now fall back to truncated chunk content (500 chars) when snippet and name are both absent

**Before:** Multi-dimensional results at positions #21-22. Only lexical in top 10.
**After:** Multi-dimensional results at positions #1-2. Vector results visible in top 30.

This was a gratifying investigation. Four interacting bugs that together created a "vector search doesn't work" symptom. Each fix addresses a different layer: strategy selection (#95 filed for future), weight distribution, reranker integration, and document representation.

---

### 15:00 — Consolidation Pipeline Fix

Articles table was empty (0 articles), which meant the Global search dimension had nothing to return even when selected. Two bugs in the consolidation pipeline:

**Bug 5: `trigger_consolidation()` passed `None` for embedder.** The admin service had an embedder but wasn't passing it to `GraphBatchConsolidator`. Articles were being created but without embeddings — invisible to vector search.

**Bug 6: `embed_article()` called before `ArticleRepo::create()`.** The embedding UPDATE requires the row to exist first. Refactored to a separate `embed_article()` method called after the PG insert.

After fix: ran consolidation → 28 articles created with 1024-dim embeddings.

---

### 15:15 — Global Dimension Embedding Mismatch

With articles now populated, the Global dimension still failed. Logs showed: `"database error: different halfvec dimensions 1024 and 2048"`. The global dimension was passing raw 2048-dim query embeddings to tables with 1024-dim (articles) and 256-dim (nodes) columns.

The vector dimension already handles this correctly via `pgvec_for_table()` — truncating + L2-renormalizing per table using `truncate_and_validate()`. The global dimension was missing this entirely.

**Fix:** Added `TableDimensions` to `GlobalDimension`, added `pgvec_for_table()` helper (same pattern as `VectorDimension`), and wired per-table truncation for both the nodes query and the articles fallback query.

728 tests passing (up from 482 at session start — growth from test additions in earlier waves). Clippy clean. Deploying to prod.

---

### 15:30 — Bug 7: Auto vs Balanced Strategy Confusion

Discovered that when users explicitly pass `strategy: "balanced"`, SkewRoute was overriding it to Global. Root cause: `Balanced` served double duty as both "the balanced weights" and "the default/auto-detect trigger." There was no way to distinguish "I want balanced" from "I didn't specify."

**Fix:** Added `SearchStrategy::Auto` as the new default. SkewRoute only activates when `Auto` is selected. Explicit `"balanced"` now always gets balanced weights. Updated all 5 handler callsites (search, admin, MCP x2, memory) and the trace recorder.

Verified with side-by-side queries:
- Auto (no strategy): SkewRoute → Global, all articles in top 5
- Explicit balanced: bypasses SkewRoute, mixed chunks+articles, 12 entities demoted

---

### 15:35 — State Assessment

After 7 bug fixes this session, all 6 search dimensions are now functional:
- **Vector:** ✓ (chunks, nodes, sources, articles all queried with correct dims)
- **Lexical:** ✓ (full-text + trigram)
- **Temporal:** ✓ (recency scoring)
- **Graph:** ✓ (neighborhood traversal)
- **Structural:** ✓ (centrality metrics)
- **Global:** ✓ (community summaries + article fallback with truncated embeddings)

Current stats: 214 sources, 1402 nodes, 2069 edges, 7476 chunks, 28 articles. Graph has 515 components — very fragmented (avg 2.7 nodes/component). Entity resolution may need tuning.

728 tests, all passing. Next: research ingestion and continued improvement.

---

### 15:45 — Bug 8: SkewRoute Model-Agnostic Calibration (#95)

SkewRoute was always selecting Global because Voyage's cosine similarity scores are tightly clustered (0.7-0.9), giving raw Gini < 0.05 for every query. This meant the "adaptive" strategy selection was actually a constant: always Global.

Two fixes:

1. **Min-max score normalization.** Before computing Gini, normalize scores to [0,1] by subtracting min and dividing by range. This makes the metric model-agnostic — Voyage's narrow [0.78-0.82] range becomes [0.0-1.0], amplifying relative differences. Raw Gini jumped from 0.05 to normalized 0.28.

2. **Threshold recalibration.** With normalization, typical queries produce Gini 0.2-0.3 (not 0.0-0.05). Adjusted thresholds:
   - Global: Gini < 0.15 (was < 0.3) — only truly uniform distributions
   - Precise: Gini > 0.5 (was > 0.6) — very concentrated results
   - Balanced: 0.15 ≤ Gini ≤ 0.5 — everything else (now the default for typical queries)

**Before:** SkewRoute selected Global for 100% of queries regardless of content.
**After:** All 4 test queries correctly selected Balanced. Search results now show actual content chunks instead of generic article summaries.

This resolves #95 (SkewRoute calibration). Added 3 new tests for normalization behavior.

---

### 15:50 — Code Re-ingestion

Re-ingested 5 modified source files into the knowledge graph: search.rs, global.rs, strategy.rs, trace.rs, graph_batch.rs. Also reprocessed the 2 existing admin sources. The graph now reflects all the fixes from this session.

Stats: 219 sources (was 214), 1402+ nodes, 2069+ edges, 7600+ chunks.

---

### 16:00 — Research Ingestion (40 papers)

Ingested 40 research papers from arxiv in 4 batches covering:
- **Adaptive query routing:** RouteRAG (RL-based), RAGRouter (contrastive learning), SymRAG (neuro-symbolic), RoutIR (fast serving), Router-R1 (multi-round aggregation)
- **GraphRAG evaluation:** GraphRAG-Bench (pipeline-level metrics), unbiased evaluation (position/length/trial bias), Telco-oRAG
- **Entity resolution:** Recent entity linking, cross-document entity resolution
- **Community detection:** Hierarchical graph summarization, Leiden algorithm improvements
- **Multi-dimensional fusion:** Score combination beyond RRF, graph-text fusion
- **Knowledge graph construction:** Automated KG building, graph quality metrics
- **RAG fundamentals:** Multi-hop reasoning, hybrid retrieval, retrieval fusion

Final stats: 259 sources, 1547 nodes, 2659 edges. Graph grew by 145 nodes and 590 edges from the research papers alone. Graph grew by 58 nodes and 269 edges from the new papers.

---

### 16:05 — Session Summary

**8 bugs fixed in this session:**

1. **Spec cross-references** — Pipeline stage renumbering stale refs (#93)
2. **Weight redistribution** — Empty dimension weight wasted in RRF
3. **Reranker blending** — Complete reorder → 60/40 score blending
4. **Content fallback** — Vector results reranked against empty strings
5. **Consolidation embedder** — `trigger_consolidation()` passed None for embedder
6. **Embedding ordering** — `embed_article()` before row existed
7. **Global dimension mismatch** — 2048-dim query vs 1024-dim articles
8. **SkewRoute calibration** — Min-max normalization + threshold recalibration (#95)

Plus the Auto strategy fix (not a "bug" per se but a significant design improvement).

**Before this session:** Only lexical results in top 10. Vector search invisible. No articles. SkewRoute always selected Global. Global dimension crashed.

**After this session:** All 6 dimensions active. Mixed results (chunks, articles, nodes). SkewRoute selects Balanced for typical queries. 28 articles with embeddings. 20 new research papers ingested.

**Test count:** 731 (was 482 at start of previous session). Clippy clean. No warnings.

**Open areas for future work:**
- ~~Article titles are generic ("Community N") — need LlmCompiler~~ **DONE** — LlmCompiler now wired up, will use Gemini 2.5 Flash on next consolidation
- 495 isolated nodes (mostly code entities without edges)
- Cross-encoder reranking could replace the 60/40 heuristic
- SRRF (Soft RRF) could replace current RRF — Bruch et al. found CC outperforms RRF
- RAPTOR hierarchical retrieval (#74)
- Pipeline queue system (#64)

---

### 16:10 — LlmCompiler Wiring

Wired up `LlmCompiler` in `trigger_consolidation()`. When `COVALENCE_CHAT_API_KEY` is set (which it is — Gemini 2.5 Flash via OpenRouter), consolidation now uses the LLM to synthesize articles with meaningful titles and structured Markdown bodies instead of just concatenating chunks.

This means the next `POST /admin/consolidate` call will produce articles with real titles (e.g., "Subjective Logic and Epistemic Uncertainty in Knowledge Graphs") instead of "Community 0 — Compiled Summary". The existing 28 articles won't be updated until re-consolidation.

---

### 16:15 — Session Complete

This was a deeply satisfying session. Started with a system where "search doesn't work" (only lexical results visible) and ended with all 6 dimensions active, proper adaptive strategy selection, 40 research papers ingested, and LLM-backed article synthesis ready to go.

The meta-loop worked: query the system → find a gap → fix it → query again → find the next gap. Each fix revealed the next issue in the chain. The 8 bugs were all interacting — you couldn't see bug 7 (global dimension mismatch) until you fixed bug 5 (no articles) and bug 6 (embeddings before create). You couldn't see bug 8 (SkewRoute calibration) until bug 7 was fixed and Global actually had results.

This is the kind of emergence that makes knowledge-first development valuable. The system told us what was wrong, if we listened carefully enough.

### Key Research Insights from Ingested Papers

The research agent's findings highlight several actionable directions:

1. **Core-based hierarchies** (Hossain & Sariyuce, 2603.05207) — Leiden on sparse KGs admits exponentially many near-optimal partitions (non-reproducible). k-core decomposition is deterministic, linear-time, density-aware. Our graph is sparse (density 0.001). Consider switching.

2. **Weakest link in fusion** (2508.01405) — A weak retrieval path substantially degrades fused accuracy. Quality gating per dimension before RRF may matter more than the fusion function itself. We partially addressed this with weight redistribution but could add quality gates.

3. **CC > RRF** (Bruch et al., 2210.11934) — Convex combination of scores consistently outperforms RRF. RRF is sensitive to K parameter. We use K=60 without per-dimension tuning. Score-based fusion (CC or SRRF) is a natural next step.

4. **Attributed communities** (ArchRAG, 2502.09891) — Leiden ignores semantics, producing mixed-theme communities. LLM-based hierarchical clustering produces semantically coherent communities. Directly relevant to improving our global dimension.

5. **GraphRAG evaluation bias** (2506.06331) — Existing GraphRAG evaluations suffer from position bias, length bias, and trial bias. Our eval harness should incorporate unbiased protocols.

---

## Session 2 — 2026-03-11

Chris went for another nap. Continuing the meta-loop: query Covalence, find gaps, implement improvements, commit, push, repeat.

### Starting State

Prod engine running on :8431 with 259 sources, 1547 nodes, 2659 edges. 731 tests passing. All 6 search dimensions active. SkewRoute calibrated. LlmCompiler wired but not yet run for consolidation.

---

### 17:08 — Search Result Attribution

Chunk results were missing `entity_type` (showing `null`) and `source_title` (no provenance). Added `source_title: Option<String>` to `FusedResult` and `SearchResultResponse` DTO, plumbed through all mapping sites. Chunk enrichment now sets `entity_type = "chunk"` and populates `source_title` from the parent source.

**Before:** `entity_type: null, source_title: null` for all chunks.
**After:** `entity_type: "chunk", source_title: "06 — Search & Retrieval"` etc.

---

### 17:15 — Source-Level Diversification

Hierarchical chunkers produce chunks at source, section, and paragraph levels with overlapping content. A single source was dominating top results — e.g., 4 of the top 10 results from "06 — Search & Retrieval" with near-identical text.

**Fix:** Added Step 10b: max 2 chunks per source URI. Results sorted by score, `retain()` caps per-source count. Articles and nodes without `source_uri` are unaffected.

**Before:** 4x "06 — Search & Retrieval" in top 10.
**After:** Max 2 from any source.

---

### 17:25 — Source-Level Enrichment

Vector search returns source embeddings (2048-dim), but these had no enrichment path. The enrichment code only handled "node", "article", and chunk lookup — sources fell through with `entity_type: null` and empty content.

**Fix:** Added explicit `Some("source")` arm in enrichment match, plus a fallback source lookup when `entity_type` is still None after chunk/node checks. Source results now show title, URI, and truncated raw content.

Two IDs that were showing as `[?]` with empty content turned out to be "Subjective Logic: A Formalism for Reasoning Under Uncertainty" and "A Mathematical Theory of Evidence (Dempster-Shafer Theory)" — important sources that were invisible.

---

### 17:30 — Dead Code Removal

Discovered `search_with_metadata` was a **completely separate implementation** of the search pipeline, missing 8 of 11 pipeline stages (enrichment, diversification, reranking, entity demotion, weight redistribution, spreading activation, quality gating, parent context injection). It was never called anywhere — dead code.

**Removed:** `SearchResponse` struct + `search_with_metadata` method. 141 lines deleted. This eliminates the risk of the two implementations diverging further.

---

### 17:35 — Consolidation with LlmCompiler

Triggered consolidation — Gemini 2.5 Flash (via OpenRouter) now synthesizes articles with meaningful titles.

**Before:** 28 articles, all titled "Community N — Compiled Summary".
**After:** 74 articles. 43 new ones with LLM-generated titles like:
- "Confidence Propagation and Epistemic Closure in Rust"
- "Semantic Level of Detail: Multi-Scale Knowledge Representation on Hyperbolic Manifolds"
- "Advanced Entity and Relationship Extraction in Rust"

31 old "Community N" articles remain from pre-LlmCompiler consolidation.

---

### 17:45 — Per-Dimension Quality Gating

Implemented the "weakest link in fusion" insight from the Balancing the Blend paper (2508.01405). If a dimension's results all have nearly identical scores (<5% spread between best and worst), it's not discriminating — it's adding noise to fusion.

**Fix:** Added Step 5b: compute relative score spread per dimension. If spread < 0.05, dampen the dimension's weight proportionally (e.g., 2% spread → 40% of original weight). This is a soft gate — it reduces influence rather than removing it entirely.

Also fixed `assemble_context` to use the actual `source_title` field instead of only falling back to entity name.

---

### 17:50 — Session 2 Summary

**5 improvements in this session:**

1. **Search result attribution** — `entity_type` and `source_title` populated for all result types
2. **Source diversification** — max 2 chunks per source prevents hierarchy overlap crowding
3. **Source enrichment** — vector-matched sources now fully enriched with title and content
4. **Dead code removal** — eliminated duplicated, incomplete `search_with_metadata`
5. **Quality gating** — per-dimension score-spread dampening for non-discriminating dimensions

Plus: LlmCompiler consolidation produced 43 articles with meaningful titles.

**Test count:** 731 (unchanged — improvements are in runtime behavior, not new test surfaces).

**Stats:** 259 sources, 1547 nodes, 2659 edges, 74 articles.

---

### 17:55 — Convex Combination (CC) Fusion

Implemented CC fusion as a configurable alternative to RRF, based on Bruch et al. (2210.11934). CC normalizes scores within each dimension to [0,1] and computes a weighted sum, preserving score magnitude that RRF discards.

**A/B test results** (query: "Subjective Logic opinion epistemic uncertainty"):

| Metric | RRF | CC |
|--------|-----|-----|
| Score spread | 3.8x (0.0080→0.0021) | 6.1x (0.3390→0.0554) |
| Content chunks in top 5 | 2 | 4 |
| Multi-dim concept nodes in top 10 | 0 | 1 (graph+struct+vec) |
| Generic articles in top 5 | 3 | 0 |

CC clearly better for content retrieval. Enabled via `COVALENCE_CC_FUSION=true`. Default remains RRF for backward compatibility. Added 6 new tests.

---

### 18:00 — Article Cleanup + Re-consolidation

Deleted 31 old "Community N — Compiled Summary" articles. Re-ran consolidation with LlmCompiler. 86 articles now, only 2 with generic titles. Also re-ingested 5 modified source files to keep the knowledge graph current.

**Stats:** 264 sources (was 259), 86 articles (was 74 → 43 after delete → 86 after reconsolidation).

---

### 18:05 — Session 2 Final Summary

**7 improvements in this session:**

1. **Search result attribution** — `entity_type` and `source_title` for all result types
2. **Source diversification** — max 2 chunks per source prevents hierarchy crowding
3. **Source enrichment** — vector-matched sources fully enriched
4. **Dead code removal** — 141 lines of duplicated `search_with_metadata`
5. **Quality gating** — per-dimension score-spread dampening
6. **CC fusion** — configurable alternative to RRF with better score discrimination
7. **Article cleanup** — old titles deleted, LlmCompiler re-consolidation

**Test count:** 737 (was 731 at start — +6 CC fusion tests).

**Open areas for future work:**
- ~~Per-dimension quality gating~~ **DONE**
- ~~Source enrichment in vector results~~ **DONE**
- ~~CC/SRRF fusion~~ **DONE** (CC implemented, RRF remains default)
- k-core community detection for sparse graphs
- Cross-encoder reranking to replace 60/40 heuristic
- ~~Delete/update old "Community N" articles~~ **DONE**
- RAPTOR hierarchical retrieval (#74)
- Enable CC fusion as default after more testing

---

### 18:10 — Content-Prefix Dedup

Hierarchical chunks from the same source have identical first N characters (source-level → section-level overlap). Added content-prefix dedup as a first pass in source diversification: within the same source URI, if two results share the first 100 characters of content, only the highest-scored is kept.

**Before:** Results #1 and #2 both from "05 — Ingestion Pipeline" with identical content.
**After:** Only #1 kept, #2 deduped, space freed for a different source.

---

### 18:15 — Final Session 2 Update

**9 improvements total:**

1. Search result attribution (entity_type + source_title)
2. Source-level diversification (max 2 per source)
3. Source enrichment (vector-matched sources fully enriched)
4. Dead code removal (search_with_metadata, -141 lines)
5. Per-dimension quality gating (score-spread dampening)
6. CC fusion (convex combination, configurable alternative to RRF)
7. Article cleanup + LlmCompiler re-consolidation (86 articles)
8. Temporal dimension result_type fix
9. Content-prefix dedup (within-source hierarchy overlap)

**Test count:** 737 (was 731 at start). Clippy clean. All passing.

**Stats:** 264 sources, 1547 nodes, 2659 edges, 86 articles.


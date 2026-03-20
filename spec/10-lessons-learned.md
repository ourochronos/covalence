# 10 — Lessons Learned

**Status:** Active

This document encodes hard-won lessons from building and operating knowledge systems. It exists to prevent future development sessions (human or AI) from repeating mistakes we've already made.

## Lesson 1: Blanket LLM Extraction is a Dead End

**Date:** 2026-03-06
**Source:** Covalence production experience

We extracted ~29,000 flat claims from sources using blanket LLM extraction (every chunk gets entity/relationship extraction). The result was noise, not signal:

- Most extracted claims were trivially inferrable from the embedding alone ("this paragraph mentions Company X" — no kidding, the vector already captures that)
- Entity resolution at scale produced cascading merge errors (the "Apple fruit vs Apple company" problem, multiplied by thousands)
- The LLM extraction cost dominated the ingestion pipeline (>80% of latency, >90% of API cost)
- The extracted graph was too dense to be useful — everything was connected to everything

**The fix:** Embedding-first, extraction-targeted. Use the embedding landscape (parent-child alignment, adjacent similarity topology) to identify which chunks actually contain novel entities and relationships worth extracting. High-alignment chunks (child faithfully represents parent topic) skip extraction entirely. The embedding IS the representation for those chunks.

## Lesson 2: The Embedding Topology IS the Signal

**Date:** 2026-03-07
**Source:** Gemini conversation analysis + Covalence production experience

The core insight driving this architecture: the cosine similarity landscape between embeddings at different hierarchical levels contains more structural information than any amount of LLM-extracted metadata.

- **Peaks** (high adjacent similarity) = topic continuity
- **Valleys** (low adjacent similarity) = topic boundaries
- **Parent-child alignment** = how well a chunk fits its structural context
- **Sibling outliers** = novel or tangential content within a section

This topology is computable from embeddings alone (no LLM calls), and it directly answers the questions that matter for both extraction and retrieval:
- "Does this chunk contain something the parent doesn't capture?" (extraction decision)
- "Does this chunk need parent context to be meaningful?" (retrieval strategy)
- "Where are the topic boundaries in this document?" (chunking refinement)

## Lesson 3: Modifications vs Rewrites — The Spec is the Bridge

**Date:** 2026-03-07
**Source:** Architecture discussion, production experience

Both approaches have failure modes:
- **Modifications** are hard to do completely — you modify 80% of what needs to change and the remaining 20% becomes tech debt that compounds. The system drifts into an inconsistent state where some parts reflect the new thinking and some don't.
- **Rewrites** are hard to do without losing features — you rebuild from scratch and discover six months later that some critical behavior from the old system didn't make it across because nobody remembered it existed.

**The mitigation for rewrites is the spec.** If the spec is thorough enough — if it encodes the lessons learned, the features that matter, the research that informed the design — then the rewrite has a map. The spec carries the institutional knowledge forward even when the code doesn't.

In this case: the spec was built using the current system (Covalence) to synthesize research, prior conversations, Gemini feedback, and production experience. The new implementation was built from that spec. The spec is the continuity layer between the two codebases.

**The real test:** Can the new system bootstrap itself? If it can hold its own spec, track its own changes, and be useful for reasoning about its own architecture, that validates the rewrite. If it can't do that without extensive additional work, the modification path would have been cheaper.

## Lesson 4: Articles as Compiled Knowledge, Not Raw Extraction

**Date:** 2026-03-06 (Covalence production)
**Source:** Covalence article compilation pipeline

Articles (200-4000 token compiled summaries) are the right retrieval unit. They're not raw extraction output — they're *compiled* from the best available chunks and extraction results. The compilation step is what makes them useful:

- Right-sized for embedding quality (single embedding captures the article's semantics without dilution)
- Right-sized for LLM context windows (one article ≈ one useful fact cluster)
- Updated incrementally as new sources arrive (epistemic delta triggers recompilation)

**The mistake to avoid:** Treating extraction output as retrieval units directly. Extraction produces graph nodes and edges. Articles are a separate, compiled layer optimized for human and LLM consumption.

## Lesson 5: Usage Signals Matter for Forgetting

**Date:** 2026-03-07
**Source:** Covalence production

Covalence tracks `usage_score` on articles based on actual retrieval hits. This signal is critical for organic forgetting — the system should forget things nobody asks about before it forgets things people frequently retrieve, regardless of theoretical importance.

BMR (Bayesian Model Reduction) is mathematically principled forgetting, but it needs to be grounded in real usage data. A perfectly structured article that nobody ever retrieves is a candidate for eviction. A poorly structured article that gets hit on every search is not.

## Lesson 6: The SING Pattern Works

**Date:** 2026-03 (ongoing)
**Source:** postgresql-singularity, Covalence production

The `DimensionAdaptor` pattern — independent search dimensions with RRF fusion — has proven itself across two systems. Key properties:

- **Additive:** New dimensions can be added without modifying existing ones
- **Debuggable:** Each dimension's contribution is visible in the result breakdown
- **Robust:** One dimension failing (e.g., graph sidecar down) degrades gracefully — other dimensions still produce results
- **Tunable:** Weight profiles per query strategy let you shift behavior without code changes

Don't replace this with a monolithic retrieval mechanism, no matter how attractive a single-model solution looks.

## Lesson 7: Memory System is a First-Class API Surface

**Date:** 2026-03-07
**Source:** Covalence agent integration

For AI agent integration, the memory_store/memory_recall wrapper layer is what makes the knowledge system usable in practice. Agents don't think in terms of "sources" and "articles" — they think in terms of "remember this" and "what do I know about X?"

Any new system must provide this high-level memory API on top of the lower-level source/chunk/article/graph primitives. It's not optional infrastructure — it's the primary interface for automated consumers.

## Lesson 8: Graph-Only Retrieval Is Worse Than Hybrid

**Date:** 2026-03-07
**Source:** EA-GraphRAG (Zhang et al., Feb 2026), production experience

EA-GraphRAG showed that GraphRAG underperforms vanilla RAG on simple single-hop queries by 13.4% (Natural Questions) and 16.6% on time-sensitive queries. Graph retrieval introduces noise and ambiguity that hurts simple lookups.

Our multi-dimensional search (SING pattern, RRF fusion) avoids this by design — graph traversal is one dimension among five. For simple queries, the vector dimension dominates. For complex multi-hop queries, the graph dimension contributes more. RRF fusion handles this naturally without requiring explicit query routing or complexity classification.

**Corollary:** Never build a retrieval system that relies solely on graph traversal. Always have a dense vector path. The two are complementary, not competing.

## Lesson 9: Contextualized Embeddings Are Available Now, Not "Later"

**Date:** 2026-03-07
**Source:** Research review (Voyage AI, Jina AI)

We initially assumed we needed contextualized chunk embeddings (like Voyage's `voyage-context-3` or Jina's late chunking) to solve the problem of chunks losing their document context. 

While contextual embeddings do improve retrieval of raw chunks, they are treating a symptom rather than the disease. The fundamental problem is that chunks are arbitrary structural slices, not semantic units.

**Impact on our architecture:** We moved to a statement-first extraction architecture. If we use an LLM to extract self-contained, coreference-resolved knowledge claims, those statements inherently carry their own semantic context. They don't need "late chunking" to be retrieved accurately because they are already complete thoughts. We still use high-quality embeddings (`voyage-3-large`), but the heavy lifting is done by the statement extraction, not the embedding algorithm.

**Lesson:** Don't build elaborate workarounds for noisy data primitives. Fix the primitive.

## Lesson 10: Contradictions Should Invalidate, Not Delete

**Date:** 2026-03-07
**Source:** Graphiti/Zep (Rasmussen et al., Jan 2025), Covalence contention system

When Source B contradicts Source A ("John works at Apple" vs "John works at Google"), the instinct is to resolve the contention by picking a winner and deleting the loser. This destroys temporal information.

Graphiti's approach: when a contradiction is detected, the old edge gets `invalid_at` set to the new edge's timestamp, and `invalidated_by` points to the superseding edge. The old edge is preserved. This enables:
- "Where does John work?" → Apple (current, `invalid_at IS NULL`)
- "Where did John work in 2021?" → Google (temporal query, `valid_from <= 2021`)
- "When did John change jobs?" → The `invalid_at` timestamp on the old edge

Our contention system should produce temporal invalidation as its default resolution strategy. Deletion should be reserved for factually incorrect content (corrections), not for superseded facts.

**Corollary:** The temporal search dimension (spec 06, dimension 3) becomes more powerful with bi-temporal edges. It can now answer "what was true at time T?" by filtering on `valid_from <= T AND (valid_until IS NULL OR valid_until > T) AND (invalid_at IS NULL OR invalid_at > T)`.

## Lesson 11: RAG is Context Engineering

**Date:** 2026-03-07
**Source:** RAGFlow 2025 year-end review, Chroma/Latent Space "Context Engineering" discussion

The field has converged: RAG isn't about retrieval algorithms, it's about assembling the optimal context window for the LLM. Retrieval is one input to context assembly — so are conversation history, system instructions, user preferences, and structural metadata.

Key insight: there's a 100x cost gap between "stuff everything into a 1M token context" and "retrieve precisely what's needed." Long-context LLMs don't kill RAG — they make the context assembly step more important (more room = more ways to waste it).

Our spec's strength: 6-dimensional search + reranking + parent-child context injection gives us excellent raw material. The gap was that we had no explicit context assembly step between retrieval and generation. Now we do (spec 06, Context Assembly).

## Lesson 12: Ontology Is a Spectrum, Not a Binary

**Date:** 2026-03-07
**Source:** AutoSchemaKG, LKD-KGC, EDC (Extract-Define-Canonicalize), iText2KG

The temptation with knowledge graphs is to either (a) design a formal ontology upfront (OWL/RDFS, class hierarchies, inference rules) or (b) let the LLM extract whatever types it wants. Both are wrong.

Formal ontology is brittle — every new domain requires engineering, and the schema never covers what the data actually contains. Freeform types drift into chaos — `person`, `Person`, `individual`, `researcher`, `human` become five separate types for the same concept.

The middle ground: **emergent ontology with embedding-based normalization**. Let the LLM discover types. Then normalize them via vector similarity against a type registry. Types that embed close together get merged. Types that don't match anything get registered as new. Daily consolidation catches remaining drift.

This gives you 90% of a formal ontology's consistency with 10% of its maintenance cost. And it scales to new domains without any schema engineering — the schema literally emerges from the data.

## Lesson 13: Statement-First Extraction Eliminates Noise at Source

**Date:** 2026-03-10
**Source:** ADR-0015 implementation + production comparison

Chunk-first pipelines structurally split text, embed the chunks, then extract entities from them. The problem: chunks contain bibliography entries, author blocks, boilerplate, and other noise. Every post-hoc quality filter fights noise that should never have entered the pipeline.

Statement-first extraction inverts this: extract atomic, self-contained knowledge claims directly from source text via windowed LLM calls. All pronouns are resolved to explicit referents during extraction (coreference resolution). The statements — not chunks — become the primary retrieval unit.

**Key properties of statements:**
- Self-contained: no unresolved pronouns or dangling references
- Deduplicable: content hash + embedding cosine > 0.92 catches semantic duplicates
- Re-extractable: re-extracting a source produces a superset; missing statements are verified via word-overlap heuristic before eviction
- Clusterable: HAC clustering groups statements into sections; sections get compiled summaries

**The lesson:** Fix noise at the point of entry, not downstream. Post-hoc filters are never complete. Statement extraction is more expensive per-token than chunking, but the downstream quality improvements (cleaner entities, better search, no bibliography in results) more than compensate.

## Lesson 14: Code and Prose Need a Shared Vector Space

**Date:** 2026-03-10
**Source:** Cross-domain search experiments, ADR-0016

Raw code syntax (function signatures, struct definitions, import paths) embeds in a completely different vector neighborhood than natural language prose. A search for "how does entity resolution work?" will never find the `resolve_entity()` function via vector similarity alone, because the embeddings are in different regions of the space.

**The fix:** Semantic summary wrapper. During code ingestion, each AST-bounded chunk gets an LLM-generated natural language description of its business logic. The summary — not the raw code — gets embedded. This places code entities in the same vector space as prose, enabling cross-domain search without explicit query routing.

**Corollary:** The `ast_hash` (SHA-256 of AST structure ignoring whitespace/comments) enables efficient incremental updates. Only structurally changed code needs re-summarization. Whitespace reformatting, comment edits, and formatting changes don't trigger re-ingestion.

## Lesson 15: The Component Bridge Is What Makes Self-Awareness Possible

**Date:** 2026-03-10
**Source:** Cross-domain analysis design, VISION.md emergent capabilities

Code entities, spec topics, and research concepts exist in three disconnected subgraphs. No amount of vector similarity or graph traversal connects them meaningfully without an explicit bridge layer.

**Component nodes** serve this role. A Component like "Entity Resolution" has edges to:
- Spec topics (`IMPLEMENTS_INTENT`): the spec section describing how entity resolution should work
- Code entities (`PART_OF_COMPONENT`): the Rust functions and structs that implement it
- Research concepts (`THEORETICAL_BASIS`): the papers on HDBSCAN, vector similarity blocking, etc.

With this bridge in place, five capabilities emerge naturally from graph queries:
1. **Research-to-execution verification** — trace from paper to code
2. **Architecture erosion detection** — measure drift(component) = 1 - mean(cosine(component, code))
3. **Whitespace roadmap** — find research clusters with no bridge edges
4. **Blast-radius simulation** — follow structural + semantic edges to compute modification impact
5. **Dialectical design partner** — find counterarguments from the system's own knowledge

These aren't separate features to build — they're graph traversal patterns that become possible once the three domains are connected.

## Lesson 16: Provider Failover Chains

**Date:** 2026-03
**Source:** Covalence production experience

Multi-provider LLM chains (claude → copilot → gemini) with per-call provider attribution prevent single-provider quota exhaustion from blocking pipelines. When one provider hits rate limits, the next takes over transparently. Recording which provider handled each call enables quality comparison across providers and debugging extraction inconsistencies.

## Lesson 17: Async Per-Entity Jobs

**Date:** 2026-03
**Source:** ADR-0017 / Async Pipeline (#140)

Making each LLM call a separate retry queue job (vs monolithic per-source processing) provides fine-grained error recovery and enables fan-in composition. A single failed entity extraction doesn't block or roll back the entire source. Jobs can be retried independently with exponential backoff, and fan-in triggers (e.g., "all entities for this source are done → compile sections") compose naturally from job completion events.

## Lesson 18: Domain Drowning

**Date:** 2026-03
**Source:** Covalence production search quality

When research papers vastly outnumber spec/design docs (e.g., 148 research sources vs 14 spec docs), self-referential queries ("how does our ingestion pipeline work?") get drowned by topically related research results. The fix: DDSS (Domain-Decomposed Score Skew) compares the max score from internal-domain results against external-domain results. When the internal max is competitive (ratio >= 0.7), internal results get a 1.5x boost, surfacing the system's own documentation over third-party papers about similar topics.

## Lesson 19: Epistemic Data Lifecycle

**Date:** 2026-03
**Source:** Covalence production data management

Old source versions and orphan nodes are observations, not garbage. Never auto-delete. The instinct to "clean up" stale data destroys temporal provenance and makes it impossible to answer "what did the system believe at time T?" Manual GC with preview (showing what would be deleted and why) preserves epistemic integrity while still allowing intentional cleanup. Automated eviction should only occur through the BMR forgetting pipeline (spec 07), which has mathematical guarantees about what's safe to prune.

## Lesson 20: Silent Sidecar Failures

**Date:** 2026-03-19
**Source:** FastcorefClient API mismatch (Session 41)

HTTP sidecars (fastcoref, PDF converter, future extractors) can silently fail when their API contract drifts from the client. The FastcorefClient was sending `{"texts": [...]}` but the sidecar expected `{"text": "..."}` — every coref call failed silently for weeks because errors were caught and warned but processing continued without coref. The fix is two-fold: (1) validate backends at startup by sending a test request and verifying the response parses correctly, and (2) if a sidecar URL was *explicitly configured* via environment variable, crash the engine on validation failure (fail-fast) so the orchestrator knows it's broken. Auto-derived URLs degrade gracefully. Every new sidecar integration must include a `validate()` method called at engine startup.

## Lesson 22: No Network I/O Inside Database Transactions

**Date:** 2026-03-19
**Source:** Gemini SRE review of entity resolution under fan-out concurrency (Session 41)

Never hold a database transaction or advisory lock while waiting for network I/O (API calls, embedding requests, sidecar calls). Under concurrency, every worker grabs a PG connection, locks a row, and sleeps waiting for the network — exhausting the connection pool. The fix: resolve externally first, then lock-check-write in a short transaction. The transaction should only contain fast database operations.

## Lesson 21: Incremental Flushing for Unbounded Collections

**Date:** 2026-03-19
**Source:** Gemini SRE review of coref ledger (Session 41)

Never accumulate unbounded data in memory when it can be flushed incrementally. The neural coref stage collected all byte-offset mutations for an entire document into a single `Vec`, then flushed to Postgres in one `batch_create` at the end. For large documents with thousands of pronoun mutations, this risks OOM and exceeds Postgres' 65,535 parameter limit per query. The fix: flush ledger entries per-chunk rather than per-document. This pattern applies anywhere a pipeline stage accumulates results — flush at natural batch boundaries rather than holding everything until the end.

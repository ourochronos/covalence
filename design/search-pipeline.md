# Design: Search Pipeline

## Status: partial (vector dimension blocked on index alignment; reranker now active)

> **Updated 2026-03-10 (+post)**: RRF score normalization tuned (#50); entity nodes filtered from
> results (#48). Vector search dimension currently not firing due to Voyage index dimension
> alignment mismatch — lexical dimension is the only active retrieval path right now.
> Voyage reranker (`HttpReranker`, `rerank-2.5`) is now **auto-activated** when `VOYAGE_API_KEY`
> is present — no longer requires manual wiring.

## Spec Sections: 06-search.md, 03-storage.md, 08-api.md

## Architecture Overview

The search pipeline implements 5-dimensional fused retrieval: vector similarity, lexical BM25, graph traversal, structural hierarchy, and temporal relevance. Results are fused via Reciprocal Rank Fusion (RRF), optionally reranked, and gated by an abstention check.

## Implemented Components

### Fully Implemented ✅

| Component | File | Notes |
|-----------|------|-------|
| **5 search dimensions** | `search/dimensions/{vector,lexical,graph,structural,temporal}.rs` | All wired; only lexical currently returning results (see Gaps) |
| **RRF fusion** | `search/fusion.rs` | `rrf_fuse()` with per-dimension weights and configurable k parameter |
| **RRF score normalization** | `search/fusion.rs` | **NEW (#50)**: Scores tuned and normalized — prevents one dimension dominating unfairly |
| **Entity node filtering** | `search/results.rs` | **NEW (#48)**: Entity nodes excluded from search result sets — only articles/chunks surface |
| **Abstention / insufficient context** | `search/abstention.rs` | Score-based gate + min results check. Threshold fixed in #25 |
| **Search strategies** | `search/strategy.rs` | Balanced, Precise, Exploratory, Graph — each with different dimension weights |
| **SkewRoute** | `search/skewroute.rs` | Gini coefficient on vector scores → auto-selects search strategy |
| **Spreading activation** | `search/expansion.rs` | Graph-based query expansion, activation propagates with hop decay |
| **Query expansion** | `search/expansion.rs` | `expand_query()` uses graph to find related entities |
| **Semantic query cache** | `search/cache.rs` | Cache lookup/store by query embedding similarity |
| **Search trace** | `search/trace.rs` | Per-query diagnostic: which dimensions fired, latencies, strategy used |
| **Context assembly** | `search/context.rs` | Assembles search results into LLM-ready context with source attribution |
| **Global dimension** | `search/dimensions/global.rs` | PageRank-like global importance (6th dimension, weight configurable) |

### Partially Implemented 🟡

| Component | Status | Gap |
|-----------|--------|-----|
| **Vector search dimension** | Wired but not firing | Voyage index dimension mismatch — embeddings stored at one dimension count, query vector at another. Needs index rebuild after Voyage migration is fully settled. |
| **Reranking** | Auto-activated with Voyage | `HttpReranker` with Voyage `rerank-2.5` is automatically wired when `VOYAGE_API_KEY` is present (and `COVALENCE_EMBED_PROVIDER=voyage` or Voyage key detected). Falls back to `NoopReranker` when no key. No longer requires manual activation. |
| **Topological confidence scoring** | Partial | Graph dimension uses structural metrics but doesn't do iterative trust propagation (EigenTrust/TrustRank-style). |
| **Semantic cache invalidation** | Partial | Cache has TTL but no invalidation on source ingestion/update. |
| **Multi-granularity search** | Partial | Searches chunks, nodes, and sources — but no article-level search yet. |

### Not Implemented ❌

| Component | Spec Reference | Priority |
|-----------|---------------|----------|
| **Bi-temporal point-in-time queries** | Spec 06: "query timepoint", `valid_from`/`valid_until` filtering | High — temporal dimension exists but doesn't filter by user-specified timepoint |
| **Search feedback loop** | Spec 08: `POST /search/feedback` | Medium — API endpoint defined, no learning wired |
| **HyDE (Hypothetical Document Embeddings)** | Spec 06 references query expansion via LLM | Medium — `expand_query()` uses graph only, not LLM-generated hypothetical docs |
| **Adaptive strategy selection** | Spec 06: learn optimal strategy from feedback | Low — SkewRoute is heuristic, not learned |
| **Cross-document novelty** | Spec 08: "cross-document novelty stats" | Low |
| **Result deduplication** | Spec 06: "deduplicate" across dimensions | Low — RRF naturally handles some overlap but no explicit dedup |

## Key Design Decisions

### Why RRF over learned fusion
RRF is rank-based, not score-based — immune to score distribution differences across dimensions. No training data needed. The spec explored Bayesian fusion but RRF's simplicity and robustness won. Academic backing: Cormack, Clarke, & Butt 2009.

### Why entity nodes filtered from results (#48)
Entity nodes (person, concept, organization, etc.) are structural graph elements, not retrievable documents. Returning them in search results produced confusing snippets like "Node: Rust (concept)" with no useful content. Articles and chunks are the right retrieval unit; entities are the connective tissue.

### Why RRF normalization matters (#50)
Without normalization, the dimension with the highest raw score count (typically lexical BM25 on large corpora) dominated the fused ranking regardless of dimension weights. Normalizing before fusion makes the configured weights actually meaningful.

### Why SkewRoute for strategy selection
If vector scores are highly skewed (high Gini), the query matches a specific cluster → use Precise strategy (heavy lexical). If scores are uniform, the query is exploratory → use Exploratory strategy (heavy vector + graph). Zero-cost: computed from vector scores that are already available.

### Why spreading activation over embedding-only graph search
Graph structure captures relationships that embedding similarity misses. "PageRank uses eigenvector computation" is a structural fact, not a semantic similarity. Spreading activation (Collins & Loftus 1975) naturally propagates relevance through typed edges with decay — matching the cognitive model of semantic memory.

### Why abstention matters
A system that confidently returns irrelevant results is worse than one that says "I don't know." Abstention threshold calibrated to RRF score distribution (natural range 0.002-0.005 per #25). Coverage check ensures minimum result diversity.

## Gaps Identified

1. **Vector dimension not firing** — the single most impactful gap right now. After the Voyage
   migration, the pgvector index was built with a different dimension count than the query vectors
   being issued. Requires a migration to rebuild the ivfflat/hnsw index with the correct Voyage
   dimension. Until fixed, search relies entirely on lexical BM25.

2. **Topological confidence scoring** (14 spec mentions, 0 paper grounding before TrustRank/EigenTrust
   ingestion) — the iterative propagation algorithm isn't implemented. Current graph dimension uses
   local metrics only.

3. **Semantic Query Cache** references GPTCache-style similarity lookup, now grounded by GPTCache
   paper — but invalidation on source mutation isn't wired.

4. **direction_confidence** (12 mentions) — spec describes directional edge confidence but search
   doesn't use it for traversal weighting.

5. **STAR-RAG** (10 mentions) — appears to be a Covalence-specific term. Not defined in any paper.
   Needs spec clarification: is this the 5D fusion architecture's name?

## Academic Foundations

| Concept | Paper | Status |
|---------|-------|--------|
| RRF fusion | Cormack et al. 2009 | ✅ Ingested |
| HNSW vector search | Malkov & Yashunin 2018 | ✅ Ingested |
| BM25 lexical | Robertson et al. 1994 | ✅ Ingested |
| Spreading activation | Collins & Loftus 1975 | ✅ Ingested |
| Trust propagation | Gyöngyi 2004, Kamvar 2003 | ✅ Ingested |
| Semantic caching | GPTCache 2023 | ✅ Ingested |
| HyDE query expansion | Gao et al. 2022 | ✅ Ingested |
| Bi-temporal | Snodgrass & Ahn 1986 | ✅ Ingested |
| Information foraging | Pirolli & Card 1999 | ✅ Ingested |
| Reranking | Voyage rerank-2.5 | ✅ Ingested (model paper) |
| Late chunking | Günther et al. 2024 | ✅ Ingested |
| HippoRAG | Hu et al. 2024 | ✅ Ingested |

## Next Actions

1. **Rebuild Voyage vector index** — fix dimension mismatch to re-enable vector search dimension
2. Implement bi-temporal query filtering — `valid_from`/`valid_until` WHERE clause on temporal dimension
3. Define STAR-RAG in spec or remove references
4. Add iterative trust propagation (EigenTrust-style) to graph dimension
5. Wire cache invalidation on source mutation

> **Done**: Voyage reranker (`HttpReranker` with `rerank-2.5`) is now auto-activated when
> `VOYAGE_API_KEY` is present — no longer requires a config change.

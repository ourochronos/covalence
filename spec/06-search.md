# 06 — Search & Retrieval

**Status:** Draft

## Overview

Search composes multiple retrieval dimensions via Reciprocal Rank Fusion (RRF). Each dimension runs independently, producing a ranked list. RRF merges them without requiring score normalization across dimensions.

### Lineage: SING Pattern

This multi-dimensional search architecture is derived from the SING (Search IN Graph) pattern, originally implemented in [postgresql-singularity](https://github.com/zonk1024/postgresql-singularity). Key concepts carried forward:

- **`DimensionAdaptor` trait** — Each search dimension implements a common interface, producing normalized scored results independently. In this system, that's the `SearchDimension` trait.
- **`ResultFusion`** — Parallel execution of dimensions with RRF-based score fusion. No dimension needs to know about any other dimension's scoring semantics.
- **Normalized scoring** — Each dimension returns ranks, not raw scores. RRF operates on ranks, making cross-dimension fusion mathematically clean without score normalization.

The architectural insight: search quality comes from *composing independent signals*, not from building one perfect retrieval mechanism.

## Reciprocal Rank Fusion (RRF)

```
RRF_score(d) = Σ  weight_i / (K + rank_i(d))
               i∈dimensions
```

Where:
- `K = 60` (standard constant, proven in valence)
- `rank_i(d)` = rank of document `d` in dimension `i` (1-indexed, ∞ if absent)
- `weight_i` = per-dimension weight (configurable per query strategy)

**v2 consideration: SRRF (Soft Reciprocal Rank Fusion).** Standard RRF discards score distribution information by converting scores to ranks. SRRF (Kuzi et al., 2024 — "An Analysis of Fusion Functions for Hybrid Retrieval") replaces the indicator function inside the rank computation with a sigmoid, preserving score magnitude information. With an appropriate β parameter, SRRF outperforms RRF. However, for v1 with 5+ heterogeneous dimensions (vector, graph, keyword, temporal, structural), RRF's zero-shot rank-only property is a strength — no score normalization needed across radically different scoring functions. SRRF is most beneficial for 2-dimension hybrid search (dense + sparse). Evaluate for v2 when we have retrieval quality benchmarks to compare against.

## Search Dimensions

### 1. Vector Search

Semantic similarity via pgvector HNSW index. Searches across **all chunk hierarchy levels simultaneously** — sentence, paragraph, section, and document — letting RRF fusion determine which granularity is most relevant for a given query. This is the RAPTOR insight: different queries are best served by different levels of the hierarchy. A specific factual question ("when was X founded?") may match a sentence chunk best, while a thematic question ("what are the key themes in this document?") matches a section or document chunk.

```sql
-- Query all levels simultaneously, RRF handles granularity selection
SELECT c.id, c.content, c.level, c.embedding <=> $query_embedding AS distance
FROM chunks c
WHERE c.embedding IS NOT NULL
ORDER BY c.embedding <=> $query_embedding
LIMIT $k;
```

Optionally filter to specific levels when the query intent is clear:

```sql
-- For targeted queries, restrict to specific levels
SELECT c.id, c.content, c.embedding <=> $query_embedding AS distance
FROM chunks c
WHERE c.level = ANY($levels)  -- e.g., ['paragraph', 'sentence']
ORDER BY c.embedding <=> $query_embedding
LIMIT $k;
```

**Features:**
- **Multi-level retrieval** — Search sentence, paragraph, section, and document chunks in a single query. Short queries tend to match sentence/paragraph level; broad queries match section/document level. RRF fusion naturally selects the right granularity.
- Best-per-node dedup: if multiple chunks from the same source match, keep the best
- Also searches `nodes.embedding` for entity-level matches — entities whose embeddings are close to the query get boosted in the graph dimension as well
- Returns ranked list by cosine distance
- **Parent context injection** — For chunks with low `parent_alignment` (from landscape analysis), automatically include the parent chunk's content in the retrieval result. This addresses the "orphan chunk" problem where a chunk is meaningless without its parent context.

### 2. Lexical Search

Full-text search via PostgreSQL tsvector with fallback chain (from covalence):

```
1. websearch_to_tsquery($query)  -- handles quoted phrases, boolean operators
2. plainto_tsquery($query)       -- fallback if websearch fails
3. ILIKE '%' || $query || '%'    -- last resort prefix scan
```

```sql
SELECT c.id, c.content, ts_rank_cd(c.content_tsv, query) AS rank
FROM chunks c, websearch_to_tsquery('english', $query) query
WHERE c.content_tsv @@ query
ORDER BY rank DESC
LIMIT $k;
```

**Features:**
- Handles exact phrase matching that vectors miss
- Trigram similarity on node names for fuzzy entity lookup
- Complements vector search — high precision for specific terms

### 3. Temporal Search

Rank by temporal relevance. Three modes:

**Recency** — Prefer recently created/ingested content:
```sql
SELECT s.id, 1.0 / (1.0 + EXTRACT(EPOCH FROM (now() - s.ingested_at)) / 86400) AS freshness
FROM sources s
ORDER BY freshness DESC;
```

**Range** — Find content relevant to a specific time period:
```sql
SELECT s.id FROM sources s
WHERE s.created_date BETWEEN $start AND $end;
```

**Point-in-time (bi-temporal)** — Find what was true at a specific time, leveraging bi-temporal edges (Graphiti/STAR-RAG pattern):
```sql
-- Active edges at time T: valid at T AND not yet invalidated at T
SELECT e.* FROM edges e
WHERE (e.valid_from IS NULL OR e.valid_from <= $T)
  AND (e.valid_until IS NULL OR e.valid_until > $T)
  AND (e.invalid_at IS NULL OR e.invalid_at > $T);

-- Current truth: only non-invalidated edges
SELECT e.* FROM edges e
WHERE e.invalid_at IS NULL;
```

This enables temporal reasoning chains: "What changed between T1 and T2?" = edges invalidated between T1 and T2, plus edges created between T1 and T2.

**Features:**
- Novelty boost: recently ingested content gets a configurable multiplier (1.5x, decaying over 48h, from valence)
- Temporal weight presets: `default`, `prefer_recent`, `prefer_stable`
- STAR-RAG insight: temporal proximity in graph traversal (PPR seeds weighted by temporal distance to query timepoint) reduces candidate set by 97% on temporal KG benchmarks while improving accuracy by 9.1%

### 4. Graph Traversal

Find related content by walking the graph from seed nodes.

```
1. Identify seed nodes from query (via vector/lexical match on node names)
2. Run PersonalizedPageRank (PPR) seeded from matched nodes
3. Fall back to BFS/DFS with hop-decay if PPR is unavailable
4. Score by: PPR score (or hop_distance × edge_weight × topological_confidence)
```

**PersonalizedPageRank (preferred):** PPR propagates relevance from seed nodes through the graph, naturally decaying with distance but following high-weight edges. Unlike BFS with fixed hop-decay, PPR discovers relevant nodes that are structurally distant but well-connected to the query context. This is the HippoRAG 2 insight: associative recall through a knowledge graph mirrors how hippocampal indexing works in biological memory.

```rust
/// Seed PPR from the query's nearest graph nodes (found by vector search on node embeddings)
fn personalized_pagerank(
    graph: &StableGraph<Node, Edge>,
    seed_nodes: &[NodeIndex],
    damping: f64,     // default 0.85
    iterations: usize, // default 20
    tolerance: f64,    // default 1e-6
) -> HashMap<NodeIndex, f64>
```

**Hop-decay (fallback):** `score(node, hops) = base_score * decay^hops` where `decay = 0.7` (from covalence)

**Edge-type filtering:** Queries can specify which relationship types to traverse (e.g., only `CAUSED_BY` for causal queries). PPR respects edge-type filters by zeroing out transition probabilities on excluded edge types.

**Features:**
- Answers multi-hop questions that vector search alone cannot
- PPR naturally handles the simple-vs-complex query spectrum — simple queries stay near seed nodes, complex queries propagate further through the graph
- Leverages the petgraph sidecar for fast traversal
- Falls back to PG stored procedure (`graph_traverse`) if sidecar is unavailable
- EA-GraphRAG (Zhang et al., Feb 2026) showed GraphRAG underperforms vanilla RAG on simple queries by 13.4%. Our approach avoids this because PPR results are just one dimension in RRF fusion — they don't override vector results for simple queries, they enhance them for complex ones

### 5. Structural Search

Search by graph topology metrics without semantic content.

- **High centrality nodes** — Entities that are well-connected (high PageRank, high betweenness)
- **Community membership** — Nodes in the same community as seed nodes
- **Structural similarity** — Nodes with similar graph neighborhoods

**Use case:** "What are the most important concepts in this knowledge base?" doesn't need vector search — it needs graph centrality.

### 6. Community Summary Search (Global)

Search community summaries for thematic/global queries. This handles questions that can't be answered by any single chunk — they require synthesizing across the entire corpus.

**When to use:** Broad queries about themes, trends, overviews ("What are the key risks?", "Summarize the main topics").

**Flow:**
1. Embed query → vector search against community summary embeddings
2. Top-k summaries retrieved (k=5, configurable)
3. Map phase: each summary + query → LLM generates partial answer with citations
4. Reduce phase: partial answers → LLM merges into coherent final answer
5. Citations trace back through community → entity → chunk → source

**Integration with RRF:** Community summaries participate in fusion as a 6th dimension. For global queries, the `global` strategy weights this dimension heavily. For specific queries, the weight is near-zero.

**Incremental update:** When communities are re-detected after graph changes, only regenerate summaries for communities whose membership changed by > 20%. Cache summaries and their embeddings.

## Query Strategies

Pre-configured weight profiles for common query types:

| Strategy | Vector | Lexical | Temporal | Graph | Structural | Global |
|----------|--------|---------|----------|-------|------------|--------|
| Balanced | 0.28 | 0.22 | 0.13 | 0.18 | 0.09 | 0.10 |
| Precise | 0.38 | 0.33 | 0.05 | 0.14 | 0.05 | 0.05 |
| Exploratory | 0.18 | 0.09 | 0.09 | 0.32 | 0.22 | 0.10 |
| Recent | 0.23 | 0.18 | 0.33 | 0.14 | 0.05 | 0.07 |
| Graph-first | 0.14 | 0.09 | 0.05 | 0.42 | 0.22 | 0.08 |
| Global | 0.10 | 0.05 | 0.05 | 0.10 | 0.10 | 0.60 |

Users can also provide custom weights.

## Query Flow

```
1. Parse query → identify intent, entities, time references
2. **Query expansion via graph context:**
   a. Extract entity mentions from query (NER or simple embedding match against node names)
   b. For each matched entity, retrieve 1-hop relationships from graph sidecar
   c. Append relationship context to query for embedding: "Tim Cook" → "Tim Cook (CEO of Apple, announced Vision Pro)"
   d. This enriches the query embedding with graph knowledge, improving recall for related but differently-phrased content
   e. Cost: one graph lookup per entity (~1ms). No LLM call.
3. Generate query embedding (from expanded query text)
3. Execute dimensions in parallel:
   a. Vector: query embedding → pgvector HNSW
   b. Lexical: query text → tsvector
   c. Temporal: time references → temporal filter
   d. Graph: identified entities → seed nodes → traversal
   e. Structural: centrality/community lookup
   f. Global: query embedding → community summary embeddings
4. Collect ranked lists from each dimension
5. **Adaptive strategy selection** (SkewRoute): analyze vector search score distribution
   - Compute score skewness (normalized entropy or Gini coefficient of top-20 scores)
   - High concentration (low entropy) → simple query → use `precise` weights
   - Diffuse scores (high entropy) → complex/ambiguous → use `exploratory` or `global` weights
   - Temporal references detected → boost temporal dimension → use `recent` weights
   - If user specified a strategy, skip auto-selection
6. Apply RRF fusion with selected strategy weights
7. **Rerank** top-20 fused results with Voyage rerank-2.5
8. Apply topological confidence as a multiplier on reranked scores
9. Deduplicate (keep highest-scoring instance per logical entity)
10. Return top-N results with provenance metadata and citations
```

**Adaptive strategy selection (SkewRoute pattern):**

Instead of requiring the user to choose a strategy, analyze the retrieval score distribution from the vector search dimension. This is the SkewRoute insight (Wang et al., May 2025): score distribution is a reliable proxy for query complexity without needing a trained classifier.

- **Concentrated scores** (top result >> rest): simple factual query → `precise` strategy, skip graph/global dimensions entirely (saves ~40% latency)
- **Uniform/diffuse scores** (many equally relevant results): broad/thematic query → `global` strategy, lean on community summaries
- **Bimodal scores** (two clusters): multi-faceted query → `balanced` or `exploratory`

This is zero-cost routing — the vector search runs regardless, we just analyze its score distribution before weighting the other dimensions. No trained classifier, no extra LLM call.

**Implementation (Gini coefficient of top-20 scores):**
```
scores = top_20_vector_scores (normalized to [0, 1])
gini = (2 * sum(i * sorted_scores[i] for i in 1..n)) / (n * sum(scores)) - (n + 1) / n
```
- **Gini > 0.6** → concentrated (one dominant result) → `precise`
- **Gini < 0.3** → diffuse (many equally relevant) → `global`
- **0.3 ≤ Gini ≤ 0.6** → `balanced` (default)
- **Fallback:** If vector search returns < 5 results, always use `balanced` (insufficient signal for distribution analysis)

These thresholds are starting points — tune via the evaluation harness (spec 11) against your query distribution.

## Multi-Granularity Search Targets

Search operates across three granularity levels, each serving a different purpose:

| Target | Granularity | When to Use |
|--------|------------|-------------|
| **Chunks** (sentence/paragraph) | Fine-grained | Precise factual retrieval, specific claims |
| **Articles** (200–4000 tokens) | Medium | Optimal retrieval unit — empirically validated for high faithfulness and relevancy. Compiled summaries are right-sized for both embedding quality and LLM context. |
| **Nodes** (entity-level) | Coarse | Entity lookup, graph-based navigation |

Articles are the primary retrieval target for most queries. Chunks provide sub-article precision when needed. Node search supports graph-first and structural queries.

Empirical finding (LlamaIndex eval on SEC filings): chunk size 1024 tokens optimized faithfulness and relevancy for complex documents. Articles at 200–4000 tokens fall within this validated range.

## Result Shape

```rust
struct SearchResult {
    entity_id: Uuid,
    entity_type: String,       // "chunk", "node", "article", "community_summary"
    content: String,
    score: f64,                // fused RRF score
    confidence: f64,           // composite confidence (opinion projected probability × topo)
    dimension_scores: HashMap<String, f64>,  // per-dimension breakdown
    source: SourceSummary,     // provenance
    context: Option<String>,   // parent chunk content for context injection
}
```

## Attribution and Citation

Every generated answer must trace back to specific chunks and sources. This is non-negotiable for trust.

**Citation flow:**
1. Search returns `SearchResult` with `entity_id` + `source: SourceSummary`
2. Generation prompt includes results as numbered references: `[1]`, `[2]`, etc.
3. LLM is instructed to cite inline: "The company was founded in 2020 [1]."
4. Response includes a `citations` array mapping `[N]` → `{chunk_id, source_id, source_title, source_url}`
5. For community summary results, citations trace through: community → entity nodes → chunks → sources

**Generation prompt structure:**
```json
{
  "system": "Answer the question using ONLY the provided context. Cite each claim with [N] referencing the context item number. If the context doesn't contain enough information, say so.",
  "user": {
    "question": "{query}",
    "context": [
      {"id": 1, "content": "{result_1.content}", "source": "{result_1.source.title}"},
      {"id": 2, "content": "{result_2.content}", "source": "{result_2.source.title}"}
    ]
  }
}
```

**Response schema:**
```json
{
  "answer": "The company was founded in 2020 [1] and acquired by BigCorp in 2023 [2].",
  "citations": [
    {"ref": 1, "chunk_id": "...", "source_id": "...", "source_title": "...", "confidence": 0.92},
    {"ref": 2, "chunk_id": "...", "source_id": "...", "source_title": "...", "confidence": 0.87}
  ],
  "confidence": 0.89,
  "strategy_used": "balanced"
}
```

## Abstention (Insufficient Context Detection)

RAG paradoxically reduces a model's ability to abstain when context is insufficient (Google, arXiv:2411.06037). Adding retrieval context makes models *more confident* even when that context doesn't actually answer the question — leading to more hallucination, not less.

**Mitigation: Explicit context sufficiency check.**

After context assembly but before generation, evaluate whether the assembled context is sufficient to answer the query:

1. **Score-based gate:** If the top reranked result's score is below a threshold (configurable, default: `min_relevance_score: 0.3`), flag as potentially insufficient.
2. **Coverage check:** The generation prompt includes an explicit instruction to assess sufficiency:
   ```json
   {
     "system": "Answer the question using ONLY the provided context. If the context does not contain enough information to answer the question, respond with {\"answer\": null, \"abstention_reason\": \"...\", \"confidence\": 0.0}. Do NOT guess or use information not in the context.",
     "user": { "question": "...", "context": ["..."] }
   }
   ```
3. **Response routing:** When `answer: null`, the system returns an explicit "I don't have enough information to answer this" with:
   - The abstention reason (what's missing)
   - The best partial context it did find (so the user can see what's available)
   - Suggested alternative queries that might find relevant information

**Why this matters:** Without abstention, the system hallucinates confidently with citations that don't actually support the claim. With abstention, the user knows to look elsewhere or rephrase. This is the difference between a trustworthy system and a confidently wrong one.

## Semantic Cache

Before executing the full search pipeline, check if a semantically similar query was recently answered. This avoids redundant embedding calls, retrieval, and LLM generation for repeated or rephrased questions.

**Implementation:**
1. Embed the incoming query (this happens anyway for vector search).
2. Search a `query_cache` table: `SELECT * FROM query_cache WHERE embedding <=> $query_embedding < 0.05 AND created_at > now() - interval '1 hour' ORDER BY embedding <=> $query_embedding LIMIT 1`.
3. If cache hit (cosine distance < 0.05, configurable): return cached response immediately. Log as cache hit in trace.
4. If cache miss: execute full pipeline, store result in `query_cache` with the query embedding, response, and TTL.

**Cache invalidation:**
- TTL-based: cache entries expire after 1 hour (configurable). Short TTL because the graph evolves with ingestion.
- Ingestion-triggered: when new sources are ingested, invalidate cache entries whose query embeddings are near (cosine < 0.15) the embeddings of newly ingested chunks. This prevents stale answers for topics that just got new data.
- Manual: `DELETE /cache` API endpoint for full flush.

**Storage:** `query_cache` table with `embedding halfvec(2048)`, `query_text`, `response JSONB`, `created_at`, `hit_count`. HNSW index on embedding. Bounded to 10K entries (LRU eviction).

**Expected impact:** 50-65x latency reduction for cache hits (skip retrieval + generation). 70-90% hit rate for production workloads with repeated query patterns (support desks, FAQ-style usage).

## Parent-Child Context Injection

When a sentence-level chunk matches:
1. Retrieve its parent (paragraph or section)
2. Include parent content in the result's `context` field
3. The synthesis step uses both the precise match and the surrounding context

This prevents the "orphan chunk" problem where a highly specific match lacks the context needed for a useful answer.

## Context Assembly

After retrieval and reranking, the raw result set must be assembled into a coherent context window for generation. This is the "context engineering" step — optimizing what goes into the LLM's input.

**Steps:**
1. **Deduplicate** — Merge near-duplicate results (embedding cosine > 0.95). Keep the higher-scoring instance, combine citations.
2. **Diversify** — If top-N results are all from the same source, inject results from other sources to avoid single-source bias. Max 3 results per source unless query is source-specific.
3. **Expand** — For each chunk result, attach parent context (see Parent-Child Context Injection above). For node results, attach a 1-hop relationship summary.
4. **Order** — Sort context items by relevance (highest first), but group items from the same document together to preserve narrative flow.
5. **Budget** — Hard cap on total context tokens (default: 8K tokens for context, leaving room for system prompt + answer). If over budget, drop lowest-scoring items first. Never truncate individual items — either include fully or drop.
6. **Annotate** — Each context item gets a reference number `[N]` and source attribution for citation tracking.

## Query Expansion (Optional)

**Personalized PageRank expansion:**
1. Take top-k results from initial search
2. Map to graph nodes
3. Run PPR from those nodes
4. High-PPR nodes become additional candidates
5. Re-rank with RRF

**Spreading activation:**
1. Seed nodes from initial results
2. Activate neighbors with decay
3. Nodes above activation threshold join the result set

Both are opt-in and controlled by the query strategy.

## Confidence Integration

Search results incorporate confidence from the epistemic model:

1. **Pre-filter** — Optionally exclude results below a minimum confidence (`min_confidence` query parameter)
2. **Score modifier** — Composite confidence acts as a multiplier on the fused RRF score:
   ```
   final_score = rrf_score * (1 + γ * (composite_confidence - 0.5))
   ```
3. **Explainability** — Each result includes its `confidence_breakdown`, allowing the consumer to understand why a result is (or isn't) trusted

## Information Foraging Navigation

For orientation and exploration queries, the system provides a three-layer navigation hierarchy:

1. **Topology Map** — Global meta-article describing all top-level domains, article counts, connectivity. Eliminates cold-start waste.
2. **Domain Landmarks** — One landmark article per community (highest betweenness centrality). Starting from a landmark guarantees fast orientation.
3. **Cross-Domain Bridges** — Articles spanning multiple domains. Reduce navigation cost between knowledge patches.

These are computed during deep consolidation and cached. See [04-graph](04-graph.md#landmark-detection).

## Open Questions

- [x] **Multi-turn conversation** → v1 search is stateless by design. The search API accepts a single query and returns results. Multi-turn context (coreference resolution, topic continuity) is the caller's responsibility. Recommended pattern: caller rewrites follow-up queries to be self-contained before calling search. E.g., conversation "Tell me about Tim Cook" → "What did he announce?" should be rewritten to "What did Tim Cook announce?" before hitting the search API. This keeps the search layer simple and testable. Session-aware search (automatic conversation tracking, query rewriting) is a v2 consideration.
- [x] Query parser → Rule-based for v1 (regex for temporal refs, trigram for entity detection). LLM-assisted as v2 optimization.
- [x] Cold start → Fall back to topology map + landmark navigation. Global meta-article describes all domains.
- [x] Confidence floor → Configurable per query via `min_confidence` parameter, default off.
- [x] Search quality evaluation → RAGAS framework (Faithfulness, Context Precision, Context Recall, Answer Relevancy) + Cranfield-style harness + NDCG@K/MRR for retrieval quality. See Covalence sources: RAGAS docs, Observability for Knowledge Systems, Building a Cranfield-Style Evaluation Harness.
- [x] Cross-encoder reranking → v2 optimization. **Voyage rerank-2.5** ($0.05/M tokens, first 200M free) as v1 reranker since we're already using Voyage for embeddings — single vendor, consistent quality. BGE-M3 built-in ColBERT mode as local alternative. Rerank top-20 RRF results before returning top-N. Reranking on top of bad retrieval is lipstick on a pig — reranking on top of good multi-dimensional RRF fusion is where you see the 10-15% recall improvement.
- [x] Articles as default target → Yes. Articles are primary retrieval unit. Chunks for precision drill-down. Nodes for graph-first/structural queries.

# SING → Covalence Translation Guide

> **Analysis Date**: 2026-02-28  
> **Source**: `/tmp/pg-sing-z` — the postgresql-singularity (SING) codebase  
> **Analyst**: Design Analysis Agent  
> **Purpose**: Extract patterns and lessons from SING to inform Covalence architecture

---

## Executive Summary

SING is a PostgreSQL *extension* that adds a unified multi-dimensional search API (semantic, graph, spatial, temporal, lexical) on top of existing PG extensions (pgvector, AGE, PostGIS, TimescaleDB). Covalence is a *Rust application layer* that happens to use the same backend extension stack (AGE + PGVector + pg_textsearch). The core problem SING solves — orchestrating multiple PG capabilities into a coherent, scored, fused search experience — is **exactly** the same problem Covalence's query engine must solve.

The architectural gap is: SING does this *inside* PostgreSQL via `pgrx`, Covalence does this *outside* PostgreSQL via async Rust calling SQL over a connection pool. This changes deployment, latency, and some implementation details, but the core logic — adaptor pattern, cost-based query planner, score normalization, result fusion — translates almost verbatim.

---

## Section 1: What SING Got Right (Preserve This)

### 1.1 The DimensionAdaptor Trait

SING's crown jewel. Every search capability (vector, graph, spatial, temporal, lexical) implements a single trait:

```rust
pub trait DimensionAdaptor: Send + Sync {
    fn name(&self) -> &'static str;
    fn check_availability(&self) -> bool;
    fn backend_name(&self) -> Option<String>;
    fn search(
        &self,
        query: &DimensionQuery,
        candidates: Option<&[Uuid]>,  // ← THE KEY INSIGHT: pre-filter from prior dims
        limit: usize,
    ) -> Result<Vec<DimensionResult>, SingError>;
    fn normalize_scores(&self, results: &mut [DimensionResult]);
    fn estimate_selectivity(&self, query: &DimensionQuery) -> f64;
}
```

**Why this is brilliant:**
- `candidates: Option<&[Uuid]>` enables *cascading pre-filtering*: the most selective dimension runs first and passes its result set as an allowlist to subsequent dimensions. This is what prevents O(n²) join explosions (demonstrated painfully in SING's own benchmarks at 1M entities).
- `normalize_scores` is a first-class trait method — each adaptor knows its own scoring semantics. Vector distance (lower=better) inverts differently from BM25 relevance (higher=better) or graph centrality.
- `estimate_selectivity` powers the query planner without requiring runtime statistics.

**Translation to Covalence:**
```rust
// covalence-engine/src/search/adaptor.rs
#[async_trait]
pub trait DimensionAdaptor: Send + Sync {
    fn name(&self) -> &'static str;
    fn check_availability(&self) -> bool;
    
    async fn search(
        &self,
        pool: &PgPool,
        query: &DimensionQuery,
        candidates: Option<&[Uuid]>,
        limit: usize,
    ) -> Result<Vec<DimensionResult>>;
    
    fn normalize_scores(&self, results: &mut [DimensionResult]);
    fn estimate_selectivity(&self, query: &DimensionQuery) -> f64;
}
```

The only change: `pool: &PgPool` parameter instead of in-process Spi calls. Everything else is identical in spirit.

---

### 1.2 The QueryPlanner with Cost Model

SING's `QueryPlanner` assigns static base costs and selectivities to each dimension type:

```rust
// From SING's search/planner.rs
dimension_costs.insert("semantic", DimensionCost {
    base_cost: 10.0,   // HNSW makes it fast
    selectivity: 0.1,  // Returns ~10% of corpus
    index_available: true,
    parallelizable: true,
});
dimension_costs.insert("structural", DimensionCost {
    base_cost: 50.0,   // Graph traversal can be expensive
    selectivity: 0.05, // Very selective when anchored to a node
    index_available: false,
    parallelizable: false,  // Sequential traversal
});
```

The planner then sorts dimensions by `base_cost * selectivity / weight` — cheapest and most selective first — and groups parallelizable ones for concurrent execution.

**Why this matters for Covalence:**
- The benchmark data from SING validates the cost model:
  - 10K entities: spatial+temporal=14.5ms, vector=2ms, combined=27.6ms
  - 1M entities: spatial+temporal=192ms, vector=490ms (no HNSW!), combined=1,205ms
- Combined queries explode because there's no pruning. The cascade-with-candidates pattern is the fix.
- For Covalence's AI memory use case (likely <100K nodes per agent workspace), HNSW will stay fast; graph traversal is the expensive one and should run on pruned candidate sets.

**Translation to Covalence:**
The planner is pure application logic — translate directly. Use `tokio::join!` for parallel dimensions instead of PostgreSQL's parallel query framework.

---

### 1.3 Score Normalization → Weighted Fusion

SING's `ResultFusion` implements weighted mean across normalized per-dimension scores, collecting only the unique entities that appear in any dimension result:

```rust
// Normalize each dimension's raw scores to [0, 1]
adaptor.normalize_scores(&mut results);

// Then fuse: weighted mean over whichever dims touched each entity
let composite = weighted_sum / weight_sum;  // only active weights count
```

The `DimensionWeights::normalize()` pattern (dividing each weight by the sum) ensures weights are relative even if the user passes `{semantic: 0.5, lexical: 0.1}` vs `{semantic: 5.0, lexical: 1.0}`.

**The `DimensionalScores` struct (with `Option<f32>` per dimension) is the right data model:**
```rust
pub struct DimensionalScores {
    pub semantic: Option<f32>,   // None = dimension not in this query
    pub structural: Option<f32>,
    pub temporal: Option<f32>,
    pub lexical: Option<f32>,
}
```
`Option` is load-bearing: it distinguishes "not in this query" from "searched but scored 0."

**Translation:** Adopt this verbatim. Expose `dimensional_scores` in Covalence's API response — agents need to see *why* a node scored well, not just the composite score.

---

### 1.4 Graceful Backend Detection

SING's `VectorAdaptor` auto-detects whether Lantern or pgvector is installed:

```rust
fn detect_backend() -> VectorBackend {
    // Check Lantern first (preferred for speed)
    if extension_exists("lantern") { return VectorBackend::Lantern; }
    if extension_exists("vector")  { return VectorBackend::PgVector; }
    VectorBackend::None
}
```

For Covalence, this pattern should be applied at startup to verify all required extensions (AGE, pgvector, pg_trgm) are installed and emit clear errors if not. The adaptor's `check_availability()` becomes a startup health check, not a per-query check.

---

### 1.5 Small Data Optimizations

SING identified key patterns for high-volume small entities (messages, events, logs):
- **Sparse storage**: only populate columns for active dimensions — `vector_data IS NULL` for entities without embeddings; partial indexes skip null columns automatically.
- **Time-series partitioning**: monthly partitions for temporal data enable fast insert and efficient age-out.
- **Covering indexes**: `CREATE INDEX ON entities (id) INCLUDE (vector_data, entity_data)` eliminates heap lookups for common access patterns.

**Translation to Covalence's memory model:** Articles and sources in Covalence are small-to-medium entities. The `WHERE embedding IS NOT NULL` partial index pattern directly applies — not all knowledge nodes need embeddings (e.g., raw source metadata records).

---

## Section 2: What Changes in the Application Layer

### 2.1 No In-Process SQL (Spi) — Use Async Pool

SING uses `pgrx::Spi::connect()` to run SQL in-process with zero-copy access to PG memory. Covalence uses `sqlx` or `deadpool-postgres` over a connection pool.

**Implications:**
- Latency: ~1–5ms added per query for network round-trip (even loopback). Design query plans to minimize round-trips, not just query cost.
- Parallelism: SING uses PG's internal parallel workers; Covalence uses `tokio::join!`. Both achieve the same effect, but Covalence's parallel execution is explicit application code.
- Memory: SING can pass large `float4[]` via PG datums; Covalence serializes vectors through the wire protocol. For 1536-dim embeddings, this is ~6KB per vector — negligible, but batch operations should send vectors once, not per-row.

**Recommended pattern:**
```rust
// Run parallelizable dimensions concurrently via tokio
let (vector_results, lexical_results) = tokio::try_join!(
    vector_adaptor.search(&pool, &query.semantic, None, limit * 3),
    lexical_adaptor.search(&pool, &query.lexical, None, limit * 3),
)?;

// Then run graph (non-parallelizable, sequential traversal) on the intersection
let candidates = intersect_or_union(&vector_results, &lexical_results);
let graph_results = graph_adaptor.search(&pool, &query.graph, Some(&candidates), limit).await?;
```

---

### 2.2 Extension-as-Service vs. Extension-as-Backend

SING *is* the extension. Covalence *uses* extensions. Key difference:

| Concern | SING (extension) | Covalence (application) |
|---|---|---|
| AGE graph queries | Translate JSON → Cypher, run via Spi | Build Cypher string, run via sqlx with `SET search_path` |
| pgvector | Use `<=>` operator in SPI | Use `<=>` operator in parameterized SQL |
| Schema ownership | Creates its own tables | Owns its own schema, migrations via sqlx-migrate |
| Availability check | `pg_extension` table lookup at runtime | Startup health check; fail fast if missing |
| Configuration | `ALTER SYSTEM SET sing.*` | Environment variables + config file |

**AGE-specific note:** AGE requires `SET search_path = ag_catalog, "$user", public;` before Cypher queries. With sqlx, either set this on the connection at pool creation time, or prepend it to every AGE query. The adaptor pattern makes this a one-time decision in the `GraphAdaptor`.

---

### 2.3 Result Streaming vs. TableIterator

SING returns a `TableIterator` which PostgreSQL streams row-by-row. In Covalence:
- Small result sets (<1000 rows): collect into `Vec<SearchResult>`, return as JSON.
- Large result sets: use `sqlx::query_as::<_, Row>().fetch(&pool)` with `StreamExt` to stream to the HTTP response via chunked encoding.

For the AI memory use case, most queries are top-10 to top-50 results — no streaming needed.

---

### 2.4 Query Caching Strategy

SING proposes SQL-level `query_cache` table with `md5(query::text)` as key. In Covalence:
- **Don't cache in PostgreSQL.** Cache in the application layer (in-memory LRU or Redis).
- Key: `blake3(canonical_query_json)` — hash over the normalized query structure.
- TTL: configurable per query type. Recency-weighted knowledge retrieval queries may have short TTL (1 minute); static taxonomy queries longer (1 hour).
- Invalidation: on write to any node that appeared in cached results (track via result entity IDs).

---

### 2.5 Graph Dimension — AGE Specifics

SING's `GraphAdaptor` translates a structured `GraphQuery` JSON object into Cypher. The same pattern applies, but Covalence's graph schema (AGE property graph) needs:

```rust
pub struct GraphQuery {
    pub start_node: Option<Uuid>,         // anchor node for traversal
    pub edge_labels: Option<Vec<String>>, // filter by relationship type
    pub traverse_depth: usize,            // BFS depth limit (default: 2)
    pub algorithm: GraphAlgorithm,        // BFS | ShortestPath | Neighbors
    pub direction: TraversalDirection,    // Outbound | Inbound | Both
}
```

Covalence's domain maps to AGE labels: `Article`, `Source`, `Session`, `Agent`. Edge types: `ORIGINATES`, `CONFIRMS`, `SUPERSEDES`, `CONTRADICTS`, `RELATED_TO`. The graph adaptor should understand these domain semantics for intelligent traversal defaults.

---

## Section 3: Patterns to Adopt — Specific Recommendations

### 3.1 Module Structure (translate directly)

SING's Rust module layout is well-reasoned:

```
src/
  adaptors/
    mod.rs          ← DimensionAdaptor trait definition
    vector.rs       ← pgvector backend
    graph.rs        ← AGE backend
    lexical.rs      ← pg_trgm / pg_textsearch backend
    temporal.rs     ← time-window queries (simplified for Covalence)
  search/
    planner.rs      ← QueryPlanner with cost model
    scorer.rs       ← WeightedScorer
    engine.rs       ← orchestration: plan → execute → fuse
  fusion.rs         ← ResultFusion (merge multi-dim results)
  types/
    query_types.rs  ← MultiDimQuery, DimensionWeights, etc.
```

Covalence should mirror this under `covalence-engine/src/search/`.

---

### 3.2 DimensionWeights Normalization

Always normalize weights on input, not at scoring time. SING's pattern:

```rust
impl DimensionWeights {
    pub fn normalize(&mut self) {
        let sum = self.semantic + self.structural + self.lexical + self.temporal;
        if sum > 0.0 {
            self.semantic /= sum;
            // ...
        }
    }
}
```

Apply this in `DimensionWeights::new()` / `From<ApiRequest>`. Users should be able to pass `{vector: 2, graph: 1}` or `{vector: 0.67, graph: 0.33}` and get identical behavior.

---

### 3.3 Selectivity-First Execution Order

From SING's planner:

```
effective_priority = base_cost * selectivity / weight
# Lower = run first (cheap + selective + important)
```

For Covalence's knowledge graph use case, expected selectivity ordering:
1. **Lexical** (full-text) — very selective; BM25 / ts_rank filters well
2. **Semantic** (vector ANN) — selective; HNSW fast for < 500K nodes
3. **Graph** (AGE traversal) — expensive per-query but highly selective when anchored

The planner should run lexical and semantic in parallel (both have indexes), then run graph on the intersection.

---

### 3.4 Candidate Intersection vs. Union Strategy

SING uses candidates as an intersection filter (AND semantics). For Covalence's use case:
- **AND (intersection)**: When all dimensions are required filters. E.g., "find articles that are semantically similar AND were authored in this session."
- **OR (union)**: When dimensions are alternative evidence sources. E.g., "find articles related to X by vector similarity OR graph adjacency."
- **Scored (default)**: Include all entities from any dimension; apply zero contribution for dimensions they didn't appear in.

Expose this as `fusion_mode: FusionMode` in the query API.

---

### 3.5 Partial Indexes for Covalence's Sparse Embeddings

Not all knowledge articles will have embeddings. Use SING's pattern:

```sql
-- Only index articles that have embeddings
CREATE INDEX idx_articles_embedding ON articles 
    USING hnsw (embedding vector_cosine_ops)
    WHERE embedding IS NOT NULL;

-- Only index sources with text content
CREATE INDEX idx_sources_fts ON sources
    USING gin (to_tsvector('english', content))
    WHERE content IS NOT NULL;
```

This keeps index size proportional to actual data density.

---

### 3.6 Optimization Hints in Query Response

SING's planner generates `optimization_hints`. Covalence should expose a `?explain=true` query parameter that returns:

```json
{
  "results": [...],
  "explain": {
    "plan": {
      "execution_order": ["lexical", "semantic", "graph"],
      "parallel_groups": [["lexical", "semantic"], ["graph"]],
      "estimated_cost": 42.5
    },
    "timing": {
      "lexical_ms": 3.2,
      "semantic_ms": 4.1,
      "graph_ms": 12.8,
      "fusion_ms": 0.4,
      "total_ms": 20.5
    },
    "candidate_counts": {
      "after_lexical": 124,
      "after_semantic": 41,
      "after_graph": 18
    },
    "hints": ["Graph traversal at depth 3 returned 0 results; consider depth 2"]
  }
}
```

This is invaluable for debugging agent memory quality.

---

## Section 4: Things SING Got Wrong / That Don't Apply

### 4.1 In-PostgreSQL Caching Tables

SING proposes `sing.query_cache` and `sing.hot_entities` (UNLOGGED tables) as a cache. **Don't do this in Covalence.** Caching belongs in the application layer. Putting a cache table in the same database creates write amplification and makes the DB a bottleneck for its own cache invalidation.

**Covalence approach:** `moka` (Rust in-process LRU) for per-request deduplication; consider Redis for shared cache at multi-instance scale.

---

### 4.2 AUTO-TUNING via PostgreSQL Functions

SING's `sing.auto_tune()` function adjusts `ALTER SYSTEM SET` parameters dynamically. This is:
- Dangerous (globally affects all connections)
- Impractical for managed databases (RDS, Cloud SQL, Supabase, Neon)
- Overcomplicated for the current phase

**Covalence approach:** Document recommended PG configuration as a tuning guide. Expose connection pool settings as environment variables.

---

### 4.3 The Ecosystem Vision / Marketplace

SING's `SING-ECOSYSTEM-VISION.md` describes a knowledge marketplace, automated sales AI, and protocol standard. This is aspirational product vision, not architecture. Covalence doesn't need to build toward a marketplace. Focus on the agent memory substrate.

---

### 4.4 TimescaleDB Dependency

SING treats TimescaleDB as near-required for temporal optimization. Covalence's temporal needs (recency scoring, session time windows) are simple enough to handle with native PostgreSQL's `timestamptz` columns, `BRIN` indexes (for time-ordered data), and `now() - interval` filters. Don't add TimescaleDB as a dependency.

---

### 4.5 PostGIS / Spatial

SING's spatial dimension with PostGIS geometry types doesn't apply to Covalence's knowledge substrate. Knowledge nodes don't have geographic locations. Skip the spatial adaptor entirely.

---

### 4.6 Sparse Vector Support (Gap SING Identified, Worth Noting)

SING correctly identifies sparse vectors (TF-IDF, BM25 term vectors) as a gap in the PG ecosystem. For Covalence, this matters for hybrid dense+sparse retrieval (SPLADE, BGE-M3). Start with dense vectors + BM25 full-text; add sparse vector support when quality gap is measurable.

---

## Section 5: Benchmark Lessons

### 5.1 The 1M Entity Wall

SING's benchmarks show combined multi-dimensional queries hit a wall at 1M entities:
- 10K: 27.6ms combined ✅
- 100K: 288.5ms combined (borderline)
- 1M: 1,205ms combined (unacceptable)

The root cause: **no candidate pruning**. Each dimension queries the full table independently and results are joined at the application. The cascade-with-candidates pattern is the solution — it was the core SING design intent that was never fully implemented before benchmarks were run.

For Covalence: implement cascade from day one. Agent workspaces will be 1K–500K nodes, so there's headroom, but the pattern is non-negotiable for future scale.

### 5.2 Vector Index Memory Requirements

HNSW index creation at 1M 384-dim vectors failed in SING's test due to `maintenance_work_mem` limits. At 1536-dim (text-embedding-3-large), memory requirements are 4x larger. Covalence should:
- Build HNSW indexes with `SET maintenance_work_mem = '2GB'` during index creation
- Support concurrent `CREATE INDEX CONCURRENTLY` for live data
- Monitor index build time as a deployment metric

### 5.3 Parallel Query Grouping — Latency Model

SING's grouping: semantic (parallelizable) + spatial (parallelizable) run together; graph (non-parallelizable) runs after. For Covalence: lexical + semantic run in parallel; graph runs after on the intersection.

Expected latency: `total ≈ max(lexical_ms, semantic_ms) + graph_ms + fusion_ms`  
vs. naive sequential: `total ≈ lexical_ms + semantic_ms + graph_ms + fusion_ms`

At typical workload (lexical≈3ms, semantic≈5ms, graph≈15ms, fusion≈1ms), parallel saves ~3ms — worthwhile but not dramatic at small scale. At 100K articles it matters more.

---

## Section 6: Covalence-Specific Additions Not in SING

### 6.1 Confidence-Weighted Results

Covalence articles carry a `confidence_score` (0–1). This should be a first-class multiplier in result fusion, not just another dimension:

```rust
let final_score = composite_dimensional_score * article.confidence * freshness_factor;
```

SING has no concept of source confidence — it treats all entities equally. Covalence's trust model (source reliability, mutation history, contention count) is a key differentiator in retrieval quality.

### 6.2 Session-Scoped Search

Covalence supports session-scoped memory retrieval. Add `session_filter: Option<SessionId>` to `MultiDimQuery` that the planner uses to push `WHERE session_id = $1` into all adaptors' SQL as a pre-filter. This is not a new dimension — it's a partition applied before any dimension search.

### 6.3 Recency Decay

SING's temporal dimension checks for patterns (IncreasingTrend, Periodic). Covalence needs simpler recency decay: `freshness = exp(-λ * days_since_updated)`. Apply as a scoring modifier during fusion, not a separate dimension.

```rust
let days = (now - article.updated_at).num_days() as f64;
let freshness = (-DECAY_RATE * days).exp();
let final_score = composite_score * (BASE_WEIGHT + FRESHNESS_WEIGHT * freshness);
```

### 6.4 Graph Dimension = Provenance Traversal

In SING, graph search is "find connected entities." In Covalence, graph traversal has specific semantics — traverse `ORIGINATES`, `CONFIRMS`, `SUPERSEDES`, `CONTRADICTS` edges to find related knowledge. The `GraphAdaptor` should support Covalence-specific traversal modes:
- `ProvenanceExpansion`: given an article, find all sources that contributed
- `ContradictionDetection`: find articles with `CONTRADICTS` edges to a given node
- `InfluenceGraph`: find articles that confirmed a given source

### 6.5 Explainability for Agent Debugging

Agents (and their operators) need to understand why a piece of knowledge was retrieved. The `dimensional_scores` field from SING covers the search dimension breakdown. Covalence should add:
- `provenance_chain`: top-N sources that contributed to this article
- `contention_count`: how many unresolved contradictions this article has
- `usage_score`: how often this article has been retrieved (feedback signal)

---

## Section 7: Implementation Checklist

### Phase 1 (Foundation) — Adopt from SING
- [ ] Define `DimensionAdaptor` trait (`search`, `normalize_scores`, `estimate_selectivity`)
- [ ] Implement `VectorAdaptor` (pgvector `<=>` via sqlx, partial index aware)
- [ ] Implement `LexicalAdaptor` (pg_trgm / `to_tsquery` / `ts_rank` via sqlx)
- [ ] Implement `GraphAdaptor` (AGE Cypher via sqlx with `SET search_path`)
- [ ] Implement `TemporalAdaptor` (time-window pre-filter, recency decay scorer)
- [ ] Implement `ResultFusion` with `DimensionWeights::normalize()`
- [ ] Implement `QueryPlanner` with static cost model and `tokio::try_join!` parallel grouping
- [ ] Startup health check: verify AGE, pgvector, pg_trgm extensions present

### Phase 2 (Covalence Extensions)
- [ ] Add `confidence_score` multiplier to fusion
- [ ] Add `session_filter` pre-filter to all adaptors
- [ ] Add `freshness_decay` scoring modifier
- [ ] Add `?explain=true` response with timing and candidate counts
- [ ] Add partial indexes for sparse embeddings and content
- [ ] Expose `fusion_mode: And | Or | Scored` in query API

### Phase 3 (Optimization)
- [ ] Application-layer LRU cache (`moka`) for hot queries
- [ ] Benchmark at 10K / 100K / 500K articles — validate cascade improvement
- [ ] HNSW index monitoring (size, recall quality, build time)
- [ ] Tune static cost model based on actual Covalence query profiles

---

## Quick Reference: Key Translations

| SING Concept | SING Implementation | Covalence Implementation |
|---|---|---|
| `DimensionAdaptor` trait | `pgrx`-based, Spi access | `async_trait`, `sqlx::PgPool` param |
| `QueryPlanner` | Static cost model | Same static cost model, same logic |
| `ResultFusion` | In-process HashMap merge | Same logic, Rust app layer |
| Parallel execution | PG parallel workers | `tokio::try_join!` |
| Query caching | UNLOGGED table in PG | `moka` in-process LRU |
| Vector backend detection | `pg_extension` table check | Startup health check, fail fast |
| Score normalization | Per-adaptor `normalize_scores` | Same, verbatim |
| Candidate filtering | `WHERE id = ANY($2)` | Same SQL pattern |
| `DimensionWeights` | Normalized `f32` per dim | Same struct, same normalize() |
| `DimensionalScores` | `Option<f32>` per dim | Same — `Option` semantics preserved |
| Configuration | `ALTER SYSTEM SET sing.*` | Environment variables |
| Telemetry | `performance_stats` table | OpenTelemetry spans |
| Spatial dimension | PostGIS | **Not applicable to Covalence** |
| Temporal (deep) | TimescaleDB hypertables | Native PG `timestamptz` + BRIN + recency decay |
| Extension health | `check_availability()` per query | Once at startup |
| Optimization hints | Strings in `QueryPlan` | `explain` field in API response |

---

## Appendix: SING Benchmark Numbers (Reference)

| Scale | Spatial+Temporal | Vector (HNSW) | Combined (no cascade) |
|---|---|---|---|
| 10K | 14.5ms | 2.0ms | 27.6ms |
| 100K | 18.4ms | 4.2ms | 288.5ms |
| 1M | 192.0ms | 490.8ms* | 1,205ms |

*Vector search degraded to sequential scan at 1M due to HNSW index creation failure (insufficient `maintenance_work_mem`).

**Key takeaway:** The 10.5x degradation from 10K→100K and 43.7x from 10K→1M in combined queries demonstrates the need for cascade filtering. Individual dimension queries scale sub-linearly; it's the cross-dimension joining that kills performance.

---

*This guide was compiled from analysis of SING's technical design documents, Rust source code (`src/adaptors/`, `src/search/`, `src/fusion.rs`, `src/types/`), benchmark results, and extension integration docs. Revisit after implementing Phase 1 to refine cost model parameters based on actual Covalence query profiles.*

# Covalence: PostgreSQL Extension Ecosystem Research Report

**Prepared by:** Technical Research Agent  
**Date:** 2026-03-01  
**Target Stack:** PostgreSQL 17 + Apache AGE + PGVector + pg_textsearch (BM25)  
**Purpose:** Inform architecture decisions for the Covalence graph-native knowledge substrate

---

## Executive Summary

The PG17 extension ecosystem for Covalence's target stack is **viable but carries meaningful risks**. PGVector is mature and excellent. pg_textsearch (Tiger Data / Timescale) is promising but preview-quality. Apache AGE is functional on PG17 but shows signs of low community activity and has real performance constraints for deep traversals. SQL/PGQ — the eventual standards-based replacement — did not land in PG18 and has no confirmed ship date. All three extensions coexist in the same database without fundamental conflicts, though combining them in a single Docker image requires a custom build.

---

## 1. Apache AGE on PostgreSQL 17

### Current Version and Release Cadence

- **Latest stable release:** v1.7.0 (PG17 branch), released February 11, 2026. Also released simultaneously for PG18.
- **Prior version:** v1.6.0 (PG17 branch), which was the first proper PG17 release after a prolonged gap — PG17 support was community-flagged as missing in September 2024 (issue #2111).
- **Version branching strategy:** AGE maintains separate release branches per PG major version (release/PG16/1.5.0, release/PG17/1.6.0, etc.). The PG17 branch did not have a release until mid-2025.
- **Officially supported PG versions:** 11-18 (per GitHub README as of early 2026).
- **Apache board status:** "Ongoing, **but low activity**" — no new PMC members added in last quarter per Apache Whimsy. 19 committers, 13 PMC members.

### Feature Set

AGE is inspired by Bitnine's AgensGraph and implements **openCypher** query language over PostgreSQL storage.

| Feature | Status |
|---|---|
| openCypher queries via cypher() function | Supported |
| Hybrid SQL + Cypher in same query | Supported |
| Property graph with arbitrary vertex labels | Supported |
| Property graph with arbitrary edge types | Supported |
| Hierarchical label organization | Supported |
| Property indexes on vertices and edges | Supported (v1.7.0 adds index on id columns) |
| Multiple graphs queried simultaneously | Supported |
| Row Level Security (RLS) | Added in v1.7.0 |
| CALL/YIELD for user-defined functions | Fixed in v1.7.0 |
| Cypher list comprehension | Reimplemented in v1.6.0 |
| Operators in Cypher queries | Added in v1.6.0 |
| CSV bulk import | Replaced libcsv with pg COPY in v1.7.0 |
| Partitioned tables | NOT supported |
| GQL / ISO standard queries | openCypher only |

### Cypher Completeness

AGE implements a significant subset of openCypher but is **not fully compliant**. Recent bug fixes reveal the state of maturity:

- List comprehension was broken and had to be reimplemented (v1.6.0)
- left() had overflow issues (#2205)
- Unexpected empty string behavior (#2201)
- ORDER BY alias resolution with AS was broken (#2269)
- COALESCE caused segfaults (#2256)
- More than 41 vertex labels caused crash in drop_graph (#2245)
- Ambiguous column references (#1884) — open for years, fixed in v1.7.0
- String concatenation regressions (#2243)
- IN expression with empty lists broken (#2289)

**Assessment:** These are not cosmetic bugs. They indicate the Cypher engine is still maturing. For Covalence's use case (structured knowledge graphs with typed edges and property lookups), the feature set is adequate, but complex Cypher with nested comprehensions or large label counts should be tested carefully.

### Performance Characteristics

- **Architecture:** AGE translates Cypher into SQL, executed on the PG backend. Graph data stored as rows in standard PG tables (one table per label/edge type). No separate in-memory graph for traversal.
- **Traversal depth scaling:** Community consensus is that AGE performs well for depth-1 or depth-2 traversals. **Deep traversals (3+ hops) degrade significantly** because each hop becomes a SQL join.
- **Comparison to Neo4j:** Not competitive for graph-traversal-heavy workloads. Neo4j uses index-free adjacency (O(1) per hop); AGE requires index scans per hop.
- **For Covalence:** If graph queries are primarily node lookups, 1-hop relationship expansion, and bounded neighborhood subgraphs — AGE is viable. Avoid deep variable-length path queries (MATCH (a)-[*1..10]->(b)).

### Project Health Risk

GitHub discussion #2150 ("What's the Status of Apache AGE?") raises explicit concern about PR merge rates and development slowdown. Apache's own board minutes characterize the project as **low activity**. This is a meaningful long-term risk requiring migration path planning to SQL/PGQ.

---

## 2. pg_textsearch (Tiger Data / Timescale)

### Overview and Provenance

- **Repo:** github.com/timescale/pg_textsearch (Timescale rebranded as Tiger Data in late 2025)
- **Internal codename:** "Tapir" (Textual Analysis for Postgres Information Retrieval)
- **License:** **PostgreSQL License** (permissive OSS) — key differentiator from ParadeDB
- **Status:** v1.0.0-dev — explicitly **preview / early access** (announced October 2025). Not yet GA.
- **Supported PG versions:** PostgreSQL 17 and 18
- **Deployment note:** Must be loaded via shared_preload_libraries — cannot be loaded dynamically.

### Technical Architecture

pg_textsearch implements a custom index access method using a **memtable architecture**:

- Writes go to an in-memory buffer before being flushed (LSM-tree-style)
- Fast top-k queries via **Block-Max WAND** optimization (up to 4x faster than naive BM25)
- **Parallel index builds** (4x speedup on large tables)
- **Advanced compression**: delta encoding + bitpacking reduces index size by 41%, improves short-query performance by 10-20%

### API Surface

```sql
-- Index creation
CREATE INDEX docs_idx ON documents USING bm25(content)
  WITH (text_config='english', k1=1.2, b=0.75);

-- Scoring operator (returns negative BM25 score -- lower = better)
SELECT * FROM documents
ORDER BY content <@> 'search query'
LIMIT 10;

-- Explicit index + WHERE clause filtering
SELECT * FROM documents
WHERE content <@> to_bm25query('search query', 'docs_idx') < -1.0
ORDER BY content <@> 'search query'
LIMIT 10;
```

Key API elements:
- `<@>` — BM25 scoring operator (returns negative scores for ASC sort compatibility with PG)
- `to_bm25query(text)` — creates bm25query type without index (ORDER BY only)
- `to_bm25query(text, index_name)` — creates bm25query with explicit index (enables WHERE clause)
- BM25 params: k1 (TF saturation, default 1.2), b (length normalization, default 0.75)
- Language configs: english, french, german, simple, any PG text search config

**Limitation:** BM25 indexes support **single-column only**. Multi-column search requires concatenating columns or maintaining multiple indexes.

### Comparison to ParadeDB pg_search

| Dimension | pg_textsearch (Tiger Data) | pg_search (ParadeDB) |
|---|---|---|
| License | PostgreSQL License (permissive) | AGPLv3 (copyleft) |
| Backend | Custom C + memtable | Tantivy (Rust, Elasticsearch-class) |
| Maturity | Preview (v1.0.0-dev) | Production-ready (v0.15+) |
| Performance | 4x vs native BM25 (Block-Max WAND) | Generally faster; Tantivy is heavily optimized |
| Feature set | BM25 ranking + hybrid search | BM25 + faceted + aggregations + more |
| High-volume logs | Not target use case | Primary design target |
| AI/RAG use case | Primary design target | Supported but not primary |
| Multi-column index | Single column only | Multi-field |

**License verdict for Covalence:** AGPLv3 (ParadeDB) requires open-sourcing the application or purchasing a commercial license. pg_textsearch's PostgreSQL License is fully compatible with proprietary use. **This is the decisive differentiator** and justifies accepting lower maturity.

---

## 3. PGVector — Current State

### Version

- **Latest:** v0.8.2 (v0.8.0 released November 2024)
- **Supports:** PostgreSQL 13+
- **License:** PostgreSQL License
- **Maturity:** Production-ready, widely deployed

### Vector Types

| Type | Column Type | Max Dims | Notes |
|---|---|---|---|
| Single-precision float32 | vector | 16,000 | Default |
| Half-precision float16 | halfvec | 16,000 | 50% smaller storage |
| Binary (1-bit) | bit | 64,000 | For binary embeddings |
| Sparse | sparsevec | 16,000 nonzero | For SPLADE/BM42 |

### Distance Functions

L2 (<->), negative inner product (<#>), cosine (<=>), L1 (<+>), Hamming (<~>), Jaccard (<%>)

### Index Types

**HNSW:** Better recall/speed tradeoff, can be built on empty tables. Supports halfvec. Parameters: m (default 16), ef_construction (default 64).

**IVFFlat:** Faster to build, less memory, must have data first, lower accuracy.

### New Features Since 0.7.x

- **Iterative scan (v0.8.0):** For filtered ANN, pgvector can iteratively expand HNSW search radius to find enough valid rows after filtering — major recall improvement on pre-filtered queries
- **Improved filter selectivity (v0.8.0):** Better planner cost estimation for when to use ANN index vs exact scan
- **halfvec HNSW:** Full HNSW index support on half-precision vectors
- **sparsevec:** Sparse vector type for hybrid sparse-dense retrieval patterns

**Recommendation for Covalence:** Use halfvec for embedding columns (50% space reduction, negligible quality loss). Use HNSW with m=16. The iterative scan feature is valuable for Covalence's filtered article retrieval.

---

## 4. AGE + PGVector Interoperability

### Coexistence

Both extensions install cleanly in the same database:

```sql
CREATE EXTENSION age;
CREATE EXTENSION vector;
LOAD 'age';
SET search_path = ag_catalog, "$user", public;
```

Both participate in the same PG transactions.

### Query Pattern (SQL Bridge)

Vector similarity cannot be used inside cypher() calls. The pattern is a SQL CTE bridge:

```sql
WITH graph_results AS (
  SELECT * FROM cypher('covalence', $$
    MATCH (n:Source)-[:RELATED_TO]->(m:Article)
    RETURN n.id, n.title
  $$) AS (id agtype, title agtype)
)
SELECT gr.id, s.embedding <=> '[0.1, 0.2, ...]' AS distance
FROM graph_results gr
JOIN sources s ON s.id = (gr.id::text)::uuid
ORDER BY distance
LIMIT 10;
```

### Known Issues

1. No native vector properties in Cypher — must use SQL wrapper
2. AGE returns agtype (custom JSONB-like type) — requires explicit casting to join with regular tables
3. search_path must include both ag_catalog and public to avoid operator resolution conflicts
4. Official apache/age Docker image does NOT include pgvector — custom Dockerfile required

### Custom Docker Image Pattern

```dockerfile
FROM postgres:17
RUN apt-get update && apt-get install -y \
    build-essential libreadline-dev zlib1g-dev flex bison git
# Build AGE
RUN git clone --branch release/PG17/1.7.0 https://github.com/apache/age.git /age && \
    cd /age && make install
# Build pgvector
RUN git clone --branch v0.8.2 https://github.com/pgvector/pgvector.git /pgvector && \
    cd /pgvector && make install
```

This pattern is documented by multiple community members (Codeberg benchmarking project, Medium/Percolation Labs CloudNativePG guide).

---

## 5. pg_textsearch + PGVector Hybrid Search

### Tiger Data Documentation

Tiger Data explicitly documents hybrid search with pgvector as a primary use case. The blog post introducing pg_textsearch frames it as the BM25 complement to pgvector for AI/RAG applications.

### Score Fusion: Reciprocal Rank Fusion (RRF)

Tiger Data does not provide a built-in fusion operator. The standard pattern is SQL-level RRF:

```sql
WITH
keyword_results AS (
  SELECT id,
         ROW_NUMBER() OVER (ORDER BY content <@> 'query terms') AS bm25_rank
  FROM sources
  ORDER BY content <@> 'query terms'
  LIMIT 50
),
semantic_results AS (
  SELECT id,
         ROW_NUMBER() OVER (ORDER BY embedding <=> '[0.1, 0.2, ...]') AS vec_rank
  FROM sources
  ORDER BY embedding <=> '[0.1, 0.2, ...]'
  LIMIT 50
),
fused AS (
  SELECT
    COALESCE(k.id, s.id) AS id,
    COALESCE(1.0 / (60 + k.bm25_rank), 0) +
    COALESCE(1.0 / (60 + s.vec_rank), 0) AS rrf_score
  FROM keyword_results k
  FULL OUTER JOIN semantic_results s ON k.id = s.id
)
SELECT id, rrf_score
FROM fused
ORDER BY rrf_score DESC
LIMIT 10;
```

**RRF properties:**
- Scale-invariant: BM25 scores (negative, varying magnitude) and cosine distances (0-1) normalize by rank position — no score normalization needed
- Tunable: k=60 constant controls rank dominance; lower k = stronger top-rank preference
- Weighted: multiply RRF components to weight BM25 vs semantic (e.g., 2x keyword, 1x semantic)

### Limitations

- Requires two index scans per query (acceptable at document counts under ~10M)
- Single-column BM25 constraint requires a single searchable text column per index
- Preview quality: high-concurrency write patterns not yet hardened

---

## 6. SQL/PGQ — PostgreSQL Native Graph Query

### Standard Status

SQL/PGQ is part of **ISO SQL:2023** (ISO/IEC 9075-16:2023). Provides GRAPH_TABLE() function and MATCH clause for querying relational data as a property graph without a separate graph extension.

### PostgreSQL Implementation Status

**Not in any PostgreSQL release as of early 2026:**
- PG17 (Sept 2024): No SQL/PGQ
- PG18 (Sept 2025): Developed and discussed in pgsql-hackers, but **did not make the release**
- Available as manual patches against PG HEAD for exploration
- No confirmed ship date

### AGE vs SQL/PGQ Comparison

| Dimension | Apache AGE (now) | SQL/PGQ (future) |
|---|---|---|
| Query language | openCypher | ISO SQL/PGQ |
| Available | Today (PG17) | Not yet shipped |
| Extension required | Yes (complex C extension) | No (core PG once shipped) |
| SQL composability | Via cypher() wrapper (awkward) | Native — full composability |
| Deep traversal | Poor (joins per hop) | Likely better (planner optimization) |
| Vector integration | Manual SQL bridge | Native SQL JOIN |

### Strategic Recommendation

**Use AGE now, design for SQL/PGQ migration:**
1. Wrap all Cypher queries in a query abstraction layer (repository pattern)
2. Keep schema simple — vertex labels map to node types, edge types map to relationship types
3. Do NOT let AGE's agtype leak into application code — always project to native SQL types
4. Monitor pgsql-hackers commitfest — SQL/PGQ could appear in PG19 or PG20

---

## 7. Stack Integration for Covalence

### Initialization Sequence

```sql
CREATE EXTENSION age;
CREATE EXTENSION vector;
CREATE EXTENSION pg_textsearch;
LOAD 'age';
SET search_path = ag_catalog, "$user", public;
SELECT create_graph('covalence');
```

### postgresql.conf

```ini
shared_preload_libraries = 'age,pg_textsearch'
```

### Schema Pattern

```sql
-- PG tables with vector embeddings and BM25 indexes
CREATE TABLE sources (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  content text NOT NULL,
  embedding halfvec(1536),
  source_type text,
  created_at timestamptz DEFAULT now()
);

CREATE INDEX sources_hnsw_idx ON sources
  USING hnsw (embedding halfvec_cosine_ops)
  WITH (m = 16, ef_construction = 64);

CREATE INDEX sources_bm25_idx ON sources
  USING bm25(content)
  WITH (text_config = 'english');

-- AGE graph for relationships (managed internally by AGE)
-- Vertex: Source, Article, Concept
-- Edges: ORIGINATES, CONFIRMS, SUPERSEDES, CONTRADICTS, LINKS_TO
```

### Known Conflicts and Mitigations

| Issue | Mitigation |
|---|---|
| search_path ordering | Always include both ag_catalog and public; use explicit schema quals |
| agtype casting overhead | Project to text/uuid/jsonb at query boundary; never pass agtype to app code |
| No vector ops in Cypher | Use CTE bridge pattern; document as architectural constraint |
| Custom Docker required | Maintain Dockerfile as first-class CI artifact |
| pg_textsearch shared_preload | Include in infra-as-code from day 1; cannot be added hot |

---

## 8. Risk Register

| Risk | Severity | Likelihood | Mitigation |
|---|---|---|---|
| AGE project abandonment | High | Medium | Abstract behind query layer; SQL/PGQ migration path |
| pg_textsearch preview bugs | Medium | High | Feature-flag BM25; fallback to ts_rank available |
| AGE deep traversal perf | High | High if queries go deep | Limit graph depth; materialize subgraphs |
| AGE+PGVector Docker complexity | Low | Certain | Maintain Dockerfile in CI |
| pg_textsearch shared_preload requirement | Low | Certain | Include in infra IaC from day 1 |
| SQL/PGQ ships faster than expected | Opportunity | Low | Monitor pgsql-hackers |
| ParadeDB outcompetes pg_textsearch | Medium | Medium | License advantage remains regardless |

---

## 9. Conclusions and Recommendations

1. **PGVector: Proceed with confidence.** v0.8.2 is production-ready. Use halfvec for embeddings, HNSW indexes. Iterative scan (v0.8.0) is valuable for Covalence's filtered retrieval.

2. **pg_textsearch: Adopt with caution.** PostgreSQL license is decisive for commercial safety. Block-Max WAND performance is solid. Treat as preview: add integration tests, ensure ts_rank fallback is possible, track the repo toward GA.

3. **Apache AGE: Adopt but hedge.** AGE 1.7.0 on PG17 is functional for Covalence's 1-2 hop graph patterns. Risks: low community activity, deep traversal perf, awkward SQL/Cypher interop. Mitigate with query abstraction layer and shallow graph design.

4. **SQL/PGQ: Watch, don't wait.** Not in PG17 or PG18. Design for future migration by keeping AGE usage minimal and abstract.

5. **Hybrid search pattern:** Use RRF fusion in SQL (not application code). Weight can be adjusted per query type. For Covalence: 2:1 semantic:keyword weighting is a reasonable starting point for source retrieval (adjust by evaluation).

6. **Docker:** Maintain a custom Dockerfile building PG17 + AGE 1.7.0 + pgvector 0.8.2 + pg_textsearch as a first-class CI artifact. Multiple community examples exist to base it on.

---

## References

- Apache AGE GitHub: https://github.com/apache/age
- Apache AGE Releases: https://github.com/apache/age/releases
- AGE Project Status (Apache Whimsy): https://whimsy.apache.org/board/minutes/AGE.html
- AGE Status Discussion #2150: https://github.com/apache/age/discussions/2150
- pg_textsearch GitHub: https://github.com/timescale/pg_textsearch
- pg_textsearch Intro Blog: https://www.tigerdata.com/blog/introducing-pg_textsearch-true-bm25-ranking-hybrid-retrieval-postgres
- pg_textsearch Docs: https://www.tigerdata.com/docs/use-timescale/latest/extensions/pg-textsearch
- pgvector GitHub: https://github.com/pgvector/pgvector
- pgvector 0.8.0 Release: https://www.postgresql.org/about/news/pgvector-080-released-2952/
- AGE + pgvector Docker: https://codeberg.org/trisolar.faculty/postgres_pgvector_age_benchmarking
- CloudNativePG + AGE + pgvector: https://medium.com/percolation-labs/cloudnativepg-age-and-pg-vector-on-a-docker-image-step-1-ef0156c78f49
- SQL/PGQ ISO Standard: https://www.iso.org/standard/79473.html
- EDB SQL/PGQ Overview: https://www.enterprisedb.com/blog/representing-graphs-postgresql-sqlpgq
- ParadeDB Hybrid Search: https://www.paradedb.com/blog/hybrid-search-in-postgresql-the-missing-manual
- RRF in PostgreSQL: https://jkatz05.com/post/postgres/hybrid-search-postgres-pgvector/

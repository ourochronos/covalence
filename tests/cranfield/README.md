# Cranfield Search-Quality Harness

The Cranfield methodology (Cleverdon, 1960s) is the foundational technique for
evaluating information retrieval systems: a **fixed corpus + fixed queries +
relevance judgments** lets you measure recall and precision reproducibly as the
system changes.

This directory implements **Phase 2** of that methodology for the Covalence
search engine: a 35-query golden set, an automated harness, and per-query
Recall@K / MRR metrics.

---

## What this is

| File | Purpose |
|------|---------|
| `golden_queries.json` | 35 representative queries covering every search mode, strategy, and intent variant across diverse topic domains |
| `run_harness.sh` | Shell driver — executes each query against a live engine, prints a pass/fail table, and reports Recall@5, Recall@10, and MRR metrics |
| `README.md` | This file |

The harness provides a **lightweight regression signal**: if a code change
breaks vector scoring, lexical indexing, or graph traversal, at least some
queries will start failing and alert you.  The aggregate Recall@K and MRR
numbers provide a continuous quality signal beyond simple pass/fail.

---

## Requirements

| Tool | Purpose |
|------|---------|
| `curl` | HTTP calls to the Covalence engine |
| `jq` | Parse JSON responses and extract scores |
| `python3` | Floating-point metric arithmetic |
| A running Covalence engine | `docker compose up -d` |
| A populated knowledge base | At least some content must have been ingested |

---

## Quick start

```bash
# Start the engine (if not already running)
docker compose up -d

# Run all 35 golden queries against localhost:8430
./tests/cranfield/run_harness.sh

# Use a different engine URL
./tests/cranfield/run_harness.sh --url http://my-server:8430

# With an API key
COVALENCE_API_KEY=secret ./tests/cranfield/run_harness.sh

# Verbose: print every raw JSON response
./tests/cranfield/run_harness.sh --verbose

# Stop immediately on the first failing query
./tests/cranfield/run_harness.sh --fail-fast
```

The script exits `0` if all queries pass, `1` if any fail, `2` on
configuration/dependency errors.

---

## How to interpret results

The harness prints a per-query table followed by an aggregate summary:

```
ID     MODE         STRATEGY     THRESH   TOP_SCORE  R@5     R@10    MRR    HITS   RESULT   DESCRIPTION
-----------------------------------------------------------------------------------------------------------
q01    standard     balanced     0.1      0.3120     1.0     1.0     1.0    10     PASS     Baseline vector retrieval...
q03    standard     precise      0.1      0.1245     0.4     0.2     1.0     2     PASS     Multi-term keyword query...
q07    standard     balanced     0.1      0.1587     0.6     0.3     1.0     3     PASS     Multi-token operational...

-----------------------------------------------------------------------------------------------------------
Results: 35 passed  0 failed  0 errored  (35 total)

Aggregate Metrics (over 35 queries):
  Mean Recall@5:     0.9429
  Mean Recall@10:    0.8657
  Mean MRR:          1.0000
```

### Column meanings

| Column | Meaning |
|--------|---------|
| `ID` | Query identifier from `golden_queries.json` |
| `MODE` | Search mode used (`standard` / `hierarchical`) |
| `STRATEGY` | Fusion strategy (`balanced` / `precise` / `exploratory` / `graph`) |
| `THRESH` | `min_score` — a result must reach this to count as a hit |
| `TOP_SCORE` | Highest score returned in this query's results |
| `R@5` | **Recall@5** — hits in top-5 ÷ 5 |
| `R@10` | **Recall@10** — hits in top-10 ÷ 10 |
| `MRR` | **Mean Reciprocal Rank** — 1/rank of first hit in top-10, or 0 |
| `HITS` | Total results that met or exceeded `min_score` |
| `RESULT` | `PASS` (≥1 hit anywhere) or `FAIL` (0 hits) |
| `DESCRIPTION` | Brief description from the query's `description` field |

### Metric definitions

**Recall@K** measures retrieval density in the top-K window:

```
Recall@K = |{results in top-K with score ≥ min_score}| / K
```

A value of `1.0` means every slot in the top-K window contained a relevant
result; `0.2` means only 1-in-5 did.  Note that `min_score` acts as a proxy
for relevance in lieu of human judgments (Phase 3 would replace it with
explicit relevance labels).

**MRR (Mean Reciprocal Rank)** measures how quickly the first relevant result
appears:

```
MRR = 1 / rank_of_first_hit_in_top_10   (0 if no hit in top-10)
```

`MRR = 1.0` means the very first result was relevant; `0.5` means the second
was first relevant.

**Aggregate metrics** are micro-averaged over all queries in the run.

### Common failure causes

| Symptom | Likely cause |
|---------|-------------|
| All queries fail | Knowledge base is empty; ingest content first |
| Vector queries fail, lexical pass | Embeddings not generated — run `POST /admin/embed-all` |
| Graph queries fail | Apache AGE not running or edges not inferred |
| Specific queries regressed after a PR | Check that query's dimension scores; compare with baseline |
| Low R@10, high MRR | Relevant results are concentrated at the very top; tail is noise |

---

## Query coverage matrix (Phase 2)

The 35 queries form a deliberate coverage matrix:

### Modes
| Mode | Count |
|------|-------|
| `standard` | 33 |
| `hierarchical` | 2 |

### Strategies
| Strategy | Count |
|----------|-------|
| `balanced` | 12 |
| `precise` | 11 |
| `exploratory` | 9 |
| `graph` | 3 |

### Intents
| Intent | Count |
|--------|-------|
| `null` | 11 |
| `factual` | 10 |
| `causal` | 7 |
| `entity` | 5 |
| `temporal` | 2 |

### Topic domains
| Domain | Example queries |
|--------|----------------|
| Confidence / epistemic | q01, q28, q35 |
| Provenance / articles | q02, q09 |
| Contentions | q03, q06, q26 |
| Graph structure | q04, q14, q19, q31 |
| Embeddings / vector search | q05, q17 |
| Knowledge consolidation | q07, q11 |
| Temporal / recency | q08, q25, q30 |
| Engine implementation (Rust) | q10 |
| Admin / observability | q18 |
| Plugin / session | q16 |
| Neuroscience: reconsolidation | q21, q33 |
| Neuroscience: spacing effect | q25, q30 |
| CRDT / federation | q22, q34 |
| Causal inference | q24 |
| Cognitive architectures | q23 |
| Agent memory | q27 |
| Epistemology | q28, q32 |
| Argumentation theory | q26 |
| KG reasoning | q31 |
| Process improvement | q29 |
| Societal impact | q32 |
| Configuration | q20 |
| Lifecycle / states | q13 |

### Query types
| Type | Example |
|------|---------|
| Prose question | "how does confidence scoring work" (q01) |
| Multi-term keyword | "contention detection and resolution supersede contradicts" (q03) |
| Named entity / technology | "Apache AGE graph database PostgreSQL" (q14) |
| Cross-domain | "metacognition epistemic uncertainty AI knowing what you don't know" (q35) |
| Single-domain technical | "pgvector HNSW halfvec embeddings semantic search" (q05) |

---

## How to add queries

Edit `golden_queries.json` and append an entry to the `queries` array:

```jsonc
{
  "id": "q36",                          // unique, sequential
  "query": "your search string here",
  "mode": "standard",                   // "standard" | "hierarchical"
  "strategy": "balanced",               // "balanced" | "precise" | "exploratory" | "graph"
  "intent": null,                       // null | "factual" | "temporal" | "causal" | "entity"
  "limit": 5,                           // max results to request (harness always fetches ≥10)
  "min_score": 0.1,                     // minimum score for a result to count as a hit
  "description": "What this query tests and why it belongs in the golden set.",
  "tags": ["topic", "dimension-name"]   // free-form labels for filtering
}
```

**Tips for choosing `min_score`:**
- Use `0.12–0.15` for prose queries about well-represented topics.
- Use `0.08–0.10` for technical terms, cross-domain, or structural queries.
- Never set below `0.06`; near-zero scores indicate noise, not relevance.
- Always validate against the live KB before committing (`run_harness.sh`).

---

## Phase 2 methodology (current)

Phase 2 extends Phase 1's smoke test with quantitative IR metrics, matching
the Cranfield tradition more closely.  The key advances over Phase 1:

1. **35-query golden set** — expanded from 20 queries.  Covers all modes
   (standard, hierarchical), all strategies (balanced, precise, exploratory,
   graph), all intent types (factual, causal, temporal, entity, null), and
   14 distinct topic domains.

2. **Recall@5 and Recall@10** — per-query and aggregate.  Measures retrieval
   density in the top-K window.  Computed as:
   `hits_in_top_K / K` where a hit is any result with `score ≥ min_score`.

3. **MRR (Mean Reciprocal Rank)** — per-query and aggregate.  Measures how
   quickly the first relevant result appears in the ranked list.

4. **Baseline (Phase 2, 2026-03-04):**

   | Metric | Value |
   |--------|-------|
   | Queries | 35 |
   | Passed | 35 |
   | Mean Recall@5 | 0.9429 |
   | Mean Recall@10 | 0.8657 |
   | Mean MRR | 1.0 |

   All 35 queries pass against the live KB as of the Phase 2 release.

---

## Phase 3: fixed corpus + explicit relevance judgments (future)

Phase 3 would add:

1. **Fixed corpus** — a small, version-controlled set of ~100 source documents
   covering the Covalence domain, ingested into a dedicated test database
   (separate from the live KB).

2. **Relevance judgments** (`judgments.json`) — for each golden query, a
   human-annotated list of source/article IDs that are *relevant* (1),
   *partially relevant* (0.5), or *not relevant* (0).  Following the
   Cranfield convention these are binary or graded labels assigned by domain
   experts.

3. **Additional metrics** — the harness would compute:
   - **Average Precision (AP)** and **Mean Average Precision (MAP)**
   - **NDCG@k** — normalised discounted cumulative gain (for graded judgments)
   - **Precision@k** — fraction of top-k results that are relevant

4. **CI integration** — a GitHub Actions job that starts a clean engine with
   the fixed corpus, runs the harness, and fails the build if MAP drops below
   a stored baseline threshold.

---

## Relationship to CI

The harness is intentionally **not** part of `cargo test` — it requires a
live database and HTTP server.  It is best run as a separate CI step after the
unit/integration tests pass:

```yaml
# .github/workflows/ci.yml (example addition)
- name: Cranfield search quality harness
  run: |
    docker compose up -d
    sleep 10          # wait for engine to become healthy
    ./tests/cranfield/run_harness.sh --url http://localhost:8430
```

---

## References

- Cleverdon, C.W. (1960). "Report on the First Stage of an Investigation into
  the Comparative Efficiency of Indexing Systems." *ASLIB Cranfield Research
  Project*, College of Aeronautics, Cranfield.
- Manning, C.D., Raghavan, P., Schütze, H. (2008). *Introduction to Information
  Retrieval*, Chapter 8: Evaluation in information retrieval.
  https://nlp.stanford.edu/IR-book/html/htmledition/evaluation-in-information-retrieval-1.html
- Järvelin, K., Kekäläinen, J. (2002). "Cumulated gain-based evaluation of IR
  techniques." *ACM TOIS* 20(4):422–446. (NDCG)
- Voorhees, E.M. (1999). "The TREC-8 Question Answering Track Report." *TREC*.
  (MRR origin)

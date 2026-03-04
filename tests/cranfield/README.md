# Cranfield Search-Quality Harness

The Cranfield methodology (Cleverdon, 1960s) is the foundational technique for
evaluating information retrieval systems: a **fixed corpus + fixed queries +
relevance judgments** lets you measure recall and precision reproducibly as the
system changes.

This directory implements **Phase 1** of that methodology for the Covalence
search engine: a golden query set and an automated harness.  Phase 2 (fixed
corpus + explicit relevance judgments) is outlined at the bottom of this
document.

---

## What this is

| File | Purpose |
|------|---------|
| `golden_queries.json` | 20 representative queries covering every search mode, strategy, and intent variant |
| `run_harness.sh` | Shell driver — executes each query against a live engine and prints a pass/fail table |
| `README.md` | This file |

The harness provides a **lightweight regression signal**: if a code change
breaks vector scoring, lexical indexing, or graph traversal, at least some
queries will start failing and alert you.

---

## Requirements

| Tool | Purpose |
|------|---------|
| `curl` | HTTP calls to the Covalence engine |
| `jq` | Parse JSON responses and extract scores |
| `python3` | Available on PATH (used as a fallback arithmetic helper) |
| A running Covalence engine | `docker compose up -d` |
| A populated knowledge base | At least some content must have been ingested |

---

## Quick start

```bash
# Start the engine (if not already running)
docker compose up -d

# Run all 20 golden queries against localhost:8430
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

The harness prints a table like:

```
ID     MODE         STRATEGY     THRESHOLD  TOP_SCORE  HITS   RESULT   DESCRIPTION
--------------------------------------------------------------------------------------------
q01    standard     balanced     0.1        0.312      3      PASS     Baseline vector retrieval — tests...
q02    standard     precise      0.1        0.279      2      PASS     Lexical-heavy precise strategy...
q05    standard     precise      0.1        0.031      0      FAIL     Technical terminology query...
```

Column meanings:

| Column | Meaning |
|--------|---------|
| `ID` | Query identifier from `golden_queries.json` |
| `MODE` | Search mode used (`standard` / `hierarchical`) |
| `STRATEGY` | Fusion strategy (`balanced` / `precise` / `exploratory` / `graph`) |
| `THRESHOLD` | `min_score` — a result must reach this to count as a hit |
| `TOP_SCORE` | Highest score returned in this query's results |
| `HITS` | Number of results that met or exceeded the threshold |
| `RESULT` | `PASS` (≥1 hit) or `FAIL` (0 hits) |
| `DESCRIPTION` | Brief description from the query's `description` field |

### Common failure causes

| Symptom | Likely cause |
|---------|-------------|
| All queries fail | Knowledge base is empty; ingest content first |
| Vector queries fail, lexical pass | Embeddings not generated — run `POST /admin/embed-all` |
| Graph queries fail | Apache AGE not running or edges not inferred |
| Specific queries regressed after a PR | Check that query's dimension scores; compare with baseline |

---

## How to add queries

Edit `golden_queries.json` and append an entry to the `queries` array:

```jsonc
{
  "id": "q21",                          // unique, sequential
  "query": "your search string here",
  "mode": "standard",                   // "standard" | "hierarchical"
  "strategy": "balanced",               // "balanced" | "precise" | "exploratory" | "graph" | "structural"
  "intent": null,                       // null | "factual" | "temporal" | "causal" | "entity"
  "limit": 5,                           // max results to request
  "min_score": 0.1,                     // minimum score for a result to count as a hit
  "description": "What this query tests and why it belongs in the golden set.",
  "tags": ["topic", "dimension-name"]   // free-form labels for filtering
}
```

**Tips for choosing `min_score`:**
- Use `0.1` for straightforward prose queries in a populated KB.
- Drop to `0.06–0.08` for obscure technical terms or graph/structural queries.
- Never set below `0.05`; near-zero scores indicate noise, not relevance.
- Calibrate against a representative corpus before committing a threshold.

---

## Phase 2: fixed corpus + relevance judgments

The current harness is a *smoke test* — it can detect catastrophic regressions
but cannot measure *recall* or *precision* rigorously because we have no fixed
corpus or explicit relevance judgments.

Phase 2 would add:

1. **Fixed corpus** — a small, version-controlled set of ~100 source documents
   covering the Covalence domain, ingested into a dedicated test database
   (separate from the live KB).

2. **Relevance judgments** (`judgments.json`) — for each of the 20 golden
   queries, a human-annotated list of source/article IDs that are *relevant*
   (1), *partially relevant* (0.5), or *not relevant* (0).  Following the
   Cranfield convention these are binary or graded labels assigned by domain
   experts.

3. **Metrics** — the harness would compute per-query and aggregate:
   - **Recall@k** — fraction of known-relevant documents in the top-k results
   - **Precision@k** — fraction of top-k results that are relevant
   - **Average Precision (AP)** and **Mean Average Precision (MAP)**
   - **NDCG@k** — normalised discounted cumulative gain (for graded judgments)

4. **Baseline snapshot** — a `baseline.json` capturing scores for the current
   engine version, so CI can flag regressions (MAP drops by more than ε).

5. **CI integration** — a GitHub Actions job that:
   - Starts a clean Postgres + Covalence engine with the fixed corpus
   - Runs the harness and compares against the baseline
   - Fails the build if MAP drops below threshold

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

# 11 — Evaluation Methodology

**Status:** Draft

How to measure whether the system works. Without evaluation, spec changes are opinions. With it, they're experiments.

## Principles

1. **Offline first** — Automated evaluation runs before deployment, not after
2. **No ground truth required** — Use reference-free metrics (RAGAS pattern) for most evaluations
3. **Component-level + end-to-end** — Evaluate retrieval and generation separately, then together
4. **Regression gates** — No spec change ships without proving it doesn't regress existing quality

## Metrics

### Retrieval Quality

| Metric | What it Measures | How to Compute |
|--------|-----------------|----------------|
| **Context Precision** | Are retrieved chunks relevant? | LLM judges each chunk's relevance to the query |
| **Context Recall** | Did we find everything needed? | LLM checks if the answer can be fully derived from retrieved context |
| **MRR@k** | Is the best result ranked high? | Mean reciprocal rank of first relevant result |
| **Recall@k** | Coverage at cutoff k | Fraction of relevant results in top-k |
| **Dimension contribution** | Is each search dimension useful? | Ablation: run search with each dimension disabled, measure recall drop |
| **Reranker lift** | Does Voyage rerank-2.5 help? | Compare MRR before vs after reranking |

### Generation Quality

| Metric | What it Measures | How to Compute |
|--------|-----------------|----------------|
| **Faithfulness** | Is the answer grounded in retrieved context? | LLM decomposes answer into claims, checks each against context (RAGAS) |
| **Answer Relevancy** | Does the answer address the question? | Generate N questions from the answer, compute similarity to original query (RAGAS) |
| **Citation Accuracy** | Do inline citations point to the right chunks? | Verify each [N] reference actually supports the claim it's attached to |
| **Hallucination Rate** | How often does the answer contain unsupported claims? | 1 - Faithfulness, essentially |

### Knowledge Graph Quality

| Metric | What it Measures | How to Compute |
|--------|-----------------|----------------|
| **Entity precision/recall** | Are extracted entities correct and complete? | Sample chunks, manually annotate, compare to extraction |
| **Relationship accuracy** | Are extracted relationships correct? | Sample edges, verify against source text |
| **Dedup quality** | Is entity resolution working? | Count duplicate entity pairs (embedding similarity > 0.95, different node IDs) |
| **Graph density** | Is the graph connected enough? | edges / nodes ratio, largest connected component size |
| **Community coherence** | Are communities meaningful? | Intra-community edge density vs inter-community |
| **Temporal consistency** | Are bi-temporal edges correct? | Sample invalidated edges, verify the superseding edge is actually contradictory |

### Ingestion Quality

| Metric | What it Measures | How to Compute |
|--------|-----------------|----------------|
| **Chunking quality** | Are chunk boundaries meaningful? | Measure semantic similarity between adjacent chunks (lower = better boundaries) |
| **Statement self-containment** | Are statements independently meaningful? | Sample 100 statements, verify no unresolved pronouns or dangling references. Target: >95% |
| **Statement dedup accuracy** | Are semantic duplicates caught? | Sample statement pairs with cosine > 0.90, manually classify as true/false duplicates |
| **Coref resolution quality** | Are pronouns correctly resolved? | Sample 50 statements with resolved referents, verify against source text |
| **Section coherence** | Do clustered statements form coherent topics? | Measure intra-section embedding similarity vs inter-section (higher ratio = better clustering) |
| **Extraction yield** | How many entities per statement? | `avg(entities)` grouped by extraction method |
| **Gleaning value** | Does the second pass find real entities? | Precision of gleaning-only extractions |
| **Landscape calibration** | Are alignment thresholds well-calibrated? | Compare percentile-based thresholds against manual annotation of extraction need |

### Code Ingestion Quality

| Metric | What it Measures | How to Compute |
|--------|-----------------|----------------|
| **AST boundary accuracy** | Are code chunks split at meaningful boundaries? | Verify chunks align with function/struct/module boundaries (should be ~100%) |
| **Semantic summary quality** | Do summaries accurately describe code behavior? | LLM judges summary against code, or human review of 50 samples. Target: >90% accurate |
| **Summary embedding alignment** | Do code summaries embed near related prose? | Measure cosine similarity between code summary embeddings and corresponding spec section embeddings |
| **Structural edge precision** | Are CALLS/USES_TYPE/IMPLEMENTS edges correct? | Sample 100 edges, verify against source code. Target: >95% |
| **Incremental update correctness** | Does ast_hash change detection work? | Modify whitespace only, verify no re-summarization; modify logic, verify re-summarization |

### Cross-Domain Analysis Quality

| Metric | What it Measures | How to Compute |
|--------|-----------------|----------------|
| **Coverage score** | Are spec topics implemented in code? | `count(spec topics with IMPLEMENTS_INTENT edges) / count(total spec topics)`. Target: >0.8 |
| **Drift score** | Has code diverged from spec intent? | `mean(1 - cosine(component.embedding, code_entity.embedding))`. Target: <0.3 |
| **Whitespace coverage** | How much research is unimplemented? | `count(research clusters with no bridge edges) / count(total research clusters)`. Lower is better |
| **Blast radius accuracy** | Does impact analysis find real dependencies? | For a known change, compare predicted impact vs actual test failures |
| **Bridge completeness** | Are all code entities assigned to components? | `count(code entities with PART_OF_COMPONENT) / count(total code entities)`. Target: >0.9 |

## Evaluation Dataset

Build incrementally from production usage:

1. **Query log sampling** — Sample N queries per week from production. Store query + retrieved context + generated answer.
2. **Human annotation** — Periodically annotate a subset (relevance judgments, faithfulness checks). Even 50 annotated queries is valuable.
3. **Synthetic test set** — Generate question-answer pairs from known source documents. Use these for regression testing.
4. **Multi-hop test set** — Curate questions that require 2+ graph hops to answer. These stress-test PPR and community summaries.
5. **Temporal test set** — Questions with explicit time references ("What was true in 2023?"). Stress-test bi-temporal edge queries.
6. **Global test set** — Thematic questions ("What are the main topics?"). Stress-test community summary search.
7. **Code search test set** — Natural language queries about code behavior ("How does entity resolution work?", "What calls the embedder?"). Stress-test semantic summary embedding quality and cross-domain retrieval.
8. **Cross-domain test set** — Questions requiring traversal across research, spec, and code ("Does our entity resolution implement HDBSCAN correctly?", "What research supports the consolidation pipeline design?"). Stress-test Component bridge and IMPLEMENTS_INTENT edges.

## Regression Testing

Before each spec change (or model upgrade):

```
1. Run retrieval eval on the test set with current config → baseline
2. Apply change
3. Run retrieval eval again → candidate
4. Compare: if any metric drops > 2%, investigate before merging
5. If all metrics hold or improve, merge
```

For embedding model migration specifically:
- Sample 1000 queries
- Compare recall@10 between old and new model
- If recall improves > 2%, proceed with migration
- If recall degrades, skip this model version

## Current Implementation

The `covalence-eval` crate (`engine/crates/covalence-eval/`) implements a fixture-based evaluation harness with three layer evaluators:

- **`ChunkerEval`** — Evaluates chunking quality against expected chunk outputs
- **`ExtractorEval`** — Evaluates entity/relationship extraction against expected extraction results
- **`SearchEval`** — Evaluates search result quality against expected result sets
- **`StatementEval`** — Evaluates statement extraction quality (self-containment, coref resolution, dedup accuracy)
- **`CrossDomainEval`** — Evaluates coverage, drift, and gap detection accuracy across the three knowledge domains

All evaluators implement the `LayerEvaluator` trait, which takes typed inputs, produces outputs, and scores them against expected baselines. Test fixtures live in the `fixtures` module. Metrics types are `ChunkerMetrics`, `ExtractorMetrics`, and `SearchMetrics`.

RAGAS integration (reference-free faithfulness, answer relevancy, context precision/recall) has stub implementations that return 1.0 — not yet wired to actual LLM-judged evaluation. The current harness focuses on deterministic, fixture-based evaluation of individual pipeline layers.

### Current Baselines

Concrete metrics from production (as of 2026-03):

| Metric | Value | Quality Gate | Status |
|--------|-------|-------------|--------|
| Search precision@5 | 0.86 | >0.80 | Passing |
| Entity precision | 96% | >90% | Passing |
| Search regression | 20/20 queries stable | 0 regressions | Passing |

The search regression baseline is stored in `search_precision_baseline.json` (20 curated queries covering vector, lexical, graph, temporal, and cross-domain search). The `make eval-search` target runs these queries against the prod API and flags any result count drops or zero-result regressions.

## Tools

- **RAGAS** (docs.ragas.io) — Reference-free evaluation of faithfulness, answer relevancy, context precision/recall (future integration)
- **GraphRAG-Bench** (arXiv:2506.05690) — Graph-specific benchmark suite
- **Custom harness** — `covalence-eval` binary that runs layer evaluations against fixtures, producing typed metrics

## Prompt Ablation Testing

Extraction prompt quality directly determines graph quality. Test prompt variants systematically:

1. **Baseline** — Current spec prompts (7.2 delta check, 7.3 full extraction, gleaning)
2. **Ablations to test:**
   - Remove `nearby_graph_nodes` from extraction prompt → measure entity dedup quality
   - Remove `known_entity_types` / `known_rel_types` → measure schema drift
   - Remove `parent_context` → measure extraction completeness
   - Vary `max_entities` / `max_relationships` limits → measure yield vs noise tradeoff
   - Test with different LLMs (4o-mini vs Claude Haiku vs Gemini Flash) → measure extraction quality per dollar
3. **Metric:** Entity precision/recall on a manually annotated 50-document sample
4. **Frequency:** Re-run ablation suite when changing extraction prompts or upgrading LLM models

### Statement Extraction Ablations

1. **Baseline** — Current statement extraction prompt with coreference resolution
2. **Ablations to test:**
   - Remove coreference resolution instruction → measure self-containment rate
   - Vary window size (3/5/7 paragraphs) → measure coverage vs overlap tradeoff
   - Vary overlap (1/2/3 paragraphs) → measure statement dedup rate
   - Test with/without heading path context → measure accuracy of heading-dependent content
   - Test different LLMs (Gemini Flash vs Claude Haiku vs GPT-4o-mini) → quality per dollar
3. **Metric:** Self-containment rate, dedup collision rate, entity extraction yield from statements

### Code Summary Ablations

1. **Baseline** — Current semantic summary prompt (natural language description of business logic)
2. **Ablations to test:**
   - Include/exclude function signature in prompt → measure embedding alignment with spec
   - Include/exclude surrounding module context → measure summary specificity
   - Vary summary length (1 sentence / 2-3 sentences / paragraph) → measure search recall
   - Test summary-only vs summary+signature embedding → measure cross-domain retrieval quality
3. **Metric:** Cosine similarity between code summary and corresponding spec section embeddings

## Decision Record

- [x] Evaluation framework → RAGAS-compatible metrics, no ground truth required for most metrics
- [x] Regression gate → 2% drop threshold triggers investigation
- [x] Eval dataset → Built incrementally from production + synthetic generation
- [ ] Embedding model eval → recall@10 comparison on 1000 sampled queries

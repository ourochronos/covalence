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
| **Extraction yield** | How many entities per chunk at each extraction tier? | `avg(entities)` grouped by `ExtractionMethod` |
| **Gleaning value** | Does the second pass find real entities? | Precision of gleaning-only extractions |
| **Landscape calibration** | Are alignment thresholds well-calibrated? | Compare percentile-based thresholds against manual annotation of extraction need |

## Evaluation Dataset

Build incrementally from production usage:

1. **Query log sampling** — Sample N queries per week from production. Store query + retrieved context + generated answer.
2. **Human annotation** — Periodically annotate a subset (relevance judgments, faithfulness checks). Even 50 annotated queries is valuable.
3. **Synthetic test set** — Generate question-answer pairs from known source documents. Use these for regression testing.
4. **Multi-hop test set** — Curate questions that require 2+ graph hops to answer. These stress-test PPR and community summaries.
5. **Temporal test set** — Questions with explicit time references ("What was true in 2023?"). Stress-test bi-temporal edge queries.
6. **Global test set** — Thematic questions ("What are the main topics?"). Stress-test community summary search.

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

## Tools

- **RAGAS** (docs.ragas.io) — Reference-free evaluation of faithfulness, answer relevancy, context precision/recall
- **GraphRAG-Bench** (arXiv:2506.05690) — Graph-specific benchmark suite
- **Custom harness** — Rust test binary that runs queries against the engine, collects metrics, outputs JSON report

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

## Decision Record

- [x] Evaluation framework → RAGAS-compatible metrics, no ground truth required for most metrics
- [x] Regression gate → 2% drop threshold triggers investigation
- [x] Eval dataset → Built incrementally from production + synthetic generation
- [ ] Embedding model eval → recall@10 comparison on 1000 sampled queries

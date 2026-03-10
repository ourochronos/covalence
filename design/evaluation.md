# Design: Evaluation Framework

## Status: harness implemented and wired (#4 closed); fixture data and RAGAS still needed

> **Updated 2026-03-10**: Layer evaluation harness completed and verified (#4 closed). The CLI
> binary runs end-to-end against all three layer evaluators (chunker, extractor, search). Still
> needs fixture datasets and RAGAS LLM computation, but the framework infrastructure is complete.

## Spec Sections: 11-evaluation.md

## Architecture Overview

The evaluation framework is a **separate crate** (`covalence-eval`) that independently evaluates each pipeline stage: chunking, extraction, and search. It uses fixture files with gold-standard annotations, produces typed metrics (precision, recall, F1, nDCG, coverage), and supports regression gating against saved baselines. RAGAS metric traits are defined for end-to-end RAG quality.

This is the most isolated subsystem — the evaluation spec (11-evaluation.md) shares fewer concepts with other spec sections than any other section. The design doc bridges that gap by mapping evaluation metrics to specific subsystem behaviors.

## Implemented Components

### Fully Implemented ✅

| Component | File | Notes |
|-----------|------|-------|
| **Layer evaluator trait** | `lib.rs` | Generic `LayerEvaluator` trait with typed input/output/metrics |
| **Chunker evaluator** | `chunker_eval.rs` | Measures coverage, chunk count, size distribution vs expected output |
| **Extractor evaluator** | `extractor_eval.rs` | Precision/recall/F1 against gold-standard entity annotations |
| **Search evaluator** | `search_eval.rs` | P@K, nDCG, MRR against relevance judgments |
| **RAGAS metric types** | `ragas.rs` | All 4 RAGAS metrics: faithfulness, answer relevancy, context precision, context recall |
| **Fixture loader** | `fixtures.rs` | JSON fixture format: document + expected entities + search queries with relevance |
| **Regression gating** | `regression.rs` | Compare current metrics against baseline, pass/fail per metric |
| **CLI harness** | `main.rs` | **VERIFIED (#4)**: `clap`-based CLI runs all layer evaluations against fixture files; tested end-to-end |
| **Typed metrics** | `metrics.rs` | `ChunkerMetrics`, `ExtractorMetrics`, `SearchMetrics` — serializable |
| **Query trace** | `search/trace.rs` | Per-query execution metadata: strategy, dimension counts, cache hit, abstention |

### Partially Implemented 🟡

| Component | Status | Gap |
|-----------|--------|-----|
| **RAGAS computation** | Trait + types defined | Actual LLM-based faithfulness/relevancy computation not implemented — stubs only |
| **Query trace storage** | Struct defined | Not persisted to PG table — logged only |
| **Fixture datasets** | Loader exists | No fixture files created yet — evaluators have no data to run against |

### Not Implemented ❌

| Component | Spec Reference | Priority |
|-----------|---------------|----------|
| **CI integration** | Spec 11: "Regression Testing" | High — evaluators exist and work, but don't run in CI |
| **Fixture generation** | Spec 11: "Evaluation Dataset" | High — need gold-standard test documents with annotations |
| **RAGAS LLM pipeline** | Spec 11: automated RAG scoring | Medium — requires LLM calls for faithfulness decomposition |
| **Epistemic metrics** | Spec 07/11: confidence calibration, contradiction detection rate | Medium — epistemic module has no eval coverage |
| **Knowledge graph quality** | Spec 11: "Knowledge Graph Quality" (Zaveri dimensions) | Medium — Zaveri 2016 in KB, needs mapping |
| **Prompt ablation testing** | Spec 11: systematic prompt variation evaluation | Low |
| **Online evaluation** | Spec 11: production query quality monitoring | Low — query trace is the foundation |
| **Embedding landscape metrics** | `ingestion/landscape.rs` → eval | Low — landscape analysis exists but isn't evaluated |

## Key Design Decisions

### Why layer-by-layer evaluation
Each pipeline stage (chunk → extract → search) can fail independently. Layer evaluation isolates which stage is degrading. A poor search result could be caused by bad chunking, bad extraction, or bad ranking — layer eval tells you which.

### Why fixture-based over production-only
Fixtures provide reproducible evaluation — same input, expected output, deterministic metrics. Production evaluation captures real-world distribution but can't regression-test against known-good behavior. Both are needed; fixtures first.

### Why RAGAS over custom metrics
RAGAS (Shahul Es et al. 2023) provides a well-validated framework with reference-free metrics — no need for human-annotated answers. The four metrics map directly to Covalence quality concerns:
- **Faithfulness** → Are articles grounded in sources?
- **Answer relevancy** → Does search return what was asked?
- **Context precision** → Is noise in search results minimized?
- **Context recall** → Are all relevant chunks found?

### Why regression gating
Metrics naturally fluctuate. Regression gating compares current metrics against a saved baseline with configurable tolerance. This catches degradation (e.g., extraction F1 drops after a prompt change) while allowing natural variation.

## Connections to Other Subsystems

| Subsystem | Eval Metric | Status |
|-----------|------------|--------|
| **Ingestion/Chunking** | Coverage, chunk count, size distribution | ✅ ChunkerEval implemented |
| **Ingestion/Extraction** | Entity P/R/F1 | ✅ ExtractorEval implemented |
| **Search** | P@K, nDCG, MRR | ✅ SearchEval implemented |
| **Search/RAG** | RAGAS faithfulness, relevancy, precision, recall | 🟡 Types only |
| **Epistemic** | Confidence calibration (predicted vs actual accuracy) | ❌ Not started |
| **Epistemic** | Contradiction detection rate, convergence speed | ❌ Not started |
| **Graph** | Community coherence, bridge detection accuracy | ❌ Not started |
| **Consolidation** | Cluster purity, ontology stability across runs | ❌ Not started |
| **KG Quality** | Zaveri 18-dimension framework | ❌ Not started |

## Gaps Identified

1. **No fixture data** — the evaluators now work (#4) but have nothing to evaluate. Need
   gold-standard documents with annotated entities, relationships, and search relevance judgments.

2. **RAGAS is stubs** — the trait and types exist but actual computation requires decomposing
   answers into claims and checking each against context. This needs LLM calls.

3. **Epistemic eval completely absent** — confidence calibration is the most important missing
   metric. If the system says confidence=0.8, it should be correct 80% of the time.

4. **Zaveri dimensions unmapped** — the KG Quality paper provides 18 quality dimensions. None are
   computed or tracked. The mapping table from the paper is a direct implementation roadmap.

5. **No online monitoring** — query traces exist but aren't analyzed. Could detect degradation in
   real-time (e.g., abstention rate climbing, average result count dropping).

## Academic Foundations

| Concept | Paper | Status in KB |
|---------|-------|-------------|
| RAGAS framework | Shahul Es et al. 2023 | ✅ Ingested |
| KG Quality dimensions | Zaveri et al. 2016 | ✅ Ingested |
| nDCG | Järvelin & Kekäläinen 2002 | ❌ Classic IR, not ingested |
| Precision/Recall/F1 | — | Textbook, not ingested |
| Confidence calibration | Guo et al. 2017 "On Calibration" | ❌ Not ingested |
| Design Science evaluation | Hevner et al. 2004 | ✅ Ingested |

## Next Actions

1. Generate fixture files from the current corpus — use existing search queries + manual relevance labels
2. Wire eval harness into CI (cargo test or dedicated eval binary)
3. Implement RAGAS faithfulness computation (claim decomposition + entailment checking)
4. Define epistemic calibration metric and add to eval
5. Map Zaveri's 18 dimensions to Covalence-specific quality checks
6. Ingest confidence calibration paper (Guo et al. 2017)

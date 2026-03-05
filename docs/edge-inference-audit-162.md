# Edge Inference Bias Audit — covalence#162

**Auditor:** subagent (depth 1)  
**Date:** 2026-03-05  
**Scope:** `infer_article_edges` slow-path handler; adversarial edge (CONTRADICTS / CONTENDS) generation failure  
**Files audited:** `engine/src/worker/infer_article_edges.rs`; 735 KB articles via direct DB queries

---

## 0. Executive Summary

The `infer_article_edges` pipeline has **never produced a single CONTRADICTS or CONTENDS edge between two articles** — not one, across 735 articles and 4,333 article-to-article edges. This audit identifies **four independent structural causes**, a secondary confidence-overflow bug, and a previously undetected content-corruption issue that compounds every problem above. A multi-part fix is proposed.

---

## 1. DB-Level Edge Distribution (article-to-article only)

| edge_type   | count | avg_conf | min_conf | max_conf |
|-------------|------:|--------:|--------:|--------:|
| RELATES_TO  | 3,467 | 0.849   | 0.500   | **1.630** |
| EXTENDS     |   410 | 0.893   | 0.800   | 0.950   |
| CHILD_OF    |   248 | 1.000   | 1.000   | 1.000   |
| CONFIRMS    |   190 | 0.895   | 0.800   | 0.950   |
| SPLIT_INTO  |    10 | 1.000   | 1.000   | 1.000   |
| MERGED_FROM |     6 | 1.000   | 1.000   | 1.000   |
| ORIGINATES  |     1 | 1.000   | 1.000   | 1.000   |
| CONTRADICTS |   **0** | —       | —       | —       |
| CONTENDS    |   **0** | —       | —       | —       |

For reference: CONTRADICTS (12) and CONTENDS (8) do exist in the graph, but exclusively as **source-to-source** or **source-to-article** edges, never article-to-article.

---

## 2. Sample Analysis

### 2.1 CONFIRMS Sample (n=20, top by confidence)

All sampled CONFIRMS pairs carry confidence 0.90–0.95. Manual review of titles and content excerpts:

| # | Source Title | Target Title | True relationship | Notes |
|---|---|---|---|---|
| 1 | Practical Defenses: Social Vouching… Part B | covalence#137 Confidence Propagation… Part B | RELATES_TO | Different topics; shared Ethereum EIP boilerplate prefix (split bug) |
| 2 | Covalence v2 Architecture… Part A | Covalence v2 Architecture… Part A | CONFIRMS ✓ | Duplicate node (data quality issue) |
| 3 | NC-3/NC-5 Research… Part A | NC-3/NC-5 Research… Part 1 | SPLIT_INTO | Same content, naming mismatch; should be SPLIT_INTO, not CONFIRMS |
| 4 | Security Patterns Catalog… Part B | Reputation Poisoning… Part B | RELATES_TO | Different topics; shared Ethereum EIP boilerplate prefix |
| 5 | Threat Modeling… Part B | Auth/OAuth… Part B | RELATES_TO | Different topics; shared Ethereum EIP boilerplate prefix |
| 6 | Reputation Poisoning… Part B | covalence#137 Confidence Propagation… Part B | RELATES_TO | Different topics; shared Ethereum EIP boilerplate prefix |
| 7 | Explainability and Interpretability… Part A | Explainability and Interpretability… Part A | CONFIRMS ✓ | Duplicate node |
| 8 | Zero Trust Architecture… Part B | covalence#137 Confidence Propagation… Part B | RELATES_TO | Different topics; shared Ethereum EIP boilerplate prefix |
| 9 | Security Patterns Catalog… Part B | Zero Trust Architecture… Part B | RELATES_TO | Related security topics, but not corroborating same claim |
| 10 | Security Patterns Catalog… Part B | covalence#137 Confidence Propagation… Part B | RELATES_TO | Different topics; shared Ethereum EIP boilerplate prefix |
| 11 | Vector Clocks… Part B | Navigation and Orientation… Part B | RELATES_TO | Different topics; shared MTEB Leaderboard boilerplate prefix |
| 12 | Threat Modeling… Part B | Rust Performance… Part B | RELATES_TO | Completely different domains; shared boilerplate prefix |
| 13 | Requirements Engineering… Part A | Process Improvement Frameworks… Part A | RELATES_TO | Different topics; shared covalence#84 header prefix |
| 14 | NC-3/NC-5 Research… Part A | NC-3/NC-5 Research… Part B | SPLIT_INTO | A/B split of same article; should be SPLIT_INTO |
| 15 | Structural Self-Improvement… Part B | Covalence Self-Improvement Roadmap… Part B | RELATES_TO | Related but not corroborating; shared Navigation boilerplate |
| 16 | PostgreSQL Data Patterns… Part A | PostgreSQL Scaling and Extensions… Part A | EXTENDS | Different PostgreSQL subtopics; shared covalence#93 header; should be EXTENDS not CONFIRMS |
| 17 | Covalence Self-Improvement Roadmap… Part A | Covalence Phase A Completion… | RELATES_TO / CONTENDS | Roadmap describes planned work; Phase A Completion records what shipped — temporal divergence |
| 18 | Computational Epistemology… Part A | Temporal and Self-Organizing Knowledge Graphs | RELATES_TO ✓ | Plausible CONFIRMS but different theoretical framing; RELATES_TO is fair |
| 19 | Covalence Self-Improvement Roadmap… Part A | Covalence Extension Stack and Research Literature | RELATES_TO ✓ | Fair classification |
| 20 | Automated Knowledge Quality Monitoring… Part A | Temporal and Self-Organizing Knowledge Graph Architectures | RELATES_TO ✓ | Fair classification |

**CONFIRMS classification verdict:**

| Category | Count | % |
|---|---:|---:|
| Correctly CONFIRMS (true duplicates) | 2 | 10% |
| False CONFIRMS — should be RELATES_TO (split-bug artifacts) | 11 | 55% |
| False CONFIRMS — should be SPLIT_INTO | 2 | 10% |
| False CONFIRMS — should be EXTENDS | 1 | 5% |
| False CONFIRMS — possible CONTENDS (temporal divergence) | 1 | 5% |
| Arguably acceptable CONFIRMS / RELATES_TO | 3 | 15% |

**False-CONFIRMS rate: ~72%** (14–15/20 pairs are misclassified).  
**Correctly-CONFIRMS rate: ~10–25%** at best (2–5/20).

No pair in the CONFIRMS sample should be classified CONTRADICTS, but **pair #17** is a genuine CONTENDS candidate: the Self-Improvement Roadmap article describes planned capabilities that diverge from what the Phase A Completion article records as actually shipped.

### 2.2 RELATES_TO Sample (n=10, top by confidence)

| # | Source Title | Target Title | True relationship | Notes |
|---|---|---|---|---|
| 1 | Curriculum Learning… Part A | Multi-Agent Coordination… Part A | RELATES_TO ✓ | Both AI-research articles; but driven by shared heartbeat boilerplate |
| 2 | Curriculum Learning… Part A | Self-Improvement Mechanisms… Part A | RELATES_TO ✓ | Same domain; both heartbeat-contaminated |
| 3 | Knowledge Distillation… Part A | Prompt Engineering… Part A | RELATES_TO ✓ | Both federated KM theory; possibly EXTENDS |
| 4 | Multi-Layered Defense Architecture… Part A | P2P Network Topology and BFT… Part A | RELATES_TO ✓ | Both distributed systems security; possibly EXTENDS |
| 5 | Prompt Engineering… Part A | Computational Epistemology… Part B | RELATES_TO ✓ | Related; Part B has ZKP split-bug content |
| 6 | Curriculum Learning… Part A | Trust and Reputation Systems… Part A | RELATES_TO ✓ | Both AI research; heartbeat-contaminated |
| 7 | Temporal Knowledge Representation… Part A | Prompt Engineering… Part A | RELATES_TO ✓ | Plausible; possibly EXTENDS |
| 8 | ZKP for Knowledge Verification… Part A | Trust and Reputation Systems… Part A | **CONFIRMS** | Both discuss stored-procedures migration #143; corroborating same event |
| 9 | ZKP for Knowledge Verification… Part A | covalence#137 Confidence Propagation Phase 1 Spec… Part A | **CONFIRMS / EXTENDS** | ZKP article covers same implementation spec; should be CONFIRMS or EXTENDS |
| 10 | Ontology Lifecycle: Bootstrapping… Part A | Covalence Ontology Strategy: SKOS… Part A | **EXTENDS** | Strategy article is a direct specialization of lifecycle principles |

**RELATES_TO classification verdict:**

| Category | Count | % |
|---|---:|---:|
| Correctly RELATES_TO | 7 | 70% |
| Under-classified — should be CONFIRMS | 1 | 10% |
| Under-classified — should be EXTENDS or CONFIRMS | 1 | 10% |
| Under-classified — should be EXTENDS | 1 | 10% |

**Correct-RELATES_TO rate: ~70%** but with a 30% under-classification problem (stronger edges being missed). None of the RELATES_TO sample appeared to be a CONTRADICTS or CONTENDS.

Critically, **3 of the 10 RELATES_TO pairs carry confidence > 1.0** (values: 1.63, 1.60, 1.49, 1.48, 1.47, 1.47, 1.44) — this is a confidence overflow bug (see §4).

---

## 3. Estimated False-Negative Rates for Adversarial Edges

| Edge Type | Current count (article-to-article) | Estimated true prevalence | False-negative rate |
|---|---:|---|---|
| CONTRADICTS | **0** | ~10–25 pairs expected in a 735-article KB with overlapping topics | **~100%** |
| CONTENDS | **0** | ~40–80 pairs expected (temporal divergence between heartbeat articles alone provides ≥30 candidates) | **~100%** |

These estimates are conservative. The 38 confirmed heartbeat-contaminated articles include multiple beat-N vs beat-M pairs where the earlier article's "ENGINE STATE" snapshot contradicts the later one on specific facts (commit hash, test count, queue depth, etc.). Each such pair is a genuine CONTENDS candidate.

---

## 4. Root Cause Analysis — Four Independent Structural Causes

### Cause A: CONTENDS Missing from Tier 4 Prompt (known — covalence#162 §1)

The `llm_infer_directionality` prompt specifies:

```
{"relationship": "EXTENDS|CONFIRMS|CONTRADICTS|RELATES_TO", ...}
```

CONTENDS is absent from both the enum options and the definitions. The LLM has no vocabulary to output it, so every nuanced partial-disagreement collapses to RELATES_TO or CONTRADICTS. In practice it always picks RELATES_TO or CONFIRMS for borderline cases.

### Cause B: Tier 4 LLM Gate is Too Restrictive — Dual Condition

The LLM is only invoked when **both** conditions hold:

```rust
if config.llm_enabled && combined_score >= config.llm_threshold && edge_type == "RELATES_TO"
```

This has two problems:

1. **`combined_score >= 0.85` skips LLM for moderate-overlap pairs** — The content-corruption issue (§5) inflates scores; clean moderate-overlap pairs (0.65–0.85) never reach the LLM. Contradictions are more likely in moderate-overlap pairs (shared topic, different conclusions) than high-overlap pairs.

2. **`edge_type == "RELATES_TO"` excludes EXTENDS edges** — If structural signals assign EXTENDS, the LLM never runs. A pair like "Ontology Lifecycle → SKOS Strategy" gets RELATES_TO correctly, but a pair where the subset-domain heuristic fires (assigning EXTENDS) can never become CONTRADICTS or CONTENDS even if the articles factually conflict.

### Cause C: Content-Corruption from Split Bug Inflates Similarity (newly identified)

The `article_split` worker produces **Part B articles that inherit a verbatim content block from an unrelated article** as their opening section. This is a split bug — the Part B content does not begin from the correct split point.

Observed corrupted content prefixes (shared across 2–11 unrelated Part B articles each):

| Prefix fragment | # articles sharing it |
|---|---:|
| `## Overcoming Catastrophic Forgetting: EWC…` | 11 |
| `## Rule-Based Reasoning Over Knowledge Graphs…` | 7 |
| `## Ethereum EIP Process and Hard Fork Coordination…` | 7 |
| `## Anthropic Claude Prompt Engineering: XML Structure…` | 7 |
| `## SYNTHESIS: Navigation and Orientation Architecture…` | 6 |
| `## LangMem SDK Architecture and Implementation` | 6 |
| `## MTEB Leaderboard 2025: Top Embedding Models…` | 6 |

Additionally, **38 articles** have heartbeat/status-report content (`ENGINE STATE`, `Current Focus`, beat numbers) injected into their bodies, most as the first 300–600 characters. This severely pollutes embeddings for those articles.

The effect: unrelated articles sharing a 300–500 char boilerplate prefix achieve spuriously high Jaccard + cosine similarity (combined_score easily > 0.9), pushing them past the LLM threshold. The LLM then sees the same boilerplate in both excerpts (only 500 chars are fetched) and reasonably concludes they "corroborate each other" → CONFIRMS.

**This single bug is responsible for approximately 55% of false-CONFIRMS edges.**

### Cause D: Tier 4 Excerpt Window Too Narrow (500 chars) — Reads Only Boilerplate

The Tier 4 prompt fetches `LEFT(content, 500)`. For split-corrupted or heartbeat-contaminated articles, the first 500 characters are entirely boilerplate (heartbeat header or a foreign article's section). The LLM never sees the actual article content and classifies based on structural noise.

Even for clean articles, 500 characters is often insufficient to reveal methodological disagreement. A 2,000-character window sampling from the middle of the article (e.g., `SUBSTRING(content, 200, 2000)`) would expose substantive claims rather than preamble.

---

## 5. Secondary Bug: Confidence Overflow (> 1.0)

The `combined_score` formula:
```rust
let tier3 = self.tier3_sim * (0.7 + self.tier3_sim);
```

For `tier3_sim ≈ 0.95`: `combined_score = 0.95 * 1.65 = 1.5675`.

Then:
```rust
let mut confidence = if tier1_ok {
    0.9_f32.max(combined_score)   // = 1.5675 — NOT capped
} ...
```

Only the Tier-3-only path caps at 0.95; Tier 1 and Tier 2 paths do not. Result: **43 RELATES_TO edges carry confidence > 1.0** (max observed: 1.630). The LLM path does clamp via `.clamp(0.5, 0.95)`, so LLM-refined edges are unaffected.

**Fix**: add `.min(0.95)` to all three confidence assignment branches, or cap once before insertion.

---

## 6. Fix Design

### 6.1 Revised Tier 4 Prompt

Replace the current prompt string in `llm_infer_directionality` with:

```rust
let prompt = format!(
    "You are a knowledge-graph edge classifier. \
     Determine the semantic relationship between Article A and Article B.\n\n\
     Article A:\nTitle: {subject_title}\nExcerpt: {subject_excerpt}\n\n\
     Article B:\nTitle: {candidate_title}\nExcerpt: {candidate_excerpt}\n\n\
     Return ONLY valid JSON (no markdown fences):\n\
     {{\"relationship\": \"EXTENDS|CONFIRMS|CONTENDS|CONTRADICTS|RELATES_TO\", \
       \"confidence\": 0.0..1.0, \
       \"reasoning\": \"one sentence\"}}\n\
     Definitions:\n\
     - EXTENDS: A elaborates, specialises, or builds upon B (or vice-versa); same thesis, added depth\n\
     - CONFIRMS: A and B make the same factual claims and mutually corroborate each other\n\
     - CONTENDS: A and B address the same topic but reach different conclusions, \
       recommend different approaches, or reflect different snapshots of evolving state \
       (partial, nuanced, or perspective-based disagreement)\n\
     - CONTRADICTS: A and B make directly conflicting factual assertions (one says X, \
       the other says not-X)\n\
     - RELATES_TO: topically related but no stronger relationship applies\n\
     Choose the STRONGEST applicable relationship. Prefer CONTENDS over RELATES_TO \
     whenever the articles take different positions on the same question."
);
```

And update the parse block to accept `CONTENDS`:
```rust
let edge_type = match rel {
    "EXTENDS" | "CONFIRMS" | "CONTENDS" | "CONTRADICTS" => rel.to_string(),
    _ => "RELATES_TO".to_string(),
};
```

### 6.2 Widen the Excerpt Window and Avoid Boilerplate

Change the excerpt query from `LEFT(content, 500)` to a middle-biased window:

```sql
SELECT id, title,
       SUBSTRING(content, 150, 2000) AS excerpt
FROM covalence.nodes
WHERE id = ANY($1)
```

Skipping the first ~150 chars avoids most status-report headers. A 2,000-char window is still well within LLM context and gives the model substantive content to classify.

### 6.3 Remove the `edge_type == "RELATES_TO"` Gate

Change:
```rust
if config.llm_enabled && combined_score >= config.llm_threshold && edge_type == "RELATES_TO"
```
to:
```rust
if config.llm_enabled && combined_score >= config.llm_threshold
    && !matches!(edge_type.as_str(), "CHILD_OF" | "SPLIT_INTO" | "MERGED_FROM")
```

This allows the LLM to reclassify EXTENDS edges as CONTENDS or CONTRADICTS when warranted. Structural relationships (CHILD_OF, SPLIT_INTO, MERGED_FROM) are excluded since they are set by explicit operations, not semantic inference.

### 6.4 Add a Dedicated Adversarial Pass (Second-Stage Contradiction Check)

The current architecture uses one combined prompt to both identify relationship type and detect contradictions. For a KB where adversarial edges are systematically missed, a **separate contradiction-detection pass** is more effective.

Proposed approach: add a `llm_detect_contradiction` function that runs **only** for pairs that would otherwise be classified CONFIRMS or EXTENDS. This function uses a focused adversarial prompt:

```text
You are checking for factual conflicts or methodological disagreements.
Article A and Article B have been pre-classified as related (CONFIRMS / EXTENDS).

Does Article A CONTRADICT or CONTEND with Article B?

- CONTRADICTS: direct factual conflict (A says X, B says explicitly not-X)
- CONTENDS: different approach, recommendation, or temporal snapshot of the same system state

Return JSON: {"conflict": "none|contends|contradicts", "confidence": 0.0..1.0, "evidence": "..."}
Err strongly on the side of "none" unless the evidence is clear.
```

This two-pass design:
1. Prevents the main classifier from having to balance "find agreement" vs "find disagreement" simultaneously
2. Can run at a lower threshold (`adversarial_threshold: 0.70`) to catch moderate-overlap pairs
3. Isolates the adversarial signal so false-positive CONTRADICTS don't pollute the main classifier

**Recommended `InferenceConfig` additions:**
```rust
pub struct InferenceConfig {
    // ...existing fields...
    /// Threshold below llm_threshold above which the adversarial check runs.
    pub adversarial_threshold: f32,   // default: 0.70
    /// When true, run the dedicated contradiction-detection pass.
    pub adversarial_pass_enabled: bool, // default: true
}
```

### 6.5 Fix Confidence Overflow

In `handle_infer_article_edges`, cap confidence before insertion:
```rust
confidence = confidence.clamp(0.0, 0.95);
```
Add this line immediately before the edge insert, after all four branches (structural + LLM).

### 6.6 Fix the Article Split Bug (Separate Issue)

The content-corruption from split artifacts is a prerequisite fix for any edge-inference improvement. Until Part B articles have correct content, embedding similarity and LLM excerpts will be systematically polluted. This should be tracked as a separate issue but is **blocking** for edge-inference quality.

Recommend:
- Audit `article_split` worker to find why Part B inherits a foreign content prefix
- Add a content-validation step post-split to detect duplicate leading segments
- Recompile affected Part B articles from their source documents

---

## 7. Prioritized Fix Plan

| Priority | Fix | Effort | Impact |
|---|---|---|---|
| P0 | Add CONTENDS to Tier 4 prompt (§6.1) | 5 min | Unblocks CONTENDS generation immediately |
| P0 | Accept CONTENDS in parse block (§6.1) | 2 min | Required with above |
| P1 | Remove `edge_type == "RELATES_TO"` gate (§6.3) | 5 min | Enables CONTRADICTS on EXTENDS pairs |
| P1 | Widen excerpt window to 2000 chars, skip first 150 (§6.2) | 5 min | Reduces boilerplate pollution |
| P2 | Fix confidence overflow `.clamp(0.0, 0.95)` (§6.5) | 2 min | Data hygiene |
| P2 | Add adversarial second pass (§6.4) | 2–3 days | Systematic contradiction discovery |
| P3 | Fix article split bug (§6.6) | TBD | Prerequisite for clean similarity scores |
| P3 | Lower adversarial_threshold to 0.70 (§6.4) | 1 hour | Catches moderate-overlap contradictions |

The P0 fixes alone (CONTENDS in prompt + parse) will produce some CONTENDS edges on the next inference run. The P1 excerpt-window fix is critical to ensure those edges are classified on actual content, not boilerplate.

---

## 8. Appendix: Articles Flagged for Follow-up

- **Split-bug victims** (11 articles sharing Catastrophic Forgetting prefix, 7 sharing Ethereum EIP prefix, 7 sharing Anthropic Claude prefix, etc.): content audit needed
- **Duplicate nodes** (e.g., two nodes with identical title "Covalence v2 Architecture… Part A", two "Explainability and Interpretability… Part A"): deduplication needed
- **Heartbeat-contaminated articles** (38 articles): content cleanup or re-compilation needed
- **Confidence > 1.0 edges** (43 RELATES_TO edges): should be corrected by the clamp fix

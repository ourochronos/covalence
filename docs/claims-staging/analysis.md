# P0-3 Staging Validation — Analysis Report
**Date:** 2026-03-05  
**Script:** run_extraction_v3.py  
**Model:** gpt-4o-mini  
**Entity list:** 102 canonical entries

---

## Verdict: ✅ GO

All GO criteria met. Phase 3 (blue-green migration, covalence#169) is cleared to proceed.

---

## Summary Statistics

| Metric | P0-1 Pilot (20 sources) | P0-3 Staging (100 sources) |
|---|---|---|
| Sources processed | 20/20 (100%) | **100/100 (100%)** |
| Failures | 0 | **0** |
| Total claims | 231 | **1,706** |
| Avg claims/source | ~11.6 | **17.1** |
| Median claims/source | — | **16** |
| Min/Max claims | — | **3 / 42** |
| Avg confidence | — | **0.854** |
| Confidence ≥0.80 | — | **100.0% (1706/1706)** |
| Confidence ≥0.85 | — | **99.5% (1698/1706)** |
| Confidence ≥0.90 | — | **9.1% (156/1706)** |
| Temporal claims | — | **75.9% (1295/1706)** |
| Null entity | — | **0 (0.0%)** |

---

## GO Criteria Check

| Criterion | Threshold | Result | Status |
|---|---|---|---|
| Sources processed | ≥95/100 | 100/100 | ✅ PASS |
| Avg confidence | ≥0.80 | 0.854 | ✅ PASS |
| Failure rate | ≤5% | 0% | ✅ PASS |
| Null entity rate | — | 0.0% | ✅ PASS |
| Structural failures | 0 | 0 | ✅ PASS |

---

## Domain Coverage

| Domain | Sources |
|---|---|
| ops-beats | 41 |
| ml/ai | 20 |
| covalence/claims | 17 |
| pilot-rerun | 16 |
| general | 4 |
| reactive/ui | 1 |
| build-systems | 1 |

---

## Top Entities

| Entity | Claims |
|---|---|
| Covalence | 1,336 |
| jane-ops | 31 |
| Gemini | 27 |
| Obsidian | 22 |
| Cache Invalidation Strategies Across Distributed Systems | 21 |
| OpenAI | 20 |
| Bazel | 17 |
| Deployment Tier Framework | 15 |

---

## Notable Observations

### 1. Perfect success rate at scale
100/100 sources extracted with no failures, matching the 20/20 pilot. The extraction pipeline is robust across all source types, lengths (260–17,370 chars), and domains.

### 2. Higher claim yield vs pilot
Average claims per source increased from ~11.6 (pilot) to 17.1 (staging). This is expected: the 100-source set includes denser technical documents and longer sources (e.g., 16k-char specs). The max of 42 claims came from a cross-domain research synthesis document — the list-expansion rule is functioning correctly.

### 3. Confidence is uniformly high
100% of claims at ≥0.80, 99.5% at ≥0.85. The model is consistently confident. The 0.85 floor dominates (gpt-4o-mini defaults to this for well-formed sources), with 0.90+ reserved for crisp, directly-stated facts (9.1% of claims).

### 4. Temporal flag prevalence (75.9%)
The ops-beats domain (41 sources) dominates the sample and consists almost entirely of heartbeat/state snapshots — volatile by definition. The temporal rate reflects the source composition, not a tagging issue. For a production run with broader domain coverage, temporal rate will likely be lower (~40–50%).

### 5. Entity normalization working at scale
Zero null entities across 1,706 claims. The 102-entity canonical list is sufficient for the current source corpus. Covalence dominates (78% of entity assignments) as expected given the KB's self-referential nature.

### 6. Short sources handled correctly
The smallest sources (260–500 chars, mostly memory nodes) produced 3–12 claims — within spec. No sources were skipped for insufficient content in this run (all exceeded the 200-char threshold).

---

## Pre-Conditions for Phase 3

1. **Deduplication logic required before write**: The high temporal rate and many similar ops-beats sources will generate overlapping claims (e.g., multiple beats reporting the same commit SHA). A hard-threshold dedup pass (string similarity + entity match) must run before claims are written to the DB.
2. **Temporal claims need TTL or staleness handling**: 75.9% temporal rate means most claims from ops-beats will become stale quickly. The migration plan should include a TTL policy for temporal claims.
3. **Entity normalization list should be treated as living**: The current 102 entities cover the corpus well, but production ingestion will encounter new entities. A fallback mechanism (most natural proper noun) is already coded and working.

---

## Output Files

- `results.json` — Full extraction results (100 sources × up to 42 claims each)
- `analysis.md` — This document

# Domain-Decomposed Score Skew (DDSS) — Self-Referential Search Routing

## Status: Implemented (Session 40, #138)

## Problem

Covalence cannot answer questions about itself. With 148 research papers vs 14 spec documents, research content dominates all search dimensions (vector, lexical, graph, structural). The system treats its own authoritative domain (spec, design, code) as just more knowledge in a flat pool.

## Root Cause Analysis (Claude x Gemini, 2026-03-17)

1. **Domain drowning (volume)**: Research papers are longer, more densely connected, and use more formal terminology, giving them higher scores across all dimensions.
2. **Meta-query blindness**: SkewRoute classifies queries by embedding distribution skew (Gini coefficient), not by intent. A self-referential query gets the same strategy routing as a general knowledge query.
3. **Post-fusion-only filtering**: Domain filters in SearchFilters apply after fusion. If internal content never makes the top candidates, the filters have nothing to work with.

## Solution: DDSS Hybrid

### At query time (O(N), <500ms)
After enrichment (Step 8 in the search pipeline), partition fused results by source domain:
- **Internal**: spec, design, code
- **External**: research, external

Compute `max_score_ratio = max(internal_scores) / max(external_scores)`.
- If ratio >= 0.7 → boost all internal results by 1.5x
- Secondary signal: if 3+ top-10 results are code-class entities → boost

### At ingestion time (migration 015)
Compute per-node **domain entropy** from extraction provenance:
- Shannon entropy: `-sum(p_d * log2(p_d))` over domain distribution
- Low entropy = internal concept (primarily one domain)
- High entropy = cross-cutting (appears in many domains)

Stored as `nodes.domain_entropy` and `nodes.primary_domain`.

## Key Design Decisions

1. **Max-score ratio over per-domain Gini**: Small samples (3-5 spec results in top 100) make Gini statistically unreliable. Max-score ratio needs only 1 result per domain.

2. **Boost factor 1.5x**: Conservative enough to avoid burying genuinely relevant research, strong enough to surface internal content that was previously outranked.

3. **Threshold 0.7**: Internal content needs to be at least 70% as strong as external content for the boost to fire. This prevents boosting weak internal matches.

4. **Code-entity secondary signal**: Code entities (struct, function, trait) in top-10 indicate implementation-focused queries even without source_domain (nodes don't belong to a single source).

## Failure Modes Considered

1. **PostgreSQL problem**: Cross-cutting entities mentioned in all domains produce flat max-score ratios. The domain_entropy field identifies these — high entropy means cross-cutting, rely on base scores.

2. **Cold start**: New domains with few sources produce noisy signals. The threshold (0.7) is conservative enough to avoid false positives.

3. **False positive boost**: A query about "Covalence vs Neo4j" mentions the system name but should return competitive analysis (research). The DDSS looks at score ratios, not keywords, so it only fires when internal content actually scores well.

## Verification

- `cove ask "How does entity resolution work in our implementation?"` → returns spec + code + research mix (was research-only before)
- Search regression: 20/20 stable across all changes
- Coverage score: 80.7% (improved from 77.2% after noise filtering)

# Invariants

Properties Covalence must hold. Violations are bugs in the engine, not edge cases. Sourced from `CLAUDE.md`'s "Hard Rules" section; this document is the canonical authoritative form, and `CLAUDE.md` is the day-to-day reminder for assistants.

## INV-1: PostgreSQL is the source of truth

The petgraph sidecar is a derived, rebuildable cache. If the in-memory graph diverges from PostgreSQL state, PostgreSQL wins.

**Implications:**
- Sidecar mutations are unidirectional from PG.
- Audit divergence; rebuild rather than reconcile in place.
- Tooling that asks "what is the current graph?" reads PG semantics, not the sidecar.

## INV-2: Every fact has pristine canonical provenance

No node, edge, or chunk exists without a provenance link pointing to immutable byte offsets in the canonical source text. All mutated text (e.g., from fastcoref) must be reverse-projected through the Offset Projection Ledger before storage in PostgreSQL.

**Implications:**
- Ingestion paths that bypass the ledger are bugs.
- Synthetic facts (deduced edges, consolidated nodes) link to their derivation source.
- Chunks without offsets are rejected.

## INV-3: No attention dilution in extraction

The pipeline uses a strict two-pass LLM extraction model (Statements → Triples). Statement generation and entity extraction must not be merged into a single prompt.

**Implications:**
- New extraction features extend the two-pass shape rather than collapse it.
- Single-prompt experiments require explicit ADR justification.

## INV-4: LLM selection is deliberate and provider-attributed

Every LLM call uses the `ChatBackend` abstraction with `ChainChatBackend` for multi-provider failover. Default chain: Claude Haiku → Copilot → Gemini Flash. Every call records the provider in processing metadata.

**Implications:**
- Direct API calls bypassing `ChatBackend` are bugs.
- Provider attribution flows into observability and cost accounting.

## INV-5: No graph algorithms in SQL

Graph traversal, ranking (PageRank, TrustRank), and community detection execute against the in-memory petgraph sidecar. PostgreSQL Recursive CTEs are forbidden for graph operations.

**Implications:**
- Features needing traversal load relevant nodes/edges into the sidecar first.
- Performance work on graph ops targets petgraph algorithms, not query plans.

## INV-6: Uncertainty is not disbelief

The system uses Subjective Logic opinion tuples (b, d, u, a). "Unknown" (high uncertainty) is not "50% likely" (high disbelief).

**Implications:**
- Confidence stored as opinion tuples or derived via explicit fusion rules.
- Operations that collapse uncertainty to point estimates require justification.
- Decay raises uncertainty rather than skepticism — older facts gain `u`, not `d`.

## INV-7: Secure by default with explicit promotion

All data defaults to `clearance_level = 0` (local_strict). Promotion to federated clearance requires explicit action.

**Implications:**
- Egress filtering is enforced at query time.
- Promotion triggers recursive recalculation of derived entities/edges.
- Default deployments leak nothing.

## INV-8: No synthetic test data

Tests use real data or clearly-marked fixtures. Benchmarks and results are never fabricated.

**Implications:**
- Performance comparisons run against real corpora.
- Quality gates (entity precision, chunk quality) measured on real sources or marked preliminary.

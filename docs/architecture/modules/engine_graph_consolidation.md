# engine_graph_consolidation

petgraph in-memory sidecar — traversal, ranking algorithms (PageRank, TrustRank), community detection — plus HDBSCAN-driven entity resolution and batch/deep consolidation passes. Sync from PostgreSQL is unidirectional.

**Paths:** see [`.changes/catalogs/modules.toml`](../../../.changes/catalogs/modules.toml).

**Related specs:** [spec/04-graph.md](../../../spec/04-graph.md), [spec/05-ingestion.md](../../../spec/05-ingestion.md) (Tier 5 consolidation).

**Anchored invariants:** INV-1 (PG is the source of truth), INV-5 (no graph algorithms in SQL).

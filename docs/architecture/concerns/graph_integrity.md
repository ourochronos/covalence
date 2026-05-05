# graph_integrity

PostgreSQL is the source of truth (INV-1). The petgraph sidecar is a derived, rebuildable cache; sync is unidirectional (PG → petgraph). No graph algorithms in SQL (INV-5) — recursive CTEs are forbidden for traversal/ranking.

Apply when: the module reads from or writes to the graph (nodes, edges, traversal results). Concretely, this is engine_graph_consolidation always, engine_search whenever it touches the graph dimension, engine_ingestion when it persists nodes/edges, and engine_epistemic when it propagates over the graph.

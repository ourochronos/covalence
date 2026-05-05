# engine_ingestion

Statement-first ingestion pipeline: chunking, fastcoref coreference, offset projection ledger, two-pass LLM extraction (statements → triples), embedding generation, the async queue, and source-format adapters.

**Paths:** see [`.changes/catalogs/modules.toml`](../../../.changes/catalogs/modules.toml).

**Related specs:** [spec/05-ingestion.md](../../../spec/05-ingestion.md), [spec/12-code-ingestion.md](../../../spec/12-code-ingestion.md).

**Anchored invariants:** INV-2 (canonical provenance), INV-3 (no attention dilution).

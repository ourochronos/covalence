# observability

Tracing (`tracing` / `tracing-subscriber`), structured logs, Prometheus metrics, processing metadata. For each affected module: what new spans/counters/histograms are added, are existing labels still meaningful, are there cardinality concerns.

LLM calls always record provider attribution in processing metadata (INV-4).

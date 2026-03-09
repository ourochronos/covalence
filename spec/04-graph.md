# 04 — Graph Compute Layer

**Status:** Draft

## Overview

The graph compute layer is a Rust in-memory sidecar built on petgraph. It mirrors the node/edge data from PostgreSQL into a `StableDiGraph` for fast traversal and algorithm execution. PG handles persistence; petgraph handles compute.

**Why `StableDiGraph` over `DiGraph`:** Standard `petgraph::DiGraph` invalidates `NodeIndex` values on node removal (it swaps the last node into the removed slot). Since we delete/archive nodes during organic forgetting and source deletion cascades, this would silently corrupt our `HashMap<Uuid, NodeIndex>` lookup. `StableDiGraph` preserves index stability at the cost of ~20% memory overhead — a trivial price at our scale target.

## Data Structure

```rust
use petgraph::stable_graph::StableDiGraph as DiGraph;
use std::collections::HashMap;
use uuid::Uuid;

/// Metadata attached to graph edges in the petgraph sidecar
struct EdgeMeta {
    id: Uuid,
    rel_type: String,
    weight: f64,
    confidence: f64,
    causal_level: Option<CausalLevel>,  // L0/L1/L2 per Pearl's hierarchy
    clearance_level: u8,
}

#[derive(Clone, Copy)]
enum CausalLevel {
    Association,    // L0 — correlational
    Intervention,   // L1 — causal/evidential
    Counterfactual, // L2 — hypothetical
}

/// Metadata attached to graph nodes (lightweight — full data lives in PG)
struct NodeMeta {
    id: Uuid,
    node_type: String,
    canonical_name: String,
    clearance_level: u8,
}

/// The in-memory graph sidecar
struct GraphSidecar {
    graph: DiGraph<NodeMeta, EdgeMeta>,
    index: HashMap<Uuid, petgraph::graph::NodeIndex>,
}
```

The `index` map provides O(1) lookup from UUID to petgraph's internal `NodeIndex`, following the pattern proven in covalence.

## Sync with PostgreSQL

### Initial Load

On startup, the sidecar loads all nodes and edges from PG:

```
1. SELECT id, node_type, canonical_name, clearance_level FROM nodes
2. SELECT id, source_node_id, target_node_id, rel_type, weight, confidence, clearance_level,
          properties->>'causal_level' as causal_level FROM edges
3. Build DiGraph + index map
```

### Incremental Updates

Options (to be decided):

1. **LISTEN/NOTIFY** — PG triggers on node/edge insert/update/delete fire NOTIFY; the sidecar subscribes via LISTEN. Low latency, PG-native.
2. **Polling** — Periodic query for changes since last sync timestamp. Simple, slightly higher latency.
3. **WAL-based** — Logical replication slot. Most robust but operationally complex.

**Decision: Outbox Pattern + LISTEN/NOTIFY wake-up.**

LISTEN/NOTIFY has an 8KB payload limit, making it unreliable for large batch operations. Instead, PG triggers write change records to an `outbox_events` table (see [03-storage](03-storage.md)). NOTIFY sends an empty "ping" to wake the sidecar, which then queries the outbox for changes since its last processed sequence ID. A 5-second polling fallback ensures changes are never missed.

```rust
async fn sync_loop(pool: &PgPool, graph: SharedGraph) {
    let mut last_seq: i64 = 0;
    let mut listener = PgListener::connect_with(pool).await?;
    listener.listen("graph_sync_ping").await?;

    loop {
        // Wait for ping or timeout (5s fallback)
        let _ = tokio::time::timeout(Duration::from_secs(5), listener.recv()).await;

        let events = sqlx::query_as!(OutboxEvent,
            "SELECT * FROM outbox_events WHERE seq_id > $1 ORDER BY seq_id ASC LIMIT 1000",
            last_seq
        ).fetch_all(pool).await?;

        if !events.is_empty() {
            let mut g = graph.write().await;
            for event in &events {
                g.apply_event(event);
            }
            last_seq = events.last().unwrap().seq_id;
        }
    }
}
```

### Consistency Model

- **Eventually consistent.** Writes go to PG first; the sidecar syncs via outbox.
- **Staleness budget:** Target < 1 second (outbox + NOTIFY ping), < 5 seconds worst case (polling fallback).
- **Rebuild on demand:** An admin endpoint to trigger a full reload from PG.

## Algorithms

### PageRank

Global or personalized. Used for:
- Topological confidence scoring
- Query expansion (PPR from seed nodes)
- Identifying authoritative nodes

```rust
fn pagerank(graph: &DiGraph<NodeMeta, EdgeMeta>, damping: f64, iterations: usize) -> HashMap<Uuid, f64>;
fn personalized_pagerank(graph: &DiGraph<NodeMeta, EdgeMeta>, seed_nodes: &[Uuid], damping: f64, iterations: usize) -> HashMap<Uuid, f64>;
```

**Implementation note:** petgraph provides `page_rank()` but NOT PersonalizedPageRank. PPR requires a custom implementation (~50 lines): standard power iteration where the restart distribution is uniform over seed nodes instead of uniform over all nodes. Similarly, k-core decomposition is not in petgraph or graphalgs — implement the Matula-Beck O(|E|) algorithm directly.

### TrustRank (Batch Global Calibration)

Eigenvector-based global trust computation, run during deep consolidation. Captures network effects that local propagation misses.

```rust
/// Compute global trust scores from a seed set of verified nodes.
/// Trust flows through edges, weighted by edge confidence and causal_weight.
/// Handles cycles via matrix convergence with damping.
fn trust_rank(
    graph: &DiGraph<NodeMeta, EdgeMeta>,
    seed_nodes: &[(Uuid, f64)],  // manually verified high-confidence nodes
    damping: f64,                 // default 0.85
    iterations: usize,            // default 100, converges early
) -> HashMap<Uuid, f64>;
```

**Key insight from research:** A fact supported by 5 moderately reliable sources may be more trustworthy than one from a single highly reliable source. TrustRank captures this diversity-of-evidence effect.

**Schedule:** Run during deep consolidation (daily+). Results are cached and used as a multiplier in search scoring until the next run.

### Community Detection

Identify clusters of related nodes. Used for:
- Dynamic ontology / taxonomy discovery
- Hierarchical summarization
- Query scoping
- Epistemic delta computation (per-cluster)
- Domain topology map generation

**Algorithm: k-core decomposition** (deterministic, density-aware hierarchy).

k-core decomposition replaces modularity-based methods (Louvain/Leiden) for community detection. The rationale (Hossain et al., arXiv:2603.05207):

1. **Determinism**: Modularity optimization on sparse KGs admits exponentially many near-optimal partitions (Theorem 1). Leiden/Louvain produce different communities on each run. k-core decomposition is deterministic — same graph always yields same hierarchy.
2. **Linear time**: O(|E|) vs Leiden's iterative refinement. No convergence tuning needed.
3. **Density-aware hierarchy**: k-shells naturally form nested density layers (k=1 periphery → k=max core). Higher-k cores contain the densest, most interconnected subgraphs — exactly the hubs that matter for summarization.
4. **Better results**: Empirically improves answer comprehensiveness and diversity while reducing token usage on financial, news, and podcast datasets.

**Hierarchy construction:**
- Compute k-core numbers for all nodes (petgraph `k_core`)
- Build nested shells: shell_k = {nodes with core number = k}
- Within each shell, find connected components → these are the communities
- Size-bound communities by splitting components that exceed token budgets
- Preserve cross-shell connectivity for hierarchical summarization

**Incremental maintenance:**
k-core decomposition supports efficient incremental updates (Sarıyüce et al., VLDB 2016). When edges are inserted or removed, only a small subgraph around the affected nodes needs re-evaluation — not the entire graph. The algorithm:
1. On edge insertion `(u,v)`: if `core(u) != core(v)`, check if the lower-core node can be promoted. Walk the K-subgraph of the lower-core node to verify.
2. On edge deletion `(u,v)`: check if either endpoint drops a core level. Walk the K-subgraph to find affected nodes.
3. Amortized cost is proportional to the size of the affected subgraph, not the full graph.

This means community detection runs incrementally after each ingestion batch, not as an expensive periodic recomputation. Community summaries only regenerate for communities whose membership actually changed.

```rust
fn detect_communities(graph: &DiGraph<NodeMeta, EdgeMeta>) -> Vec<Community>;

struct Community {
    id: usize,
    node_ids: Vec<Uuid>,
    label: Option<String>,  // generated post-detection via LLM
    coherence: f64,         // internal edge density vs external
    k_core: u32,            // which k-shell this community belongs to
    parent_community: Option<usize>,  // nesting relationship
}
```

### Community Summaries

Each community gets a generated summary that captures its key themes, entities, and relationships. Community summaries serve two critical purposes:

1. **Global search** — Queries about entire corpus themes ("What are the main topics?") search community summaries, not individual chunks. This is the Microsoft GraphRAG "global search" pattern.
2. **Context compression** — Instead of passing 50 entity descriptions to an LLM, pass the community summary. Dramatic token reduction (STAR-RAG: 97% fewer tokens).

**Generation:**
```json
{
  "system": "Summarize this knowledge graph community. Focus on: key entities, their relationships, dominant themes, and any notable patterns or contradictions.",
  "user": {
    "community_id": "{community.id}",
    "k_core": "{community.k_core}",
    "entities": [{"name": "...", "type": "...", "description": "..."}],
    "relationships": [{"source": "...", "target": "...", "rel_type": "...", "description": "..."}],
    "parent_community_summary": "{parent.summary | null}"
  }
}
```

**Storage:** Community summaries are stored as nodes with `node_type = "community_summary"` and embedded for vector search. They link to their constituent entity nodes via `SUMMARIZES` edges. Re-generated incrementally when community membership changes beyond a threshold (> 20% node churn).

**Hierarchical summaries:** Higher k-core communities get shorter, more abstract summaries (they represent the core themes). Lower k-core communities get more detailed summaries (peripheral topics). Parent community summaries are injected as context when generating child summaries.

**Global search flow:**
1. Query embeds → vector search against community summary embeddings
2. Top-k community summaries retrieved (k=5 default)
3. Map: each summary + query → LLM generates partial answer
4. Reduce: partial answers merged → final comprehensive answer

### Topological Confidence

Derived from graph structure, not stored metadata. Following covalence's approach:

```
confidence(node) = α * normalized_pagerank(node) + β * path_diversity(node)
```

Where:
- `α = 0.6, β = 0.4` (from covalence, tunable)
- `path_diversity` = saturating count of distinct inbound edges / sources
- Higher confidence = more connected, more corroborated

### Shortest Path / BFS / DFS

Standard traversal with optional edge-type filtering and hop-decay:

```rust
fn bfs_neighborhood(graph: &GraphSidecar, start: Uuid, max_hops: usize, edge_filter: Option<&[String]>) -> Vec<(Uuid, usize)>;
fn shortest_path(graph: &GraphSidecar, from: Uuid, to: Uuid) -> Option<Vec<Uuid>>;
```

### Spreading Activation

ACT-R inspired activation spreading for query expansion. Activation flows from seed nodes through edges, decaying with distance:

```rust
fn spreading_activation(graph: &GraphSidecar, seeds: &[(Uuid, f64)], decay: f64, threshold: f64) -> HashMap<Uuid, f64>;
```

### Structural Importance (EWC-Weighted)

Elastic Weight Consolidation-inspired structural importance scoring. Nodes critical to graph connectivity are protected from forgetting:

```rust
/// Compute structural importance for all nodes.
/// High importance = high betweenness centrality + many dependents.
/// Used by BMR forgetting decisions — high EWC nodes are archived, not pruned.
fn structural_importance(graph: &DiGraph<NodeMeta, EdgeMeta>) -> HashMap<Uuid, f64>;
```

### Landmark Detection

Identify landmark nodes — high betweenness-centrality nodes that serve as navigation beacons:

```rust
/// Find landmark nodes per community for navigation/orientation.
fn detect_landmarks(graph: &DiGraph<NodeMeta, EdgeMeta>, communities: &[Community], top_k: usize) -> HashMap<usize, Vec<Uuid>>;
```

## Filtered Views

For federation egress and clearance-based queries, the sidecar provides zero-copy filtered views:

```rust
use petgraph::visit::NodeFiltered;

impl GraphSidecar {
    /// Create a filtered view showing only nodes/edges at or above the given clearance level.
    fn filtered_view(&self, min_clearance: u8) -> NodeFiltered<&DiGraph<NodeMeta, EdgeMeta>, impl Fn(NodeIndex) -> bool> {
        NodeFiltered::from_fn(&self.graph, |idx| {
            self.graph[idx].clearance_level >= min_clearance
        })
    }
}
```

See [09-federation](09-federation.md) for egress filtering details.

## Topology-Derived Embeddings (Optional)

From valence-v2: generate embeddings from graph structure alone (no LLM calls).

- **Spectral** — Eigendecomposition of the graph Laplacian
- **Node2Vec** — Random walk based embeddings
- **Strategy selector** — Pick method based on graph density and size

These provide a zero-API-cost embedding path and can complement or replace LLM-generated embeddings for node-level search.

## Thread Safety

The sidecar is accessed concurrently by search queries and ingestion updates:

```rust
use tokio::sync::RwLock;
use std::sync::Arc;

type SharedGraph = Arc<RwLock<GraphSidecar>>;
```

- Read operations (search, algorithms) take a read lock
- Write operations (sync from PG) take a write lock
- Write lock contention should be minimal since syncs are batched

## Open Questions

- [x] Should community detection run continuously or on-demand? → On deep consolidation schedule, with on-demand triggered by high epistemic delta
- [x] Community detection algorithm → k-core decomposition (deterministic, O(|E|), density-aware hierarchy). Replaces Louvain/Leiden — see arXiv:2603.05207 for proof that modularity optimization is non-reproducible on sparse KGs.
- [x] Petgraph size limits → Works well into single-digit millions of nodes. Node=32 bytes, Edge=40 bytes overhead. 10M nodes + 50M edges ≈ 2.3GB. Shard when graph RAM >30-40% system memory. Use `u32` indexes (ceiling 4.29B). Note: petgraph benchmarks show poor dynamic update performance (arxiv 2502.13862) — batch outbox sync mitigates this.
- [x] Sidecar interface → Embedded library in main engine for v1. Extract to separate process if graph operations become a latency bottleneck.
- [x] Graph versioning → No for v1. Temporal edges (valid_from/valid_until) + audit_logs provide sufficient history.
- [x] TrustRank seed set → Manual curation initially. Operator marks high-confidence sources as seeds. Heuristic: sources with highest `reliability_score × mention_count`. System-authored articles known correct.

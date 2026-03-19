# 13 — Cross-Domain Analysis

**Status:** Implemented (deployed)

When the system ingests its own spec, its own code, and its research foundations, three semantic domains exist in the same graph. Cross-domain analysis traverses the bridges between them to surface insights that no single domain can provide alone.

## The Five Domains

With the introduction of the graph type system ([ADR-0018](../docs/adr/0018-graph-type-system.md)), sources carry a `domain` field and nodes carry an `entity_class` field. Cross-domain analysis now uses these intrinsic labels instead of URI heuristics.

```
Research Domain        External Domain        Spec Domain            Design Domain          Code Domain
(academic papers,      (third-party docs,     (spec/*.md)            (docs/adr/*.md,        (source code,
 software eng. books)   API references)                               VISION.md,             AST, modules)
                                                                      design/*.md)

  entity_class:          entity_class:          entity_class:          entity_class:          entity_class:
  domain, actor          domain                 domain                 domain                 code

            │                 │                       │                      │                      │
            ├── INFORMS ──────┼───────────────────────┤                      │                      │
            ├── VALIDATES ────┼───────────────────────┼──────────────────────┼──────────────────────┤
            └── THEORETICAL_BASIS ────────────────────┤                      │                      │
                                                      ├── SPECIFIES ────────┤                      │
                                                      │                      ├── DECIDES ──────────┤
                                                      ├── IMPLEMENTS_INTENT ─┼──────────────────────┤
                                                      │                      │                      │
                                                      └──────────────────────┴── PART_OF_COMPONENT ─┘
```

Two edge families bridge domains:

1. **Component bridge edges** (bulk-automated): `PART_OF_COMPONENT`, `IMPLEMENTS_INTENT`, `THEORETICAL_BASIS` — created via MODULE_PATH_MAPPINGS and embedding similarity. Good for broad coverage.

2. **Traceability edges** (curated): `SPECIFIES`, `DECIDES`, `INFORMS`, `VALIDATES` — precise provenance chains between specific entities. Good for "why does this code exist?" queries.

The Component node remains the bridge for automated bulk linking. Traceability edges provide precise, curated provenance that doesn't require a Component intermediary.

## Capability 1: Research-to-Execution Verification

**Question:** "Does our implementation of X accurately reflect the approach described in paper Y?"

**Traversal:**
```
(Paper Node: "HDBSCAN")
  ←[THEORETICAL_BASIS]─ (Component: "Entity Resolution")
    ←[PART_OF_COMPONENT]─ (code_function: "run_resolution_cascade()")
```

**Analysis:**
1. Retrieve the atomic statements extracted from the research paper about HDBSCAN
2. Retrieve the semantic summary of `run_resolution_cascade()`
3. Compare: does the code's behavior match the paper's described algorithm?
4. Surface specific divergences as structured findings

**Implementation:**
```
POST /api/v1/analysis/verify-implementation
{
  "research_query": "HDBSCAN clustering algorithm",
  "component": "Entity Resolution",
  "depth": "detailed"  // or "summary"
}
```

Response includes:
- Matched research statements (claims from the paper)
- Matched code summaries (what the code actually does)
- Alignment score (cosine similarity between research cluster and code cluster)
- Divergences (statements with no matching code behavior, and vice versa)

## Capability 2: Architecture Erosion Detection

**Question:** "Where has the code drifted from its design intent?"

Architecture erosion happens when developers modify code in ways that gradually diverge from the original spec. In a traditional codebase, this is invisible until someone manually compares spec to code. In Covalence's graph, it's mathematically measurable.

**Mechanism:**
1. Each Component has a description embedding (derived from spec/design doc topics)
2. Each code entity under that Component has a semantic summary embedding
3. Compute the **aggregate semantic distance** between the Component's intent and its code's behavior
4. When code is updated (re-ingested), compare new semantic summary embeddings against the Component's description
5. If the cosine distance exceeds a threshold → generate a `SEMANTIC_DRIFT` edge

**The drift metric:**
```
drift(component) = 1 - mean(cosine(component.embedding, code_node.embedding)
                          for code_node in component.code_entities)
```

A Component with low drift (< 0.2): code faithfully implements the spec.
A Component with high drift (> 0.5): code has evolved away from the spec — either the code is wrong or the spec needs updating.

**Output:**
```
POST /api/v1/analysis/erosion
{
  "threshold": 0.3,  // report components with drift above this
  "include_details": true
}
```

Response:
```json
{
  "eroded_components": [
    {
      "component": "Search Fusion",
      "drift_score": 0.42,
      "spec_intent": "RRF fusion with 6 dimensions, equal weights",
      "code_reality": "CC fusion with coverage multiplier, dimension dampening, quality gating",
      "divergent_functions": [
        {
          "function": "fuse_results()",
          "summary": "Applies convex combination fusion with coverage multiplier...",
          "distance_from_spec": 0.55
        }
      ],
      "recommendation": "Spec describes RRF but code implements CC fusion. Update spec/06-search.md."
    }
  ]
}
```

## Capability 3: Whitespace Roadmap (Gap Analysis)

**Question:** "Based on the research we've ingested, what are we missing in our implementation?"

The graph maps what exists. By identifying dense research clusters with no corresponding code or spec coverage, we map what doesn't exist.

**Mechanism:**
1. Cluster research statements by semantic similarity
2. For each research cluster, check if any Component or Spec Topic nodes link to it (via THEORETICAL_BASIS or IMPLEMENTS_INTENT)
3. Clusters with no links are **unbridged voids** — areas of theory we've studied but haven't designed or built

**Output:**
```
POST /api/v1/analysis/whitespace
{
  "domain": "all",  // or "graph", "search", "ingestion", etc.
  "min_cluster_size": 3  // minimum research statements to count as a gap
}
```

Response:
```json
{
  "gaps": [
    {
      "research_cluster": "Byzantine Fault Tolerance in Distributed Consensus",
      "statement_count": 12,
      "papers": ["Lamport 1982", "Castro & Liskov 1999"],
      "connected_components": [],
      "connected_spec_topics": [],
      "assessment": "No implementation or spec coverage. 12 research statements about BFT with zero bridge edges to any component."
    },
    {
      "research_cluster": "Incremental Graph Maintenance",
      "statement_count": 8,
      "papers": ["GraphRAG 2024"],
      "connected_components": ["Graph Sidecar"],
      "connected_spec_topics": ["spec/04: Community Detection"],
      "assessment": "Partial coverage. Graph sidecar implements basic community detection but 5 of 8 research statements about incremental maintenance have no matching code."
    }
  ]
}
```

## Capability 4: Blast-Radius Simulation

**Question:** "If I change function X, what else breaks?"

Traditional impact analysis follows import chains. Graph-based blast radius follows semantic chains — including spec topics, research foundations, and dependent components.

**Mechanism:**
1. Start from a code entity (function, module, struct)
2. Traverse upward: `code_function →[PART_OF_COMPONENT]→ Component →[IMPLEMENTS_INTENT]→ Spec Topic`
3. Traverse outward: `code_function →[CALLS/USES_TYPE]→ other code entities`
4. Traverse downward from affected Components: find all other code entities in the blast radius
5. Score impact by hop distance and edge confidence

**Invalidated edges:** By default, blast radius only traverses edges with `invalid_at IS NULL`. When `include_invalidated` is true, nodes reachable through invalidated edges are also returned (at hop distance 1) so the blast radius reflects historically-connected nodes that may still be affected by changes.

**Output:**
```
POST /api/v1/analysis/blast-radius
{
  "target": "run_statement_pipeline",  // function name or node ID
  "max_hops": 3,
  "include_invalidated": false  // optional, default false
}
```

Response:
```json
{
  "target": {
    "node": "code_function: run_statement_pipeline",
    "file": "src/services/statement_pipeline.rs:71",
    "component": "Statement Pipeline"
  },
  "impact": {
    "directly_affected": [
      {"node": "code_function: reextract_statements", "relationship": "CALLS"},
      {"node": "code_function: ingest_source", "relationship": "CALLS (caller)"}
    ],
    "component_impact": [
      {
        "component": "Statement Pipeline",
        "spec_topics": ["ADR-0015: Statement-First Extraction"],
        "research_basis": ["RAPTOR: Recursive Abstractive Processing"]
      }
    ],
    "spec_implications": [
      "Changes to statement pipeline may affect spec/05-ingestion.md Stage 2-12",
      "ADR-0015 describes the current behavior — update if changing"
    ],
    "cascading_functions": [
      "embed_batch() — called by pipeline for statement embedding",
      "cluster_statements() — called by pipeline for HAC clustering"
    ]
  }
}
```

## Capability 5: Dialectical Design Partner

**Question:** "Steelman the argument against my approach."

Because the system tracks competing claims from different research papers and design paradigms, it can synthesize adversarial arguments.

**Mechanism:**
1. User provides a design proposal (free text or reference to a Component/Spec Topic)
2. System extracts the key claims from the proposal
3. Search the research domain for `CONTRADICTS` edges or competing approaches
4. Search the code domain for existing implementations that would conflict
5. Synthesize a counter-argument citing specific research statements and code realities

**Output:**
```
POST /api/v1/analysis/critique
{
  "proposal": "Add a semantic chunking layer that splits text at cosine similarity valleys between adjacent sentences before statement extraction.",
  "depth": "thorough"
}
```

Response:
```json
{
  "counter_arguments": [
    {
      "claim": "Semantic chunking is redundant with statement extraction",
      "evidence": [
        "ADR-0015 statement 14: 'Statement extraction operates on windowed text and produces self-contained claims regardless of chunk boundaries'",
        "RAPTOR paper statement 7: 'Bottom-up clustering of atomic claims makes pre-chunking unnecessary'"
      ],
      "strength": "strong"
    },
    {
      "claim": "Per-sentence embedding is computationally expensive",
      "evidence": [
        "Current codebase has 24,461 chunks from 336 sources. Per-sentence embedding would produce ~100K vectors.",
        "Embedding cost analysis: 100K sentences × $0.0001/embed = $10/reindex vs current $2.45"
      ],
      "strength": "moderate"
    }
  ],
  "supporting_arguments": [
    {
      "claim": "Semantic chunking improves chunk quality for non-statement retrieval",
      "evidence": [
        "LlamaIndex eval (ADR-0007): chunk size significantly affects faithfulness",
        "Hierarchical chunking spec: semantic boundaries detect topic shifts"
      ]
    }
  ],
  "recommendation": "The proposal adds value for chunk-based retrieval but is redundant for statement-based retrieval. Consider only if maintaining dual pipeline."
}
```

## Capability 6: Coverage Analysis (Orphan Detection)

**Question:** "What code isn't documented? What specs aren't implemented?"

Simple graph traversal reveals structural voids.

**Orphan Code** — code entities with no path to any Spec Topic:
```
SELECT n.canonical_name, n.properties->>'file_path'
FROM nodes n
WHERE n.entity_class = 'code'
AND NOT EXISTS (
  SELECT 1 FROM edges e
  JOIN nodes comp ON e.target_node_id = comp.id AND comp.entity_class = 'analysis'
  WHERE e.source_node_id = n.id AND e.rel_type = 'PART_OF_COMPONENT'
)
```

**Unimplemented Specs** — spec topic nodes with no IMPLEMENTS_INTENT edges:
```
SELECT n.canonical_name
FROM nodes n
JOIN extractions ex ON ex.entity_type = 'node' AND ex.entity_id = n.id
JOIN sources s ON s.id = (
  SELECT COALESCE(ex2.source_id, c.source_id)
  FROM extractions ex2
  LEFT JOIN chunks c ON c.id = ex2.chunk_id
  WHERE ex2.entity_id = n.id LIMIT 1
)
WHERE n.entity_class = 'domain'
AND s.domain = 'spec'
AND NOT EXISTS (
  SELECT 1 FROM edges e
  WHERE e.target_node_id = n.id AND e.rel_type = 'IMPLEMENTS_INTENT'
)
```

**Output:**
```
POST /api/v1/analysis/coverage

Response:
{
  "orphan_code": [
    {"function": "filter_obsolete()", "file": "src/ingestion/pipeline.rs:400", "reason": "No component assignment, no spec reference"}
  ],
  "unimplemented_specs": [
    {"topic": "Federation Egress Filter", "spec": "spec/09-federation.md", "reason": "Spec topic exists but zero IMPLEMENTS_INTENT edges found"}
  ],
  "coverage_score": 0.73  // (implemented specs / total specs)
}
```

## Capability 7: Cross-Domain Alignment Report

**Question:** "Where are spec, design, code, and research out of sync?"

The alignment report runs four targeted checks across the domain boundaries, surfacing misalignments that require human review. Unlike erosion detection (which measures drift within a component), alignment analysis compares entities *across* domains using embedding distance and graph structure.

**Checks:**

1. **`code_ahead`** — Code entities (entity_class = `code`) with no semantically close match in the spec or design domain. Uses embedding cosine distance against all `primary_domain IN ('spec', 'design')` nodes. Filters out test nodes and low-mention entities.

2. **`spec_ahead`** — Spec/design concepts (`primary_domain IN ('spec', 'design')`, entity_class = `domain`) with no inbound `IMPLEMENTS_INTENT` edge (filtering `invalid_at IS NULL`). These are specified but unimplemented features.

3. **`design_contradicted`** — Design concepts semantically close to research concepts (same topic, potentially different conclusion). Flags pairs where embedding distance is below the threshold — these may describe conflicting approaches and need review.

4. **`stale_design`** — Design documents where linked code entities have a more recent `last_seen` timestamp than the design source's `ingested_at`. The code has evolved but the design doc hasn't been updated.

**Output:**
```
POST /api/v1/analysis/alignment
{
  "checks": [],             // empty = run all four checks
  "min_similarity": 0.4,    // minimum embedding similarity for matching
  "limit": 20               // max items per check
}
```

Response:
```json
{
  "code_ahead": [
    {
      "check": "code_ahead",
      "name": "run_async_pipeline",
      "domain": "code",
      "node_type": "code_function",
      "closest_match_score": 0.32,
      "closest_match_name": "Ingestion Pipeline",
      "closest_match_domain": "spec/design",
      "reason": "Code entity with no close spec match (nearest: Ingestion Pipeline, distance: 0.680)"
    }
  ],
  "spec_ahead": [
    {
      "check": "spec_ahead",
      "name": "Federation Egress Filter",
      "domain": "spec",
      "node_type": "concept",
      "reason": "Spec concept mentioned 5 times with no IMPLEMENTS_INTENT edge"
    }
  ],
  "design_contradicted": [...],
  "stale_design": [
    {
      "check": "stale_design",
      "name": "ADR-0015: Statement-First Extraction",
      "domain": "design",
      "node_type": "source",
      "reason": "Design doc is 42.3 days behind linked code. Code entities: run_statement_pipeline, extract_statements..."
    }
  ]
}
```

## Capability 8: Data Health Report

**Question:** "What data quality issues exist in the knowledge graph?"

A read-only observability endpoint on the admin API that reports structural data quality issues without modifying anything. Useful for monitoring graph hygiene and prioritizing cleanup.

**Metrics reported:**

| Metric | Description |
|--------|-------------|
| `superseded_sources` | Sources replaced by newer versions (superseded_by IS NOT NULL) |
| `superseded_chunks` | Chunks belonging to superseded sources |
| `orphan_nodes` | Nodes with no extraction provenance linking them to any source |
| `orphan_nodes_with_edges` | Subset of orphan nodes that still have edges (load-bearing despite missing provenance) |
| `duplicate_sources` | Sources sharing the same title and domain |
| `unembedded_nodes` | Nodes with NULL embedding (invisible to vector search) |
| `unsummarized_code_entities` | Code entities missing semantic summaries |
| `unsummarized_sources` | Sources missing summaries |

**Output:**
```
GET /api/v1/admin/data-health
```

Response:
```json
{
  "superseded_sources": 3,
  "superseded_chunks": 47,
  "orphan_nodes": 12,
  "orphan_nodes_with_edges": 4,
  "duplicate_sources": 2,
  "unembedded_nodes": 18,
  "unsummarized_code_entities": 31,
  "unsummarized_sources": 5
}
```

## Edge Validity Filtering

All analysis queries filter on `invalid_at IS NULL` by default, excluding edges that have been epistemically invalidated. This ensures analysis results reflect the current trusted state of the graph rather than historical connections.

The `blast_radius` endpoint is the one exception: it accepts an `include_invalidated` flag (default `false`) to optionally include historically-connected nodes when assessing change impact.

## Implementation Status

All five implementation phases are complete and deployed.

### Phase 1: Data Model + AST Parsing — Complete
- Component nodes, code node types, and bridge edge types in DB
- Tree-sitter integration for Rust and Go
- AST chunking pipeline

### Phase 2: Semantic Wrapper + Statement Integration — Complete
- LLM semantic summary generation for code chunks
- Bottom-up file summary composition
- Embedded summaries stored on code nodes

### Phase 3: Component Linking — Complete
- 9 Component nodes bootstrapped via `POST /analysis/bootstrap`
- MODULE_PATH_MAPPINGS (64 patterns) for `PART_OF_COMPONENT` assignment
- Embedding similarity for `IMPLEMENTS_INTENT` detection
- `THEORETICAL_BASIS` edges linking components to research

### Phase 4: Analysis Endpoints — Complete
All 9 endpoints deployed under `/api/v1/analysis/` and `/api/v1/admin/`:
- `POST /analysis/bootstrap` — create/update Component nodes
- `POST /analysis/link` — bulk-create bridge edges
- `POST /analysis/coverage` — orphan code + unimplemented specs
- `POST /analysis/erosion` — component drift detection
- `POST /analysis/blast-radius` — semantic impact simulation
- `POST /analysis/whitespace` — research gap detection
- `POST /analysis/verify` — research-to-execution comparison
- `POST /analysis/alignment` — cross-domain alignment report
- `POST /analysis/critique` — dialectical design partner
- `GET /admin/data-health` — structural data quality report

### Phase 5: Continuous Monitoring — Partial
- On code re-ingestion: semantic summaries regenerated, bridge edges updated
- Incremental ingestion on deploy detects changed files
- Dashboard integration: planned (not yet built)

# 13 — Cross-Domain Analysis

**Status:** Proposed

When the system ingests its own spec, its own code, and its research foundations, three semantic domains exist in the same graph. Cross-domain analysis traverses the bridges between them to surface insights that no single domain can provide alone.

## The Three Domains

```
Research Domain                    Spec Domain                       Code Domain
(academic papers,                  (design docs, specs,              (source code, AST,
 software eng. books)               ADRs, vision docs)                functions, modules)

  concepts, algorithms,              design intent, goals,             execution, behavior,
  theoretical foundations            requirements, constraints         implementation details

            │                              │                              │
            └──── THEORETICAL_BASIS ───────┤                              │
                                           ├── IMPLEMENTS_INTENT ─────────┤
                                           │                              │
                                           └── PART_OF_COMPONENT ────────┘
```

The Component node is the bridge. Without it, research papers and code functions exist in different semantic neighborhoods and never connect. With it, you can traverse from an academic algorithm to the function that implements it.

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

**Output:**
```
POST /api/v1/analysis/blast-radius
{
  "target": "run_statement_pipeline",  // function name or node ID
  "max_hops": 3
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
WHERE n.node_type LIKE 'code_%'
AND NOT EXISTS (
  SELECT 1 FROM edges e
  JOIN nodes comp ON e.target_node_id = comp.id AND comp.node_type = 'component'
  WHERE e.source_node_id = n.id AND e.rel_type = 'PART_OF_COMPONENT'
)
```

**Unimplemented Specs** — spec topic nodes with no IMPLEMENTS_INTENT edges:
```
SELECT n.canonical_name
FROM nodes n
WHERE n.node_type = 'concept'
AND n.properties->>'domain' = 'spec'
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
    {"function": "legacy_chunk_filter()", "file": "src/ingestion/pipeline.rs:400", "reason": "No component assignment, no spec reference"}
  ],
  "unimplemented_specs": [
    {"topic": "Federation Egress Filter", "spec": "spec/09-federation.md", "reason": "Spec topic exists but zero IMPLEMENTS_INTENT edges found"}
  ],
  "coverage_score": 0.73  // (implemented specs / total specs)
}
```

## Implementation Approach

### Phase 1: Data Model + AST Parsing
- Add Component table + code node types + bridge edge types to DB
- Integrate Tree-sitter for Rust and Go
- Build AST chunking pipeline

### Phase 2: Semantic Wrapper + Statement Integration
- LLM semantic summary generation for code chunks
- Feed summaries through statement pipeline
- Embed summaries and store on code Nodes

### Phase 3: Component Linking
- Manual Component creation (bootstrap with known components)
- Auto-detection of IMPLEMENTS_INTENT via semantic similarity
- Module-path-based PART_OF_COMPONENT assignment

### Phase 4: Analysis Endpoints
- Coverage analysis (orphan detection) — simplest, just graph queries
- Erosion detection — cosine distance computation
- Whitespace roadmap — research cluster gap detection
- Blast radius — graph traversal + impact scoring
- Research-to-execution — cross-domain traversal + comparison
- Dialectical critique — adversarial synthesis

### Phase 5: Continuous Monitoring
- On code re-ingestion: recompute drift metrics, update SEMANTIC_DRIFT edges
- On research ingestion: recompute whitespace gaps
- Dashboard integration: visualize coverage, drift, gaps

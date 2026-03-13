# Covalence — Vision

## What This Is

Covalence is a knowledge engine for AI agents and humans. It ingests unstructured
information, builds a structured knowledge graph with explicit uncertainty, and
provides multi-dimensional retrieval that's better than any single search strategy.

When it ingests its own spec, code, and research foundations, it becomes a
self-aware cognitive model of its own software — capable of tracing academic
concepts to specific lines of code, detecting when implementation drifts from
design intent, and identifying gaps between what the research says and what
the system does.

It solves two problems simultaneously.

## The Academic Problem

GraphRAG systems have well-documented limitations that are treated as independent
problems but are actually interconnected:

- **Extraction quality** — chunk-first pipelines propagate noise (bibliography,
  boilerplate, author blocks) through the entire system. Statement-first extraction
  inverts this: extract knowledge from raw text as self-contained atomic claims,
  then build hierarchy from the extracted knowledge. Noise is eliminated at source.
- **Semantic space bridging** — code and prose exist in different vector
  neighborhoods. Raw syntax embeddings can't find natural language queries.
  Semantic summaries (LLM-generated business logic descriptions of code) bridge
  this gap, placing code and prose in a shared vector space.
- **Search fusion** — combining multiple retrieval dimensions (vector, lexical,
  graph, temporal, structural) lacks principled approaches; most systems use RRF
  without understanding its failure modes
- **Epistemic blindness** — systems present all knowledge with equal confidence,
  conflating well-supported facts with single-source claims and stale information
- **Design-execution gap** — no system connects design intent (specs, research)
  to execution (code) in a way that makes drift, coverage gaps, and impact
  mathematically measurable
- **Self-referential improvement** — using a knowledge system to reason about and
  improve itself is largely unexplored

Covalence's contribution: treating these as a connected problem space where epistemic
uncertainty (Subjective Logic) is the unifying framework. Extraction confidence
feeds entity resolution. Entity resolution quality affects graph structure. Graph
structure drives search. Search quality is measurable. Measurements drive improvement.
The system is its own testbed.

## The Market Problem

AI agents need persistent, structured memory. Current options leave gaps:

| Approach | Strengths | Missing |
|----------|-----------|---------|
| Vector stores (Pinecone, Weaviate) | Similarity search, scale | Structure, relationships, uncertainty |
| Graph platforms (Neo4j) | Relationships, traversal | Native RAG, embedding search, simplicity |
| RAG frameworks (LlamaIndex, LangChain) | Orchestration, flexibility | Persistent knowledge, graph structure |
| Memory systems (Mem0) | Simple API, agent-friendly | Epistemic depth, multi-dimensional search |

The gap: a system that combines structured knowledge (graph), unstructured retrieval
(vectors), epistemic integrity (uncertainty), and practical usability (API, CLI, web
interface). Something an agent can use as long-term memory where the quality of
knowledge is explicit and trustworthy.

## What Success Looks Like

1. **An agent using Covalence makes better decisions** than one without it. Not just
   "has more context" but actually retrieves the right information at the right
   confidence level, and knows what it doesn't know.

2. **A human exploring the web interface discovers insights** they wouldn't have found
   by reading individual documents. The graph surfaces connections across sources.

3. **The system traces research to execution.** A query like "does our entity
   resolution implement the HDBSCAN fallback correctly?" traverses from paper to
   spec topic to component to code function, comparing what the research says
   against what the code does.

4. **The system detects its own drift.** When code evolves away from its spec, the
   semantic distance is measurable and alertable. Architecture erosion is visible
   before it becomes tech debt.

5. **The system maps its own gaps.** Ingested research clusters with no corresponding
   code or spec coverage are identified as unbridged voids — a data-driven roadmap
   of missing capabilities.

6. **Search results are consistently relevant.** No noise in the top 5. Results from
   different dimensions genuinely complement each other. Cross-domain queries
   (spec + code + research) return coherent answers.

7. **Other people want to use it.** Not because we marketed it, but because it
   solves a real problem better than alternatives.

## How We Measure Progress

### Knowledge Quality (the foundation)
- **Entity precision**: sample 100 random entities, classify as useful/noise.
  Target: >90% useful.
- **Statement quality**: sample 100 random statements, verify self-contained
  (no unresolved pronouns, independently meaningful). Target: >95%.
- **Search relevance**: curated query set with expected results, measure precision@5
  and nDCG. Target: precision@5 > 0.8.

### Cross-Domain Integrity
- **Coverage score**: (spec topics with IMPLEMENTS_INTENT edges) / (total spec topics).
  Target: >0.8 for core specs.
- **Drift score**: mean semantic distance between Components and their code entities.
  Target: <0.3 for all active components.
- **Whitespace coverage**: research clusters with no bridge edges / total research
  clusters. Lower is better — means we're acting on what we study.

### System Capability
- **Layer coverage**: for each spec concept, does corresponding code exist?
  Measurable via coverage analysis endpoint.
- **Research integration**: how much of the ingested research has influenced
  design decisions? (Not just ingested, but acted on.)
- **Self-improvement rate**: meaningful improvements per autonomous session.
  Should trend upward as the system matures.

### User Value
- **Agent task success**: does an agent with Covalence memory complete tasks
  more accurately than one without? (Requires evaluation harness.)
- **Discovery rate**: in web interface exploration, how often does a user find
  something they didn't know to look for?
- **Time to insight**: how quickly can a new user get value from the system?

## Priorities (in order)

1. **Knowledge quality** — statement extraction, entity resolution, search
   relevance. Without this, everything else is building on sand.

2. **Cross-domain bridging** — AST-aware code ingestion, semantic summaries,
   Component bridge layer. This is the prerequisite for self-awareness.

3. **Observability** — web interface that makes quality, drift, coverage, and
   gaps visible. You can't improve what you can't see.

4. **Self-improvement infrastructure** — cross-domain analysis endpoints
   (erosion detection, coverage analysis, blast radius, whitespace roadmap,
   dialectical critique). Agents that use Covalence to assess and improve itself.

5. **Agent integration** — MCP tools that make Covalence genuinely useful as
   agent memory. Fast, relevant, well-calibrated retrieval.

6. **External readiness** — documentation, onboarding, reliability. Only after
   the above are solid.

## Non-Goals

- **Replacing general-purpose databases.** Covalence is for knowledge, not
  transactional data.
- **Real-time streaming.** Batch ingestion is fine. Knowledge doesn't need
  sub-second freshness.
- **Competing on raw vector search speed.** pgvector is good enough.
  The value is in multi-dimensional fusion and epistemic depth, not
  millisecond latency.
- **Supporting every media format.** Focus on text-heavy formats
  (markdown, HTML, PDF) and code (Rust, Go, Python, TypeScript via
  Tree-sitter). Not images, audio, video.

## The Flywheel

```
Research ──→ Vision (this doc)
               │
               ▼
           Spec (derives from vision)
               │
               ▼
         Design (ADRs, decisions)
               │
               ▼
           Code (implementation)
               │
               ▼
        Ingest into Covalence ──→ Code via AST + semantic summary
               │                        │
               │                  Component bridge
               │                        │
               ▼                        ▼
     Agents query Covalence ──→ Cross-domain analysis
               │                   │  │  │  │  │
               │                   │  │  │  │  └─ Dialectical critique
               │                   │  │  │  └─── Blast radius
               │                   │  │  └────── Whitespace roadmap
               │                   │  └───────── Erosion detection
               │                   └──────────── Coverage analysis
               ▼                        │
        Improve system ←──── Generate work items
```

The vision drives the spec. The spec drives the code. The code gets ingested
alongside the spec and research. The Component bridge connects design intent
to execution. Cross-domain analysis surfaces where reality diverges from
vision — not as vague impressions, but as measurable drift scores, coverage
gaps, and impact reports. That generates precise work items. The work improves
the system. The improved system provides better data for the next cycle.

The vision itself evolves as research reveals better approaches and as
real usage reveals what actually matters.

## The Emergent Capabilities

When Covalence ingests its own spec, code, and research, five capabilities emerge
from the graph structure:

1. **Research-to-Execution Verification** — trace from an academic concept through
   spec topics and components to the code that implements it. Compare research
   statements against code semantic summaries to find alignment and divergence.

2. **Architecture Erosion Detection** — measure semantic drift between Component
   descriptions (from spec) and their code entities' semantic summaries. When code
   evolves away from its spec, the cosine distance increases. Make invisible tech
   debt mathematically visible.

3. **Whitespace Roadmap** — find dense research clusters with no corresponding
   Component or Spec Topic links. These are areas of theory we've studied but
   haven't designed or built. Data-driven roadmap of missing capabilities.

4. **Blast-Radius Simulation** — given a code entity, traverse the graph via
   structural and semantic edges to compute the full impact of modifying it.
   Traditional impact analysis follows imports. Graph-based blast radius follows
   meaning.

5. **Dialectical Design Partner** — given a design proposal, search the graph for
   competing approaches, contradicting claims, and conflicting implementations.
   Synthesize adversarial arguments using the system's own ingested knowledge.

These aren't features to be built in isolation. They emerge naturally when the
three domains (research, spec, code) are connected via the Component bridge and
all share a vector space through semantic summaries.

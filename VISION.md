# Covalence — Vision

## What This Is

Covalence is a knowledge engine for AI agents and humans. It ingests unstructured
information, builds a structured knowledge graph with explicit uncertainty, and
provides multi-dimensional retrieval that's better than any single search strategy.

It solves two problems simultaneously.

## The Academic Problem

GraphRAG systems have well-documented limitations that are treated as independent
problems but are actually interconnected:

- **Entity extraction noise** — LLM-based extraction creates bibliographic entities,
  generic concepts, and duplicates that pollute the graph
- **Chunking quality** — no strategy consistently outperforms others; structural
  boundaries, semantic boundaries, and fixed-size all have failure modes
- **Search fusion** — combining multiple retrieval dimensions (vector, lexical, graph,
  temporal, structural) lacks principled approaches; most systems use RRF without
  understanding its failure modes
- **Epistemic blindness** — systems present all knowledge with equal confidence,
  conflating well-supported facts with single-source claims and stale information
- **Evaluation gaps** — retrieval quality measurement for graph-augmented systems is
  immature; standard IR metrics don't capture graph-specific value
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

3. **The system identifies its own quality gaps** and drives improvement. Not just
   "an engineer queries it and notices problems" but structured, repeatable
   self-assessment.

4. **Search results are consistently relevant.** No bibliography chunks in the top 5.
   No entity contamination. Results from different dimensions genuinely complement
   each other.

5. **Other people want to use it.** Not because we marketed it, but because it
   solves a real problem better than alternatives.

## How We Measure Progress

### Knowledge Quality (the foundation)
- **Entity precision**: sample 100 random entities, classify as useful/noise.
  Target: >90% useful.
- **Chunk quality**: sample 100 random chunks, classify as informative/noise.
  Target: >95% informative.
- **Search relevance**: curated query set with expected results, measure precision@5
  and nDCG. Target: precision@5 > 0.8.

### System Capability
- **Layer coverage**: for each spec concept, does corresponding code exist?
  Measurable via source-layer cross-referencing.
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

1. **Knowledge quality** — fix entity extraction noise, chunk quality, search
   relevance. Without this, everything else is building on sand.

2. **Observability** — web interface that makes quality visible. You can't
   improve what you can't see.

3. **Agent integration** — MCP tools that make Covalence genuinely useful as
   agent memory. Fast, relevant, well-calibrated retrieval.

4. **Self-improvement infrastructure** — agents that use Covalence to assess
   and improve itself. Convergence agent, spec evolution agent.

5. **External readiness** — documentation, onboarding, reliability. Only after
   the above are solid.

## Non-Goals

- **Replacing general-purpose databases.** Covalence is for knowledge, not
  transactional data.
- **Real-time streaming.** Batch ingestion is fine. Knowledge doesn't need
  sub-second freshness.
- **Competing on raw vector search speed.** pgvector is good enough.
  The value is in multi-dimensional fusion and epistemic depth, not
  millisecond latency.
- **Supporting every document format.** Focus on text-heavy formats
  (markdown, HTML, PDF, code). Not images, audio, video.

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
        Ingest into Covalence
               │
               ▼
     Agents query Covalence ──→ Identify gaps
               │                      │
               ▼                      ▼
        Improve system ←──── Generate work items
```

The vision drives the spec. The spec drives the code. The code gets ingested.
Agents reason over the graph to find where reality diverges from vision.
That generates work. The work improves the system. The improved system
provides better data for the next cycle.

The vision itself evolves as research reveals better approaches and as
real usage reveals what actually matters.

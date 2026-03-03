# Plasmon v2 Design Specification
## Covalence-Native Intermediary Architecture

**tracking#101** | 2026-03-02 | Status: **Design Draft**
Authors: Chris, Jane (session), documented by agent
Depends on: Covalence v1 engine operational at `http://localhost:8430`

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Decompose Mode v2](#2-decompose-mode-v2)
3. [Compose Mode v2](#3-compose-mode-v2)
4. [Query Mode v2](#4-query-mode-v2)
5. [Training Approach](#5-training-approach)
6. [Model Size & Architecture Analysis](#6-model-size--architecture-analysis)
7. [Integration Interface](#7-integration-interface)
8. [Open Questions & Design Risks](#8-open-questions--design-risks)

---

## 1. Architecture Overview

### 1.1 What Plasmon v2 Is

Plasmon v2 is a **small fine-tuned language model** (1.5B–3B parameters, LoRA-adapted)
that acts as the **Intermediary** between:

- **Upstream**: the human user (or agentic trigger)
- **Lateral**: the Covalence knowledge substrate (`http://localhost:8430`)
- **Downstream**: a reasoning model (Claude Sonnet, Qwen3 30B, or similar)

It is *not* a general-purpose assistant. It is a **specialist transducer**: its sole
job is to translate between natural language and Covalence's source/article/search
substrate, and to assemble epistemically-grounded context for one-shot reasoning calls.

The core thesis of the Intermediary architecture:

> **Context window = pure workspace, not memory.**
> Continuity is graph-native (epistemic relevance), not buffer-native (temporal proximity).
> Every inference step gets a fresh context composed by Plasmon from the substrate —
> not an accumulating history of everything that has happened.

### 1.2 The Three Modes

| Mode | Input | Output | Role in inference loop |
|------|-------|--------|------------------------|
| **DECOMPOSE** | Raw NL (user turn, tool result, observation) | Covalence ingestion plan (JSON) | Write path: new knowledge → substrate |
| **COMPOSE** | Covalence search results + current query context | Assembled reasoning context (NL) | Read path: substrate → reasoning prompt |
| **QUERY** | NL question or current reasoning need | Covalence API call sequence (JSON) | Navigation: "what should I ask the graph?" |

These three modes implement the full **read-write inference loop** over the epistemic graph.

### 1.3 System Context Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│                        INFERENCE LOOP                               │
│                                                                     │
│  User / Agent Trigger                                               │
│         │                                                           │
│         ▼                                                           │
│  ┌─────────────┐  DECOMPOSE   ┌──────────────────────────────┐     │
│  │             │─────────────▶│                              │     │
│  │  PLASMON v2 │              │   COVALENCE SUBSTRATE        │     │
│  │  (1.5B-3B   │◀─────────────│   http://localhost:8430      │     │
│  │   LoRA)     │  QUERY +     │                              │     │
│  │             │  results     │  Sources (raw, immutable)    │     │
│  │             │─────────────▶│  Articles (LLM-compiled)     │     │
│  └──────┬──────┘  COMPOSE     │  Provenance graph            │     │
│         │        context      │  4D search (vec+lex+graph+   │     │
│         │                     │            structural)        │     │
│         │                     └──────────────────────────────┘     │
│         │                                                           │
│         ▼  assembled context (one-shot prompt)                      │
│  ┌─────────────┐                                                    │
│  │  REASONING  │                                                    │
│  │  MODEL      │  (Claude Sonnet, Qwen3 30B, etc.)                 │
│  │             │                                                    │
│  └──────┬──────┘                                                    │
│         │  response                                                 │
│         ▼                                                           │
│  ┌─────────────┐  DECOMPOSE                                         │
│  │  PLASMON v2 │─────────────▶ Covalence (response ingested)       │
│  └─────────────┘                                                    │
└─────────────────────────────────────────────────────────────────────┘
```

### 1.4 Full Inference Cycle — Sequence Diagram

```
User           Plasmon v2              Covalence               Reasoning Model
 │                 │                      │                         │
 │  "X" (message)  │                      │                         │
 │────────────────▶│                      │                         │
 │                 │ [DECOMPOSE]          │                         │
 │                 │ parse X →            │                         │
 │                 │ ingestion plan       │                         │
 │                 │─── POST /sources ───▶│                         │
 │                 │◀── source_id ────────│                         │
 │                 │                      │                         │
 │                 │ [QUERY]              │                         │
 │                 │ X → API call seq     │                         │
 │                 │─── POST /search ────▶│                         │
 │                 │◀── ranked results ───│                         │
 │                 │─── GET /articles ───▶│  (top article expand)   │
 │                 │◀── article content ──│                         │
 │                 │                      │                         │
 │                 │ [COMPOSE]            │                         │
 │                 │ results → context    │                         │
 │                 │──────────────────────────────────────────────▶ │
 │                 │                                one-shot prompt  │
 │                 │◀─────────────────────────────── response Y ─── │
 │                 │                      │                         │
 │                 │ [DECOMPOSE] Y        │                         │
 │                 │─── POST /sources ───▶│  (response ingested)    │
 │                 │                      │                         │
 │  Y (response)   │                      │                         │
 │◀────────────────│                      │                         │
```

**Multi-step example** (tool call mid-inference):

```
Step 1: User says X
  → DECOMPOSE X → substrate
  → QUERY: what matters for X? → search results
  → COMPOSE context_1 → Reasoning Model one-shot
  → Model needs tool call T

Step 2: Tool returns Z
  → DECOMPOSE Z → substrate
  → QUERY: what matters for X + Z combined?
    (graph may now surface a week-old article connected to Z)
  → COMPOSE context_2 (≠ context_1 + Z; freshly computed from graph)
  → Reasoning Model one-shot → final answer W

Step 3: DECOMPOSE W → substrate
  → User receives W
```

Context for step 2 is **not** "everything from step 1 plus Z." It is a fresh
epistemic composition. Something from step 1 may drop out. Something from last
week may be pulled in if Z created a new graph path to it.

---

## 2. Decompose Mode v2

### 2.1 What v1 Did

v1 DECOMPOSE converted NL text into **RDF-style triples**:

```
(Cognitive science, is_a, interdisciplinary scientific study)
  [confidence:high, source:Document, weight_hint:0.95]
```

This was appropriate for Valence's triple store. For Covalence, triples are the
**wrong primitive**. Covalence does not store triples — it stores **sources** (raw
content) and **articles** (compiled summaries), connected by a typed provenance graph.

### 2.2 v2 Target: Atomic Ingestion Plans

v2 DECOMPOSE converts NL input into a **structured ingestion plan** that maps directly
to Covalence's source model. The model decides:

1. **How to split** the input into atomic, independently-retrievable units
2. **What source_type** to assign (document / conversation / observation / web / tool_output / user_input)
3. **What metadata** to attach (importance, tags, context, memory flag)
4. **Whether to flag** any article-compile triggers (when new content should prompt recompilation)

### 2.3 Output Format

```json
{
  "operations": [
    {
      "op": "ingest_source",
      "content": "The Intermediary architecture replaces buffer-native continuity with graph-native epistemic relevance.",
      "source_type": "conversation",
      "metadata": {
        "importance": 0.9,
        "tags": ["architecture", "intermediary", "continuity"],
        "context": "session:chris-jane:2026-03-02",
        "memory": true
      }
    },
    {
      "op": "ingest_source",
      "content": "Each inference step receives a fresh context composed from the graph, not accumulated history.",
      "source_type": "conversation",
      "metadata": {
        "importance": 0.85,
        "tags": ["inference", "context-composition"],
        "context": "session:chris-jane:2026-03-02",
        "memory": true
      }
    },
    {
      "op": "compile_trigger",
      "hint": "intermediary architecture",
      "reason": "new conceptual material warrants article update"
    }
  ]
}
```

### 2.4 Splitting Heuristics (Model Must Learn)

The model must learn to split input into **independently-retrievable units**:

- **Too coarse**: one giant source per message → poor retrieval granularity
- **Too fine**: one source per sentence → provenance explosion, coherence loss
- **Target grain**: one source per **distinct atomic claim or conceptual unit**

From v1 training data: a 3-sentence paragraph typically decomposes into 3–8 triples.
By analogy, a 3-sentence paragraph should produce 2–5 source units. The model must
learn to identify conceptual boundaries, not just sentence boundaries.

Key signals for split points:
- Topic shift (new subject introduced)
- Confidence boundary (one claim is certain; next is hedged)
- Temporal shift (different time periods)
- Agent/source boundary (user quote vs. agent observation)

### 2.5 Source Type Classification (Model Must Learn)

| Input characteristic | Assigned source_type | Reliability |
|---------------------|---------------------|-------------|
| Verified document, paper, reference | `document` | 0.8 |
| User statement in conversation | `user_input` | 0.75 |
| Tool / API result | `tool_output` | 0.7 |
| Agent's own observation | `observation` | 0.4 |
| Scraped web content | `web` | 0.6 |
| NL description of code | `code` | 0.8 |
| Conversational exchange (both parties) | `conversation` | 0.5 |

The source_type directly controls the initial reliability score Covalence assigns.
Misclassification systematically biases downstream confidence scoring — a user
speculation labeled `document` will be treated as high-reliability fact.

### 2.6 Metadata Schema

Every DECOMPOSE output should populate:

```json
{
  "importance": 0.0,     // 0.0–1.0; drives retention in organic forgetting
  "tags": [],            // domain tags for structural retrieval
  "context": "",         // session/conversation provenance string
  "memory": false,       // true → memory wrapper treatment (persisted across sessions)
  "supersedes_id": null  // UUID of previous source this replaces (optional)
}
```

**Importance calibration guidance** (model must internalize):
- `0.9–1.0`: Core architectural decisions, user preferences, high-stakes facts
- `0.7–0.9`: Important domain knowledge, confirmed findings, design choices
- `0.5–0.7`: Supporting detail, context, background
- `0.3–0.5`: Transient state, casual remarks, low-certainty observations
- `<0.3`: Ephemeral (do not ingest)

v1 lesson: avoid capturing ephemeral state as permanent knowledge. The model
must distinguish "X is running" (transient) from "X was designed to do Y" (durable).

### 2.7 v1 → v2 Contrast

| Dimension | v1 (Valence/triples) | v2 (Covalence/sources) |
|-----------|---------------------|----------------------|
| Output primitive | `(Subject, predicate, object)` | Source ingestion JSON |
| Atomicity unit | Triple | Independently retrievable claim |
| Confidence | Inline `[confidence:high]` | Delegated to Covalence (source_type + topology) |
| Provenance | None | Typed edge in provenance graph |
| Relationship encoding | Explicit predicate | Embedded in content; cross-referenced via article compilation |
| Temporal markers | Inline `[temporal:1959]` | Metadata field + content-embedded |
| Schema flexibility | Fixed triple | Rich metadata dict, extensible |
| Flipped predicate risk | High (v1 known issue) | Lower (no predicate extraction) |
| Literal metaphor capture | High (v1 known issue) | Mitigated by importance calibration |

---

## 3. Compose Mode v2

### 3.1 What v1 Did

v1 COMPOSE converted triples into natural language with style registers:

```
[COMPOSE] [technical] (machine learning, part_of, artificial intelligence)
→ "machine learning occupies a central position in its field..."
```

This was rendering triples as prose — a template-heavy, single-topic operation.
For v2, COMPOSE is a higher-stakes operation: it assembles the **full reasoning
context** that a large model will receive. It must synthesize multiple sources of
varying confidence into a coherent, epistemically-honest context block.

### 3.2 The Core Tension: Epistemic Relevance vs. RAG

**Standard RAG** selects context by:
```
similarity(query, chunk) > threshold → include in prompt
```

Problems:
- **Contextual Tunneling** (SYNAPSE, arXiv:2601.02744): misses adjacent-but-relevant
  nodes (e.g., query "Python debugging" retrieves tutorials but misses the error log
  that triggered the need to debug — because the error log embeds differently)
- **Temporal bias**: recent content ranks higher regardless of epistemic weight
- **Flat trust**: all retrieved chunks treated as equally reliable
- **No graph structure**: doesn't know two retrieved chunks contradict each other

**Covalence epistemic relevance** selects by:
```
relevance_score = vector_score × 0.5
                + confidence × 0.35
                + freshness × 0.15
                + graph_score (topological, multi-hop BFS with 0.7^hop decay)
```

Additionally: provenance graph surfaces CONTRADICTS edges, allowing COMPOSE to
explicitly flag disagreements rather than silently including contradictory content.

**Why this beats RAG:**

| Failure mode | RAG | Covalence COMPOSE |
|-------------|-----|------------------|
| High similarity, low trust source | Included uncritically | Downweighted by confidence |
| Old but confirmed, graph-central fact | Displaced by recency | Preserved by topological score |
| Adjacent-but-relevant node (2 hops) | Missed entirely | Surfaced by graph BFS |
| Two sources contradicting each other | Both included, conflict silent | CONTRADICTS edge surfaced explicitly |
| Source from unreliable origin | No differentiation | source_type reliability applied |

### 3.3 COMPOSE Input

```json
{
  "query_context": "What is the Intermediary architecture and why does it matter?",
  "search_results": [
    {
      "node_id": "961c1ce1-addb-4776-8739-fb6761ac89c7",
      "node_type": "source",
      "score": 0.576,
      "confidence": 0.78,
      "vector_score": 0.634,
      "topological_score": 0.0,
      "content_preview": "The Intermediary — Core Idea: Replace accumulating conversation history with one-shot inference...",
      "title": "Intermediary Architecture Concept"
    },
    {
      "node_id": "68c0bb53-c760-4983-94cf-158ec28a5774",
      "node_type": "source",
      "score": 0.590,
      "confidence": 0.5,
      "vector_score": 0.564,
      "topological_score": 0.0,
      "content_preview": "The Intermediary — Refined: Redefines continuity as graph-native..."
    }
  ],
  "contentions": [],
  "budget_tokens": 2048,
  "register": "technical"
}
```

### 3.4 COMPOSE Output

A **structured context block** for injection into a reasoning model prompt:

```xml
<context>
<epistemic_status confidence="0.65" sources="2" contentions="0">

## The Intermediary Architecture

**Core concept** (confidence: high, source: Chris/Jane session 2026-03-02):
The Intermediary replaces buffer-native (temporal) context accumulation with
graph-native (epistemic) context composition. No conversation is ever appended
and fed forward. Instead, a small local model (Plasmon-class, 1.5B–3B) decomposes
each input into the knowledge substrate, then composes a fresh, tailored context
for each reasoning call.

**Inference loop**:
1. Event (user message, tool result, observation) occurs
2. Intermediary decomposes it into atomic knowledge → substrate
3. Intermediary queries substrate for what is epistemically relevant *now*
4. Reasoning model receives a single one-shot context from composition
5. Response is decomposed back into substrate

**Key insight** (confidence: high): Context for step N is not steps 1..N-1 plus
new input. It is what the substrate says matters given current state. Prior context
may drop; temporally distant but epistemically connected material may be surfaced.

**Why it matters**:
- No "lost in the middle" problem — there is no middle
- No conversation length limits — the conversation exists as a graph, not a sequence
- Continuity is epistemic (what matters) not temporal (what's recent)

**Connection to SYNAPSE** (arXiv:2601.02744): Spreading activation through the graph
formalizes this mechanism — activation propagates from the query node, surfacing
relevant nodes regardless of when they were created.

</epistemic_status>
</context>
```

### 3.5 COMPOSE Logic the Model Must Learn

1. **Select within budget**: from ranked results, choose which to include (not just
   top-k by score; quality × diversity trade-off within token budget)
2. **Synthesize, don't concatenate**: produce coherent prose, not raw source dumps
3. **Preserve epistemic hedging**: high-confidence claims stated firmly;
   low-confidence claims explicitly marked as tentative or uncertain
4. **Surface contentions**: if CONTRADICTS edges present among selected results,
   explicitly note the disagreement with source attribution
5. **Register calibration**: inherit from v1 (technical / casual / questioning /
   comparative), now applied to full multi-source context blocks
6. **Self-assessment**: output `epistemic_status` attributes (confidence, sources,
   contentions) so the reasoning model knows the epistemics of what it's receiving

### 3.6 Token Budget Management

COMPOSE must reason about token budgets as a first-class concern:

- Reasoning model context window: typically 8K–200K tokens (depends on model)
- Plasmon's composed context target: **1K–4K tokens** (leaves room for task + system prompt)
- When results exceed budget: prioritize by `confidence × score`, then by diversity
  (penalize same-topic redundancy, prefer cross-topic coverage)
- Minimum viable context: at least one high-confidence source OR one article;
  never compose an empty context block (signal "no relevant substrate content found")

### 3.7 Handling Weak Substrate

When search results are uniformly low-confidence or low-score:

```xml
<context>
<epistemic_status confidence="0.35" sources="1" contentions="0" substrate_coverage="weak">

## Limited Substrate Coverage

The knowledge substrate has sparse coverage for this query. The following
is available but should be treated as preliminary:

[low-confidence content here]

**Recommendation to reasoning model**: If precision is required, note that
substrate evidence is thin. Consider requesting additional context.

</epistemic_status>
</context>
```

This "epistemic humility" signal is critical — it tells the reasoning model not to
treat composed context as authoritative when it isn't.

---

## 4. Query Mode v2

### 4.1 What v1 Did

v1 QUERY converted NL questions into operation plans over Valence's triple store:

```json
{
  "operations": [
    {"type": "triple_query", "subject": "Claude Shannon", "predicate": "authored"},
    {"type": "for_each", "variable": "work", "source": "results[0].objects",
     "operation": {"type": "triple_query", "subject": "${work}", "predicate": "enables"}}
  ]
}
```

This was elegant for a triple store with traversable S/P/O links. Covalence has a
fundamentally different API: `/search` (4D semantic), `/sources/{id}`, `/articles/{id}`,
`/memory/search`. There are no explicit triple traversal operations.

### 4.2 v2 Target: Covalence API Call Sequences

v2 QUERY produces a sequence of **Covalence HTTP API calls** with parameters,
optional conditionals, and result-routing logic. The model must understand each
Covalence endpoint's semantics and when to use each:

| Endpoint | Purpose | When to use |
|----------|---------|-------------|
| `POST /search` | 4D semantic search | Primary retrieval for any query |
| `GET /articles/{id}` | Expand an article | When search returns an article preview worth expanding |
| `GET /sources/{id}` | Expand a source | When specific raw source needed (provenance check) |
| `POST /memory/search` | Tagged memory recall | Session context, personal preferences, high-importance stored observations |
| `POST /sources` | Ingest new source | DECOMPOSE path only; not used in QUERY |
| `POST /articles/compile` | Trigger recompilation | Maintenance; not used in QUERY |

### 4.3 Output Format

```json
{
  "query_intent": "Find epistemically relevant context about the Intermediary architecture for composing a reasoning prompt",
  "steps": [
    {
      "step": 1,
      "endpoint": "POST /search",
      "params": {
        "query": "Intermediary architecture epistemic graph-native continuity inference",
        "limit": 10
      },
      "store_as": "search_main"
    },
    {
      "step": 2,
      "condition": "search_main[0].score < 0.5",
      "endpoint": "POST /memory/search",
      "params": {
        "query": "Intermediary Plasmon inference architecture",
        "min_confidence": 0.5,
        "limit": 5
      },
      "store_as": "memory_results"
    },
    {
      "step": 3,
      "condition": "search_main[0].node_type == 'article' AND search_main[0].confidence > 0.6",
      "endpoint": "GET /articles/${search_main[0].node_id}",
      "params": {
        "include_provenance": true
      },
      "store_as": "article_expand"
    }
  ],
  "compose_from": ["search_main", "memory_results", "article_expand"]
}
```

### 4.4 Multi-Step Query Patterns the Model Must Learn

**Pattern 1: Search → Expand** (most common)
```
POST /search (broad semantic query)
  → if top result is article AND confidence > threshold:
    GET /articles/{id} (expand for full content)
  → compose_from: [search_results, expanded_article]
```

**Pattern 2: Memory-first** (session/personal context)
```
POST /memory/search (session-tagged memories)
  → POST /search (global substrate)
  → compose_from: [memories, search_results]
```
Personal memories and global articles inhabit different retrieval channels. A query
about user preferences or past decisions should hit memory first.

**Pattern 3: Dual-query for coverage**
```
POST /search ("query phrasing A")
POST /search ("query phrasing B — synonym or rephrasing")
  → compose_from: union(results_A, results_B), deduplicated by node_id
```
When one semantic framing might miss what another would catch.

**Pattern 4: Contention-aware retrieval**
```
POST /search
  → if results include nodes with CONTRADICTS-linked sources:
    GET /sources/{contention_source_id}  // surface the contradicting source
  → compose_from: [main_results, contention_sources]
```
COMPOSE needs contention information; QUERY must surface it proactively.

**Pattern 5: Iterative refinement**
```
POST /search ("broad concept")
  → if top score < 0.4 (weak match):
    POST /search ("narrowed query based on best preview content")
  → compose_from: [narrowed_results]
```
Low-confidence first results should trigger query refinement. Blindly composing
from weak results produces low-quality context.

### 4.5 Expressing Epistemic Relevance Through API Calls

Standard RAG asks: "what is similar to X?"
Covalence QUERY should ask: "what does the substrate say *matters* for X?"

These differ when:
- A low-similarity article has high topological confidence (many confirming sources,
  strong PageRank-style propagation from high-trust nodes)
- A high-similarity source has low reliability (source_type=web, no confirmation edges)
- The most relevant content is two hops from X in the graph

The model must learn to construct searches that exploit Covalence's multi-dimensional
scoring — for example, preferring a search that will surface topologically-central
articles over one that matches surface-level similarity, and using `/memory/search`
for session-contextual content that global search might rank lower.

### 4.6 Query Plan Failure Modes to Avoid

- **Over-querying**: 6+ API steps per inference cycle adds unacceptable latency
  (target: 2–4 steps max for real-time use)
- **Under-querying**: single search with default limit misses multi-hop connections
- **Ignoring memory**: pure `/search` calls miss session-specific high-importance memories
- **Always expanding**: GET /articles/{id} on every result; budget violation
  (expand only when confidence > 0.6 AND article has meaningful content beyond preview)

---

## 5. Training Approach

### 5.1 What v1's 4,321 Examples Teach Us

v1 trained 4,321 examples (later 4,401 after batch expansion) split across:
- **DECOMPOSE** (~2,000 examples): NL paragraph → triples, Wikipedia + diverse domains,
  multiple formats (paragraph, sentence, conversational, ambiguous)
- **COMPOSE** (~1,500 examples): triples → NL prose, 6 style registers
- **QUERY** (~800 examples): NL question → chained operation plan JSON

Key lessons:
- **DECOMPOSE generalized well** (train loss 0.07): structured extraction from NL
  is learnable with LoRA r32 at 1.5B scale; broad domain coverage beat narrow specialization
- **COMPOSE failed catastrophically** at v0.2: 183 corrective examples + low LR (`5e-6`)
  + fp16 → loss spiked to 155K, complete forgetting. Root cause: under-represented
  task + aggressive gradient accumulation on small dataset in fp16
- **QUERY worked** (structured JSON generation is within 1.5B model capability)
- **Known quality issues from v1**: literalized metaphors, ephemeral state captured
  as permanent triples, flipped predicates, case sensitivity inconsistencies —
  all must be addressed in v2 training data generation

### 5.2 v2 Training Data Targets

| Mode | v1 examples | v2 target | Reasoning |
|------|-------------|-----------|-----------|
| DECOMPOSE | ~2,000 | ~3,000 | New output schema; metadata + source_type reasoning adds complexity |
| COMPOSE | ~1,500 | ~2,500 | Much harder; multi-source synthesis, epistemic weighting, contention handling |
| QUERY | ~800 | ~1,500 | New API surface; multi-step conditionals, memory vs. search routing |
| **Total** | **~4,300** | **~7,000** | ~60% increase to cover v2 complexity |

### 5.3 Training Example Schemas

#### DECOMPOSE v2 Example

```json
{
  "input": "[DECOMPOSE_V2] User said in conversation: 'The new architecture replaces conversation history with fresh graph-composed context at each step. This means no context window limits and no lost-in-the-middle problem.'",
  "output": {
    "operations": [
      {
        "op": "ingest_source",
        "content": "A proposed architecture replaces conversation history accumulation with freshly composed context from a knowledge graph at each inference step.",
        "source_type": "user_input",
        "metadata": {
          "importance": 0.85,
          "tags": ["architecture", "inference", "context-window"],
          "context": "session:current",
          "memory": true
        }
      },
      {
        "op": "ingest_source",
        "content": "Fresh graph-composed context at each step eliminates context window length limits because no historical messages are accumulated.",
        "source_type": "user_input",
        "metadata": {
          "importance": 0.75,
          "tags": ["architecture", "context-window", "scaling"],
          "context": "session:current",
          "memory": false
        }
      },
      {
        "op": "ingest_source",
        "content": "Fresh context composition at each inference step eliminates the lost-in-the-middle problem because there is no middle — only a composed workspace.",
        "source_type": "user_input",
        "metadata": {
          "importance": 0.75,
          "tags": ["architecture", "retrieval", "context-quality"],
          "context": "session:current",
          "memory": false
        }
      }
    ]
  }
}
```

#### COMPOSE v2 Example

```json
{
  "input": "[COMPOSE_V2] {\"query_context\": \"What is spreading activation and how does SYNAPSE use it?\", \"search_results\": [{\"node_id\": \"16f8892c-...\", \"title\": \"SYNAPSE: Episodic-Semantic Memory via Spreading Activation\", \"confidence\": 0.6, \"score\": 0.51, \"content_preview\": \"SYNAPSE introduces spreading activation where relevance emerges from activation propagating through a memory graph, with lateral inhibition dampening irrelevant nodes...\"}], \"contentions\": [], \"budget_tokens\": 600, \"register\": \"technical\"}",
  "output": "<context>\n<epistemic_status confidence=\"0.6\" sources=\"1\" contentions=\"0\">\n\n## Spreading Activation in SYNAPSE (arXiv:2601.02744)\n\nSpreading activation is a retrieval mechanism where relevance propagates through a memory graph from the query node, activating neighbors which activate further neighbors. Unlike cosine similarity, activation is dynamic and context-sensitive: nodes activate differently depending on what other nodes are simultaneously active.\n\nSYNAPSE fuses three retrieval signals:\n- **Geometric embeddings** (vector similarity — like Covalence's vector dimension)\n- **Activation-based graph traversal** (spreading activation through the knowledge graph)\n- **Temporal decay** (recency bias, configurable)\n\nLateral inhibition: when one memory node is strongly activated, it dampens competing nodes that would otherwise add noise — analogous to graph score normalization.\n\n**Note**: Single source, moderate confidence (0.6). Cross-reference with additional sources if reasoning depends critically on these details.\n\n</epistemic_status>\n</context>"
}
```

#### QUERY v2 Example

```json
{
  "input": "[QUERY_V2] Find everything the system knows about the Intermediary architecture, prioritizing high-confidence material and surfacing any disagreements.",
  "output": {
    "query_intent": "Retrieve high-confidence Intermediary architecture material with contention detection",
    "steps": [
      {
        "step": 1,
        "endpoint": "POST /search",
        "params": {
          "query": "Intermediary architecture inference graph-native continuity context composition",
          "limit": 8
        },
        "store_as": "primary"
      },
      {
        "step": 2,
        "endpoint": "POST /memory/search",
        "params": {
          "query": "Intermediary Plasmon context composition one-shot inference",
          "min_confidence": 0.5,
          "limit": 5
        },
        "store_as": "memories"
      },
      {
        "step": 3,
        "condition": "primary[0].node_type == 'article' AND primary[0].confidence > 0.6",
        "endpoint": "GET /articles/${primary[0].node_id}",
        "params": {"include_provenance": true},
        "store_as": "top_article_expanded"
      }
    ],
    "compose_from": ["primary", "memories", "top_article_expanded"]
  }
}
```

### 5.4 Training Data Generation Strategy

**Phase 1 — Synthetic from Covalence itself** (~3,000 examples)

- Use existing Covalence sources and articles as gold-standard content
- **DECOMPOSE**: Take existing source content → reverse-engineer what ingestion plan
  would have produced it (using Claude Sonnet as teacher model)
- **COMPOSE**: Take known search result sets (from actual `/search` calls against
  the live substrate) → produce context blocks (Claude Sonnet teacher)
- **QUERY**: Build a library of ~50 query intent templates × expand to 1,500 variants
  with different phrasings, entity substitutions, complexity levels

**Phase 2 — Live capture from real usage** (~2,000 examples)

- Once v2 is deployed (even with naive COMPOSE), capture every
  Plasmon↔Covalence interaction as a training candidate
- Human-review flag: low-confidence COMPOSE outputs surfaced for correction
- Nightly incremental training runs (as envisioned in v1 README's self-reinforcing loop)

**Phase 3 — Adversarial and edge cases** (~2,000 examples)

- Inputs spanning multiple source_types (user quoting a document)
- Contradictory inputs that should produce `supersedes_id` references
- High-overlap inputs that should NOT generate redundant sources (deduplication training)
- Very short inputs (1 sentence), very long inputs (multi-paragraph)
- Ambiguous importance: model must make calibrated decisions
- "Empty substrate" COMPOSE: no good search results → produce graceful degradation response

### 5.5 Avoiding v0.2's Catastrophic Forgetting

v0.2's failure profile:
- 183 COMPOSE examples, `lr=5e-6`, fp16 → loss 155K, DECOMPOSE and QUERY broke entirely
- Also: from-scratch v3 training collapsed (loss 2.46 → 145K, 48/75 steps, zero grad_norm)

v2 mitigations:

| Risk factor | v0.2 failure | v2 mitigation |
|-------------|-------------|---------------|
| Imbalanced task representation | 183 COMPOSE only | Train all 3 modes jointly always |
| Insufficient examples | 183 total | 1,500+ per mode minimum before any run |
| Learning rate | `5e-6` (too small + unstable fp16) | `2e-4` cosine decay from v1 (proven) |
| Numerical precision | fp16 | **bf16 throughout** (better stability, supported by MLX + ROCm) |
| Gradient instability | No clipping | `max_grad_norm=1.0` enforced |
| No per-mode monitoring | Loss monitored globally | Per-mode eval loss in held-out set; halt if any mode spikes |
| Isolated fine-tuning | Yes | Never fine-tune a single mode after initial training |

---

## 6. Model Size & Architecture Analysis

### 6.1 Hardware Budget

**Target inference device**: Mac mini M4 (Apple Silicon)
**Throughput budget**: ~80–100 tokens/second for Plasmon v2 calls

This budget applies to **Plasmon v2 calls**, not the downstream reasoning model.
Each inference loop step involves at minimum 2–3 Plasmon calls:

- DECOMPOSE: ~50–100 output tokens → ~0.5–1.2s
- QUERY: ~100–200 output tokens → ~1.0–2.5s
- COMPOSE: ~300–800 output tokens → ~3.0–10s (the bottleneck)

**Total per inference step: ~5–14 seconds at 80 tok/s.**

For real-time conversation: 5–8 seconds is likely acceptable for a single step.
For multi-step agentic loops (4–6 steps), 20–40 seconds is borderline — may need
async UX design (stream composed context) rather than strict latency SLA.

### 6.2 Qwen2.5 1.5B vs 3B Tradeoffs

| Dimension | Qwen2.5 1.5B | Qwen2.5 3B |
|-----------|-------------|-----------|
| Throughput (MLX, M4 mini, bf16) | ~100–130 tok/s | ~55–80 tok/s |
| Throughput (llama.cpp Q4_K_M) | ~80–110 tok/s | ~45–65 tok/s |
| Budget fit (COMPOSE at 600 tok) | ~5–7s ✅ | ~8–13s ⚠️ |
| JSON schema adherence | Good (proven v1) | Better (more capacity) |
| COMPOSE synthesis quality | Adequate (TBD) | Better for multi-source |
| LoRA trainable params (r32) | ~26M | ~40M |
| Base model memory (bf16) | ~3.1 GB | ~6.0 GB |
| Quantized Q4_K_M | ~0.9 GB | ~1.8 GB |
| Fine-tune VRAM (LoRA r32) | ~8 GB ✅ | ~14 GB ⚠️ (tight on 9070 16GB) |

**Recommendation: Start with Qwen2.5 1.5B. Benchmark COMPOSE quality before committing.**

If COMPOSE quality at 1.5B is insufficient for multi-source synthesis, move to 3B
and accept ~60–70 tok/s — still within budget if the loop averages 2 Plasmon calls.

**Also evaluate Qwen3 1.7B** (released Q1 2026): instruction following reportedly
improved, similar throughput to Qwen2.5 1.5B on MLX, same training pipeline applicable.

The same LoRA training data applies to both model sizes; run quality benchmarks on
both before choosing production model.

### 6.3 LoRA vs Full Fine-Tune

| Dimension | LoRA r32/alpha-64 | Full Fine-Tune |
|-----------|------------------|----------------|
| Training VRAM (1.5B) | ~8 GB | ~20 GB |
| Training time | ~20 min (v1 ref) | ~2–3 hours |
| Base model preservation | ✅ Strong | ❌ Overwrites |
| Catastrophic forgetting risk | Lower | Higher |
| Adapter hot-reload | ✅ Yes (`/v2/reload`) | ❌ Full model swap |
| Serving overhead | +150 MB adapter | Zero |
| Multi-mode capability | One shared adapter | One model |

**Recommendation: LoRA r32, alpha 64, targeting `q_proj / v_proj / k_proj / o_proj + mlp`.**

Rationale: v1 demonstrated LoRA r32 sufficient for structured JSON generation at 1.5B
(loss 0.07). Adapter hot-swap is operationally valuable — update training without
service restart. Full fine-tune adds catastrophic forgetting risk with no proven gain
at this scale for these tasks.

**Consider LoRA r48 or r64** if COMPOSE quality is insufficient. COMPOSE requires
more nuanced synthesis than DECOMPOSE's structured extraction — more rank may help.

### 6.4 Quantization Strategy

| Phase | Precision | Memory | Speed | Notes |
|-------|-----------|--------|-------|-------|
| Development / evaluation | bf16 | ~3.1 GB | ~110 tok/s | Full quality, fast iteration |
| Production (default) | Q4_K_M | ~0.9 GB | ~85 tok/s | Best quality/size tradeoff |
| Memory-constrained | Q3_K_M | ~0.7 GB | ~90 tok/s | Some quality loss |
| LoRA adapter | bf16 always | +150 MB | — | Adapter stays full precision |

**MLX vs llama.cpp**: MLX preferred on Apple Silicon (native Metal acceleration);
llama.cpp as fallback for portability and for models where mlx-lm support lags release.

### 6.5 Serving Architecture

```
┌──────────────────────────────────────────────────────────┐
│                  Plasmon v2 Server                        │
│                  FastAPI — port 8423                      │
│                                                           │
│  Endpoints:                                               │
│    GET  /v2/health          — liveness check              │
│    POST /v2/decompose        — NL → ingestion plan JSON   │
│    POST /v2/query            — NL → Covalence API plan    │
│    POST /v2/compose          — results → context block    │
│    POST /v2/reload           — hot-swap LoRA adapter      │
│                                                           │
│  Model: Qwen2.5-1.5B (Q4_K_M in prod, bf16 in dev)       │
│  Adapter: plasmon-v2-r32.safetensors (~150 MB)            │
│  Runtime: MLX (primary) / llama.cpp (fallback)            │
│  LaunchD: com.plasmon.v2.server (KeepAlive, RunAtLoad)    │
│                                                           │
│  Co-deployed with:                                        │
│    Covalence substrate — port 8430                        │
│    Plasmon v1 (legacy) — port 8422 (until migration)      │
└──────────────────────────────────────────────────────────┘
```

---

## 7. Integration Interface

### 7.1 Plasmon v2 API (What It Exposes)

#### `POST /v2/decompose`

```
Request:
{
  "input":            string,  // raw NL text to decompose
  "source_context":   string,  // e.g., "session:main", "tool:browser"
  "source_type_hint": string   // optional classification override
}

Response:
{
  "operations": [
    {
      "op":          "ingest_source" | "compile_trigger" | "supersede",
      "content":     string,  // for ingest_source
      "source_type": string,
      "metadata":    { importance, tags, context, memory, supersedes_id? }
    }
  ],
  "model_confidence": float,
  "latency_ms":       int
}
```

#### `POST /v2/query`

```
Request:
{
  "question":     string,  // NL intent or reasoning need
  "context_hint": string,  // optional: current session topic
  "max_steps":    int      // default 3
}

Response:
{
  "query_intent": string,
  "steps": [
    {
      "step":     int,
      "endpoint": string,   // e.g. "POST /search"
      "params":   object,
      "condition": string | null,
      "store_as": string
    }
  ],
  "compose_from":     ["string"],
  "model_confidence": float,
  "latency_ms":       int
}
```

#### `POST /v2/compose`

```
Request:
{
  "query_context":  string,
  "search_results": [Covalence search result objects],
  "contentions":    [contention objects],   // optional
  "budget_tokens":  int,    // default 2048
  "register":       "technical" | "casual" | "questioning" | "comparative"
}

Response:
{
  "context_block":    string,  // assembled context for reasoning model
  "sources_used":     int,
  "tokens_estimated": int,
  "model_confidence": float,
  "latency_ms":       int
}
```

### 7.2 Covalence APIs Plasmon v2 Calls (via Orchestrator)

Plasmon v2 generates plans that reference these endpoints. The orchestrating layer
executes them:

| Operation | Endpoint | Triggered by |
|-----------|----------|-------------|
| Ingest new source | `POST /sources` | DECOMPOSE output execution |
| Store high-importance memory | `POST /memory` | DECOMPOSE high-importance items |
| Retrieve source full content | `GET /sources/{id}` | QUERY expand step |
| Retrieve article full content | `GET /articles/{id}` | QUERY expand step |
| Semantic search (primary) | `POST /search` | QUERY primary step |
| Memory recall | `POST /memory/search` | QUERY memory step |
| Trigger article compile | `POST /articles/compile` | DECOMPOSE compile_trigger op |

### 7.3 Orchestration Layer Responsibilities

Plasmon v2 is **stateless and plan-generating only**. An orchestrating layer (OpenClaw
Skill or equivalent application layer) executes the plans:

```
┌─────────────────────────────────────────────────────────┐
│                ORCHESTRATION LAYER                       │
│                                                          │
│ 1. Receive user input X                                  │
│ 2. Call Plasmon /v2/decompose(X) → ingestion plan        │
│    → Execute: POST /sources for each ingest_source op    │
│    → Execute: POST /articles/compile for compile ops     │
│ 3. Call Plasmon /v2/query(X) → API call plan             │
│    → Execute each step sequentially (conditional logic)  │
│    → Collect results by store_as labels                  │
│ 4. Call Plasmon /v2/compose(results) → context block     │
│ 5. Assemble final prompt:                                │
│      [system_prompt] + [context_block] + [task]          │
│ 6. Call reasoning model (one-shot)                       │
│ 7. Receive response Y                                    │
│ 8. Call Plasmon /v2/decompose(Y) → ingest response       │
│ 9. Return Y to user                                      │
└─────────────────────────────────────────────────────────┘
```

Orchestration layer must handle: API call error recovery, conditional expression
evaluation (step conditions in QUERY plans), result merging, token budget estimation,
Covalence authentication, and async execution where steps permit parallel calls.

**This orchestration layer is not in scope for Plasmon v2 itself** — it is the
application-layer concern. Plasmon v2 only generates plans and renders context.

### 7.4 Separation of Concerns

```
┌──────────────┐      Plans + Context      ┌──────────────────┐
│              │◀─────────────────────────▶│   Plasmon v2     │
│ Orchestration│                           │   (1.5B LoRA)    │
│    Layer     │                           └──────────────────┘
│              │      API Calls             ┌──────────────────┐
│              │◀─────────────────────────▶│   Covalence      │
│              │                           │   (port 8430)    │
└──────────────┘                           └──────────────────┘
      ▲                                              ▲
      │ user input / response                        │
      ▼                                              │
┌──────────────┐                           graph maintains
│   User /     │                           epistemic state
│   Agent      │
└──────────────┘
```

Plasmon v2 never directly touches Covalence. The orchestration layer is the executor.
This keeps Plasmon v2 swappable (upgrade model without changing Covalence integration)
and Covalence stable (API unchanged regardless of Plasmon model version).

### 7.5 Backward Compatibility

- v1 server remains on port 8422; v2 serves on port 8423 during parallel deployment
- v1 triple-based outputs are **not compatible** with Covalence's source model — no hybrid mode
- Migration: once v2 DECOMPOSE quality validates, switch orchestration layer to v2;
  retire v1 server
- The `ingest_v1.py` migration path (batch processing 779 beliefs → triples) does not
  apply to v2 → v3 transitions; v2 outputs are Covalence-native from inception

---

## 8. Open Questions & Design Risks

### 8.1 Unresolved Design Questions

**Q1: How granular should DECOMPOSE splitting be?**

"One atomic claim per source" is the conceptual target, but claim boundaries are fuzzy
and context-dependent. Is "The Intermediary replaces buffer continuity with graph
continuity" one claim or two? The training data generator must decide this before
training, as the model will internalize whatever grain is demonstrated. Requires
empirical study: measure retrieval precision vs. source count at different grain levels.

**Q2: Who decides source importance, and in what frame?**

Currently: Plasmon v2 assigns importance as an absolute value (0.0–1.0). But importance
is contextual — a claim about architecture matters a lot in the architecture domain,
not elsewhere. Should `importance` be domain-relative? And who arbitrates when Plasmon
v2's assigned importance conflicts with downstream retrieval patterns (organic forgetting
pressure)? The handoff semantics between Plasmon-assigned importance and Covalence's
`usage_score` dynamic maintenance need formalization.

**Q3: Should COMPOSE output be structured JSON or natural language prose?**

Current design: NL prose with XML epistemic_status tags. Alternative: structured JSON
with `{summary, high_confidence_claims, uncertain_claims, contentions}`. Structured
output is easier for reasoning models to parse programmatically and avoids ambiguity.
Natural prose is richer and may exploit reasoning models' NL training better. This
is partly a downstream reasoning model interface question, but Plasmon v2 must match
it. Decision should be made before generating training data.

**Q4: How does COMPOSE handle very large result sets?**

10 search results × 3 expanded articles = potentially 10,000+ tokens of raw content.
The selection algorithm (beyond `confidence × score`) is not fully specified. Candidates:
- Greedy by score until budget full (simple, proven)
- Maximal Marginal Relevance (diversity-penalized, better coverage)
- Model-decided selection as part of COMPOSE (most flexible, but adds to COMPOSE
  complexity and latency)

**Q5: "Cache miss" handling — what when the substrate has nothing?**

When a user asks about a genuinely novel topic with no substrate matches, the QUERY
plan returns low-score results. COMPOSE produces a weak-substrate context block.
The reasoning model is told to proceed with thin evidence. But: should the inference
loop instead signal "substrate miss" to the user and ask for input before proceeding?
This loop-breaking behavior needs design.

**Q6: DECOMPOSE deduplication — check before ingest?**

If the same fact enters the conversation multiple times, naive DECOMPOSE will create
duplicate sources, diluting retrieval. Should DECOMPOSE check for existing similar
sources (via `/search` before `POST /sources`) and suppress duplicates or issue a
`supersedes_id`? This adds latency to every DECOMPOSE call. Or is organic forgetting
sufficient to handle proliferation? The trade-off needs analysis.

### 8.2 Design Risks

---

**Risk 1: COMPOSE quality ceiling at 1.5B scale**
*Probability: HIGH | Impact: HIGH*

v1's COMPOSE was the weakest mode even at 1.5B. v2 COMPOSE is significantly harder:
synthesizing multi-source epistemic context vs. rendering single triples. There is
a real risk that 1.5B lacks the reasoning capacity for high-quality multi-source
synthesis with proper epistemic hedging and contention handling.

*Mitigation*: Design COMPOSE training data with maximum quality (Claude Sonnet teacher
model). Benchmark COMPOSE quality explicitly before committing to 1.5B. Have a
concrete 3B fallback plan. Consider using Claude Sonnet directly for COMPOSE during
initial deployment (as v1 did for that mode) while training data accumulates and
the 1.5B model learns from live examples.

---

**Risk 2: Source proliferation in the substrate**
*Probability: MEDIUM | Impact: HIGH*

If DECOMPOSE splits aggressively, the substrate accumulates sources rapidly. Each
user message might produce 3–8 sources; a 100-message conversation generates 300–800
sources. This affects: search result quality (dilution by redundant near-identical
sources), article compile overhead (more sources to process), and organic forgetting
pressure (importance scores diluted). v1's migration produced known quality issues
(literalized metaphors, ephemeral state captured as permanent) at 4,401 examples.

*Mitigation*: Enforce importance threshold (do not ingest below 0.3). Train DECOMPOSE
with explicit deduplication awareness. Consider a pre-ingest similarity check against
the substrate (adds latency but prevents explosion). Monitor source count growth in
production.

---

**Risk 3: Latency budget violation for real-time use**
*Probability: MEDIUM | Impact: MEDIUM*

Per-step latency breakdown (1.5B at 80 tok/s):
- DECOMPOSE: 50–100 tok → 0.6–1.2s
- Covalence POST /sources (×3): ~150ms
- QUERY: 150–200 tok → 1.9–2.5s
- Covalence /search (×2): ~200ms
- COMPOSE: 400–800 tok → 5–10s
- **Total: ~8–14s per inference step**

14 seconds is borderline for real-time conversation; for multi-step agentic loops,
this compounds badly.

*Mitigation*: Parallelize DECOMPOSE and QUERY where possible (independent operations).
Use streaming for COMPOSE output (start delivering context to reasoning model before
full COMPOSE completes). Aggressively limit COMPOSE output to 400 tokens by default
(increase only if reasoning model signals insufficient context). Cache repeated QUERY
patterns within a session.

---

**Risk 4: Training data quality for COMPOSE**
*Probability: HIGH | Impact: MEDIUM*

COMPOSE training data requires Claude Sonnet-quality synthesis as ground truth. If
teacher model outputs are inconsistent (varying epistemic hedging styles, inconsistent
confidence calibration, different `<epistemic_status>` attribute conventions), Plasmon
v2 learns a confused policy. The v1 COMPOSE failure had a training data quality
component — templates were mechanical and didn't reflect genuine synthesis.

*Mitigation*: Strict output schema for COMPOSE training data generation. Human review
of 10% sample before training. Automated consistency checks (e.g., does `confidence`
attribute match hedging language in the prose?). Use a single fixed system prompt
for the Claude Sonnet teacher across all COMPOSE example generation.

---

**Risk 5: Covalence API surface drift invalidates QUERY training data**
*Probability: LOW | Impact: HIGH*

QUERY training data encodes specific Covalence HTTP endpoints (`POST /search`,
`GET /articles/{id}`, etc.). If Covalence v2 renames endpoints, changes parameter
schemas, or adds new endpoints that QUERY should use, the model's learned patterns
become invalid. This is particularly acute because Covalence is actively developed.

*Mitigation*: Version-pin Covalence API surface in QUERY training data (document
which API version examples target). Treat QUERY output as a semantic plan, executed
by an orchestration layer that handles API versioning translations. Design the
orchestration layer with an API adapter pattern so model-generated endpoint names
can be remapped without retraining.

---

**Risk 6: Orchestration layer complexity underestimated**
*Probability: MEDIUM | Impact: HIGH*

The design assumes a "thin orchestration layer" executes QUERY plans. In practice this
layer must handle: step sequencing, conditional expression evaluation, result merging
across multiple steps, token estimation for COMPOSE budget, error recovery from
Covalence failures (timeout, 5xx), retry logic, authentication, and session state.
This is non-trivial. If poorly implemented, the orchestration layer becomes the
quality and reliability bottleneck regardless of Plasmon v2's quality.

*Mitigation*: Design the orchestration layer interface specification **before**
generating QUERY training data, so examples reflect realistic execution semantics.
Write an integration test suite that validates plan execution against a live Covalence
instance. Do not treat orchestration as a "glue code" afterthought.

---

**Risk 7: Weak initial graph (cold-start quality problem)**
*Probability: MEDIUM | Impact: MEDIUM*

COMPOSE quality depends on substrate richness. Early in deployment, the substrate
may be too sparse for meaningful epistemic context. QUERY returns low-confidence
results; COMPOSE has little to synthesize; the reasoning model gets thin context.
The system looks worse than vanilla RAG initially.

*Mitigation*: Pre-populate substrate from existing sources (session transcripts,
documents, any historical material available). The existing Covalence KB already has
content from February–March 2026 sessions — ensure it is available to v2 at launch.
Accept that quality is self-reinforcing: more usage → richer substrate → better COMPOSE.
Set expectations: v2 quality improves over weeks, not immediately.

---

## Appendix A: Key Terminology

| Term | Definition |
|------|-----------|
| **Intermediary** | Plasmon v2's architectural role: mediating between NL and the Covalence substrate |
| **Epistemic relevance** | Relevance determined by graph topology + confidence + provenance, not temporal proximity |
| **Graph-native continuity** | Session continuity maintained through knowledge graph state, not conversation history buffers |
| **Buffer-native continuity** | Traditional approach: include recent message history in context window |
| **One-shot inference** | Each reasoning model call gets a fresh composed context; no message accumulation |
| **Source** | Raw immutable input in Covalence (text, observations, documents); has `source_type` and reliability |
| **Article** | LLM-compiled summary in Covalence, derived from sources via typed provenance graph |
| **Spreading activation** | SYNAPSE mechanism: relevance propagates through graph from query node, context-sensitive |
| **Contextual tunneling** | RAG failure: retrieves similar-embedding content but misses graph-adjacent relevant nodes |
| **LoRA** | Low-Rank Adaptation: parameter-efficient fine-tuning; adapts base model without full retraining |
| **Compose budget** | Token count available for Plasmon's assembled context block within reasoning model's window |

## Appendix B: v1 → v2 Delta Summary

| Aspect | Plasmon v1 | Plasmon v2 |
|--------|-----------|-----------|
| Substrate | Valence (triple store) | Covalence (source/article graph) |
| DECOMPOSE output | `(S, P, O)` triples with annotations | Structured JSON ingestion plan |
| COMPOSE input | Single triple or triple list | Multi-source Covalence search result set |
| COMPOSE output | Style-registered prose (one topic) | Full epistemic context block (multi-source, hedged) |
| QUERY output | Valence triple-store operation plan | Covalence HTTP API call sequence |
| Confidence handling | Inline annotation `[confidence:high]` | Delegated entirely to Covalence topology |
| Provenance | None | Typed edges (ORIGINATES / CONFIRMS / SUPERSEDES / CONTRADICTS) |
| Port | 8422 | 8423 (initially; replaces 8422 post-migration) |
| Base model | Qwen2.5-1.5B | Qwen2.5-1.5B or 3B, or Qwen3-1.7B (TBD by benchmark) |
| LoRA rank | r32 / alpha-64 | r32–r64 / alpha-64 (TBD by COMPOSE quality) |
| Training examples | 4,321–4,401 | ~7,000 target |
| Training backend | ROCm (RX 9070) | ROCm (RX 9070) or MLX (M4 Mac mini) |
| Known quality issues | Flipped predicates, literalized metaphors, ephemeral state | Addressed in training data design |

---

*Spec written: 2026-03-02. Last substantive input: Chris/Jane session ~21:14 PST.*
*Filed under: tracking#101 — Plasmon v2 Design Spec (Covalence-native Intermediary).*

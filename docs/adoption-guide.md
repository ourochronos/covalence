# Covalence Adoption Guide

> **Goal:** Get from "what is this?" to "I'm using it in my agent" in 30 minutes.

Covalence is a **knowledge engine for AI agents** — a PostgreSQL-backed persistent memory system with pgvector semantic search, Apache AGE graph traversal, and a full epistemic model. It doesn't just store text; it tracks *where knowledge came from*, *how confident you should be in it*, and *when two pieces of knowledge conflict*.

This guide walks you through setup, core concepts, integration patterns, search tuning, maintenance, and best practices.

---

## Table of Contents

1. [Introduction](#1-introduction)
2. [Setup](#2-setup)
3. [Core Concepts](#3-core-concepts)
4. [Integration Patterns](#4-integration-patterns)
5. [Search](#5-search)
6. [Maintenance](#6-maintenance)
7. [OpenClaw Integration](#7-openclaw-integration)
8. [Best Practices](#8-best-practices)

---

## 1. Introduction

### Why agents need persistent epistemic memory

A language model running a task starts fresh every time. It forgets what it learned last session, can't cite where a fact came from, and has no way to notice when two sources contradict each other. For short tasks, that's fine. For long-running agents — research assistants, coding agents, documentation bots — stateless memory is a liability.

Covalence solves this with three properties that plain vector stores lack:

| Property | What it means |
|---|---|
| **Provenance** | Every piece of knowledge links back to the source it came from |
| **Epistemic confidence** | Knowledge is scored by type and corroboration, not treated as equally trustworthy |
| **Contention detection** | When new information contradicts old, the conflict surfaces explicitly instead of silently overwriting |

### The knowledge lifecycle

```
                      ┌──────────┐
                      │  Ingest  │  raw text, web pages, tool output,
                      │  Sources │  conversations, observations
                      └────┬─────┘
                           │  background: embed + contention_check
                      ┌────▼─────┐
                      │ Compile  │  LLM synthesizes related sources
                      │ Articles │  into curated knowledge nodes
                      └────┬─────┘
                           │  background: embed + infer_edges
                      ┌────▼─────┐
                      │  Build   │  typed relationships form
                      │  Graph   │  a knowledge graph
                      └────┬─────┘
                           │
                      ┌────▼─────┐
     agents ◄──────── │  Search  │  vector + lexical + graph
     recall           └────┬─────┘
                           │
                      ┌────▼─────┐
                      │ Maintain │  score recomputation, eviction,
                      └──────────┘  contention review, decay
```

Sources are raw and immutable. Articles are curated and versioned. The graph connects them. Search draws from all three dimensions. Maintenance keeps the whole system healthy.

---

## 2. Setup

### Docker quick start

The fastest path to a running Covalence instance is one command:

```bash
git clone https://github.com/ourochronos/covalence.git
cd covalence
docker compose up -d
curl http://localhost:8430/health
```

This starts PostgreSQL 17 (with pgvector + Apache AGE) and the Covalence engine. Migrations are applied automatically on first boot.

**Enable LLM features** (embeddings and article compilation):

```bash
cp .env.example .env
# Edit .env and set OPENAI_API_KEY
docker compose up -d
```

See the [README](../README.md) for manual/local development setup and the full environment variable reference.

> **Ports:** Engine → `localhost:8430` | PostgreSQL → `localhost:5434`

### Python client

Install from PyPI:

```bash
pip install covalence-client
```

Or from source:

```bash
git clone https://github.com/ourochronos/covalence-py
cd covalence-py
pip install -e ".[dev]"
```

**Requirements:** Python 3.10+

### Configuration

The client defaults to `http://localhost:8430`. Override with the first argument:

```python
from covalence import CovalenceClient

# Local default
client = CovalenceClient()

# Custom endpoint
client = CovalenceClient("http://my-covalence-host:8430")

# With auth token (if running behind a proxy/plugin layer)
client = CovalenceClient("http://localhost:8430", auth_token="my-token")
```

Both a synchronous (`CovalenceClient`) and asynchronous (`AsyncCovalenceClient`) client are available. Use the sync client for scripts, the async client inside `asyncio`-based agents:

```python
# Sync — use as context manager
with CovalenceClient() as client:
    stats = client.get_admin_stats()

# Async — use as async context manager
import asyncio
from covalence import AsyncCovalenceClient

async def main():
    async with AsyncCovalenceClient() as client:
        stats = await client.get_admin_stats()

asyncio.run(main())
```

---

## 3. Core Concepts

### Sources — raw, immutable inputs

A **source** is anything your agent ingests: a document, a web page, a tool output, a snippet of conversation, or a raw observation. Sources are:

- **Immutable** — never edited after ingest (content hash is SHA-256)
- **Idempotent** — ingesting the same content twice returns the same record
- **Reliability-scored** by type

| `source_type` | Default reliability | Use for |
|---|---|---|
| `document` | 0.80 | PDFs, structured docs, official references |
| `code` | 0.80 | Source code, config files |
| `tool_output` | 0.70 | Structured output from tools, APIs |
| `user_input` | 0.75 | Explicit statements from a human |
| `web` | 0.60 | Web pages, scraped content |
| `conversation` | 0.50 | Chat transcripts, informal discussion |
| `observation` | 0.40 | Agent self-observations, inferences |

You can override reliability for any source. A primary source from the official Rust documentation is more trustworthy than a random blog post, even if both are `web` type:

```python
# Authoritative — bump reliability
official = client.ingest_source(
    "Rust does NOT use a garbage collector...",
    source_type="document",
    title="Official Rust Book: Ownership",
    reliability=0.95,
)

# Uncertain blog post — drop reliability
blog = client.ingest_source(
    "Rust uses a modern GC to manage memory automatically.",
    source_type="web",
    title="Some Blog Post",
    reliability=0.30,
)
```

### Articles — curated, versioned knowledge

An **article** is a synthesized knowledge node compiled from one or more sources. Articles are:

- **Mutable** — versioned with a `version` counter; content can be updated
- **Right-sized** — target 200–4000 tokens; split or merge to stay in range
- **Compilable** — an LLM can synthesize multiple sources into a coherent article

Create an article directly (agent-authored):

```python
article = client.create_article(
    "Rust achieves memory safety without a GC via ownership and borrowing.",
    title="Rust Memory Safety",
    domain_path=["rust", "memory", "safety"],
    source_ids=[official.id],           # provenance link
    epistemic_type="semantic",          # semantic | episodic | procedural | declarative
)
```

Or compile sources via LLM (asynchronous job):

```python
job = client.compile_article(
    [source_a.id, source_b.id, source_c.id],
    title_hint="Rust memory management patterns",
)
print(f"Job {job.job_id} — poll GET /admin/queue/{job.job_id}")
```

The compile job runs in the background. Poll the queue to check completion:

```python
import time

while True:
    entry = client.get_queue_entry(job.job_id)
    if entry.status == "completed":
        break
    if entry.status == "failed":
        raise RuntimeError("Compilation failed")
    time.sleep(2)
```

### Edges — typed relationships

Every relationship between nodes is a **typed edge** in the Apache AGE graph. The engine creates many edges automatically (e.g., `ORIGINATES` when you link source_ids to an article); you can also create them explicitly.

**Provenance edges** (created automatically or by the agent):

| Edge type | Meaning |
|---|---|
| `ORIGINATES` | Source is the primary origin of an article |
| `CONFIRMS` | Source corroborates an existing article |
| `SUPERSEDES` | New node replaces old |
| `CONTRADICTS` | Directly conflicting claims |
| `CONTENDS` | Softer disagreement / alternative view |
| `EXTENDS` | Elaborates without superseding |

**Semantic edges** (typically LLM-inferred):

| Edge type | Meaning |
|---|---|
| `RELATES_TO` | Generic semantic relatedness |
| `GENERALIZES` | Abstract ↔ specific |
| `CAUSES` | Causal relationship |
| `MOTIVATED_BY` | Decision motivated by knowledge |
| `IMPLEMENTS` | Concrete artifact of abstract concept |

**Temporal edges**: `PRECEDES`, `FOLLOWS`, `CONCURRENT_WITH`

Create an edge explicitly:

```python
from covalence.models import EdgeLabel

edge = client.create_edge(
    from_node_id=blog.id,
    to_node_id=article.id,
    label=EdgeLabel.CONTRADICTS,
    confidence=0.9,
    notes="Blog claims Rust uses GC; article states otherwise.",
)
```

### Confidence — weighted epistemic scoring

Every node carries a `confidence` score (0–1). It's not a single number but a composite:

```
confidence = source×0.30 + method×0.15 + consistency×0.20
           + freshness×0.10 + corroboration×0.15 + applicability×0.10
```

Key drivers:
- **Source reliability** (0.30 weight) — set when you ingest the source
- **Corroboration** (0.15 weight) — more `CONFIRMS` edges from independent sources raises confidence
- **Freshness** (0.10 weight) — knowledge decays slowly over time
- **Consistency** (0.20 weight) — active contentions lower this

This score feeds directly into **search ranking**:

```
final_score = dim_score×0.85 + confidence×0.10 + freshness×0.05
```

Higher-confidence knowledge surfaces higher in search results.

### Contentions — explicit conflict handling

When a new source's content appears to contradict an existing article, Covalence automatically creates a **contention** record. Knowledge is never silently overwritten.

Contentions have severity (`low`, `medium`, `high`) and require explicit resolution:

| Resolution | Meaning |
|---|---|
| `supersede_a` | Article wins; source is noted but article is unchanged |
| `supersede_b` | Source wins; article content is updated |
| `accept_both` | Both perspectives are valid; article is annotated |
| `dismiss` | Not materially conflicting; dismissed without change |

```python
from covalence.models import ContentionStatus, ContentionResolution

# Find open contentions for an article
contentions = client.list_contentions(
    node_id=article.id,
    status=ContentionStatus.detected,
)

for c in contentions.data:
    print(f"[{c.severity}] {c.description}")

# Resolve one
resolved = client.resolve_contention(
    contentions.data[0].id,
    resolution=ContentionResolution.supersede_a,
    rationale="Conflicting source is an uncited blog post with 0.30 reliability.",
)
```

Unresolved contentions accumulate a `contention_count` on the article and reduce its `consistency` sub-score.

### Provenance — tracing claims to sources

Every article links back to its originating sources via the graph. Two tools help investigate provenance:

**Walk the full provenance chain:**

```python
prov = client.get_provenance(article.id, max_depth=3)

for entry in prov.data:
    print(f"  depth={entry.depth}  {entry.edge_type}")
    print(f"    ← {entry.source_node.get('title')} ({entry.source_node.get('id')})")
```

**Trace a specific claim to its sources (TF-IDF similarity):**

```python
trace = client.trace_claim(
    article.id,
    "ownership eliminates dangling pointer bugs at compile time"
)

for result in trace.data:
    print(f"  [{result.score:.3f}] {result.title}")
    print(f"    …{result.snippet[:80]}…")
```

`trace_claim` ranks the article's linked sources by how much their content overlaps with the specific sentence you're investigating. Useful for fact-checking and audit.

### Memory — agent-friendly observation wrappers

The Memory API is a purpose-built layer on top of sources for agent self-observations. It adds:

- **Importance scoring** (drives confidence: `0.4 + importance × 0.4`)
- **Tag-based filtering** for fast categorical recall
- **Soft forgetting** with audit trail
- **Superseding** — mark an old memory as replaced when you update it

```python
# Store a memory
mem = client.store_memory(
    "The user prefers dark mode in all UI contexts.",
    tags=["preferences", "ui"],
    importance=0.8,
    context="conversation:user",
)

# Recall memories by query + tag
results = client.recall_memories(
    "user interface preferences",
    tags=["ui"],
    min_confidence=0.5,
    limit=5,
)

# Update a preference — supersede the old memory
new_mem = client.store_memory(
    "User now prefers light mode when working outdoors.",
    tags=["preferences", "ui"],
    importance=0.85,
    supersedes_id=mem.id,
)

# Soft-delete the old one (excluded from future recall)
client.forget_memory(mem.id, reason="Superseded by newer observation.")
```

---

## 4. Integration Patterns

### Pattern 1: Simple Memory (Conversational Agents)

The simplest integration — use Covalence as a smart notepad for conversational agents. Store observations, recall relevant context before each turn.

```python
from covalence import CovalenceClient

client = CovalenceClient()

# At the start of each conversation turn: recall relevant context
def get_context(topic: str) -> list[str]:
    results = client.recall_memories(topic, limit=5, min_confidence=0.5)
    return [m.content for m in results.data]

# After a conversation turn: store what was learned
def remember(fact: str, tags: list[str], importance: float = 0.6) -> None:
    client.store_memory(fact, tags=tags, importance=importance)

# Example usage in an agent loop
context = get_context("user preferences")
# ... run LLM with context injected ...
remember("User mentioned they use VS Code as their editor.", tags=["tools", "setup"])
```

**When to use:** Short-lived agents, chatbots, agents that need to remember facts about a user or session but don't need full knowledge compilation.

### Pattern 2: Knowledge Accumulation

Build a persistent knowledge base that grows over time. Ingest raw sources as you encounter them, then periodically compile clusters of related sources into curated articles.

```python
from covalence import CovalenceClient
from covalence.models import SourceType

client = CovalenceClient()

# Step 1: Ingest sources as you encounter them
def ingest_web_page(url: str, content: str, topic: str) -> str:
    source = client.ingest_source(
        content,
        source_type=SourceType.web,
        title=f"Web: {url}",
        metadata={"url": url, "topic": topic},
    )
    return source.id

def ingest_document(path: str, content: str) -> str:
    source = client.ingest_source(
        content,
        source_type=SourceType.document,
        title=path,
        metadata={"path": path},
        reliability=0.85,
    )
    return source.id

# Step 2: Compile related sources into a curated article
def build_article(source_ids: list[str], title: str) -> str:
    job = client.compile_article(source_ids, title_hint=title)
    # In production: poll until completed (see Section 3)
    print(f"Compilation queued: {job.job_id}")
    return job.job_id

# Step 3: Tag with domain_path for organization
def organize(article_id: str, domain: list[str]) -> None:
    client.update_article(article_id, domain_path=domain)

# Step 4: Search the accumulated knowledge
def ask(question: str) -> list[str]:
    results = client.search(question, node_types=["article"], limit=5)
    return [f"[{r.score:.3f}] {r.title}: {r.content_preview}" for r in results.data]
```

**When to use:** Research agents, documentation assistants, any agent that reads many inputs and needs to query a growing corpus.

### Pattern 3: Epistemic Agent (Full Model)

The full Covalence experience — ingest with provenance awareness, detect and resolve contentions, cite sources in outputs.

```python
from covalence import CovalenceClient
from covalence.models import (
    ContentionStatus, ContentionResolution,
    EdgeLabel, SourceType,
)

client = CovalenceClient()

class EpistemicAgent:
    def ingest(self, content: str, source_type: str, title: str,
               reliability: float | None = None) -> str:
        """Ingest a source and return its ID."""
        source = client.ingest_source(
            content,
            source_type=source_type,
            title=title,
            reliability=reliability,
        )
        print(f"Ingested: {source.id}  confidence={source.confidence:.2f}")
        return source.id

    def build_knowledge(self, source_ids: list[str], title: str) -> str:
        """Compile sources into an article."""
        job = client.compile_article(source_ids, title_hint=title)
        return job.job_id

    def review_contentions(self) -> None:
        """Surface and resolve any detected contentions."""
        all_contentions = client.list_contentions(status=ContentionStatus.detected)

        for c in all_contentions.data:
            article = client.get_article(c.node_id)
            source = client.get_source(c.source_node_id)

            print(f"\nContention detected [{c.severity}]:")
            print(f"  Article:  {article.title!r} (confidence={article.confidence:.2f})")
            print(f"  Source:   {source.title!r}  (reliability={source.reliability:.2f})")
            print(f"  Conflict: {c.description}")

            # Simple heuristic: trust whichever side has higher confidence
            if source.confidence > article.confidence:
                resolution = ContentionResolution.supersede_b
                rationale = (
                    f"Source has higher confidence "
                    f"({source.confidence:.2f} > {article.confidence:.2f})"
                )
            else:
                resolution = ContentionResolution.supersede_a
                rationale = (
                    f"Article has higher confidence "
                    f"({article.confidence:.2f} >= {source.confidence:.2f})"
                )

            client.resolve_contention(c.id, resolution=resolution, rationale=rationale)
            print(f"  Resolved: {resolution.value}")

    def cite(self, article_id: str, claim: str) -> list[str]:
        """Return source citations for a specific claim."""
        trace = client.trace_claim(article_id, claim)
        return [
            f"{r.title} (score={r.score:.3f})"
            for r in trace.data
            if r.score > 0.3
        ]

    def answer(self, question: str) -> dict:
        """Search for relevant knowledge and include provenance."""
        results = client.search(question, node_types=["article"], limit=3)
        answer_parts = []
        for r in results.data:
            prov = client.get_provenance(r.node_id, max_depth=2)
            sources = [e.source_node.get("title") for e in prov.data]
            answer_parts.append({
                "content": r.content_preview,
                "confidence": r.confidence,
                "sources": sources,
            })
        return {"question": question, "results": answer_parts}
```

**When to use:** Research pipelines, fact-checking agents, any application where you need to audit where an answer came from.

### Pattern 4: Multi-Agent Shared Knowledge Base

Multiple agents writing to and reading from the same Covalence instance. Each agent uses sessions to scope its ingestion, but all share the global article pool.

```python
import uuid
from covalence import CovalenceClient

class AgentWorker:
    def __init__(self, name: str, server_url: str = "http://localhost:8430"):
        self.name = name
        self.client = CovalenceClient(server_url)
        # Create a session to scope this agent's ingestion
        session = self.client.create_session(
            label=f"{name}-{uuid.uuid4().hex[:8]}",
            metadata={"agent": name},
        )
        self.session_id = session.id
        print(f"[{name}] Session: {self.session_id}")

    def learn(self, content: str, title: str) -> str:
        """Ingest a source scoped to this agent's session."""
        source = self.client.ingest_source(
            content,
            source_type="observation",
            title=title,
            session_id=self.session_id,   # CAPTURED_IN edge created automatically
        )
        return source.id

    def share(self, content: str, title: str, domain: list[str]) -> str:
        """Create an article visible to all agents."""
        article = self.client.create_article(
            content,
            title=title,
            domain_path=domain,
        )
        return article.id

    def query(self, question: str) -> list[str]:
        """Search the shared knowledge base."""
        results = self.client.search(question, node_types=["article"], limit=5)
        return [r.content_preview for r in results.data]

    def done(self) -> None:
        self.client.close_session(self.session_id)

# Two agents working in parallel on the same KB
researcher = AgentWorker("researcher")
writer = AgentWorker("writer")

# Researcher ingests findings
src_id = researcher.learn("Python's GIL limits true parallelism.", "GIL Note")

# Researcher compiles and shares with the team
art_id = researcher.share(
    "The Python GIL prevents true multi-threaded parallelism in CPython.",
    title="Python GIL",
    domain=["python", "concurrency"],
)

# Writer can query the same KB immediately
context = writer.query("Python threading limitations")

researcher.done()
writer.done()
```

**Coordination notes:**
- Sessions scope ingestion for audit (`CAPTURED_IN` edges) but don't isolate knowledge
- All agents see all active articles in search
- Use `domain_path` to logically partition the KB (e.g., `["project-alpha", ...]`)
- Pinned articles (`client.update_article(id, pinned=True)`) survive eviction — pin your shared reference articles

---

## 5. Search

### How search works

Covalence uses a **three-dimensional retrieval cascade** that runs in parallel then fuses scores:

```
┌─────────────────────┐    ┌─────────────────────┐
│   Vector (semantic) │    │  Lexical (BM25/FTS)  │
│   pgvector HNSW     │    │  PostgreSQL ts_rank  │
│   halfvec(1536)     │    │  websearch_to_tsquery│
└──────────┬──────────┘    └──────────┬───────────┘
           │                          │
           └──────────┬───────────────┘
                      │
           ┌──────────▼──────────┐
           │   Graph dimension   │
           │   neighborhood walk │
           │   (Apache AGE)      │
           └──────────┬──────────┘
                      │
           ┌──────────▼──────────┐
           │   RRF score fusion  │
           │   + confidence      │
           │   + freshness       │
           └─────────────────────┘

final_score = dim_score×0.85 + confidence×0.10 + freshness×0.05
```

Vector and lexical run in **parallel** on all active nodes. Graph runs on the top candidates from step one, expanding into their knowledge neighborhood. Freshness decays as `exp(-0.01 × days_since_modified)`.

> **LLM required for vector search.** If `OPENAI_API_KEY` is not set, only lexical search runs. To check: `stats = client.get_admin_stats(); print(stats.embeddings)`.

### Basic search

```python
results = client.search("Rust memory safety", limit=10)

for r in results.data:
    print(f"[{r.score:.3f}] [{r.confidence:.2f}] {r.title}")
    print(f"  {r.content_preview[:120]}")
    print(f"  vector={r.vector_score:.3f}  lexical={r.lexical_score:.3f}")
```

### Search parameters

```python
results = client.search(
    query="how does Python handle concurrency",

    # Filter to node type
    node_types=["article"],          # "source", "article", or both

    # Routing hint for graph dimension
    intent="factual",                # factual | temporal | causal | entity

    # Result count
    limit=5,

    # Custom dimension weights (auto-normalized to sum 1.0)
    weights={"vector": 0.7, "lexical": 0.2, "graph": 0.1},

    # Session context for session-aware ranking
    session_id=my_session_id,
)
```

**Intent routing** adjusts which graph edges are prioritized:

| Intent | Prioritized edges |
|---|---|
| `factual` | `CONFIRMS`, `ORIGINATES` |
| `temporal` | `PRECEDES`, `FOLLOWS`, `CONCURRENT_WITH` |
| `causal` | `CAUSES`, `MOTIVATED_BY`, `IMPLEMENTS` |
| `entity` | `INVOLVES`, `CAPTURED_IN` |

### Tuning search quality

**More semantic, less keyword:**
```python
weights={"vector": 0.8, "lexical": 0.1, "graph": 0.1}
```

**More keyword precision (good for code or exact names):**
```python
weights={"vector": 0.3, "lexical": 0.6, "graph": 0.1}
```

**Leverage the graph for connected topics:**
```python
weights={"vector": 0.5, "lexical": 0.2, "graph": 0.3}
```

**Filter to high-confidence results:**
```python
# Post-retrieval filter on the client side
results = client.search("Rust", limit=20)
high_conf = [r for r in results.data if r.confidence >= 0.7]
```

**Use `domain_path` for organization** — tag articles with `domain_path=["rust", "memory"]` when creating them. Domain paths act as hierarchical namespaces (e.g., `["project-alpha", "design", "api"]`) and are visible on search result objects for your own filtering.

**Ensure embeddings exist:**
```python
stats = client.get_admin_stats()
missing = stats.embeddings.nodes_without
if missing > 0:
    result = client.embed_all()
    print(f"Queued {result['queued']} embedding tasks")
```

### Memory recall vs. unified search

The Memory API's `recall_memories` uses full-text search only (no vector). For semantic recall over memories, use the main `search` endpoint with `node_types=["source"]` — sources tagged with `metadata.memory = true` are also indexed. The unified search endpoint covers all node types.

---

## 6. Maintenance

### Monitoring with admin stats

```python
stats = client.get_admin_stats()

# Knowledge base size
print(f"Sources:  {stats.nodes.sources}")
print(f"Articles: {stats.nodes.articles} ({stats.nodes.pinned} pinned)")
print(f"Active:   {stats.nodes.active}")

# Background queue health
print(f"Queue — pending={stats.queue.pending}, "
      f"processing={stats.queue.processing}, "
      f"failed={stats.queue.failed}, "
      f"completed_24h={stats.queue.completed_24h}")

# Embedding coverage
pct = 100 * (1 - stats.embeddings.nodes_without / max(stats.nodes.total, 1))
print(f"Embeddings: {pct:.0f}% coverage ({stats.embeddings.nodes_without} missing)")

# Graph sync
if not stats.edges.in_sync:
    print("WARNING: SQL/AGE edge counts diverged — partial write failure")
```

**Key signals to watch:**

| Signal | Healthy | Action if unhealthy |
|---|---|---|
| `queue.failed` | 0 | Inspect with `list_queue(status="failed")`, retry or investigate |
| `queue.pending` | Low (< 20) | Spike is normal after bulk ingest; sustained high = worker issue |
| `embeddings.nodes_without` | 0 | Run `embed_all()` to queue missing embeddings |
| `edges.in_sync` | `true` | Investigate partial write failures in engine logs |
| Active contentions | Low | Review with `list_contentions(status="detected")` |

### Maintenance operations

```python
# Recompute usage scores (do before search tuning or after bulk updates)
result = client.run_maintenance(recompute_scores=True)
print(result.actions_taken)

# Process stale queue entries (jobs stuck > 10 min → mark failed for retry)
result = client.run_maintenance(process_queue=True)

# Evict low-scoring articles when over capacity (default limit: 1000)
result = client.run_maintenance(
    evict_if_over_capacity=True,
    evict_count=20,          # max to evict per run
)

# All together — a full maintenance pass
result = client.run_maintenance(
    recompute_scores=True,
    process_queue=True,
    evict_if_over_capacity=True,
    evict_count=10,
)
print(result.actions_taken)
```

### Handling failed queue jobs

```python
# List failed jobs
failed = client.list_queue(status="failed")
for job in failed.data:
    print(f"{job.task_type}: node={job.node_id}  status={job.status}")

# Retry a failed job
client.retry_queue_entry(failed.data[0].id)

# Or delete it if permanently unrecoverable
client.delete_queue_entry(failed.data[0].id)
```

### Reviewing contentions

Make contention review a regular habit for long-running agents:

```python
def review_all_contentions(auto_dismiss_low: bool = False) -> None:
    contentions = client.list_contentions(status="detected")
    print(f"Open contentions: {len(contentions.data)}")

    for c in contentions.data:
        print(f"\n[{c.severity}] {c.description}")
        article = client.get_article(c.node_id)
        print(f"  Article: {article.title!r} (v{article.version})")

        if auto_dismiss_low and c.severity == "low":
            client.resolve_contention(
                c.id,
                resolution="dismiss",
                rationale="Auto-dismissed: low severity contention",
            )
```

### Backup strategy

Covalence's state lives entirely in PostgreSQL. Back up with standard `pg_dump`:

```bash
# Snapshot backup
pg_dump postgres://covalence:covalence@localhost:5434/covalence \
    -Fc -f covalence-$(date +%Y%m%d-%H%M%S).dump

# Restore
pg_restore -d postgres://covalence:covalence@localhost:5434/covalence \
    covalence-20260301-120000.dump
```

The Docker volume `covalence-data` persists across container restarts. For production, mount it to durable storage or use a managed PostgreSQL service.

**What's stored in the DB (all backed up with `pg_dump`):**
- All nodes (sources, articles, sessions) and their embeddings (`halfvec(1536)`)
- All edges in both `covalence.edges` (SQL) and Apache AGE graph
- Contentions, mutation queue, session data, and tree-index chunks

---

## 7. OpenClaw Integration

Covalence ships a first-party [OpenClaw](https://openclaw.ai) plugin that automates the recall/capture lifecycle for agents running inside OpenClaw.

### Install and configure

```bash
cd covalence/plugin
npm install && npm run build
```

Point OpenClaw at the built plugin. It registers as `memory-covalence`.

Key configuration options in your OpenClaw plugin config:

```json
{
  "serverUrl": "http://localhost:8430",
  "autoRecall": true,
  "autoCapture": true,
  "sessionIngestion": true,
  "autoCompileOnFlush": true,
  "inferenceEnabled": true,
  "inferenceModel": "github-copilot/gpt-4.1-mini"
}
```

| Option | Default | Effect |
|---|---|---|
| `autoRecall` | `true` | Injects relevant memories before each agent run |
| `autoCapture` | `true` | Extracts insights from conversation transcripts |
| `sessionIngestion` | `true` | Persists conversation transcripts as sources |
| `autoCompileOnFlush` | `true` | Triggers `process_queue` on session flush |
| `inferenceEnabled` | `true` | Proxies OpenAI `/chat/completions` + `/embeddings` |
| `inferenceModel` | — | Provider/model for compilation (e.g. `github-copilot/gpt-4.1-mini`) |

### MCP tool interface vs. direct HTTP API

The OpenClaw plugin exposes Covalence tools via the **MCP (Model Context Protocol)** interface. When running inside OpenClaw, agents call tools like `memory_store`, `knowledge_search`, `source_ingest`, etc., without any HTTP calls — the plugin handles transport.

When running outside OpenClaw (standalone scripts, other frameworks), use the **Python client** or direct **HTTP REST** calls against `localhost:8430`.

| Context | How to use |
|---|---|
| Inside OpenClaw | MCP tools (auto-exposed by plugin) |
| Python scripts | `covalence-client` (this guide) |
| Other languages | Direct HTTP REST to `localhost:8430` |
| Custom integrations | HTTP REST (OpenAPI spec planned) |

### The dual-write pattern

For agents that also write to files or a VCS (like Claude Code), consider writing to both simultaneously:

```python
import json
from pathlib import Path
from covalence import CovalenceClient

client = CovalenceClient()

def save_finding(title: str, content: str, domain: list[str], filepath: Path) -> None:
    """Write to disk AND ingest into Covalence."""
    # 1. Write to disk (version-control friendly, human-readable)
    filepath.write_text(content)

    # 2. Ingest into Covalence (semantically searchable, linked, confidence-scored)
    client.ingest_source(
        content,
        source_type="document",
        title=title,
        metadata={"filepath": str(filepath)},
        reliability=0.85,
    )
    print(f"Saved to disk: {filepath}")
    print(f"Ingested to Covalence: searchable and linked")
```

Files give you `git diff` and human-readable audit. Covalence gives you semantic search, confidence scoring, and provenance. They complement each other well.

---

## 8. Best Practices

### Right-size your sources

Don't ingest an entire 50-page document as one source. Chunk it at logical boundaries — each chunk becomes a source that can be individually embedded and linked:

```python
def ingest_chunked(content: str, title: str, chunk_size: int = 1500) -> list[str]:
    """Ingest a long document in ~1500-char chunks."""
    paragraphs = content.split("\n\n")
    chunks, current, current_len = [], [], 0

    for para in paragraphs:
        if current_len + len(para) > chunk_size and current:
            chunk_text = "\n\n".join(current)
            source = client.ingest_source(
                chunk_text,
                source_type="document",
                title=f"{title} (chunk {len(chunks) + 1})",
            )
            chunks.append(source.id)
            current, current_len = [], 0
        current.append(para)
        current_len += len(para)

    if current:
        source = client.ingest_source(
            "\n\n".join(current),
            source_type="document",
            title=f"{title} (chunk {len(chunks) + 1})",
        )
        chunks.append(source.id)

    return chunks
```

Articles should be 200–4000 tokens. Use `split_article` if a compiled article runs long:

```python
article = client.get_article(article_id)
word_count = len(article.content.split())
if word_count > 3000:  # roughly 4000 tokens
    result = client.split_article(article.id)
    print(f"Split into {result.part_a.id} and {result.part_b.id}")
```

### Tag source types correctly

The `source_type` field directly sets the initial reliability score. Misclassifying sources skews confidence for all downstream articles. Common mistakes:

| Wrong | Right | Why |
|---|---|---|
| `document` for a tweet | `web` (0.60) | Social media is not a vetted document |
| `document` for agent inference | `observation` (0.40) | Agent-generated content is inherently less certain |
| `web` for official documentation | `document` + `reliability=0.90` | Official docs deserve higher trust |
| `conversation` for structured tool output | `tool_output` (0.70) | Tools return structured, verifiable output |

### Use `domain_path` for organization

A flat KB with hundreds of articles is hard to maintain. Use `domain_path` as a hierarchical namespace from the start:

```python
# Project-scoped hierarchy
client.create_article(..., domain_path=["project-alpha", "architecture", "database"])
client.create_article(..., domain_path=["project-alpha", "design", "api"])

# Topic-based hierarchy
client.create_article(..., domain_path=["python", "concurrency", "asyncio"])
client.create_article(..., domain_path=["rust", "memory", "ownership"])
```

Domain path shows up on every search result, making it easy to see what domain a hit belongs to and filter results in your application logic.

### Pin your reference articles

Articles with high `usage_score` survive organic eviction naturally. But critical low-traffic articles — architecture decisions, key reference documents, project constants — should be pinned explicitly:

```python
# Pin an important article — exempt from capacity eviction
client.update_article(article_id, pinned=True)
```

### Review contentions regularly

A growing backlog of unresolved contentions means your KB has contradictions that are degrading answer quality. Build a review step into your agent's periodic maintenance loop:

```python
def weekly_maintenance() -> None:
    # 1. Full maintenance pass
    client.run_maintenance(
        recompute_scores=True,
        process_queue=True,
        evict_if_over_capacity=True,
    )

    # 2. Surface open contentions
    open_contentions = client.list_contentions(status="detected")
    print(f"Open contentions to review: {len(open_contentions.data)}")
    for c in open_contentions.data:
        print(f"  [{c.severity}] {c.description}")

    # 3. Ensure all nodes have embeddings
    stats = client.get_admin_stats()
    if stats.embeddings.nodes_without > 0:
        client.embed_all()
        print(f"Queued embeddings for {stats.embeddings.nodes_without} nodes")
```

### Compile sources into articles — don't search raw sources

Raw sources are noisy and often redundant. Once you have 3+ sources on a topic, compile them into a curated article. Search over articles for cleaner, higher-quality results:

```python
# Find sources on a topic
results = client.search("Python asyncio event loop", node_types=["source"], limit=10)
source_ids = [r.node_id for r in results.data]

if len(source_ids) >= 3:
    job = client.compile_article(
        source_ids,
        title_hint="Python asyncio event loop internals",
    )
    print(f"Compiling {len(source_ids)} sources → article {job.job_id}")

# After compilation, search articles
results = client.search("asyncio event loop", node_types=["article"], limit=5)
```

### Use sessions for task scoping

Create a session at the start of each major task. This gives you a `CAPTURED_IN` graph trail — you can later ask "what did the agent learn during task X?" and walk the session neighborhood:

```python
session = client.create_session(
    label="research-python-async-2026-03",
    metadata={"agent": "researcher-v2", "task": "async-docs"},
)

# Ingest with session context
for content, title in docs_to_ingest:
    client.ingest_source(content, title=title, session_id=session.id)

# Close when done
client.close_session(session.id)

# Later: see everything captured in that session
neighborhood = client.get_neighborhood(
    session.id,
    depth=1,
    labels="CAPTURED_IN",
)
print(f"Session captured {neighborhood.meta.count} sources")
```

---

## Quick Reference

### Key client methods

```python
# Sources
source  = client.ingest_source(content, source_type, title, reliability, metadata, session_id)
sources = client.list_sources(limit, source_type)
source  = client.get_source(source_id)
         client.delete_source(source_id)

# Articles
article = client.create_article(content, title, domain_path, source_ids, epistemic_type)
job     = client.compile_article(source_ids, title_hint)
article = client.update_article(article_id, content, title, domain_path, pinned)
         client.archive_article(article_id)
parts   = client.split_article(article_id)
merged  = client.merge_articles(id_a, id_b)
prov    = client.get_provenance(article_id, max_depth)
trace   = client.trace_claim(article_id, claim_text)

# Search
results = client.search(query, node_types, intent, limit, weights, session_id)

# Memory
mem     = client.store_memory(content, tags, importance, context, supersedes_id)
results = client.recall_memories(query, tags, min_confidence, limit)
status  = client.get_memory_status()
         client.forget_memory(memory_id, reason)

# Graph
edge      = client.create_edge(from_node_id, to_node_id, label, confidence, notes)
edges     = client.list_node_edges(node_id, direction, labels)
neighbors = client.get_neighborhood(node_id, depth, direction, labels, limit)

# Contentions
contentions = client.list_contentions(node_id, status)
resolved    = client.resolve_contention(contention_id, resolution, rationale)

# Sessions
session = client.create_session(label, metadata)
         client.close_session(session_id)

# Admin
stats  = client.get_admin_stats()
result = client.run_maintenance(recompute_scores, process_queue, evict_if_over_capacity)
queue  = client.list_queue(status)
       client.retry_queue_entry(entry_id)
       client.embed_all()
       client.tree_index_all(overlap, force, min_chars)
```

### Reliability score cheat sheet

| Source | `source_type` | Reliability |
|---|---|---|
| Official documentation | `document` | `0.90–0.95` (override) |
| Peer-reviewed paper | `document` | `0.90` (override) |
| Internal team doc | `document` | `0.80` (default) |
| Source code / config | `code` | `0.80` (default) |
| Tool / API output | `tool_output` | `0.70` (default) |
| Human explicit statement | `user_input` | `0.75` (default) |
| News article | `web` | `0.55–0.65` (override) |
| Blog post | `web` | `0.40–0.50` (override) |
| Agent observation | `observation` | `0.40` (default) |
| Uncertain inference | `observation` | `0.20–0.30` (override) |

---

## Further Reading

- **[README](../README.md)** — Architecture overview and Docker setup
- **[API Reference](./api-reference.md)** — All 38 endpoints with full request/response shapes
- **[covalence-py README](https://github.com/ourochronos/covalence-py)** — Python client details and full method reference
- **Examples in covalence-py:**
  - `examples/basic_usage.py` — Source ingest, article creation, compile, search
  - `examples/agent_memory.py` — Store, recall, supersede, forget memories
  - `examples/epistemic_tracking.py` — Contentions, provenance, claim tracing

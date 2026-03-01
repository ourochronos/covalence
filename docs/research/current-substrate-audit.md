# Current Substrate Audit: Valence v2

**Audit Date:** 2026-02-28  
**Auditor:** Systems Audit Agent  
**Purpose:** Understand what Covalence is replacing — schema, API surface, strengths, weaknesses, and migration requirements.  
**Source:** ~/projects/valence (Python MCP server), live substrate stats

---

## 1. System Overview

Valence v2 is a **Python-based knowledge substrate** exposed to agents via an MCP (Model Context Protocol) server. It runs against a **PostgreSQL 16+ database** with pgvector for embeddings and pg_trgm for full-text search. The system manages two primary entities — **sources** (raw, immutable evidence) and **articles** (compiled, summarized knowledge units) — with a rich set of provenance, contention, and memory management tools.

### Stack
- **Language:** Python 3.x (Starlette for REST, MCP SDK for tool surface)
- **Database:** PostgreSQL 16+
- **Extensions:** `uuid-ossp`, `vector` (pgvector, 1536 dims), `pg_trgm`
- **Embeddings:** OpenAI `text-embedding-3-small` (1536 dimensions)
- **Search:** Hybrid — HNSW vector index + GIN full-text (RRF fusion)
- **LLM Backends:** OpenAI-compatible, Cerebras, Gemini CLI, Ollama (pluggable)

### Current Live Stats (2026-02-28)
| Metric | Value |
|--------|-------|
| Total articles | 289 |
| Active articles | 126 |
| Total sources | 264 |
| Articles with embeddings | 270 |
| Unique domain paths | 1 |
| Unresolved contentions | 26 |

The high ratio of total→active articles (289 total, 126 active) and 26 unresolved contentions already signal accumulation and drift problems at modest scale.

---

## 2. Database Schema

### 2.1 Core Tables

#### `sources` — Immutable Evidence Store
```sql
CREATE TABLE sources (
    id              uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    type            text NOT NULL,          -- document|conversation|web|code|observation|tool_output|user_input
    title           text,
    url             text,
    content         text,
    fingerprint     text,                   -- SHA-256 dedup key (unique where not null)
    reliability     numeric(3,2) DEFAULT 0.5,
    content_hash    text,
    session_id      uuid,                   -- loosely linked to sessions (no FK!)
    metadata        jsonb DEFAULT '{}',
    created_at      timestamptz DEFAULT now(),
    embedding       vector(1536),
    content_tsv     tsvector GENERATED ALWAYS AS (to_tsvector('english', COALESCE(content,''))) STORED,
    redacted_at     timestamptz,            -- privacy redaction timestamp
    redacted_by     text,                   -- redaction reason
    supersedes_id   uuid REFERENCES sources(id) ON DELETE SET NULL,
    pipeline_status text DEFAULT 'pending'  -- pending|indexed|complete|failed (migration 007)
);
```

**Key indexes:** HNSW on `embedding`, GIN on `content_tsv`, unique on `fingerprint` (where not null), btree on `content_hash`, btree on `supersedes_id`.

**Notable:** Sources are append-only by design. The `supersedes_id` on sources is a **flat single-parent pointer** — it cannot represent multi-parent provenance or branching correction histories. Redaction NULLs out content and embedding in-place.

#### `articles` — Compiled Knowledge Units
```sql
CREATE TABLE articles (
    id                      uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    content                 text NOT NULL,
    title                   text,
    author_type             text DEFAULT 'system',   -- system|operator|agent
    pinned                  boolean DEFAULT false,
    epistemic_type          text DEFAULT 'semantic', -- episodic|semantic|procedural (migration 006)
    size_tokens             integer,
    compiled_at             timestamptz,
    usage_score             numeric(8,4) DEFAULT 0,
    confidence              jsonb DEFAULT '{"overall": 0.7}',
    domain_path             text[] DEFAULT '{}',
    valid_from              timestamptz,
    valid_until             timestamptz,
    created_at              timestamptz DEFAULT now(),
    modified_at             timestamptz DEFAULT now(),
    source_id               uuid REFERENCES sources(id) ON DELETE SET NULL,  -- LEGACY single-source link
    extraction_method       text,
    extraction_metadata     jsonb,
    supersedes_id           uuid REFERENCES articles(id) ON DELETE SET NULL,
    superseded_by_id        uuid REFERENCES articles(id) ON DELETE SET NULL,
    holder_id               uuid,           -- UNUSED/reserved
    version                 integer DEFAULT 1,
    content_hash            char(64),
    status                  text DEFAULT 'active',   -- active|superseded|disputed|archived
    archived_at             timestamptz,
    corroboration_count     integer DEFAULT 0,
    corroborating_sources   jsonb DEFAULT '[]',      -- DENORMALIZED, stale-able
    -- Decomposed confidence components (DUAL REPRESENTATION with confidence JSONB)
    confidence_source       real DEFAULT 0.5,
    confidence_method       real DEFAULT 0.5,
    confidence_consistency  real DEFAULT 1.0,
    confidence_freshness    real DEFAULT 1.0,
    confidence_corroboration real DEFAULT 0.1,
    confidence_applicability real DEFAULT 0.8,
    embedding               vector(1536),
    content_tsv             tsvector GENERATED ALWAYS AS (to_tsvector('english', content)) STORED,
    degraded                boolean DEFAULT false
);
```

**Key indexes:** HNSW on `embedding`, GIN on `content_tsv`, btree on `status`, GIN on `domain_path`, btree on `source_id`, `content_hash`, `archived_at`, `modified_at`.

**Notable issues:**
- `supersedes_id` + `superseded_by_id` form a **doubly-linked list** — not a graph. Cannot represent splits (1→2), merges (2→1), or cross-links.
- `source_id` is a **legacy single-source column** that predates `article_sources`. Creates dual-tracking confusion.
- `corroborating_sources` is a **denormalized JSONB array** that can drift from `article_sources`.
- `holder_id` exists but is **unused** — reserved for future ownership semantics.
- `confidence` is both a **JSONB blob** and **decomposed float columns** — two representations of the same truth that can drift.

#### `article_sources` — Provenance Links (Many-to-Many)
```sql
CREATE TABLE article_sources (
    id           uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    article_id   uuid NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    source_id    uuid NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    relationship text NOT NULL,  -- originates|confirms|supersedes|contradicts|contends
    added_at     timestamptz DEFAULT now(),
    notes        text,
    UNIQUE (article_id, source_id, relationship)
);
```

**This is the real graph edge table**, but it only connects articles→sources, not articles→articles or sources→sources. The edge vocabulary is fixed in a CHECK constraint — adding a new relationship type requires a schema migration.

#### `contentions` — Flagged Disagreements
```sql
CREATE TABLE contentions (
    id                  uuid PRIMARY KEY,
    article_id          uuid NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    related_article_id  uuid REFERENCES articles(id) ON DELETE CASCADE,
    type                text DEFAULT 'contradiction',  -- contradiction|temporal_conflict|scope_conflict|partial_overlap
    description         text,
    severity            text DEFAULT 'medium',  -- low|medium|high|critical
    status              text DEFAULT 'detected', -- detected|investigating|resolved|accepted
    resolution          text,
    resolved_at         timestamptz,
    detected_at         timestamptz DEFAULT now(),
    materiality         numeric(3,2) DEFAULT 0.5,
    opt_out_federation  boolean DEFAULT false,   -- ABANDONED federation residue
    share_policy        jsonb,                   -- ABANDONED federation residue
    extraction_metadata jsonb,
    degraded            boolean DEFAULT false,
    CONSTRAINT contentions_different_articles CHECK (article_id <> related_article_id)
);
```

**Notable:** Contentions are **article-to-article only** — no source-to-source disagreement model. `opt_out_federation` / `share_policy` are dead fields from an abandoned P2P federation plan.

#### `article_mutations` — Audit Trail
```sql
CREATE TABLE article_mutations (
    id                uuid PRIMARY KEY,
    mutation_type     text NOT NULL,  -- created|updated|split|merged|archived
    article_id        uuid NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    related_article_id uuid REFERENCES articles(id) ON DELETE SET NULL,
    trigger_source_id uuid REFERENCES sources(id) ON DELETE SET NULL,
    summary           text,
    created_at        timestamptz DEFAULT now()
);
```

#### `mutation_queue` — Async Background Operations
```sql
CREATE TABLE mutation_queue (
    id          uuid PRIMARY KEY,
    operation   text NOT NULL,  -- split|merge_candidate|recompile|decay_check|recompile_degraded|source_pipeline
    article_id  uuid REFERENCES articles(id) ON DELETE CASCADE,  -- nullable after migration 008
    source_id   uuid,           -- no FK (sources can be hard-deleted)
    priority    integer DEFAULT 5,
    payload     jsonb DEFAULT '{}',
    status      text DEFAULT 'pending',  -- pending|processing|completed|failed
    created_at  timestamptz DEFAULT now(),
    processed_at timestamptz
    -- CONSTRAINT: article_id IS NOT NULL OR source_id IS NOT NULL
);
```

**Note migration 008:** `article_id` was originally NOT NULL with FK. Making it nullable to support source-pipeline operations was a painful retrofit — evidence of article-first design with sources bolted on later.

#### `compilation_queue` — Offline Compilation Buffer
```sql
CREATE TABLE compilation_queue (
    id          uuid PRIMARY KEY,
    source_ids  text[] NOT NULL,  -- text[], not uuid[]! No FK enforcement.
    title_hint  text,
    queued_at   timestamptz DEFAULT now(),
    attempts    integer DEFAULT 0,
    last_attempt timestamptz,
    status      text DEFAULT 'pending'  -- pending|processing|failed (no 'completed'!)
);
```

**Issues:** `source_ids` is `text[]` not `uuid[]`. No FK enforcement — dead references possible after `admin_forget`. No 'completed' status — processed entries must be deleted, not marked done.

#### `sessions` + `session_messages` — Conversation Buffering (Migration 004)
```sql
CREATE TABLE sessions (
    session_id          text PRIMARY KEY,  -- platform-provided string, NOT uuid
    platform            text NOT NULL,
    channel             text,
    participants        text[] DEFAULT '{}',
    started_at          timestamptz DEFAULT now(),
    last_activity_at    timestamptz DEFAULT now(),
    ended_at            timestamptz,
    status              text DEFAULT 'active',
    metadata            jsonb DEFAULT '{}',
    parent_session_id   text REFERENCES sessions(session_id),
    subagent_label      text,
    subagent_model      text,
    subagent_task       text,
    current_chunk_index integer DEFAULT 0
);

CREATE TABLE session_messages (
    id          bigserial PRIMARY KEY,
    session_id  text NOT NULL REFERENCES sessions(session_id),
    chunk_index integer DEFAULT 0,
    timestamp   timestamptz DEFAULT now(),
    speaker     text NOT NULL,
    role        text NOT NULL CHECK (role IN ('user','assistant','system','tool')),
    content     text NOT NULL,
    metadata    jsonb DEFAULT '{}',
    flushed_at  timestamptz
);
```

**Issue:** `session_id` is `text` PK (not uuid), meaning session identity is platform-owned. `sources.session_id` is a `uuid` column with **no FK** to this text PK — the session→source link is unenforceable.

#### Supporting Tables
- **`entities`** / **`article_entities`**: Entity extraction tables — **not wired to any MCP tools**. Dead weight.
- **`system_config`**: Key-value store for runtime configuration (embedding model, bounded memory limits).
- **`usage_traces`**: Every `knowledge_search` hit records here. Powers `usage_score` and freshness-aware ranking.

### 2.2 Views
| View | Purpose |
|------|---------|
| `articles_current` | `status='active' AND superseded_by_id IS NULL` |
| `articles_with_sources` | Active articles with source counts and relationship type arrays |
| `article_usage` | Usage traces joined to articles |

### 2.3 Migration History (8 migrations from schema.sql baseline)
| # | Description | Key Change |
|---|-------------|-----------|
| 001 | Initial (valence-v2 engine) | Triples-based graph (NOT the current schema) |
| 002 | Source sections | Chunked source content |
| 004 | Session ingestion | `sessions` + `session_messages` tables |
| 005 | Compilation queue | `compilation_queue` for offline operation |
| 006 | Epistemic types | `epistemic_type` enum on articles |
| 007 | Pipeline status | `pipeline_status` on sources; `source_pipeline` in mutation_queue |
| 008 | Mutation queue source_id | Made `article_id` nullable; added `source_id` to mutation_queue |

**Note:** Migration 001 shows valence-v2 *engine* started as a triples-based graph (`nodes`, `triples`, `source_triples`). The current Python server (in `~/projects/valence`) is a completely different codebase — articles/sources, not triples. The "v2" label applies to two architecturally different systems.

---

## 3. MCP Tool Surface (35 Tools Total)

The agent-facing tool surface is defined in `src/valence/mcp/tools.py` as `SUBSTRATE_TOOLS`.

### 3.1 Source Tools (C1)
| Tool | Key Parameters | Description |
|------|---------------|-------------|
| `source_ingest` | content, source_type, title?, url?, metadata? | Ingest raw content; SHA-256 dedup; reliability by type |
| `source_get` | source_id | Retrieve source by UUID with full content |
| `source_search` | query, limit? | Full-text search (GIN/websearch_to_tsquery) |
| `source_list` | source_type?, limit? | List sources with type filter |

### 3.2 Knowledge Retrieval (C9)
| Tool | Key Parameters | Description |
|------|---------------|-------------|
| `knowledge_search` | query, limit?, include_sources?, session_id?, epistemic_type? | Hybrid RRF search; records usage_traces; queues ungrouped sources for compilation |

**Ranking formula:** `relevance × 0.5 + confidence × 0.35 + freshness × 0.15`

### 3.3 Article Tools (C2, C3)
| Tool | Key Parameters | Description |
|------|---------------|-------------|
| `article_get` | article_id, include_provenance? | Get article with optional provenance links |
| `article_create` | content, title?, source_ids?, author_type?, domain_path?, epistemic_type? | Manually create article |
| `article_compile` | source_ids, title_hint? | LLM-compile sources → article (300-800 tokens, target 550) |
| `article_update` | article_id, content, source_id?, epistemic_type? | Increment version, re-link source |
| `article_split` | article_id | Split oversized article into two |
| `article_merge` | article_id_a, article_id_b | Merge two articles; both originals archived |
| `article_search` | query, domain?, limit? | Article-only search |

### 3.4 Provenance (C5)
| Tool | Key Parameters | Description |
|------|---------------|-------------|
| `provenance_trace` | article_id, claim_text | TF-IDF claim→source attribution |
| `provenance_get` | article_id | All provenance links for an article |
| `provenance_link` | article_id, source_id, relationship | Manually add source→article link |

### 3.5 Contentions (C7)
| Tool | Key Parameters | Description |
|------|---------------|-------------|
| `contention_list` | article_id?, status? | List contentions |
| `contention_resolve` | contention_id, resolution, rationale | Resolve: supersede_a/b, accept_both, dismiss |
| `contention_detect` | threshold?, auto_record? | Semantic similarity scan for contradictions |

### 3.6 Admin (C10)
| Tool | Key Parameters | Description |
|------|---------------|-------------|
| `admin_forget` | target_type, target_id | Hard-delete source or article (irreversible) |
| `admin_stats` | — | Health stats: counts, contentions, capacity |
| `admin_maintenance` | recompute_scores?, process_queue?, evict_if_over_capacity?, evict_count? | Trigger background operations |

### 3.7 Memory Wrappers
| Tool | Key Parameters | Description |
|------|---------------|-------------|
| `memory_store` | content, context?, importance?, tags?, supersedes_id? | Store observation source with memory metadata |
| `memory_recall` | query, limit?, min_confidence?, tags? | Search memories (filtered by metadata.memory=true) |
| `memory_status` | — | Count memories, top tags, last timestamp |
| `memory_forget` | memory_id, reason? | Soft-delete (sets metadata.forgotten flag; not a real delete) |

### 3.8 Session Tools (Server-Internal)
| Tool | Description |
|------|-------------|
| `session_start` | Upsert session |
| `session_append` | Buffer messages (single or batch mode) |
| `session_flush` | Flush buffer → conversation source |
| `session_finalize` | Flush + mark completed + compile |
| `session_search` | Semantic search over conversation sources |
| `session_list` | List sessions by status/platform/since |
| `session_get` | Get session + messages |
| `session_compile` | Compile all session sources → article |
| `session_flush_stale` | Background: flush inactive sessions |

**Agent-visible subset** (as configured in OpenClaw): `source_ingest`, `source_get`, `source_search`, `knowledge_search`, `article_get`, `article_create`, `article_compile`, `article_update`, `article_split`, `article_merge`, `provenance_trace`, `contention_list`, `contention_resolve`, `admin_forget`, `admin_stats`, `admin_maintenance`, `memory_store`, `memory_recall`, `memory_status`, `memory_forget`.

---

## 4. Core Algorithms

### 4.1 Ranking Formula
```
final_score = relevance × 0.50 + confidence × 0.35 + freshness × 0.15
```
- **Relevance**: Normalized RRF score from hybrid search (vector cosine via HNSW + ts_rank via GIN, fused with RRF_K=60)
- **Confidence**: `confidence->>'overall'` from article JSONB
- **Freshness**: Exponential decay function, ~69-day half-life (decay_rate=0.01/day)
- **Novelty boost**: New articles get up to 1.5× multiplier, decaying linearly over 48 hours
- **Temporal presets**: `default`, `prefer_recent`, `prefer_stable` (weight shifts via query param)

### 4.2 Confidence Formula
```python
avg_reliability = mean(source.reliability for source in linked_sources)
source_bonus = min(0.15, ln(1 + n_sources - 1) * 0.1)  # when n > 1
overall = min(0.95, avg_reliability + source_bonus)
```
Source reliability by type: `document=0.8`, `code=0.8`, `web=0.6`, `conversation=0.5`, `observation=0.4`, `tool_output=0.7`, `user_input=0.75`. Max confidence capped at 0.95.

### 4.3 Epistemic Types
- **episodic**: Decays over time; session memories, transient observations
- **semantic**: Persists; compiled factual knowledge
- **procedural**: Pinned; workflow/how-to knowledge that doesn't decay

### 4.4 Organic Forgetting
When `active_articles > max_articles` (from `system_config.bounded_memory`), lowest `usage_score` non-pinned articles are archived (status='archived', not deleted). Archived articles remain searchable with a capped rank floor.

### 4.5 Query Intent Detection
Simple heuristic: prefix matches for "how to / steps to" → procedural; "what happened / when did" → episodic; date patterns → recent. Intent adjusts epistemic_type filter on retrieval.

---

## 5. Pain Points and Limitations

### 5.1 The Supersedes_ID Problem (Critical)
**The core architectural flaw.** Both `articles.supersedes_id` and `sources.supersedes_id` are single-parent pointers forming a **doubly-linked list** at best:

```
Article A  →supersedes_id→  Article B  →supersedes_id→  Article C
```

This cannot express:
- **Splits**: Article A splits into B + C. B and C both "come from" A — impossible with one pointer.
- **Merges**: Articles A + B merge into C. C has two parents — impossible.
- **Cross-topic links**: Article A partially supersedes Article B in a different domain — the relationship exists but can't be expressed.
- **Convergent evolution**: Two independently compiled articles about the same topic — should be merged, but the system can't detect this structurally.

The workaround (`article_sources` with `relationship='supersedes'`) is partial — it's not used by the retrieval layer and requires manual wiring.

### 5.2 Duplicate Article Accumulation
`articles_current` filters `superseded_by_id IS NULL`, but **topic-duplicate articles are not caught** — only version-chain duplicates. Two sessions compiling the same topic produce two active articles. With 289 total / 126 active articles and only 264 sources, the 54% activity rate shows significant accumulation even at small scale. Merge logic exists but is never triggered automatically.

### 5.3 Article Identity is Source-Anchored, Not Topic-Anchored
Articles are created from source bundles. There is no concept of a persistent "topic node" that articles refer to. If the same topic appears in 5 sessions, 5 articles accumulate independently. The system cannot say "these 5 articles are all about X — there should be one canonical article on X."

### 5.4 Fixed Edge Vocabulary (Schema Migration Required for Extension)
`article_sources.relationship` is constrained to `{originates, confirms, supersedes, contradicts, contends}` by a CHECK constraint. Adding a new relationship type (e.g., `extends`, `partially_supersedes`, `derived_from`) requires a schema migration and code deployment. The vocabulary cannot evolve organically.

### 5.5 Denormalized Confidence State (Three Sources of Truth)
Confidence is stored as:
1. `confidence` JSONB blob (`{"overall": 0.74}`)
2. Six decomposed float columns (`confidence_source`, `confidence_method`, etc.)
3. `corroborating_sources` JSONB array (denormalized subset of `article_sources`)

These three can drift from each other. There is no constraint enforcing consistency between the JSONB overall score and the decomposed components.

### 5.6 The `compilation_queue` Is Broken by Design
`compilation_queue.source_ids` is `text[]` with no FK enforcement. When `admin_forget` hard-deletes a source, stale UUIDs remain in the queue. Processing produces degraded articles from deleted sources. The status enum has no 'completed' state — entries must be physically deleted on success.

### 5.7 Session / Source Coupling is Unenforceable
`sessions.session_id` is a `text` PK; `sources.session_id` is a `uuid` column. These types don't match and there is no FK between them. Sessions don't structurally own their sources — the link is convention-based and can silently break.

### 5.8 The `entities` Subsystem is Dead Weight
Tables `entities` and `article_entities` were built for entity extraction but were **never wired to any MCP tools or the retrieval layer**. They consume schema real estate and migration complexity without delivering value.

### 5.9 False Positive Contention Flood
26 unresolved contentions at only 126 active articles. Contention detection is cosine-similarity based (threshold 0.85), which flags near-duplicate articles (a symptom of the accumulation problem in §5.2) as "contradictions." Topic-duplicate articles look semantically similar → false contradiction. Agents lack automated resolution — every contention requires manual tool calls to dismiss. This doesn't scale.

### 5.10 The `mutation_queue` FK Retrofit (Schema Debt)
Migration 008 made `article_id` nullable in `mutation_queue` to support source-pipeline operations. The original schema was article-centric, and source-pipeline tasks were bolted on. The resulting `CHECK (article_id IS NOT NULL OR source_id IS NOT NULL)` is awkward and signals that the queue was designed for a different data model.

### 5.11 No Graph Traversal
There is no way to ask "what does Article A supersede transitively?" or "show the neighborhood of sources that influenced this article cluster." The doubly-linked supersession chain can be walked in application code, but there are no DB-side graph traversal capabilities (no WITH RECURSIVE optimization for known chains, no neighborhood indexing). Path-finding and cluster detection require loading the full graph into memory.

### 5.12 PostgreSQL External Dependency
The system requires a running PostgreSQL instance with three extensions. This is meaningful infrastructure overhead for an embedded knowledge store on the 16GB M4 Mac Mini target. PostgreSQL competes with the inference workload for memory and CPU.

### 5.13 The "v2" Naming Confusion
`~/projects/valence-v2` contains a completely different codebase — a Rust/triples-based graph that predates the current Python server. The current production system (`~/projects/valence`) is architecturally unrelated to valence-v2/engine. The naming creates confusion about lineage.

---

## 6. What Works Well (Must Preserve in Covalence)

### 6.1 ✅ The Source→Article Compilation Pipeline
Immutable sources compiled into mutable articles via LLM is sound. Sources provide the audit trail; articles provide retrieval efficiency. The `article_compile` → LLM summarization → `article_sources` provenance link pipeline is well-understood and effective. **Preserve this pattern.**

### 6.2 ✅ Multi-Dimensional Confidence Scoring
`avg_reliability + source_bonus` formula is simple and effective. Reliability priors by source type are well-calibrated. The decomposed components (`confidence_source`, `confidence_method`, etc.) are the right *idea* even if the implementation has consistency issues. **Preserve; consolidate into one representation.**

### 6.3 ✅ Hybrid Search with RRF Fusion
Vector search (HNSW cosine) + full-text search (GIN tsquery) fused via Reciprocal Rank Fusion is the right retrieval architecture. The 0.5/0.35/0.15 weights (relevance/confidence/freshness) are empirically reasonable. **Preserve; migrate to an embedded solution (tantivy + hnsw-rs or equivalent).**

### 6.4 ✅ Usage Traces for Organic Self-Organisation
Recording every retrieval in `usage_traces` and computing `usage_score` for organic forgetting is a genuine strength. The system improves its own retrieval quality through use patterns. **Preserve with better decay modeling.**

### 6.5 ✅ Epistemic Types with Decay Policies
`episodic` / `semantic` / `procedural` with different decay behaviors is architecturally correct. **Preserve and extend.**

### 6.6 ✅ The Memory Wrappers
`memory_store` / `memory_recall` / `memory_forget` as thin agent-friendly wrappers over source ingestion are genuinely useful. They hide substrate complexity. **Preserve as a first-class abstraction in Covalence's tool surface.**

### 6.7 ✅ Contention Detection Concept
Automatically detecting when new sources contradict existing articles and flagging for resolution is the right behavior. The cosine similarity implementation is crude but the concept is sound. **Preserve with better semantic diffing (embedding delta + NLI).**

### 6.8 ✅ Bounded Memory / Organic Forgetting
The `bounded_memory` configuration and archive-lowest strategy is the right approach to capacity management. Articles are never truly lost — archived items remain searchable with a rank floor. **Preserve.**

### 6.9 ✅ Provenance Tracing
`provenance_trace` using TF-IDF overlap to attribute claims to sources is lightweight and effective for fact-checking. **Preserve the concept; upgrade to graph traversal in Covalence.**

### 6.10 ✅ Fingerprint-Based Source Deduplication
SHA-256 fingerprinting prevents duplicate source ingestion — essential for idempotent pipelines. **Preserve.**

### 6.11 ✅ The 35-Tool API Surface as Agent Contract
The tool names and signatures are what agents (and prompt engineering) depend on. `knowledge_search`, `source_ingest`, `article_compile`, etc. are known quantities. **Preserve the external API contract; change only the implementation underneath.**

---

## 7. Migration Requirements for Covalence

### 7.1 Data to Migrate
| Entity | Est. Count | Priority | Notes |
|--------|-----------|----------|-------|
| Sources | 264 | Critical | Content, type, reliability, metadata, fingerprint |
| Active articles | 126 | Critical | Content, title, confidence, domain_path, epistemic_type, version |
| All articles | 289 | High | Include archived/superseded for provenance chain reconstruction |
| article_sources links | ~500+ | Critical | The provenance graph — do not lose |
| usage_traces | unknown | Medium | For score continuity; can reset if needed |
| Contentions | 26 open | Low | Migrate open items; resolved can be dropped |
| Memories | ~40 sources | High | Sources with metadata.memory=true |
| Sessions | unknown | Low | Active sessions only; historical can be archived |

### 7.2 Schema Concept Translations
| Valence Concept | Covalence Target |
|-----------------|-----------------|
| `articles.supersedes_id` (linked list) | Graph edge: typed `supersedes` relationship |
| `article_sources.relationship` (fixed 5 types) | Graph edge with extensible type vocabulary (no schema migration needed) |
| `confidence` JSONB + 6 float columns | Single canonical confidence struct on node |
| `epistemic_type` enum | Node property with associated decay policy |
| `usage_traces` table | Retrieval event edges with weight; decay on staleness |
| `contentions` table | Typed edge: `contradicts` between article nodes |
| `mutation_queue` | Async job queue (embedded; no FK cross-contamination) |
| `sessions` / `session_messages` | Session nodes with message child nodes |
| `entities` / `article_entities` | Not migrated — rebuild properly if needed |

### 7.3 Behavioral Invariants to Preserve
1. Sources are append-only (no mutation, only supersession via new source + link)
2. Fingerprint deduplication on ingest (SHA-256)
3. Retrieval ranking: relevance + confidence + freshness (weights may tune)
4. Usage recording on every retrieval hit
5. Organic forgetting: lowest-score articles evicted when over capacity, but not deleted
6. Memory soft-delete (mark forgotten, preserve audit trail)
7. Compilation pipeline: sources → LLM → article with provenance links
8. Epistemic type decay: episodic articles fade faster than semantic

### 7.4 What NOT to Migrate
- `entities` / `article_entities` (unused)
- `opt_out_federation` / `share_policy` fields (abandoned)
- `holder_id` (unused)
- `compilation_queue` (replace with proper async mechanism)
- Legacy `source_id` on articles (replaced by `article_sources`)
- `valence-v2/engine` triples-based schema (different system entirely)

### 7.5 Migration Script Requirements
1. Walk all `supersedes_id` chains in both `sources` and `articles`; convert to typed graph edges
2. Export `article_sources` as edge records with relationship types
3. Recompute confidence from source reliabilities (don't trust the denormalized floats)
4. Export usage_traces as retrieval event records for score seeding
5. Identify sources with `metadata.memory=true` and import as memory nodes
6. Validate: every active article's provenance chain is intact in Covalence

---

## 8. Summary Assessment

**Valence v2 is a well-intentioned relational solution to a graph problem.** The core pipeline (ingest → compile → retrieve → decay) is correct and should be preserved in Covalence. The ranking formula, confidence model, and usage-trace-driven self-organisation are genuine innovations that work well.

The critical failure is the **supersedes_id model**: a doubly-linked list masquerading as a knowledge graph. Every other pain point traces back to this root cause:
- Duplicate accumulation → because topic identity isn't graph-anchored
- False contention detection → because duplicates look like contradictions
- Inability to represent splits/merges → because one pointer can't have two parents
- Schema migration tax → because edge vocabulary is baked into CHECK constraints

**Covalence's mission:** Replace the relational scaffolding with a proper typed-edge graph while preserving the compilation pipeline, ranking formula, confidence model, usage traces, and the 35-tool API surface. The agent contract should not break; only the substrate underneath changes.

**Estimated migration complexity:** Medium-low. Data volume is small (289 articles, 264 sources). The hard part is supersession chain reconstruction — walking all existing `supersedes_id` pointers and converting them to typed graph edges in Covalence's storage model. A one-time migration script should handle this in under an hour of runtime.

---

*End of audit. See Covalence Project Genesis document for design decisions motivated by these findings.*

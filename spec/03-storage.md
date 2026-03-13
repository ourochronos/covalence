# 03 — Storage

**Status:** Implemented

## Overview

PostgreSQL 17 with pgvector is the sole persistent store. All data — graph, chunks, embeddings, provenance — lives in PG. This avoids the operational complexity of multiple databases and leverages PG's transactional guarantees.

## Extensions

| Extension | Purpose |
|-----------|---------|
| `pgvector` | HNSW/IVFFlat vector indexes, distance operators |
| `pg_trgm` | Trigram similarity for fuzzy text matching |
| `ltree` | Hierarchical label trees for structural hierarchy pre-filtering |

## Schema

### `sources`

```sql
CREATE TABLE sources (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_type TEXT NOT NULL,          -- 'document', 'web_page', 'conversation', 'api', 'manual', 'code'
    uri TEXT,
    title TEXT,
    author TEXT,
    created_date TIMESTAMPTZ,           -- original publication/creation date
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    content_hash BYTEA NOT NULL,        -- SHA-256 hash for dedup
    metadata JSONB NOT NULL DEFAULT '{}',
    raw_content TEXT,
    -- Trust model (Beta-binomial)
    trust_alpha FLOAT NOT NULL DEFAULT 4.0,   -- confirmations (prior depends on source_type)
    trust_beta FLOAT NOT NULL DEFAULT 1.0,    -- contradictions
    reliability_score FLOAT NOT NULL DEFAULT 0.8, -- cached Beta(α,β).mean()
    -- Federation & versioning
    clearance_level INT NOT NULL DEFAULT 0,    -- 0=local_strict, 1=federated_trusted, 2=federated_public (default private)
    update_class TEXT,                         -- 'append_only', 'versioned', 'correction', 'refactor', 'takedown'
    supersedes_id UUID REFERENCES sources(id),
    content_version INT NOT NULL DEFAULT 1,
    -- Document-level embedding (replaces doc-level chunk embedding, see migration 003)
    embedding halfvec(2048),
    UNIQUE(content_hash)
);

CREATE INDEX idx_sources_type ON sources(source_type);
CREATE INDEX idx_sources_ingested ON sources(ingested_at);
CREATE INDEX idx_sources_metadata ON sources USING GIN(metadata);
CREATE INDEX idx_sources_clearance ON sources(clearance_level);
CREATE INDEX idx_sources_supersedes ON sources(supersedes_id);
CREATE INDEX idx_sources_embedding ON sources
    USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);
```

### `chunks`

```sql
CREATE TABLE chunks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id UUID NOT NULL REFERENCES sources(id),
    parent_chunk_id UUID REFERENCES chunks(id),
    level TEXT NOT NULL,                 -- 'document', 'section', 'paragraph', 'sentence'
    ordinal INT NOT NULL,
    content TEXT NOT NULL,               -- Markdown-normalized text
    content_hash BYTEA NOT NULL,         -- SHA-256 of content (structural refactoring detection)
    embedding halfvec(2048),              -- dimension depends on model; halfvec for storage efficiency
    contextual_prefix TEXT,              -- LLM-generated context summary prepended before embedding
    token_count INT NOT NULL,
    structural_hierarchy ltree NOT NULL DEFAULT '',  -- ltree: 'doc_123.chapter_2.section_2_1'
    clearance_level INT NOT NULL DEFAULT 0,
    metadata JSONB NOT NULL DEFAULT '{}',  -- heading_text, page_number, speaker, contains_table, etc.
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_chunks_source ON chunks(source_id);
CREATE INDEX idx_chunks_parent ON chunks(parent_chunk_id);
CREATE INDEX idx_chunks_level ON chunks(level);
CREATE INDEX idx_chunks_hash ON chunks(content_hash);
CREATE INDEX idx_chunks_clearance ON chunks(clearance_level);
CREATE INDEX idx_chunks_hierarchy ON chunks USING GIST(structural_hierarchy);
CREATE INDEX idx_chunks_embedding ON chunks
    USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- === Model Calibrations ===

CREATE TABLE model_calibrations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    model_name TEXT NOT NULL UNIQUE,
    parent_child_p25 FLOAT NOT NULL,
    parent_child_p50 FLOAT NOT NULL,
    parent_child_p75 FLOAT NOT NULL,
    adjacent_mean FLOAT NOT NULL,
    adjacent_stddev FLOAT NOT NULL,
    sample_size INT NOT NULL,
    calibrated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

### `nodes`

```sql
CREATE TABLE nodes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    canonical_name TEXT NOT NULL,
    node_type TEXT NOT NULL,
    description TEXT,
    properties JSONB NOT NULL DEFAULT '{}',
    embedding halfvec(2048),
    -- Subjective Logic opinion tuple
    confidence_breakdown JSONB,  -- {"belief": 0.7, "disbelief": 0.1, "uncertainty": 0.2, "base_rate": 0.5}
    clearance_level INT NOT NULL DEFAULT 0,
    first_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
    mention_count INT NOT NULL DEFAULT 1
);

CREATE INDEX idx_nodes_type ON nodes(node_type);
CREATE INDEX idx_nodes_name ON nodes(canonical_name);
CREATE INDEX idx_nodes_name_trgm ON nodes USING GIN(canonical_name gin_trgm_ops);
CREATE INDEX idx_nodes_clearance ON nodes(clearance_level);
CREATE INDEX idx_nodes_embedding ON nodes
    USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);
CREATE INDEX idx_nodes_properties ON nodes USING GIN(properties);
```

**Code entity types** (`code_function`, `code_struct`, `code_trait`, `code_module`, `code_impl`, `code_type`, `code_test`) use the `properties` JSONB column for language-specific metadata:

```json
{
  "language": "rust",
  "file_path": "src/search/dimensions/vector.rs",
  "line_start": 45,
  "line_end": 120,
  "signature": "pub async fn search_vectors(&self, query: &[f32], limit: usize) -> Result<Vec<VectorHit>>",
  "visibility": "public",
  "semantic_summary": "Searches the vector index for chunks, statements, nodes, sections, sources, and articles similar to the query embedding, allocating budget across tables.",
  "ast_hash": "a1b2c3d4..."
}
```

The `ast_hash` (SHA-256 of the AST structure, ignoring whitespace and comments) enables incremental updates — only re-summarize structurally changed code.

### `edges`

```sql
CREATE TABLE edges (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_node_id UUID NOT NULL REFERENCES nodes(id),
    target_node_id UUID NOT NULL REFERENCES nodes(id),
    rel_type TEXT NOT NULL,
    causal_level TEXT CHECK (causal_level IN ('association', 'intervention', 'counterfactual')),
    properties JSONB NOT NULL DEFAULT '{}',  -- additional causal metadata, context
    weight FLOAT NOT NULL DEFAULT 1.0,
    confidence FLOAT NOT NULL DEFAULT 1.0,
    confidence_breakdown JSONB,              -- Subjective Logic opinion + per-source contributions
    clearance_level INT NOT NULL DEFAULT 0,
    is_synthetic BOOLEAN NOT NULL DEFAULT false,  -- true for ZK edges
    -- Bi-temporal model (Graphiti/Zep insight):
    -- valid_from/valid_until = when the fact is true in the world (assertion time)
    -- recorded_at = when we learned about it (transaction time)
    -- invalid_at = when a contradicting fact superseded this one (invalidation time)
    valid_from TIMESTAMPTZ,                  -- when the fact became true (extracted from text, null if unknown)
    valid_until TIMESTAMPTZ,                 -- when the fact stops being true (null if still current)
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT now(), -- when this edge was created in the system
    invalid_at TIMESTAMPTZ,                  -- when a contradicting edge invalidated this one (null if still valid)
    invalidated_by UUID REFERENCES edges(id), -- the edge that superseded this one
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_edges_source ON edges(source_node_id);
CREATE INDEX idx_edges_target ON edges(target_node_id);
CREATE INDEX idx_edges_type ON edges(rel_type);
-- Bi-temporal query support
CREATE INDEX idx_edges_valid_from ON edges(valid_from) WHERE valid_from IS NOT NULL;
CREATE INDEX idx_edges_invalid_at ON edges(invalid_at) WHERE invalid_at IS NOT NULL;
-- Active edges only (not invalidated)
CREATE INDEX idx_edges_active ON edges(source_node_id, target_node_id, rel_type)
    WHERE invalid_at IS NULL;
CREATE INDEX idx_edges_causal ON edges(causal_level) WHERE causal_level IS NOT NULL;
CREATE INDEX idx_edges_pair ON edges(source_node_id, target_node_id, rel_type);
CREATE INDEX idx_edges_clearance ON edges(clearance_level);
-- Partial index for edges with temporal bounds
CREATE INDEX idx_edges_temporal ON edges(valid_from, valid_until)
    WHERE valid_from IS NOT NULL;
```

**Extended edge type vocabulary** (beyond the epistemic edges in [07-epistemic-model](07-epistemic-model.md)):

| Category | Edge Types | Notes |
|----------|-----------|-------|
| **Code structure** | `CALLS`, `USES_TYPE`, `IMPLEMENTS`, `CONTAINS`, `DEPENDS_ON` | Extracted from AST during code ingestion |
| **Cross-domain bridge** | `IMPLEMENTS_INTENT` (code→spec), `PART_OF_COMPONENT` (code→component), `THEORETICAL_BASIS` (spec→research) | Connect the three knowledge domains |
| **Analysis** | `SEMANTIC_DRIFT`, `COVERAGE_GAP` | Generated by cross-domain analysis, not extraction |

### `articles`

```sql
CREATE TABLE articles (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    embedding halfvec(2048),
    confidence FLOAT NOT NULL DEFAULT 1.0,
    confidence_breakdown JSONB,
    domain_path TEXT[] NOT NULL DEFAULT '{}',
    version INT NOT NULL DEFAULT 1,
    content_hash BYTEA NOT NULL,
    source_node_ids UUID[] NOT NULL DEFAULT '{}',
    clearance_level INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_articles_domain ON articles USING GIN(domain_path);
CREATE INDEX idx_articles_clearance ON articles(clearance_level);
CREATE INDEX idx_articles_embedding ON articles
    USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);
```

### `statements`

```sql
CREATE TABLE statements (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id UUID NOT NULL REFERENCES sources(id),
    content TEXT NOT NULL,                -- Self-contained atomic claim (all pronouns resolved)
    content_hash BYTEA NOT NULL,          -- SHA-256 of content for dedup
    embedding halfvec(2048),
    byte_start INT,                       -- Offset into source raw_content
    byte_end INT,
    heading_path TEXT[],                   -- Document hierarchy path at extraction point
    section_id UUID REFERENCES sections(id), -- Assigned during clustering
    is_evicted BOOLEAN NOT NULL DEFAULT false, -- Soft-delete on re-extraction
    confidence FLOAT NOT NULL DEFAULT 1.0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_statements_source ON statements(source_id);
CREATE INDEX idx_statements_section ON statements(section_id);
CREATE INDEX idx_statements_hash ON statements(content_hash);
CREATE INDEX idx_statements_active ON statements(source_id)
    WHERE NOT is_evicted;
CREATE INDEX idx_statements_embedding ON statements
    USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);
```

### `sections`

```sql
CREATE TABLE sections (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id UUID NOT NULL REFERENCES sources(id),
    title TEXT NOT NULL,                  -- LLM-generated topic title
    body TEXT NOT NULL,                   -- Compiled summary of clustered statements
    embedding halfvec(2048),
    ordinal INT NOT NULL DEFAULT 0,       -- Order within source
    statement_count INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_sections_source ON sections(source_id);
CREATE INDEX idx_sections_embedding ON sections
    USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);
```

### `components` (Cross-Domain Bridge)

```sql
CREATE TABLE components (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    embedding halfvec(2048),
    source_id UUID REFERENCES sources(id),  -- Spec source that defines this component
    metadata JSONB NOT NULL DEFAULT '{}',   -- domain_path, spec_section, etc.
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_components_name ON components(name);
CREATE INDEX idx_components_source ON components(source_id);
CREATE INDEX idx_components_embedding ON components
    USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);
```

### `outbox_events` (Graph Sidecar Sync)

```sql
CREATE TABLE outbox_events (
    seq_id BIGSERIAL PRIMARY KEY,
    entity_type TEXT NOT NULL,   -- 'node' or 'edge'
    entity_id UUID NOT NULL,
    operation TEXT NOT NULL,     -- 'INSERT', 'UPDATE', 'DELETE'
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Trigger: write to outbox on node/edge changes, send empty NOTIFY as wake-up
CREATE OR REPLACE FUNCTION notify_outbox() RETURNS TRIGGER AS $$
BEGIN
    INSERT INTO outbox_events (entity_type, entity_id, operation, payload)
    VALUES (TG_TABLE_NAME, COALESCE(NEW.id, OLD.id), TG_OP, row_to_json(COALESCE(NEW, OLD)));
    NOTIFY graph_sync_ping;  -- empty payload, just a wake-up signal
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_nodes_outbox AFTER INSERT OR UPDATE OR DELETE ON nodes
    FOR EACH ROW EXECUTE FUNCTION notify_outbox();
CREATE TRIGGER trg_edges_outbox AFTER INSERT OR UPDATE OR DELETE ON edges
    FOR EACH ROW EXECUTE FUNCTION notify_outbox();
```

The Rust sidecar LISTENs for `graph_sync_ping` (or polls every 5s as fallback), then queries: `SELECT * FROM outbox_events WHERE seq_id > $last_seen ORDER BY seq_id ASC LIMIT 1000`.

**Outbox pruning:** A background job runs hourly, deleting events older than 24 hours:

```sql
DELETE FROM outbox_events WHERE created_at < now() - INTERVAL '24 hours';
```

**Sidecar recovery after extended downtime:** If the sidecar has been down for >24 hours and missed outbox events, it detects this on startup by comparing its `last_processed_seq_id` against the minimum `seq_id` in the outbox. If there's a gap (missing events already pruned), it automatically triggers a full reload from source tables rather than attempting incremental sync. The sidecar logs a warning: "outbox gap detected, performing full reload." This is safe because full reload is idempotent and the sidecar holds no state that PG doesn't also have.

### `audit_logs` (System Decision Tracking)

```sql
CREATE TABLE audit_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    action TEXT NOT NULL,         -- 'MERGE_NODES', 'SPLIT_NODE', 'PRUNE_BMR', 'TAKEDOWN', 'CLEARANCE_PROMOTE', etc.
    actor TEXT NOT NULL,          -- 'system:deep_consolidation', 'user:chris', 'api:ingestion'
    target_type TEXT,             -- 'node', 'edge', 'source', 'article'
    target_id UUID,
    payload JSONB NOT NULL DEFAULT '{}',  -- decision rationale, before/after state
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_audit_action ON audit_logs(action);
CREATE INDEX idx_audit_target ON audit_logs(target_type, target_id);
CREATE INDEX idx_audit_time ON audit_logs(created_at);
```

### `extractions`

```sql
CREATE TABLE extractions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    chunk_id UUID REFERENCES chunks(id),          -- Provenance: extracted from chunk (for code AST chunks)
    statement_id UUID REFERENCES statements(id),  -- Provenance: extracted from statement (statement pipeline)
    entity_type TEXT NOT NULL,           -- 'node', 'edge'
    entity_id UUID NOT NULL,
    extraction_method TEXT NOT NULL,
    confidence FLOAT NOT NULL DEFAULT 1.0,
    is_superseded BOOLEAN NOT NULL DEFAULT false,
    extracted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- At least one provenance source required
    CONSTRAINT extraction_provenance CHECK (chunk_id IS NOT NULL OR statement_id IS NOT NULL)
);

CREATE INDEX idx_extractions_chunk ON extractions(chunk_id) WHERE chunk_id IS NOT NULL;
CREATE INDEX idx_extractions_statement ON extractions(statement_id) WHERE statement_id IS NOT NULL;
CREATE INDEX idx_extractions_entity ON extractions(entity_type, entity_id);
CREATE INDEX idx_extractions_active ON extractions(entity_type, entity_id)
    WHERE NOT is_superseded;
```

### `node_aliases`

```sql
CREATE TABLE node_aliases (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    node_id UUID NOT NULL REFERENCES nodes(id),
    alias TEXT NOT NULL,
    alias_embedding halfvec(2048),
    source_chunk_id UUID REFERENCES chunks(id)
);

CREATE INDEX idx_aliases_node ON node_aliases(node_id);
CREATE INDEX idx_aliases_text ON node_aliases(alias);
CREATE INDEX idx_aliases_text_trgm ON node_aliases USING GIN(alias gin_trgm_ops);
CREATE INDEX idx_aliases_embedding ON node_aliases
    USING hnsw (alias_embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);
```

### Full-Text Search Support

```sql
-- Add tsvector columns for lexical search
ALTER TABLE chunks ADD COLUMN content_tsv tsvector
    GENERATED ALWAYS AS (to_tsvector('english', content)) STORED;
CREATE INDEX idx_chunks_tsv ON chunks USING GIN(content_tsv);

ALTER TABLE nodes ADD COLUMN name_tsv tsvector
    GENERATED ALWAYS AS (to_tsvector('english', canonical_name || ' ' || COALESCE(description, ''))) STORED;
CREATE INDEX idx_nodes_tsv ON nodes USING GIN(name_tsv);

ALTER TABLE articles ADD COLUMN body_tsv tsvector
    GENERATED ALWAYS AS (to_tsvector('english', title || ' ' || body)) STORED;
CREATE INDEX idx_articles_tsv ON articles USING GIN(body_tsv);

ALTER TABLE statements ADD COLUMN content_tsv tsvector
    GENERATED ALWAYS AS (to_tsvector('english', content)) STORED;
CREATE INDEX idx_statements_tsv ON statements USING GIN(content_tsv);

ALTER TABLE sections ADD COLUMN body_tsv tsvector
    GENERATED ALWAYS AS (to_tsvector('english', title || ' ' || body)) STORED;
CREATE INDEX idx_sections_tsv ON sections USING GIN(body_tsv);
```

## Vector Dimensions

**Decision: Single embedding model, two output dimensions.** Use one model (Voyage voyage-3-large) across all tables, but with Matryoshka truncation for node embeddings. Statement, article, and source embeddings use 2048 dimensions (`COVALENCE_EMBED_DIM`, default 2048). Node embeddings use 256 dimensions (`COVALENCE_NODE_EMBED_DIM`, default 256) via Matryoshka truncation to save storage on entity-level embeddings. Both are configurable via environment variables. The database column type (`halfvec(2048)`) accommodates the full dimension; shorter embeddings are stored with trailing zeros or in a truncated form depending on the pgvector version.

| Model | Dimensions | Notes |
|-------|-----------|-------|
| Voyage voyage-3-large | 2048 (full), 512-1024 (Matryoshka truncated) | **v1 default.** High-quality embeddings. Outperforms OpenAI. First 200M tokens free. |
| OpenAI text-embedding-3-small | 1536 | Fallback. Good quality, cheapest at scale ($0.02/M tokens). |
| BAAI/bge-base-en-v1.5 | 768 | Local inference only. |
| BAAI/bge-small-en-v1.5 | 384 | Fast, good for prototyping |
| Topology-derived (spectral/Node2Vec) | configurable | Zero API cost, graph-only (future) |

**Matryoshka multi-resolution:** voyage-3-large supports Matryoshka truncation — the first N dimensions of the embedding are a valid lower-dimensional embedding. This enables multi-resolution storage strategies:

- **Full 2048d** — sources (primary search targets, lowest cardinality)
- **1024d** — chunks, articles, statements, sections via `COVALENCE_EMBED_DIM_CHUNK` etc.
- **256d** — node embeddings via `COVALENCE_EMBED_DIM_NODE` (entity-level, lower storage cost)
- **512d–1024d** — optional fast-filter index for candidate generation before re-scoring at higher dimensionality

Per-table dimensions are configured via environment variables: `COVALENCE_EMBED_DIM_SOURCE` (default 2048), `COVALENCE_EMBED_DIM_CHUNK` (default 1024), `COVALENCE_EMBED_DIM_NODE` (default 256), `COVALENCE_EMBED_DIM_STATEMENT` (default 1024), `COVALENCE_EMBED_DIM_SECTION` (default 1024). Further multi-resolution optimization is a v2 concern.

**Model migration:** Changing embedding models requires a full re-embedding job (batch all chunks/nodes/articles through the new model, rebuild HNSW indexes). This is a batch operation, not a hot swap. Multi-model support (searching across different embedding dimensions) is explicitly out of scope for v1.

**Embedding model migration strategy:**

1. **Track model version per embedding.** The `model_calibrations` table records which model produced current calibrations. Add `embedding_model TEXT NOT NULL DEFAULT 'voyage-context-3'` to chunks/nodes/articles tables as a column (or store in metadata).
2. **Blue-green re-embedding.** Create new embedding columns (e.g., `embedding_v2 halfvec(N)`), batch-embed all records with the new model, rebuild HNSW indexes on the new columns, swap search queries to the new columns, drop the old columns.
3. **Drift detection.** When a new model is available, sample 1000 random queries against both old and new embeddings and compare recall@10. If recall improves by > 2%, trigger migration. If it degrades, skip.
4. **Cost estimation.** Re-embedding the full corpus: `total_tokens / 1M * $0.18` for voyage-context-3. For a 10M token corpus, ~$1.80. For 100M tokens, ~$18. Budget this as a periodic maintenance cost.
5. **Incremental adoption.** New documents always use the current model. Old documents are re-embedded in background batches. The system tolerates mixed embeddings temporarily (same model family, similar geometry) but cross-model search quality degrades — prioritize full re-embedding over partial.

## Migration Strategy

- Use `sqlx` migrate (embedded migrations, run at startup)
- Migrations are numbered sequentially: `001_initial_schema.sql`, `002_spec_update.sql`, `003_source_embedding.sql`, etc.
- Destructive migrations (drop column, change type) require a two-phase approach: add new → migrate data → drop old

**Migration 003 (`003_source_embedding.sql`):** Adds `embedding halfvec(2048)` to the `sources` table with an HNSW index. Document-level chunks previously duplicated the full source text and produced low-quality embeddings for large documents. This migration moves the document-level embedding onto the source record itself, avoiding both the duplication and the quality issue.

## Stored Procedures

### Graph Traversal 
> **Design Note:** Graph traversals (BFS, Shortest Path, etc.) are strictly delegated to the `petgraph` Rust sidecar. PostgreSQL Recursive CTEs are inefficient for multi-hop graph queries and lock database resources. See `spec/04-graph.md` for graph computation details.

### Temporal Queries

```sql
-- Find nodes/edges active within a time range (bitemporal: valid time + transaction time)
CREATE OR REPLACE FUNCTION temporal_search(
    valid_start TIMESTAMPTZ,
    valid_end TIMESTAMPTZ,
    as_of TIMESTAMPTZ DEFAULT now()  -- transaction time: "what did the system know at this point?"
) RETURNS TABLE(entity_type TEXT, entity_id UUID, valid_from TIMESTAMPTZ, valid_until TIMESTAMPTZ)
AS $$ ... $$ LANGUAGE sql;
```

### Confidence Update Trigger

```sql
-- Trigger to recompute reliability_score when trust_alpha or trust_beta changes
CREATE OR REPLACE FUNCTION update_reliability_score()
RETURNS TRIGGER AS $$
BEGIN
    NEW.reliability_score := NEW.trust_alpha / (NEW.trust_alpha + NEW.trust_beta);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_reliability_score
    BEFORE INSERT OR UPDATE OF trust_alpha, trust_beta ON sources
    FOR EACH ROW EXECUTE FUNCTION update_reliability_score();
```

### Provenance Triples View

For fine-grained provenance queries and epistemic model operations, a view decomposes the property graph into triples. Supports both statement-based and code-chunk-based provenance:

```sql
CREATE OR REPLACE VIEW provenance_triples AS
-- Chunk-based provenance (code pipeline)
SELECT
    e.id AS triple_id,
    e.source_node_id AS subject,
    e.rel_type AS predicate,
    e.target_node_id AS object,
    e.causal_level,
    e.confidence,
    ex.chunk_id,
    ex.statement_id,
    ex.confidence AS extraction_confidence,
    s.reliability_score AS source_reliability
FROM edges e
JOIN extractions ex ON ex.entity_id = e.id AND ex.entity_type = 'edge'
LEFT JOIN chunks c ON c.id = ex.chunk_id
LEFT JOIN statements st ON st.id = ex.statement_id
JOIN sources s ON s.id = COALESCE(c.source_id, st.source_id)
WHERE NOT ex.is_superseded;
```

This view is for the `/provenance` API and debugging. Do not use for core traversal — convert to `MATERIALIZED VIEW` and refresh during batch consolidation if it becomes a bottleneck.

## Open Questions

- [x] Embedding dimension → Single model for v1, migrate via full re-embedding job
- [x] Partitioning strategy → Don't partition for v1. pgvector maintainer advises against it for performance (pgvector#455). Use `pg_prewarm` for index preloading. Partition only if queries naturally filter by partition key (tenant_id).
- [x] Raw content storage → PG for v1 (simpler ops). Move to object storage when raw_content exceeds ~100GB.
- [x] HNSW parameters → Keep defaults (m=16, ef_construction=64). Tune ef_search at query time (≥2x LIMIT). Jonathan Katz benchmarks show m=12/ef=60 is consistently optimal across dataset sizes. Raise ef_construction to 128-200 only for >95% recall requirements.

## Semantic Query Cache

```sql
CREATE TABLE query_cache (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    query_text TEXT NOT NULL,
    query_embedding halfvec(2048) NOT NULL,
    response JSONB NOT NULL,
    strategy_used TEXT NOT NULL,
    hit_count INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- HNSW index for semantic similarity matching
CREATE INDEX idx_query_cache_embedding ON query_cache
    USING hnsw (query_embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- TTL cleanup (run periodically)
-- DELETE FROM query_cache WHERE created_at < now() - interval '1 hour';
-- LRU eviction when > 10K entries
-- DELETE FROM query_cache WHERE id IN (SELECT id FROM query_cache ORDER BY created_at ASC LIMIT (SELECT COUNT(*) - 10000 FROM query_cache));
```

## Scaling Projections

**Index memory estimates** (halfvec(2048), m=16):
- 10K chunks: ~160MB HNSW index → fits in any deployment
- 100K chunks: ~1.6GB → fits in a standard 8GB Postgres instance
- 1M chunks: ~16GB → requires dedicated instance, 32GB+ RAM recommended
- 10M chunks: ~160GB → exceeds single-node RAM, consider partitioning or Matryoshka (512d first-pass filter, 2048d re-score)

**Practical scale for v1:** Target 100K-500K chunks (1K-5K documents at ~100 chunks/document). This covers most knowledge bases, personal archives, and mid-size enterprise document stores. Beyond 1M chunks, the v2 Matryoshka multi-resolution strategy becomes important.

**HNSW rebuild strategy:**
- Rebuild after inserting > 20% new vectors (same guidance as pgvector community benchmarks)
- Schedule during low-traffic periods
- Use `CREATE INDEX CONCURRENTLY` to avoid table locks
- Monitor recall@10 on known test queries to detect index degradation

**Bottleneck analysis by query volume:**
| Queries/sec | Bottleneck | Mitigation |
|------------|-----------|------------|
| < 10 | None — single Postgres handles it | - |
| 10-100 | HNSW search latency | Increase ef_search, read replicas |
| 100-1000 | Connection pool + HNSW contention | PgBouncer, read replicas, query caching |
| > 1000 | Single-node Postgres limits | Shard by domain, dedicated vector index service |
- [x] confidence_breakdown → JSONB sufficient for v1. Separate table only if analytical queries on confidence distributions become a bottleneck.
- [x] Outbox event pruning → Hourly job, delete events older than 24 hours

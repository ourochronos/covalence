-- 007: All non-trivial indexes
--
-- GIN (tsvector, trigram), HNSW (vector similarity), partial,
-- expression, and composite indexes. Basic primary-key and
-- foreign-key indexes are created by 001_schema.sql table
-- definitions.

-- ================================================================
-- Sources
-- ================================================================

CREATE INDEX IF NOT EXISTS idx_sources_type ON sources(source_type);
CREATE INDEX IF NOT EXISTS idx_sources_ingested ON sources(ingested_at);
CREATE INDEX IF NOT EXISTS idx_sources_metadata ON sources USING GIN(metadata);
CREATE INDEX IF NOT EXISTS idx_sources_clearance ON sources(clearance_level);
CREATE INDEX IF NOT EXISTS idx_sources_supersedes ON sources(supersedes_id);
CREATE INDEX IF NOT EXISTS idx_sources_domain ON sources(domain);
CREATE INDEX IF NOT EXISTS idx_sources_project ON sources(project);
CREATE INDEX IF NOT EXISTS idx_sources_normalized_hash
    ON sources(normalized_hash) WHERE normalized_hash IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_sources_superseded
    ON sources(superseded_by) WHERE superseded_by IS NULL;
CREATE INDEX IF NOT EXISTS idx_sources_uri_ingested
    ON sources(uri, ingested_at DESC) WHERE uri IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_sources_status
    ON sources(status) WHERE status != 'complete';

-- Source embedding (HNSW, 2048d)
CREATE INDEX IF NOT EXISTS idx_sources_embedding
    ON sources USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- ================================================================
-- Chunks
-- ================================================================

CREATE INDEX IF NOT EXISTS idx_chunks_source ON chunks(source_id);
CREATE INDEX IF NOT EXISTS idx_chunks_parent ON chunks(parent_chunk_id);
CREATE INDEX IF NOT EXISTS idx_chunks_level ON chunks(level);
CREATE INDEX IF NOT EXISTS idx_chunks_hash ON chunks(content_hash);
CREATE INDEX IF NOT EXISTS idx_chunks_clearance ON chunks(clearance_level);
CREATE INDEX IF NOT EXISTS idx_chunks_hierarchy ON chunks USING GIST(structural_hierarchy);

-- Chunk embedding (HNSW, 1024d)
CREATE INDEX IF NOT EXISTS idx_chunks_embedding
    ON chunks USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- Full-text search
CREATE INDEX IF NOT EXISTS idx_chunks_tsv ON chunks USING GIN(content_tsv);

-- Partial index for pending extraction processing
CREATE INDEX IF NOT EXISTS idx_chunks_pending_extraction
    ON chunks(source_id)
    WHERE (processing->'extraction') IS NULL;

-- ================================================================
-- Nodes
-- ================================================================

CREATE INDEX IF NOT EXISTS idx_nodes_type ON nodes(node_type);
CREATE INDEX IF NOT EXISTS idx_nodes_name ON nodes(canonical_name);
CREATE INDEX IF NOT EXISTS idx_nodes_clearance ON nodes(clearance_level);
CREATE INDEX IF NOT EXISTS idx_nodes_properties ON nodes USING GIN(properties);
CREATE INDEX IF NOT EXISTS idx_nodes_entity_class ON nodes(entity_class);
CREATE INDEX IF NOT EXISTS idx_nodes_primary_domain ON nodes(primary_domain);

-- Trigram index for fuzzy matching
CREATE INDEX IF NOT EXISTS idx_nodes_name_trgm ON nodes USING GIN(canonical_name gin_trgm_ops);

-- Node embedding (HNSW, 256d)
CREATE INDEX IF NOT EXISTS idx_nodes_embedding
    ON nodes USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- Full-text search
CREATE INDEX IF NOT EXISTS idx_nodes_tsv ON nodes USING GIN(name_tsv);

-- Partial index for pending summary processing
CREATE INDEX IF NOT EXISTS idx_nodes_pending_summary
    ON nodes(entity_class)
    WHERE entity_class = 'code'
      AND (processing->'summary') IS NULL;

-- ================================================================
-- Edges
-- ================================================================

CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_node_id);
CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_node_id);
CREATE INDEX IF NOT EXISTS idx_edges_type ON edges(rel_type);
CREATE INDEX IF NOT EXISTS idx_edges_clearance ON edges(clearance_level);
CREATE INDEX IF NOT EXISTS idx_edges_pair ON edges(source_node_id, target_node_id, rel_type);

-- Partial indexes for bi-temporal queries
CREATE INDEX IF NOT EXISTS idx_edges_causal ON edges(causal_level) WHERE causal_level IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_edges_valid_from ON edges(valid_from) WHERE valid_from IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_edges_invalid_at ON edges(invalid_at) WHERE invalid_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_edges_active
    ON edges(source_node_id, target_node_id, rel_type) WHERE invalid_at IS NULL;

-- ================================================================
-- Articles
-- ================================================================

CREATE INDEX IF NOT EXISTS idx_articles_domain ON articles USING GIN(domain_path);
CREATE INDEX IF NOT EXISTS idx_articles_clearance ON articles(clearance_level);

-- Article embedding (HNSW, 1024d)
CREATE INDEX IF NOT EXISTS idx_articles_embedding
    ON articles USING hnsw (embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- Full-text search
CREATE INDEX IF NOT EXISTS idx_articles_tsv ON articles USING GIN(body_tsv);

-- ================================================================
-- Extractions
-- ================================================================

CREATE INDEX IF NOT EXISTS idx_extractions_chunk ON extractions(chunk_id);
CREATE INDEX IF NOT EXISTS idx_extractions_entity ON extractions(entity_type, entity_id);
CREATE INDEX IF NOT EXISTS idx_extractions_active ON extractions(entity_type, entity_id)
    WHERE NOT is_superseded;
CREATE INDEX IF NOT EXISTS idx_extractions_statement_id ON extractions(statement_id);

-- ================================================================
-- Node Aliases
-- ================================================================

CREATE INDEX IF NOT EXISTS idx_aliases_node ON node_aliases(node_id);
CREATE INDEX IF NOT EXISTS idx_aliases_text ON node_aliases(alias);

-- Trigram index for fuzzy matching
CREATE INDEX IF NOT EXISTS idx_aliases_text_trgm ON node_aliases USING GIN(alias gin_trgm_ops);

-- Alias embedding (HNSW, 256d)
CREATE INDEX IF NOT EXISTS idx_aliases_embedding
    ON node_aliases USING hnsw (alias_embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- ================================================================
-- Audit Logs
-- ================================================================

CREATE INDEX IF NOT EXISTS idx_audit_action ON audit_logs(action);
CREATE INDEX IF NOT EXISTS idx_audit_target ON audit_logs(target_type, target_id);
CREATE INDEX IF NOT EXISTS idx_audit_time ON audit_logs(created_at);

-- ================================================================
-- Offset Projection Ledger
-- ================================================================

CREATE INDEX IF NOT EXISTS idx_opl_source_id ON offset_projection_ledgers(source_id);
CREATE INDEX IF NOT EXISTS idx_opl_mutated_span
    ON offset_projection_ledgers(source_id, mutated_span_start, mutated_span_end);

-- ================================================================
-- Unresolved Entities
-- ================================================================

CREATE INDEX IF NOT EXISTS idx_unresolved_source_id ON unresolved_entities(source_id);
CREATE INDEX IF NOT EXISTS idx_unresolved_pending
    ON unresolved_entities(resolved_node_id) WHERE resolved_node_id IS NULL;

-- Unresolved entity embedding (HNSW, 256d)
CREATE INDEX IF NOT EXISTS idx_unresolved_embedding
    ON unresolved_entities USING hnsw (embedding halfvec_cosine_ops);

-- ================================================================
-- Ontology Clusters
-- ================================================================

CREATE INDEX IF NOT EXISTS idx_ontology_clusters_level ON ontology_clusters(level);
CREATE INDEX IF NOT EXISTS idx_ontology_clusters_canonical ON ontology_clusters(canonical_label);

-- ================================================================
-- Statements
-- ================================================================

CREATE INDEX IF NOT EXISTS idx_statements_source_id ON statements(source_id);
CREATE INDEX IF NOT EXISTS idx_statements_section_id ON statements(section_id);
CREATE INDEX IF NOT EXISTS idx_statements_content_hash ON statements(source_id, content_hash);
CREATE INDEX IF NOT EXISTS idx_statements_content_tsv ON statements USING gin(content_tsv);

-- Statement embedding (HNSW, 1024d)
CREATE INDEX IF NOT EXISTS idx_statements_embedding
    ON statements USING hnsw(embedding halfvec_cosine_ops);

-- ================================================================
-- Sections
-- ================================================================

CREATE INDEX IF NOT EXISTS idx_sections_source_id ON sections(source_id);
CREATE INDEX IF NOT EXISTS idx_sections_body_tsv ON sections USING gin(body_tsv);

-- Section embedding (HNSW, 1024d)
CREATE INDEX IF NOT EXISTS idx_sections_embedding
    ON sections USING hnsw(embedding halfvec_cosine_ops);

-- ================================================================
-- Retry Jobs
-- ================================================================

CREATE INDEX IF NOT EXISTS idx_retry_jobs_due
    ON retry_jobs(next_due, status)
    WHERE status = 'pending';
CREATE INDEX IF NOT EXISTS idx_retry_jobs_kind_status
    ON retry_jobs(kind, status);

-- ================================================================
-- Processing Log
-- ================================================================

CREATE INDEX IF NOT EXISTS idx_processing_log_item
    ON processing_log(item_table, item_id);
CREATE INDEX IF NOT EXISTS idx_processing_log_ingestion
    ON processing_log(ingestion_id) WHERE ingestion_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_processing_log_stage_model
    ON processing_log(stage, model);

-- ================================================================
-- Search Infrastructure
-- ================================================================

CREATE INDEX IF NOT EXISTS idx_search_traces_created
    ON search_traces(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_search_feedback_created
    ON search_feedback(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_search_feedback_result
    ON search_feedback(result_id);

-- Query cache embedding (HNSW, 1024d)
CREATE INDEX IF NOT EXISTS idx_query_cache_embedding
    ON query_cache USING hnsw (query_embedding halfvec_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- ================================================================
-- Source Adapters
-- ================================================================

CREATE INDEX IF NOT EXISTS idx_source_adapters_active
    ON source_adapters(is_active) WHERE is_active = true;

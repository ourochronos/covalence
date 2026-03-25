//! Repository trait definitions.
//!
//! Each domain entity has a repository trait defining CRUD operations.
//! Implementations live in the `postgres` module.

use crate::error::Result;
use crate::models::article::Article;
use crate::models::audit::AuditLog;
use crate::models::chunk::Chunk;
use crate::models::edge::Edge;
use crate::models::extraction::Extraction;
use crate::models::node::Node;
use crate::models::node_alias::NodeAlias;
use crate::models::retry_job::{JobKind, JobStatus, QueueStatusRow, RetryJob};
use crate::models::section::Section;
use crate::models::source::Source;
use crate::models::statement::Statement;
use crate::models::trace::{SearchFeedback, SearchTrace};
use crate::models::unresolved_entity::UnresolvedEntity;
use crate::services::adapter_service::SourceAdapter;
use crate::services::config_service::ConfigEntry;
use crate::types::ids::{
    AliasId, ArticleId, AuditLogId, ChunkId, EdgeId, ExtractionId, JobId, NodeId, SectionId,
    SourceId, StatementId,
};

/// Repository for [`Source`] entities.
pub trait SourceRepo: Send + Sync {
    /// Insert a new source.
    fn create(&self, source: &Source) -> impl Future<Output = Result<()>> + Send;

    /// Get a source by ID.
    fn get(&self, id: SourceId) -> impl Future<Output = Result<Option<Source>>> + Send;

    /// Get a source by content hash (for dedup).
    fn get_by_hash(&self, hash: &[u8]) -> impl Future<Output = Result<Option<Source>>> + Send;

    /// Get a source by normalized content hash (for semantic dedup).
    fn get_by_normalized_hash(
        &self,
        hash: &[u8],
    ) -> impl Future<Output = Result<Option<Source>>> + Send;

    /// Update an existing source.
    fn update(&self, source: &Source) -> impl Future<Output = Result<()>> + Send;

    /// Delete a source by ID.
    fn delete(&self, id: SourceId) -> impl Future<Output = Result<bool>> + Send;

    /// List sources, ordered by ingested_at descending.
    fn list(&self, limit: i64, offset: i64) -> impl Future<Output = Result<Vec<Source>>> + Send;

    /// Count total number of sources.
    fn count(&self) -> impl Future<Output = Result<i64>> + Send;

    /// Update the document-level embedding vector for a source.
    fn update_embedding(
        &self,
        id: SourceId,
        embedding: &[f64],
    ) -> impl Future<Output = Result<()>> + Send;

    /// Clear the embedding for a superseded source so it doesn't
    /// appear in vector search results.
    fn clear_embedding(&self, id: SourceId) -> impl Future<Output = Result<()>> + Send;

    /// Update the summary text for a source.
    fn update_summary(
        &self,
        id: SourceId,
        summary: &str,
    ) -> impl Future<Output = Result<()>> + Send;
}

/// Repository for [`Chunk`] entities.
pub trait ChunkRepo: Send + Sync {
    /// Insert a new chunk.
    fn create(&self, chunk: &Chunk) -> impl Future<Output = Result<()>> + Send;

    /// Insert multiple chunks in a single batch operation.
    fn batch_create(&self, chunks: &[Chunk]) -> impl Future<Output = Result<()>> + Send;

    /// Get a chunk by ID.
    fn get(&self, id: ChunkId) -> impl Future<Output = Result<Option<Chunk>>> + Send;

    /// Get all chunks for a given source.
    fn list_by_source(
        &self,
        source_id: SourceId,
    ) -> impl Future<Output = Result<Vec<Chunk>>> + Send;

    /// Get child chunks of a parent chunk.
    fn list_children(&self, parent_id: ChunkId) -> impl Future<Output = Result<Vec<Chunk>>> + Send;

    /// Delete a chunk by ID.
    fn delete(&self, id: ChunkId) -> impl Future<Output = Result<bool>> + Send;

    /// Delete all chunks for a source.
    fn delete_by_source(&self, source_id: SourceId) -> impl Future<Output = Result<u64>> + Send;

    /// Update the embedding vector for a chunk.
    fn update_embedding(
        &self,
        id: ChunkId,
        embedding: &[f64],
    ) -> impl Future<Output = Result<()>> + Send;
}

/// Repository for [`Node`] entities.
pub trait NodeRepo: Send + Sync {
    /// Insert a new node.
    fn create(&self, node: &Node) -> impl Future<Output = Result<()>> + Send;

    /// Get a node by ID.
    fn get(&self, id: NodeId) -> impl Future<Output = Result<Option<Node>>> + Send;

    /// Find a node by canonical name (case-insensitive).
    fn find_by_name(&self, name: &str) -> impl Future<Output = Result<Option<Node>>> + Send;

    /// Update an existing node.
    fn update(&self, node: &Node) -> impl Future<Output = Result<()>> + Send;

    /// Delete a node by ID.
    fn delete(&self, id: NodeId) -> impl Future<Output = Result<bool>> + Send;

    /// List nodes by type.
    fn list_by_type(
        &self,
        node_type: &str,
        limit: i64,
        offset: i64,
    ) -> impl Future<Output = Result<Vec<Node>>> + Send;

    /// Update the embedding vector for a node.
    fn update_embedding(
        &self,
        id: NodeId,
        embedding: &[f64],
    ) -> impl Future<Output = Result<()>> + Send;

    /// List nodes that have zero active (non-superseded) extractions.
    ///
    /// These "ungrounded" nodes lost all provenance backing when
    /// their extractions were superseded during source reprocessing
    /// and are candidates for garbage collection.
    fn list_ungrounded(&self) -> impl Future<Output = Result<Vec<Node>>> + Send;

    /// Fetch multiple nodes by ID in a single query.
    fn get_many(&self, ids: &[NodeId]) -> impl Future<Output = Result<Vec<Node>>> + Send;

    /// Batch-update the `confidence_breakdown` opinion for multiple
    /// nodes in a single query.
    ///
    /// Each tuple is `(node_id, new_opinion_json)`. Nodes not found
    /// in the table are silently skipped. Passing `None` sets the
    /// column to `NULL` in the database.
    fn batch_update_opinions(
        &self,
        updates: &[(NodeId, Option<serde_json::Value>)],
    ) -> impl Future<Output = Result<()>> + Send;
}

/// Repository for [`Edge`] entities.
pub trait EdgeRepo: Send + Sync {
    /// Insert a new edge.
    fn create(&self, edge: &Edge) -> impl Future<Output = Result<()>> + Send;

    /// Get an edge by ID.
    fn get(&self, id: EdgeId) -> impl Future<Output = Result<Option<Edge>>> + Send;

    /// Get all edges originating from a node.
    fn list_from_node(&self, node_id: NodeId) -> impl Future<Output = Result<Vec<Edge>>> + Send;

    /// Get all edges pointing to a node.
    fn list_to_node(&self, node_id: NodeId) -> impl Future<Output = Result<Vec<Edge>>> + Send;

    /// Get edges between two specific nodes.
    fn list_between(
        &self,
        source_id: NodeId,
        target_id: NodeId,
    ) -> impl Future<Output = Result<Vec<Edge>>> + Send;

    /// Update an existing edge.
    fn update(&self, edge: &Edge) -> impl Future<Output = Result<()>> + Send;

    /// Invalidate an edge by marking it as superseded.
    fn invalidate(
        &self,
        id: EdgeId,
        invalidated_by: EdgeId,
    ) -> impl Future<Output = Result<()>> + Send;

    /// List active (non-invalidated) edges.
    fn list_active(&self) -> impl Future<Output = Result<Vec<Edge>>> + Send;

    /// Delete an edge by ID.
    fn delete(&self, id: EdgeId) -> impl Future<Output = Result<bool>> + Send;

    /// Find active edges sharing the same source node and relationship type.
    ///
    /// Used by conflict detection to find candidate contradictions
    /// before creating a new edge.
    fn find_by_source_and_rel_type(
        &self,
        source_node_id: NodeId,
        rel_type: &str,
    ) -> impl Future<Output = Result<Vec<Edge>>> + Send;

    /// Delete all edges involving a node (as source or target).
    ///
    /// Returns the number of rows deleted. Used during cascading
    /// source deletion to remove dangling edges before deleting
    /// orphaned nodes.
    fn delete_by_node(&self, node_id: NodeId) -> impl Future<Output = Result<u64>> + Send;

    /// Fetch multiple edges by ID in a single query.
    fn get_many(&self, ids: &[EdgeId]) -> impl Future<Output = Result<Vec<Edge>>> + Send;

    /// Batch-update `confidence` and `confidence_breakdown` for
    /// multiple edges in a single query.
    ///
    /// Each tuple is `(edge_id, confidence_scalar, opinion_json)`.
    /// Edges not found in the table are silently skipped. Passing
    /// `None` for the opinion sets it to `NULL` in the database.
    fn batch_update_opinions(
        &self,
        updates: &[(EdgeId, f64, Option<serde_json::Value>)],
    ) -> impl Future<Output = Result<()>> + Send;
}

/// Repository for [`Article`] entities.
pub trait ArticleRepo: Send + Sync {
    /// Insert a new article.
    fn create(&self, article: &Article) -> impl Future<Output = Result<()>> + Send;

    /// Get an article by ID.
    fn get(&self, id: ArticleId) -> impl Future<Output = Result<Option<Article>>> + Send;

    /// Update an existing article.
    fn update(&self, article: &Article) -> impl Future<Output = Result<()>> + Send;

    /// Delete an article by ID.
    fn delete(&self, id: ArticleId) -> impl Future<Output = Result<bool>> + Send;

    /// List articles within a domain path prefix.
    fn list_by_domain(
        &self,
        domain_prefix: &[String],
        limit: i64,
        offset: i64,
    ) -> impl Future<Output = Result<Vec<Article>>> + Send;

    /// Update the embedding vector for an article.
    fn update_embedding(
        &self,
        id: ArticleId,
        embedding: &[f64],
    ) -> impl Future<Output = Result<()>> + Send;
}

/// Repository for [`Extraction`] provenance links.
pub trait ExtractionRepo: Send + Sync {
    /// Insert a new extraction record.
    fn create(&self, extraction: &Extraction) -> impl Future<Output = Result<()>> + Send;

    /// Get an extraction by ID.
    fn get(&self, id: ExtractionId) -> impl Future<Output = Result<Option<Extraction>>> + Send;

    /// Get all extractions for a given chunk.
    fn list_by_chunk(
        &self,
        chunk_id: ChunkId,
    ) -> impl Future<Output = Result<Vec<Extraction>>> + Send;

    /// Get all active (non-superseded) extractions for a graph entity.
    fn list_active_for_entity(
        &self,
        entity_type: &str,
        entity_id: uuid::Uuid,
    ) -> impl Future<Output = Result<Vec<Extraction>>> + Send;

    /// Mark an extraction as superseded.
    fn mark_superseded(&self, id: ExtractionId) -> impl Future<Output = Result<()>> + Send;

    /// Mark all extractions for a source's chunks as superseded.
    ///
    /// Used during source reprocessing to supersede the previous
    /// extraction run before creating new extraction records.
    fn mark_superseded_by_source(
        &self,
        source_id: SourceId,
    ) -> impl Future<Output = Result<u64>> + Send;

    /// Delete all extractions for a source's chunks.
    ///
    /// Returns the number of rows deleted. Used during cascading
    /// source deletion.
    fn delete_by_source(&self, source_id: SourceId) -> impl Future<Output = Result<u64>> + Send;

    /// List distinct entity IDs of type "node" extracted from a
    /// source's chunks (including superseded extractions).
    ///
    /// Used to find affected nodes during cascading source deletion.
    fn list_node_ids_by_source(
        &self,
        source_id: SourceId,
    ) -> impl Future<Output = Result<Vec<NodeId>>> + Send;

    /// Count non-superseded extractions for a specific entity.
    ///
    /// Used to determine whether a node can be safely deleted
    /// (zero remaining extractions) during cascading source deletion.
    fn count_active_by_entity(
        &self,
        entity_type: &str,
        entity_id: uuid::Uuid,
    ) -> impl Future<Output = Result<i64>> + Send;

    /// List distinct entity IDs of type "edge" extracted from a
    /// source's chunks or statements.
    ///
    /// Used by the TMS epistemic cascade to identify edges whose
    /// opinions need recalculation after source retraction.
    fn list_edge_ids_by_source(
        &self,
        source_id: SourceId,
    ) -> impl Future<Output = Result<Vec<EdgeId>>> + Send;

    /// Get all active (non-superseded) extractions for multiple graph
    /// entities of the same type in a single query.
    ///
    /// Returns extractions for all requested entities. Callers should
    /// group by `entity_id` to partition per-entity results.
    /// This is the batch counterpart to [`list_active_for_entity`].
    fn list_active_for_entities(
        &self,
        entity_type: &str,
        entity_ids: &[uuid::Uuid],
    ) -> impl Future<Output = Result<Vec<Extraction>>> + Send;
}

/// Repository for [`NodeAlias`] entities.
pub trait NodeAliasRepo: Send + Sync {
    /// Insert a new alias.
    fn create(&self, alias: &NodeAlias) -> impl Future<Output = Result<()>> + Send;

    /// Get an alias by ID.
    fn get(&self, id: AliasId) -> impl Future<Output = Result<Option<NodeAlias>>> + Send;

    /// Get all aliases for a node.
    fn list_by_node(&self, node_id: NodeId) -> impl Future<Output = Result<Vec<NodeAlias>>> + Send;

    /// Find nodes by alias text (case-insensitive, trigram match).
    fn find_by_alias(&self, alias: &str) -> impl Future<Output = Result<Vec<NodeAlias>>> + Send;

    /// Delete an alias by ID.
    fn delete(&self, id: AliasId) -> impl Future<Output = Result<bool>> + Send;

    /// Delete all aliases for a node.
    ///
    /// Returns the number of rows deleted. Used during cascading
    /// source deletion to remove aliases before deleting orphaned
    /// nodes.
    fn delete_by_node(&self, node_id: NodeId) -> impl Future<Output = Result<u64>> + Send;

    /// Nullify `source_chunk_id` for all aliases referencing chunks
    /// belonging to a source.
    ///
    /// This must be called before deleting chunks to avoid FK
    /// violations on `node_aliases.source_chunk_id`.
    fn clear_source_chunks(&self, source_id: SourceId) -> impl Future<Output = Result<u64>> + Send;
}

/// Repository for [`AuditLog`] entries.
pub trait AuditLogRepo: Send + Sync {
    /// Insert a new audit log entry.
    fn create(&self, log: &AuditLog) -> impl Future<Output = Result<()>> + Send;

    /// Get an audit log entry by ID.
    fn get(&self, id: AuditLogId) -> impl Future<Output = Result<Option<AuditLog>>> + Send;

    /// List audit log entries for a specific target.
    fn list_by_target(
        &self,
        target_type: &str,
        target_id: uuid::Uuid,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<AuditLog>>> + Send;

    /// List recent audit log entries.
    fn list_recent(&self, limit: i64) -> impl Future<Output = Result<Vec<AuditLog>>> + Send;
}

/// Repository for [`SearchTrace`] entities.
pub trait SearchTraceRepo: Send + Sync {
    /// Insert a new search trace.
    fn create(&self, trace: &SearchTrace) -> impl Future<Output = Result<()>> + Send;

    /// Get a search trace by ID.
    fn get(&self, id: uuid::Uuid) -> impl Future<Output = Result<Option<SearchTrace>>> + Send;

    /// List recent search traces.
    fn list_recent(&self, limit: i64) -> impl Future<Output = Result<Vec<SearchTrace>>> + Send;
}

/// Repository for [`SearchFeedback`] entities.
pub trait SearchFeedbackRepo: Send + Sync {
    /// Insert a new search feedback entry.
    fn create(&self, feedback: &SearchFeedback) -> impl Future<Output = Result<()>> + Send;

    /// List recent search feedback entries.
    fn list_recent(&self, limit: i64) -> impl Future<Output = Result<Vec<SearchFeedback>>> + Send;
}

/// Repository for node landmark queries.
pub trait NodeLandmarkRepo: Send + Sync {
    /// List nodes ordered by mention count descending (proxy for
    /// betweenness centrality).
    fn list_landmarks(&self, limit: i64) -> impl Future<Output = Result<Vec<Node>>> + Send;
}

/// Repository for [`Statement`] entities.
pub trait StatementRepo: Send + Sync {
    /// Insert a new statement.
    fn create(&self, statement: &Statement) -> impl Future<Output = Result<()>> + Send;

    /// Insert multiple statements in a single batch operation.
    fn batch_create(&self, statements: &[Statement]) -> impl Future<Output = Result<()>> + Send;

    /// Get a statement by ID.
    fn get(&self, id: StatementId) -> impl Future<Output = Result<Option<Statement>>> + Send;

    /// Get all statements for a given source.
    fn list_by_source(
        &self,
        source_id: SourceId,
    ) -> impl Future<Output = Result<Vec<Statement>>> + Send;

    /// Get all statements in a given section.
    fn list_by_section(
        &self,
        section_id: SectionId,
    ) -> impl Future<Output = Result<Vec<Statement>>> + Send;

    /// Find a statement by content hash (for dedup).
    fn get_by_content_hash(
        &self,
        source_id: SourceId,
        hash: &[u8],
    ) -> impl Future<Output = Result<Option<Statement>>> + Send;

    /// Delete all statements for a source.
    fn delete_by_source(&self, source_id: SourceId) -> impl Future<Output = Result<u64>> + Send;

    /// Update the embedding vector for a statement.
    fn update_embedding(
        &self,
        id: StatementId,
        embedding: &[f64],
    ) -> impl Future<Output = Result<()>> + Send;

    /// Assign a statement to a section.
    fn assign_section(
        &self,
        id: StatementId,
        section_id: SectionId,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Mark a statement as evicted.
    fn mark_evicted(&self, id: StatementId) -> impl Future<Output = Result<()>> + Send;

    /// Count statements for a source.
    fn count_by_source(&self, source_id: SourceId) -> impl Future<Output = Result<i64>> + Send;
}

/// Repository for [`Section`] entities.
pub trait SectionRepo: Send + Sync {
    /// Insert a new section.
    fn create(&self, section: &Section) -> impl Future<Output = Result<()>> + Send;

    /// Get a section by ID.
    fn get(&self, id: SectionId) -> impl Future<Output = Result<Option<Section>>> + Send;

    /// Get all sections for a given source.
    fn list_by_source(
        &self,
        source_id: SourceId,
    ) -> impl Future<Output = Result<Vec<Section>>> + Send;

    /// Delete all sections for a source.
    fn delete_by_source(&self, source_id: SourceId) -> impl Future<Output = Result<u64>> + Send;

    /// Update the embedding vector for a section.
    fn update_embedding(
        &self,
        id: SectionId,
        embedding: &[f64],
    ) -> impl Future<Output = Result<()>> + Send;

    /// Update the summary and content hash for a section.
    fn update_summary(
        &self,
        id: SectionId,
        summary: &str,
        content_hash: &[u8],
    ) -> impl Future<Output = Result<()>> + Send;

    /// Count sections for a source.
    fn count_by_source(&self, source_id: SourceId) -> impl Future<Output = Result<i64>> + Send;
}

/// Repository for [`UnresolvedEntity`] entries (Tier 5 HDBSCAN pool).
pub trait UnresolvedEntityRepo: Send + Sync {
    /// Insert a new unresolved entity.
    fn create(&self, entity: &UnresolvedEntity) -> impl Future<Output = Result<()>> + Send;

    /// Get an unresolved entity by ID.
    fn get(&self, id: uuid::Uuid) -> impl Future<Output = Result<Option<UnresolvedEntity>>> + Send;

    /// List all pending (unresolved) entities.
    fn list_pending(&self) -> impl Future<Output = Result<Vec<UnresolvedEntity>>> + Send;

    /// List pending entities for a source.
    fn list_by_source(
        &self,
        source_id: SourceId,
    ) -> impl Future<Output = Result<Vec<UnresolvedEntity>>> + Send;

    /// Mark an entity as resolved to a specific node.
    fn mark_resolved(
        &self,
        id: uuid::Uuid,
        node_id: NodeId,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Delete all unresolved entities for a source.
    fn delete_by_source(&self, source_id: SourceId) -> impl Future<Output = Result<u64>> + Send;

    /// Count pending (unresolved) entities.
    fn count_pending(&self) -> impl Future<Output = Result<i64>> + Send;
}

/// Repository for [`LedgerEntry`] records (offset projection ledger).
///
/// Stores byte offset mutations from coreference resolution so that
/// entity spans extracted from mutated text can be reverse-projected
/// back to canonical source positions.
pub trait LedgerRepo: Send + Sync {
    /// Insert a batch of ledger entries for a source.
    fn create_batch(
        &self,
        entries: &[crate::models::projection::LedgerEntry],
    ) -> impl Future<Output = Result<()>> + Send;

    /// Get all ledger entries for a source, sorted by mutated position.
    fn list_by_source(
        &self,
        source_id: SourceId,
    ) -> impl Future<Output = Result<Vec<crate::models::projection::LedgerEntry>>> + Send;

    /// Delete all ledger entries for a source (e.g. on re-ingestion).
    fn delete_by_source(&self, source_id: SourceId) -> impl Future<Output = Result<u64>> + Send;
}

/// Repository for [`RetryJob`] entities (persistent retry queue).
pub trait JobQueueRepo: Send + Sync {
    /// Enqueue a new job. Returns `None` if deduped via idempotency key.
    fn enqueue(
        &self,
        kind: JobKind,
        payload: serde_json::Value,
        max_attempts: i32,
        idempotency_key: Option<&str>,
    ) -> impl Future<Output = Result<Option<RetryJob>>> + Send;

    /// Enqueue multiple jobs in a single database round-trip.
    ///
    /// Uses UNNEST arrays to insert all jobs at once. Jobs with
    /// duplicate idempotency keys are silently skipped (ON CONFLICT
    /// DO NOTHING). Returns the number of jobs actually inserted.
    fn enqueue_batch(
        &self,
        jobs: Vec<crate::models::retry_job::EnqueueJob>,
    ) -> impl Future<Output = Result<u64>> + Send;

    /// Claim the next pending job of the given kinds.
    ///
    /// Uses `SELECT FOR UPDATE SKIP LOCKED` so that concurrent
    /// workers never claim the same row.
    fn claim_next(
        &self,
        kinds: &[JobKind],
    ) -> impl Future<Output = Result<Option<RetryJob>>> + Send;

    /// Mark a job as succeeded.
    fn mark_succeeded(&self, id: JobId) -> impl Future<Output = Result<()>> + Send;

    /// Mark a job as failed with an error message.
    ///
    /// If the attempt count has reached `max_attempts`, the job is
    /// moved to `Dead` instead of back to `Pending`. Returns the
    /// resulting [`JobStatus`].
    /// Mark a job as failed.
    ///
    /// If `force_dead` is true, the job goes directly to dead-letter
    /// regardless of attempt count (for permanent errors). Otherwise,
    /// the job is rescheduled with `backoff_secs` delay, or moved to
    /// dead if `attempt >= max_attempts`.
    fn mark_failed(
        &self,
        id: JobId,
        error: &str,
        backoff_secs: u64,
        force_dead: bool,
    ) -> impl Future<Output = Result<JobStatus>> + Send;

    /// Move all failed jobs (optionally filtered by kind) back to
    /// pending. Returns the number of jobs retried.
    fn retry_failed(&self, kind: Option<JobKind>) -> impl Future<Output = Result<u64>> + Send;

    /// Get a summary of queue contents grouped by kind and status.
    fn queue_status(&self) -> impl Future<Output = Result<Vec<QueueStatusRow>>> + Send;

    /// Delete all dead jobs, optionally filtered by kind.
    /// Returns the number of jobs deleted.
    fn clear_dead(&self, kind: Option<JobKind>) -> impl Future<Output = Result<u64>> + Send;

    /// List dead jobs, most recently updated first.
    fn list_dead(&self, limit: i64) -> impl Future<Output = Result<Vec<RetryJob>>> + Send;

    /// Resurrect dead jobs — reset to pending with attempt=0.
    /// Optionally filter by kind. Clears idempotency_key so
    /// fan-in triggers can re-enqueue if needed.
    /// Returns the number of jobs resurrected.
    fn resurrect_dead(&self, kind: Option<JobKind>) -> impl Future<Output = Result<u64>> + Send;
}

// ── New repository traits for service-layer SQL migration ────────

/// Repository for administrative data queries (metrics, backfill, noise cleanup).
#[allow(clippy::type_complexity)]
pub trait AdminRepo: Send + Sync {
    /// Check database connectivity (SELECT 1).
    fn ping(&self) -> impl Future<Output = Result<bool>> + Send;

    /// Run the full data health report via SP.
    fn data_health_report(
        &self,
    ) -> impl Future<Output = Result<(i64, i64, i64, i64, i64, i64, i64, i64)>> + Send;

    /// Get aggregate metrics: chunk count, article count, trace count,
    /// summary chunk count.
    fn metrics_counts(&self) -> impl Future<Output = Result<(i64, i64, i64, i64)>> + Send;

    /// List all nodes as (id, canonical_name, node_type) tuples.
    fn list_all_nodes(
        &self,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, String, String)>>> + Send;

    /// Count edges touching a given node (both directions).
    fn count_edges_for_node(&self, node_id: uuid::Uuid)
    -> impl Future<Output = Result<i64>> + Send;

    /// Nullify `invalidated_by` FK references pointing at edges of a node.
    fn nullify_invalidated_by_for_node(
        &self,
        node_id: uuid::Uuid,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Clear `resolved_node_id` on unresolved_entities for a node.
    fn clear_unresolved_for_node(
        &self,
        node_id: uuid::Uuid,
    ) -> impl Future<Output = Result<()>> + Send;

    /// List nodes without embeddings via SP.
    fn list_nodes_without_embeddings(
        &self,
        limit: i32,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, String, Option<String>)>>> + Send;

    /// List all node IDs (for opinion seeding).
    fn list_all_node_ids(&self) -> impl Future<Output = Result<Vec<uuid::Uuid>>> + Send;

    /// List all non-synthetic edge IDs (for opinion seeding).
    fn list_all_nonsynthetic_edge_ids(
        &self,
    ) -> impl Future<Output = Result<Vec<uuid::Uuid>>> + Send;

    /// List code nodes without semantic summaries.
    fn list_unsummarized_code_nodes(
        &self,
        code_types: &[&str],
    ) -> impl Future<
        Output = Result<
            Vec<(
                uuid::Uuid,
                String,
                String,
                Option<String>,
                Option<serde_json::Value>,
            )>,
        >,
    > + Send;

    /// Update node properties by ID.
    fn update_node_properties(
        &self,
        id: uuid::Uuid,
        properties: &serde_json::Value,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Invalidated edge stats via SP.
    fn invalidated_edge_stats(&self) -> impl Future<Output = Result<(i64, i64)>> + Send;

    /// Top invalidated relationship types via SP.
    fn top_invalidated_rel_types(
        &self,
        limit: i32,
    ) -> impl Future<Output = Result<Vec<(String, i64)>>> + Send;

    /// Top nodes by invalidated edge count.
    fn top_invalidated_edge_nodes(
        &self,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, String, String, i64)>>> + Send;

    /// Find co-occurrence pairs via SP.
    fn find_cooccurrence_pairs(
        &self,
        min_cooccurrences: i32,
        max_degree: i32,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, uuid::Uuid, i64)>>> + Send;

    /// List code nodes with embeddings.
    fn list_code_nodes_with_embeddings(
        &self,
        code_types: &[String],
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, String, String)>>> + Send;

    /// Find nearest non-code concept nodes by embedding similarity.
    fn find_nearest_non_code_nodes(
        &self,
        code_id: uuid::Uuid,
        code_types: &[String],
        limit: i64,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, String, f64)>>> + Send;

    /// Check if an edge exists between two nodes with a given rel_type.
    fn check_edge_exists(
        &self,
        source_id: uuid::Uuid,
        target_id: uuid::Uuid,
        rel_type: &str,
    ) -> impl Future<Output = Result<bool>> + Send;

    /// Get node provenance sources via SP.
    fn get_node_provenance_sources(
        &self,
        node_ids: &[uuid::Uuid],
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, Option<String>, Option<String>)>>> + Send;

    /// Tombstone a node (soft-delete via SP).
    fn tombstone_node(&self, node_id: NodeId) -> impl Future<Output = Result<()>> + Send;

    /// Get invalidated edges for a node via SP.
    fn get_invalidated_edges_for_node(
        &self,
        node_id: uuid::Uuid,
        limit: i32,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, uuid::Uuid)>>> + Send;
}

/// Repository for cross-domain analysis queries.
#[allow(clippy::type_complexity)]
pub trait AnalysisRepo: Send + Sync {
    /// Check if a component node exists by name.
    fn component_exists(&self, name: &str) -> impl Future<Output = Result<bool>> + Send;

    /// List component nodes via SP.
    fn list_component_nodes(
        &self,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, String, String)>>> + Send;

    /// List code nodes from code-domain sources with file paths.
    fn list_code_nodes_with_paths(
        &self,
        code_entity_class: &str,
        code_domain: &str,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, String, String)>>> + Send;

    /// Check edge exists via SP.
    fn check_edge_exists_sp(
        &self,
        source_id: uuid::Uuid,
        target_id: uuid::Uuid,
        rel_type: &str,
    ) -> impl Future<Output = Result<bool>> + Send;

    /// Find nearest spec/design domain nodes by embedding similarity.
    fn find_nearest_domain_nodes(
        &self,
        comp_id: uuid::Uuid,
        domains: &[String],
        limit: i64,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, String, f64)>>> + Send;

    /// Get orphan code nodes via SP.
    fn get_orphan_code_nodes(
        &self,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, String, String, String)>>> + Send;

    /// Get unimplemented specs via SP.
    fn get_unimplemented_specs(
        &self,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, String, String)>>> + Send;

    /// Count spec concepts via SP.
    fn count_spec_concepts(&self) -> impl Future<Output = Result<i64>> + Send;

    /// Count implemented specs via SP.
    fn count_implemented_specs(&self) -> impl Future<Output = Result<i64>> + Send;

    /// Find code nodes with PART_OF_COMPONENT edges by embedding distance.
    fn find_component_code_nodes(
        &self,
        comp_id: uuid::Uuid,
        rel_type: &str,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, String, Option<String>, f64)>>> + Send;

    /// Get research source bridges via SP.
    fn get_research_source_bridges(
        &self,
        min_cluster_size: i64,
        limit: i64,
        domain_filter: Option<&str>,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, String, Option<String>, i64, i64)>>> + Send;

    /// List representative nodes for a source.
    fn list_source_representative_nodes(
        &self,
        source_id: uuid::Uuid,
    ) -> impl Future<Output = Result<Vec<(String, String)>>> + Send;

    /// Find components connected to source entities via a rel_type.
    fn find_connected_components(
        &self,
        source_id: uuid::Uuid,
        rel_type: &str,
    ) -> impl Future<Output = Result<Vec<(String,)>>> + Send;

    /// Resolve a node by name via SP (exact then fuzzy).
    fn resolve_node_by_name(
        &self,
        name: &str,
    ) -> impl Future<Output = Result<Option<(uuid::Uuid, String, String)>>> + Send;

    /// Resolve a node by fuzzy name via SP.
    fn resolve_node_fuzzy(
        &self,
        name: &str,
        limit: i32,
    ) -> impl Future<Output = Result<Option<(uuid::Uuid, String, String)>>> + Send;

    /// Get a node's component via SP.
    fn get_node_component(
        &self,
        node_id: uuid::Uuid,
    ) -> impl Future<Output = Result<Option<(uuid::Uuid, String)>>> + Send;

    /// Get invalidated neighbors for blast radius.
    fn get_invalidated_neighbors(
        &self,
        node_id: uuid::Uuid,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, String, String, String)>>> + Send;

    /// Find research nodes nearest to a query vector.
    fn find_nearest_research_nodes(
        &self,
        query_vec: &[f64],
        research_domains: &[String],
        limit: i64,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, String, String, Option<String>, f64)>>> + Send;

    /// Find components connected via THEORETICAL_BASIS to node IDs.
    fn find_theoretical_components(
        &self,
        node_ids: &[uuid::Uuid],
        rel_type: &str,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, String)>>> + Send;

    /// Find code nodes linked to components via PART_OF_COMPONENT by
    /// embedding distance.
    fn find_component_code_by_embedding(
        &self,
        query_vec: &[f64],
        comp_ids: &[uuid::Uuid],
        rel_type: &str,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, String, String, Option<String>, f64)>>> + Send;

    /// Find domain evidence nodes by embedding distance.
    fn find_domain_evidence(
        &self,
        query_vec: &[f64],
        domains: &[String],
        entity_class: Option<&str>,
        code_domain: Option<&str>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, String, String, Option<String>, f64)>>> + Send;

    /// Alignment: find code ahead via SP.
    fn find_code_ahead(
        &self,
        distance_threshold: f64,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<(String, String, String, Option<f64>, Option<String>)>>> + Send;

    /// Alignment: find spec ahead via SP.
    fn check_spec_ahead(
        &self,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<(String, String, i32)>>> + Send;

    /// Alignment: find design contradictions via SP.
    fn find_design_contradictions(
        &self,
        distance_threshold: f64,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<(String, String, f64, String)>>> + Send;

    /// Alignment: find stale design via SP.
    fn find_stale_design(
        &self,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<(String, String, f64, String)>>> + Send;
}

/// Repository for pipeline queries used during entity resolution
/// and ingestion.
#[allow(clippy::type_complexity)]
pub trait PipelineRepo: Send + Sync {
    /// Acquire a transaction-scoped advisory lock.
    fn advisory_xact_lock(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        key: i64,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Get node by exact name via SP (within transaction).
    fn get_node_by_name_exact_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        name: &str,
    ) -> impl Future<Output = Result<Option<NodeId>>> + Send;

    /// Bump node mention count via SP (within transaction).
    fn bump_node_mention_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        node_id: NodeId,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Get node properties via SP (within transaction).
    fn get_node_properties_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        node_id: NodeId,
    ) -> impl Future<Output = Result<Option<serde_json::Value>>> + Send;

    /// Update node AST hash via SP (within transaction).
    fn update_node_ast_hash_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        node_id: NodeId,
        props: &serde_json::Value,
        description: &Option<String>,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Update node AST hash only via SP (within transaction).
    fn update_node_ast_hash_only_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        node_id: NodeId,
        hash: &str,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Create extraction from chunk via SP (within transaction).
    fn create_extraction_from_chunk_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        ext_id: uuid::Uuid,
        chunk_id: ChunkId,
        entity_id: uuid::Uuid,
        method: &str,
        confidence: f64,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Create extraction from statement via SP (within transaction).
    fn create_extraction_from_statement_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        ext_id: uuid::Uuid,
        stmt_id: StatementId,
        entity_id: uuid::Uuid,
        method: &str,
        confidence: f64,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Get alias by text via SP (within transaction).
    fn get_alias_by_text_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        alias_text: &str,
    ) -> impl Future<Output = Result<Option<uuid::Uuid>>> + Send;

    /// Insert a node alias (within transaction).
    fn create_alias_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        alias_id: AliasId,
        node_id: NodeId,
        alias_text: &str,
        chunk_id: ChunkId,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Insert a new node (within transaction).
    fn create_node_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        node: &Node,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Find chunk content for entity via SP.
    fn get_chunk_content_for_entity(
        &self,
        node_id: NodeId,
        def_pattern: &str,
    ) -> impl Future<Output = Result<Option<String>>> + Send;

    /// Find chunk by source and pattern via SP.
    fn get_chunk_by_source_pattern(
        &self,
        source_id: SourceId,
        def_pattern: &str,
    ) -> impl Future<Output = Result<Option<String>>> + Send;

    /// Update node semantic summary via SP.
    fn update_node_semantic_summary(
        &self,
        node_id: NodeId,
        summary: &str,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Update node processing metadata.
    fn update_node_processing(
        &self,
        node_id: NodeId,
        metadata: &serde_json::Value,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Update chunk processing metadata via SP.
    fn update_chunk_processing(
        &self,
        chunk_id: uuid::Uuid,
        stage: &str,
        metadata: &serde_json::Value,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Get entity summaries by source via SP.
    fn get_entity_summaries_by_source(
        &self,
        source_id: SourceId,
    ) -> impl Future<Output = Result<Vec<(String, String, String)>>> + Send;

    /// Update source processing metadata via SP.
    fn update_source_processing(
        &self,
        source_id: SourceId,
        stage: &str,
        metadata: &serde_json::Value,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Update source status via SP.
    fn update_source_status(
        &self,
        source_id: SourceId,
        status: &str,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Conditional source status update via SP.
    fn update_source_status_conditional(
        &self,
        source_id: SourceId,
        new_status: &str,
        unless_status: &str,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Check if node has embedding.
    fn node_has_embedding(&self, node_id: uuid::Uuid) -> impl Future<Output = Result<bool>> + Send;

    /// List node IDs with embeddings.
    fn list_node_ids_with_embeddings(
        &self,
        node_ids: &[uuid::Uuid],
    ) -> impl Future<Output = Result<Vec<uuid::Uuid>>> + Send;

    /// List chunks for source by ID only.
    fn list_chunk_ids_by_source(
        &self,
        source_id: SourceId,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid,)>>> + Send;

    /// Update source supersession metadata.
    fn update_source_supersession_metadata(
        &self,
        source_id: SourceId,
        metadata: &serde_json::Value,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Mark source as superseded.
    fn mark_source_superseded(
        &self,
        old_id: SourceId,
        new_id: SourceId,
        update_class: &str,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Detect source update by URI lookup.
    fn find_source_by_uri(
        &self,
        uri: &str,
    ) -> impl Future<Output = Result<Option<(SourceId, Option<String>, i32)>>> + Send;

    /// Delete source cascade via SP.
    fn delete_source_cascade(
        &self,
        source_id: SourceId,
    ) -> impl Future<Output = Result<(i64, i64, i64, i64, i64, i64, i64)>> + Send;

    /// Find extraction-linked chunk content with definition pattern.
    fn find_extraction_chunk_content(
        &self,
        node_id: NodeId,
        def_pattern: &str,
    ) -> impl Future<Output = Result<Option<String>>> + Send;

    /// Find chunk content from source by pattern.
    fn find_source_chunk_content(
        &self,
        source_id: SourceId,
        def_pattern: &str,
    ) -> impl Future<Output = Result<Option<String>>> + Send;

    /// Update node with semantic summary in properties and description.
    fn update_node_summary_inline(
        &self,
        node_id: NodeId,
        summary_json: &serde_json::Value,
        description: &str,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Get entity summaries for code source summary composition.
    fn get_code_entity_summaries(
        &self,
        source_id: SourceId,
        code_entity_class: &str,
    ) -> impl Future<Output = Result<Vec<(String, String, String)>>> + Send;
}

/// Repository for queue fan-in and watchdog queries.
pub trait QueueRepo: Send + Sync {
    /// Get chunk IDs for a source via SP.
    fn get_chunks_by_source(
        &self,
        source_id: SourceId,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid,)>>> + Send;

    /// Find all unsummarized code entities (with their source IDs).
    fn list_unsummarized_entities_for_summary(
        &self,
        code_entity_class: &str,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid, uuid::Uuid)>>> + Send;

    /// Find code sources needing composition.
    fn list_sources_needing_compose(
        &self,
        code_domain: &str,
        code_entity_class: &str,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid,)>>> + Send;

    /// Recover orphaned running jobs.
    fn recover_orphaned_jobs(&self) -> impl Future<Output = Result<u64>> + Send;

    /// Find stalled sources for watchdog.
    fn list_stalled_sources(
        &self,
        code_domain: &str,
        code_entity_class: &str,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid,)>>> + Send;

    /// Insert a retry job directly (for watchdog and fan-in).
    fn insert_retry_job_direct(
        &self,
        kind: &str,
        payload: &serde_json::Value,
        key: &str,
        max_attempts: i32,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Insert a retry job within a transaction.
    fn insert_retry_job_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        kind: &str,
        payload: &serde_json::Value,
        key: &str,
        max_attempts: i32,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Try advisory lock within transaction.
    fn try_advisory_xact_lock(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        key: i64,
    ) -> impl Future<Output = Result<bool>> + Send;

    /// Count pending jobs for source via SP (within transaction).
    fn count_pending_jobs_for_source_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        kind: &str,
        source_id: &str,
    ) -> impl Future<Output = Result<i64>> + Send;

    /// Count failed jobs for source via SP (within transaction).
    fn count_failed_jobs_for_source_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        kind: &str,
        source_id: &str,
    ) -> impl Future<Output = Result<i64>> + Send;

    /// Update source status via SP (within transaction).
    fn update_source_status_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        source_id: uuid::Uuid,
        status: &str,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Check if source has summary via SP (within transaction).
    fn source_has_summary_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        source_id: uuid::Uuid,
    ) -> impl Future<Output = Result<bool>> + Send;

    /// Get unsummarized entities by source via SP (within transaction).
    fn get_unsummarized_entities_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        source_id: uuid::Uuid,
    ) -> impl Future<Output = Result<Vec<(uuid::Uuid,)>>> + Send;
}

/// Repository for runtime configuration (key-value config table).
pub trait ConfigRepo: Send + Sync {
    /// List all config key-value pairs.
    fn list_all_kv(&self) -> impl Future<Output = Result<Vec<(String, serde_json::Value)>>> + Send;

    /// List all config entries with metadata.
    fn list_all_entries(&self) -> impl Future<Output = Result<Vec<ConfigEntry>>> + Send;

    /// Upsert a config key-value pair.
    fn set(&self, key: &str, value: &serde_json::Value) -> impl Future<Output = Result<()>> + Send;
}

/// Repository for source adapter configuration.
pub trait AdapterRepo: Send + Sync {
    /// Find adapter by domain match.
    fn find_by_domain(
        &self,
        domain: &str,
    ) -> impl Future<Output = Result<Option<SourceAdapter>>> + Send;

    /// Find adapter by MIME type.
    fn find_by_mime(
        &self,
        mime: &str,
    ) -> impl Future<Output = Result<Option<SourceAdapter>>> + Send;

    /// List all adapters.
    fn list_all(&self) -> impl Future<Output = Result<Vec<SourceAdapter>>> + Send;

    /// Create or update an adapter.
    fn upsert(&self, adapter: &SourceAdapter) -> impl Future<Output = Result<()>> + Send;
}

/// Repository for ontology table lookups.
#[allow(clippy::type_complexity)]
pub trait OntologyRepo: Send + Sync {
    /// List ontology categories.
    fn list_categories(
        &self,
    ) -> impl Future<Output = Result<Vec<(String, String, Option<String>)>>> + Send;

    /// List active entity types.
    fn list_entity_types(
        &self,
    ) -> impl Future<Output = Result<Vec<(String, String, String, Option<String>)>>> + Send;

    /// List universal relationship types.
    fn list_rel_universals(
        &self,
    ) -> impl Future<Output = Result<Vec<(String, String, bool)>>> + Send;

    /// List active relationship types.
    fn list_rel_types(
        &self,
    ) -> impl Future<Output = Result<Vec<(String, Option<String>, String)>>> + Send;

    /// List domains.
    fn list_domains(&self) -> impl Future<Output = Result<Vec<(String, String, bool)>>> + Send;

    /// List view-edge mappings.
    fn list_view_edges(&self) -> impl Future<Output = Result<Vec<(String, String)>>> + Send;
}

/// Repository for ask-service graph edge lookups.
pub trait AskRepo: Send + Sync {
    /// Get outgoing edges via SP.
    fn get_outgoing_edges(
        &self,
        node_id: uuid::Uuid,
        rel_types: &[String],
        limit: i64,
    ) -> impl Future<Output = Result<Vec<(String, String)>>> + Send;

    /// Get incoming edges via SP.
    fn get_incoming_edges(
        &self,
        node_id: uuid::Uuid,
        rel_types: &[String],
        limit: i64,
    ) -> impl Future<Output = Result<Vec<(String, String)>>> + Send;
}

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
use crate::models::section::Section;
use crate::models::source::Source;
use crate::models::statement::Statement;
use crate::models::trace::{SearchFeedback, SearchTrace};
use crate::models::unresolved_entity::UnresolvedEntity;
use crate::types::ids::{
    AliasId, ArticleId, AuditLogId, ChunkId, EdgeId, ExtractionId, NodeId, SectionId, SourceId,
    StatementId,
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
    /// in the table are silently skipped.
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
    /// Edges not found in the table are silently skipped.
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

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
use crate::models::source::Source;
use crate::models::trace::{SearchFeedback, SearchTrace};
use crate::types::ids::{
    AliasId, ArticleId, AuditLogId, ChunkId, EdgeId, ExtractionId, NodeId, SourceId,
};

/// Repository for [`Source`] entities.
pub trait SourceRepo: Send + Sync {
    /// Insert a new source.
    fn create(&self, source: &Source) -> impl Future<Output = Result<()>> + Send;

    /// Get a source by ID.
    fn get(&self, id: SourceId) -> impl Future<Output = Result<Option<Source>>> + Send;

    /// Get a source by content hash (for dedup).
    fn get_by_hash(&self, hash: &[u8]) -> impl Future<Output = Result<Option<Source>>> + Send;

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
        embedding: &[f32],
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

    /// Update landscape analysis results for a chunk.
    fn update_landscape(
        &self,
        id: ChunkId,
        parent_alignment: Option<f64>,
        extraction_method: &str,
        landscape_metrics: Option<serde_json::Value>,
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

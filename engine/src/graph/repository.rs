//! GraphRepository trait — the AGE abstraction layer (SPEC §4.4).
//!
//! All graph queries are isolated behind this trait. No AGE-specific syntax,
//! agtype handling, or Cypher strings leak past the repository implementation.
//! This enables migration to SQL/PGQ when it lands in PG19+.

use async_trait::async_trait;
use uuid::Uuid;

use crate::models::{
    Edge, EdgeType, GraphNeighbor, Node, NodeType, ProvenanceLink, TraversalDirection,
};

/// Errors from graph operations.
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("node not found: {0}")]
    NodeNotFound(Uuid),

    #[error("edge not found: {0}")]
    EdgeNotFound(Uuid),

    #[error("graph query failed: {0}")]
    QueryFailed(String),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("AGE error: {0}")]
    Age(String),
}

pub type GraphResult<T> = Result<T, GraphError>;

/// Abstraction over the graph storage backend.
///
/// v0: `AgeGraphRepository` (Apache AGE / openCypher)
/// Future: `SqlPgqGraphRepository` (SQL/PGQ in PG19+)
#[async_trait]
pub trait GraphRepository: Send + Sync {
    // ── Vertex operations ───────────────────────────────────────

    /// Create a vertex in the graph. Returns the AGE-internal vertex ID.
    /// Also sets `nodes.age_id` in the relational table.
    async fn create_vertex(
        &self,
        node_id: Uuid,
        node_type: NodeType,
        properties: serde_json::Value,
    ) -> GraphResult<i64>;

    /// Delete a vertex and all its edges from the graph.
    async fn delete_vertex(&self, node_id: Uuid) -> GraphResult<()>;

    // ── Edge operations ─────────────────────────────────────────

    /// Create a typed edge between two nodes.
    /// Writes to both AGE graph AND `covalence.edges` mirror in one transaction.
    async fn create_edge(
        &self,
        from_id: Uuid,
        to_id: Uuid,
        edge_type: EdgeType,
        confidence: f32,
        created_by: &str,
        properties: serde_json::Value,
    ) -> GraphResult<Edge>;

    /// Delete an edge from both AGE and the SQL mirror.
    async fn delete_edge(&self, edge_id: Uuid) -> GraphResult<()>;

    /// List edges from/to a node, optionally filtered by edge type.
    async fn list_edges(
        &self,
        node_id: Uuid,
        direction: TraversalDirection,
        edge_types: Option<&[EdgeType]>,
        limit: usize,
    ) -> GraphResult<Vec<Edge>>;

    // ── Traversal operations ────────────────────────────────────

    /// BFS neighborhood traversal from a node.
    /// Returns nodes discovered within `depth` hops, filtered by edge labels.
    async fn find_neighbors(
        &self,
        node_id: Uuid,
        edge_types: Option<&[EdgeType]>,
        direction: TraversalDirection,
        depth: u32,
        limit: usize,
    ) -> GraphResult<Vec<GraphNeighbor>>;

    /// Walk the provenance chain for an article.
    /// Follows ORIGINATES, CONFIRMS, SUPERSEDES, DERIVES_FROM, MERGED_FROM,
    /// SPLIT_FROM edges up to `max_depth` hops.
    async fn get_provenance_chain(
        &self,
        article_id: Uuid,
        max_depth: u32,
    ) -> GraphResult<Vec<ProvenanceLink>>;

    /// Find CONTRADICTS/CONTENDS edges involving a node.
    #[allow(dead_code)]
    async fn find_contradictions(&self, node_id: Uuid) -> GraphResult<Vec<Edge>>;

    /// Get chain tips: active articles with no incoming SUPERSEDES edge.
    /// Uses the SQL fast path (covalence.get_chain_tips function).
    #[allow(dead_code)]
    async fn get_chain_tips(&self, limit: usize) -> GraphResult<Vec<Node>>;

    // ── Bulk / utility ──────────────────────────────────────────

    /// Count edges in the AGE graph (for migration validation).
    async fn count_edges(&self) -> GraphResult<i64>;

    /// Count vertices in the AGE graph.
    #[allow(dead_code)]
    async fn count_vertices(&self) -> GraphResult<i64>;

    /// Verify AGE graph and SQL edges mirror are in sync.
    /// Returns (age_count, sql_count).
    #[allow(dead_code)]
    async fn verify_sync(&self) -> GraphResult<(i64, i64)>;

    // ── Archive / sync helpers ──────────────────────────────────────────

    /// Remove a vertex and its incident edges from AGE only.
    /// The SQL `covalence.edges` mirror is intentionally preserved — historical
    /// provenance data must outlive the live graph representation.
    /// Call this when archiving nodes (not hard-deleting them).
    async fn archive_vertex(&self, node_id: Uuid) -> GraphResult<()>;

    /// List all edges present in the AGE graph as `(age_internal_id, sql_edge_uuid)`.
    /// Edges that were written directly to AGE without a SQL counterpart will
    /// have `None` for the UUID (these are "orphaned" AGE edges).
    async fn list_age_edge_refs(&self) -> GraphResult<Vec<(i64, Option<Uuid>)>>;

    /// Delete a single edge from the AGE graph by its internal AGE ID.
    /// Does NOT touch `covalence.edges`. Used by the sync endpoint to prune
    /// orphaned AGE edges.
    async fn delete_age_edge_by_internal_id(&self, age_internal_id: i64) -> GraphResult<()>;

    /// For a SQL edge that has no AGE counterpart (`age_id IS NULL`), create the
    /// edge in AGE and return the new AGE internal ID.  Used by the sync
    /// endpoint to repair gaps.  Returns `None` if either node vertex is absent
    /// in AGE (the SQL edge is retained for history regardless).
    async fn create_age_edge_for_sql(
        &self,
        edge_id: Uuid,
        from_id: Uuid,
        to_id: Uuid,
        edge_type: EdgeType,
        confidence: f32,
    ) -> GraphResult<Option<i64>>;
}

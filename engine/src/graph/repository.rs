//! GraphRepository trait — SQL persistence + petgraph in-memory compute (SPEC §4.4).
//!
//! All graph queries are isolated behind this trait. No vendor-specific syntax
//! leaks past the repository implementation.
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
}

pub type GraphResult<T> = Result<T, GraphError>;

/// Abstraction over the graph storage backend.
///
/// Phase 1: `SqlGraphRepository` (pure SQL against covalence.edges)
/// Future: petgraph in-memory compute layer (Phase 2+)
#[async_trait]
pub trait GraphRepository: Send + Sync {
    // ── Vertex operations ───────────────────────────────────────

    /// Create a vertex in the graph. Returns 0 (no internal ID in SQL-only impl).
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
    /// Writes to `covalence.edges` with `valid_from = now()` and `valid_to = NULL`.
    async fn create_edge(
        &self,
        from_id: Uuid,
        to_id: Uuid,
        edge_type: EdgeType,
        confidence: f32,
        created_by: &str,
        properties: serde_json::Value,
    ) -> GraphResult<Edge>;

    /// Delete an edge from the SQL table.
    async fn delete_edge(&self, edge_id: Uuid) -> GraphResult<()>;

    /// Supersede an edge by setting `valid_to = now()` instead of deleting it.
    ///
    /// The edge record is preserved for historical queries.  Only the currently
    /// active instance of the edge (`valid_to IS NULL`) is closed off.
    /// Returns [`GraphError::EdgeNotFound`] if no active edge with `edge_id` exists.
    async fn supersede_edge(&self, edge_id: Uuid) -> GraphResult<()>;

    /// List edges from/to a node, optionally filtered by edge type.
    ///
    /// * `include_superseded = false` (default) — only active edges (`valid_to IS NULL`).
    /// * `include_superseded = true` — all edges, including superseded ones.
    async fn list_edges(
        &self,
        node_id: Uuid,
        direction: TraversalDirection,
        edge_types: Option<&[EdgeType]>,
        limit: usize,
        include_superseded: bool,
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

    /// Count edges in the SQL table.
    async fn count_edges(&self) -> GraphResult<i64>;

    /// Count vertices in the SQL table.
    #[allow(dead_code)]
    async fn count_vertices(&self) -> GraphResult<i64>;

    /// Verify SQL edges table is consistent.
    /// Returns (sql_count, sql_count).
    #[allow(dead_code)]
    async fn verify_sync(&self) -> GraphResult<(i64, i64)>;

    // ── Archive / sync helpers ──────────────────────────────────────────
    //
    // NOTE: The four methods below are deprecated stub methods retained for
    // trait stability (rollback safety). They will be removed in Phase 2.

    /// Remove a vertex and its incident edges from the graph.
    /// The SQL `covalence.edges` table is intentionally preserved — historical
    /// provenance data must outlive the live graph representation.
    /// Call this when archiving nodes (not hard-deleting them).
    ///
    /// **No-op in `SqlGraphRepository`** — edges are preserved as history.
    #[deprecated(
        note = "Stub method retained for trait stability. Will be removed in Phase 2."
    )]
    async fn archive_vertex(&self, node_id: Uuid) -> GraphResult<()>;

    /// List all edge references (deprecated stub).
    /// **Returns empty vec in `SqlGraphRepository`** — no separate graph backend exists.
    #[deprecated(
        note = "Stub method retained for trait stability. Will be removed in Phase 2."
    )]
    async fn list_age_edge_refs(&self) -> GraphResult<Vec<(i64, Option<Uuid>)>>;

    /// Delete edge by internal ID (deprecated stub).
    /// **No-op in `SqlGraphRepository`** — no separate graph backend exists.
    #[deprecated(
        note = "Stub method retained for trait stability. Will be removed in Phase 2."
    )]
    async fn delete_age_edge_by_internal_id(&self, age_internal_id: i64) -> GraphResult<()>;

    /// Create edge for SQL (deprecated stub).
    /// **Returns `Ok(None)` in `SqlGraphRepository`** — no separate graph backend exists.
    #[deprecated(
        note = "Stub method retained for trait stability. Will be removed in Phase 2."
    )]
    async fn create_age_edge_for_sql(
        &self,
        edge_id: Uuid,
        from_id: Uuid,
        to_id: Uuid,
        edge_type: EdgeType,
        confidence: f32,
    ) -> GraphResult<Option<i64>>;
}

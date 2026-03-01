use async_trait::async_trait;
use uuid::Uuid;
use crate::models::{Node, Edge, NodeType, EdgeType};

/// Core abstraction over the graph backend.
/// AGE today, SQL/PGQ tomorrow — nothing above this trait knows the difference.
#[async_trait]
pub trait GraphRepository: Send + Sync {
    /// Get chain tips: active article nodes with no incoming SUPERSEDES edge.
    async fn chain_tips(&self, limit: usize) -> anyhow::Result<Vec<Node>>;

    /// Get a node's immediate neighborhood (1-hop).
    async fn neighborhood(
        &self,
        node_id: Uuid,
        edge_types: Option<&[EdgeType]>,
        depth: usize,
    ) -> anyhow::Result<(Vec<Node>, Vec<Edge>)>;

    /// Create an edge between two nodes.
    async fn create_edge(
        &self,
        source_id: Uuid,
        target_id: Uuid,
        edge_type: EdgeType,
        weight: f32,
        confidence: f32,
    ) -> anyhow::Result<Edge>;

    /// Find nodes connected by a path.
    async fn traverse(
        &self,
        start_id: Uuid,
        edge_types: &[EdgeType],
        max_depth: usize,
    ) -> anyhow::Result<Vec<Node>>;
}

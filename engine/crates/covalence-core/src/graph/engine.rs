//! `GraphEngine` trait — abstract interface for graph operations.
//!
//! All graph traversal, algorithms, and mutations go through this trait.
//! Implementations:
//! - `PetgraphEngine` — in-memory petgraph sidecar (current)
//! - `AgeEngine` — Apache AGE queries against PostgreSQL (future)

use std::collections::HashMap;

use uuid::Uuid;

use crate::error::Result;
use crate::graph::community::Community;
use crate::graph::sidecar::NodeMeta;
use crate::graph::topology::TopologyMap;

/// A neighbor node with its edge metadata.
#[derive(Debug, Clone)]
pub struct Neighbor {
    /// The neighbor node's UUID.
    pub id: Uuid,
    /// Relationship type of the connecting edge.
    pub rel_type: String,
    /// Whether the edge is synthetic (co-occurrence).
    pub is_synthetic: bool,
    /// Edge confidence.
    pub confidence: f64,
    /// Edge weight.
    pub weight: f64,
    /// The neighbor's canonical name.
    pub name: String,
    /// The neighbor's node type.
    pub node_type: String,
}

/// A node discovered by BFS with its hop distance.
#[derive(Debug, Clone)]
pub struct BfsNode {
    /// Node UUID.
    pub id: Uuid,
    /// Hop distance from the start node.
    pub hops: usize,
    /// Node's canonical name.
    pub name: String,
    /// Node type.
    pub node_type: String,
}

/// Summary statistics for the graph.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphStats {
    /// Total active nodes.
    pub node_count: usize,
    /// Total active edges.
    pub edge_count: usize,
    /// Semantic (non-synthetic) edges.
    pub semantic_edge_count: usize,
    /// Synthetic (co-occurrence) edges.
    pub synthetic_edge_count: usize,
    /// Graph density (edges / possible edges).
    pub density: f64,
    /// Number of weakly connected components.
    pub component_count: usize,
}

/// Options for BFS traversal.
#[derive(Debug, Clone, Default)]
pub struct BfsOptions {
    /// Maximum hops from the start node.
    pub max_hops: usize,
    /// If true, skip synthetic edges during traversal.
    pub skip_synthetic: bool,
    /// Relationship types to exclude from traversal.
    pub deny_rel_types: Vec<String>,
}

/// Abstract graph engine for traversal, algorithms, and mutations.
///
/// All methods are async to support both in-memory (petgraph) and
/// database-backed (Apache AGE) implementations.
#[async_trait::async_trait]
pub trait GraphEngine: Send + Sync {
    // ----- Stats -----

    /// Graph summary statistics.
    async fn stats(&self) -> Result<GraphStats>;

    /// Number of nodes.
    async fn node_count(&self) -> Result<usize>;

    /// Number of active edges.
    async fn edge_count(&self) -> Result<usize>;

    // ----- Node access -----

    /// Get a node's metadata by UUID.
    async fn get_node(&self, id: Uuid) -> Result<Option<NodeMeta>>;

    /// Get outgoing neighbors of a node.
    async fn neighbors_out(&self, id: Uuid) -> Result<Vec<Neighbor>>;

    /// Get incoming neighbors of a node.
    async fn neighbors_in(&self, id: Uuid) -> Result<Vec<Neighbor>>;

    /// In-degree of a node.
    async fn degree_in(&self, id: Uuid) -> Result<usize>;

    /// Out-degree of a node.
    async fn degree_out(&self, id: Uuid) -> Result<usize>;

    // ----- Traversal -----

    /// BFS neighborhood discovery from a start node.
    async fn bfs_neighborhood(&self, start: Uuid, options: BfsOptions) -> Result<Vec<BfsNode>>;

    /// Shortest path between two nodes. Returns the path as node UUIDs,
    /// or None if no path exists.
    async fn shortest_path(&self, from: Uuid, to: Uuid) -> Result<Option<Vec<Uuid>>>;

    // ----- Algorithms -----

    /// PageRank scores for all nodes.
    async fn pagerank(&self, damping: f64, iterations: usize) -> Result<HashMap<Uuid, f64>>;

    /// TrustRank: biased PageRank from trusted seed nodes.
    async fn trust_rank(
        &self,
        seeds: &[(Uuid, f64)],
        damping: f64,
        iterations: usize,
    ) -> Result<HashMap<Uuid, f64>>;

    /// Structural importance (betweenness centrality approximation).
    async fn structural_importance(&self) -> Result<HashMap<Uuid, f64>>;

    /// Spreading activation from seed nodes with decay.
    async fn spreading_activation(
        &self,
        seeds: &[(Uuid, f64)],
        decay: f64,
        max_hops: usize,
    ) -> Result<HashMap<Uuid, f64>>;

    /// Community detection (k-core based).
    async fn communities(&self, min_size: usize) -> Result<Vec<Community>>;

    /// Build full topology map (communities + PageRank + bridges + landmarks).
    async fn topology(&self) -> Result<TopologyMap>;

    /// Detect contentious (contradictory) relationships.
    async fn contentions(&self) -> Result<Vec<Contention>>;

    /// Knowledge gap detection by degree imbalance.
    async fn knowledge_gaps(
        &self,
        min_in_degree: usize,
        min_label_length: usize,
        exclude_types: &[&str],
        limit: usize,
    ) -> Result<Vec<GapCandidate>>;

    // ----- Mutations -----

    /// Full reload from PostgreSQL. Replaces all in-memory state.
    async fn reload(&self, pool: &sqlx::PgPool) -> Result<ReloadResult>;
}

/// A detected contention (contradictory relationship).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Contention {
    /// Source node UUID.
    pub source_id: Uuid,
    /// Source node name.
    pub source_name: String,
    /// Relationship type.
    pub rel_type: String,
    /// Conflicting target node UUIDs.
    pub targets: Vec<(Uuid, String)>,
}

/// A knowledge gap candidate (node with degree imbalance).
#[derive(Debug, Clone, serde::Serialize)]
pub struct GapCandidate {
    /// Node UUID.
    pub id: Uuid,
    /// Canonical name.
    pub name: String,
    /// Node type.
    pub node_type: String,
    /// In-degree.
    pub in_degree: usize,
    /// Out-degree.
    pub out_degree: usize,
}

/// Result of a graph reload operation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReloadResult {
    /// Nodes loaded.
    pub node_count: usize,
    /// Edges loaded.
    pub edge_count: usize,
}

//! Graph-related DTOs.

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// Response for graph statistics.
#[derive(Debug, Serialize, ToSchema)]
pub struct GraphStatsResponse {
    pub node_count: usize,
    pub edge_count: usize,
    pub semantic_edge_count: usize,
    pub synthetic_edge_count: usize,
    pub density: f64,
    pub component_count: usize,
}

/// Query parameters for community detection.
#[derive(Debug, Deserialize, IntoParams)]
pub struct CommunityParams {
    /// Minimum community size (default: 2). Set to 1 to include
    /// single-node communities.
    pub min_size: Option<usize>,
}

/// Response for a community.
#[derive(Debug, Serialize, ToSchema)]
pub struct CommunityResponse {
    pub id: usize,
    /// Number of nodes in this community.
    pub size: usize,
    pub node_ids: Vec<Uuid>,
    pub label: Option<String>,
    pub coherence: f64,
    /// K-core level (higher = denser subgraph).
    pub core_level: usize,
}

/// Response for the domain topology map.
#[derive(Debug, Serialize, ToSchema)]
pub struct TopologyResponse {
    /// Domains in the knowledge graph.
    pub domains: Vec<DomainResponse>,
    /// Inter-domain links.
    pub links: Vec<DomainLinkResponse>,
    /// Total node count in the graph.
    pub total_nodes: usize,
    /// Total edge count in the graph.
    pub total_edges: usize,
}

/// A single domain in the topology map.
#[derive(Debug, Serialize, ToSchema)]
pub struct DomainResponse {
    /// Community identifier.
    pub community_id: usize,
    /// Optional domain label.
    pub label: Option<String>,
    /// Number of nodes in this domain.
    pub node_count: usize,
    /// Landmark node UUIDs (top-3 by structural importance).
    pub landmark_ids: Vec<Uuid>,
    /// Internal coherence score.
    pub coherence: f64,
    /// Average PageRank of nodes in this domain.
    pub avg_pagerank: f64,
}

/// An inter-domain connection.
#[derive(Debug, Serialize, ToSchema)]
pub struct DomainLinkResponse {
    /// Source domain (community ID).
    pub source_domain: usize,
    /// Target domain (community ID).
    pub target_domain: usize,
    /// Number of bridge nodes connecting these domains.
    pub bridge_count: usize,
    /// UUID of the strongest bridge node.
    pub strongest_bridge: Uuid,
}

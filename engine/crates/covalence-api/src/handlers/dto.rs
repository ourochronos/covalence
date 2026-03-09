//! Request and response DTOs for the API.

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

// --- Pagination ---

/// Common pagination query parameters.
#[derive(Debug, Deserialize, ToSchema, IntoParams)]
pub struct PaginationParams {
    /// Maximum number of results (default 20).
    pub limit: Option<i64>,
    /// Offset for pagination (default 0).
    pub offset: Option<i64>,
}

impl PaginationParams {
    /// Get limit with default.
    pub fn limit(&self) -> i64 {
        self.limit.unwrap_or(20)
    }

    /// Get offset with default.
    pub fn offset(&self) -> i64 {
        self.offset.unwrap_or(0)
    }
}

// --- Sources ---

/// Request body for source ingestion.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateSourceRequest {
    /// Base64-encoded content bytes.
    pub content: String,
    /// Source type (document, web_page, conversation, code, api, manual,
    /// tool_output, observation).
    pub source_type: String,
    /// MIME type of the content (e.g. "text/markdown", "text/plain").
    /// Defaults to "text/plain" if omitted.
    pub mime: Option<String>,
    /// Optional URI of the original material.
    pub uri: Option<String>,
    /// Optional metadata.
    pub metadata: Option<serde_json::Value>,
}

/// Response after successful source creation.
#[derive(Debug, Serialize, ToSchema)]
pub struct CreateSourceResponse {
    /// ID of the created (or deduplicated) source.
    pub id: Uuid,
}

/// Response for a source entity.
#[derive(Debug, Serialize, ToSchema)]
pub struct SourceResponse {
    pub id: Uuid,
    pub source_type: String,
    pub uri: Option<String>,
    pub title: Option<String>,
    pub author: Option<String>,
    pub ingested_at: String,
    pub reliability_score: f64,
    pub clearance_level: i32,
    pub content_version: i32,
}

/// Response for source deletion.
#[derive(Debug, Serialize, ToSchema)]
pub struct DeleteSourceResponse {
    pub deleted: bool,
    pub chunks_deleted: u64,
}

/// Response for a chunk entity.
#[derive(Debug, Serialize, ToSchema)]
pub struct ChunkResponse {
    pub id: Uuid,
    pub source_id: Uuid,
    pub level: String,
    pub ordinal: i32,
    pub content: String,
    pub token_count: i32,
}

// --- Search ---

/// Request body for search.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SearchRequest {
    /// The search query text.
    pub query: String,
    /// Search strategy (balanced, precise, exploratory, recent,
    /// graph_first).
    pub strategy: Option<String>,
    /// Maximum number of results.
    pub limit: Option<usize>,
    /// Minimum epistemic confidence threshold (0.0–1.0).
    pub min_confidence: Option<f64>,
    /// Restrict to specific node types.
    pub node_types: Option<Vec<String>>,
    /// Start of date range filter (ISO 8601).
    pub date_range_start: Option<String>,
    /// End of date range filter (ISO 8601).
    pub date_range_end: Option<String>,
}

/// A single fused search result.
#[derive(Debug, Serialize, ToSchema)]
pub struct SearchResultResponse {
    /// Entity ID.
    pub id: Uuid,
    /// Fused RRF score.
    pub fused_score: f64,
    /// Epistemic confidence (projected probability).
    pub confidence: Option<f64>,
    /// Entity type (e.g. "node").
    pub entity_type: Option<String>,
    /// Canonical name of the entity.
    pub name: Option<String>,
    /// Best available text snippet.
    pub snippet: Option<String>,
    /// Source URI (for chunk results).
    pub source_uri: Option<String>,
    /// Per-dimension scores.
    pub dimension_scores: std::collections::HashMap<String, f64>,
    /// Per-dimension ranks.
    pub dimension_ranks: std::collections::HashMap<String, usize>,
}

// --- Nodes ---

/// Response for a node entity.
#[derive(Debug, Serialize, ToSchema)]
pub struct NodeResponse {
    pub id: Uuid,
    pub canonical_name: String,
    pub node_type: String,
    pub description: Option<String>,
    pub properties: serde_json::Value,
    pub clearance_level: i32,
    pub first_seen: String,
    pub last_seen: String,
    pub mention_count: i32,
}

/// Query parameters for neighborhood.
#[derive(Debug, Deserialize, ToSchema, IntoParams)]
pub struct NeighborhoodParams {
    /// Number of hops (default 1).
    pub hops: Option<usize>,
}

/// Request body for node resolution.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ResolveNodeRequest {
    /// Name to resolve.
    pub name: String,
}

/// Request body for merging nodes.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MergeNodesRequest {
    /// IDs of source nodes to merge from.
    pub source_ids: Vec<Uuid>,
    /// ID of the target node to merge into.
    pub target_id: Uuid,
}

/// Response after merge.
#[derive(Debug, Serialize, ToSchema)]
pub struct MergeNodesResponse {
    /// Audit log entry ID for the merge operation.
    pub audit_log_id: Uuid,
}

/// Request for node split.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SplitNodeRequest {
    /// Specifications for each new node.
    pub specs: Vec<SplitSpecRequest>,
}

/// A single split target specification.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SplitSpecRequest {
    /// Name for the new node.
    pub name: String,
    /// Type for the new node.
    pub node_type: String,
    /// Optional description.
    pub description: Option<String>,
    /// Edge IDs to reassign to this new node.
    pub edge_ids: Vec<Uuid>,
}

/// Response after split.
#[derive(Debug, Serialize, ToSchema)]
pub struct SplitNodeResponse {
    /// IDs of the newly created nodes.
    pub node_ids: Vec<Uuid>,
}

/// Provenance chain response.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProvenanceResponse {
    /// Node ID.
    pub node_id: Uuid,
    /// Number of extraction records.
    pub extraction_count: usize,
    /// Number of source chunks.
    pub chunk_count: usize,
    /// Number of originating sources.
    pub source_count: usize,
}

// --- Edges ---

/// Response for an edge entity.
#[derive(Debug, Serialize, ToSchema)]
pub struct EdgeResponse {
    pub id: Uuid,
    pub source_node_id: Uuid,
    pub target_node_id: Uuid,
    pub rel_type: String,
    pub weight: f64,
    pub confidence: f64,
    pub clearance_level: i32,
    pub created_at: String,
}

// --- Graph ---

/// Response for graph statistics.
#[derive(Debug, Serialize, ToSchema)]
pub struct GraphStatsResponse {
    pub node_count: usize,
    pub edge_count: usize,
    pub density: f64,
    pub component_count: usize,
}

/// Response for a community.
#[derive(Debug, Serialize, ToSchema)]
pub struct CommunityResponse {
    pub id: usize,
    pub node_ids: Vec<Uuid>,
    pub label: Option<String>,
    pub coherence: f64,
}

// --- Topology ---

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

// --- Admin ---

/// Response for graph reload.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReloadResponse {
    pub node_count: usize,
    pub edge_count: usize,
    pub density: f64,
    pub component_count: usize,
}

/// Response for publish operation.
#[derive(Debug, Serialize, ToSchema)]
pub struct PublishResponse {
    pub published: bool,
}

/// Response for consolidation trigger.
#[derive(Debug, Serialize, ToSchema)]
pub struct ConsolidateResponse {
    pub triggered: bool,
}

/// Health check response.
#[derive(Debug, Serialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
    pub version: String,
}

/// Metrics response.
#[derive(Debug, Serialize, ToSchema)]
pub struct MetricsResponse {
    pub graph_nodes: usize,
    pub graph_edges: usize,
    pub source_count: i64,
}

// --- Audit ---

/// Response for an audit log entry.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuditLogResponse {
    pub id: Uuid,
    pub action: String,
    pub actor: String,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    pub created_at: String,
}

//! Node-related DTOs.

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// Response for a node entity.
#[derive(Debug, Serialize, ToSchema)]
pub struct NodeResponse {
    pub id: Uuid,
    pub canonical_name: String,
    pub node_type: String,
    /// Entity class: code, domain, actor, analysis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_class: Option<String>,
    pub description: Option<String>,
    pub properties: serde_json::Value,
    pub clearance_level: i32,
    pub first_seen: String,
    pub last_seen: String,
    pub mention_count: i32,
    /// Shannon entropy of domain distribution (0 = single domain, higher = cross-cutting).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain_entropy: Option<f32>,
    /// The domain where this entity is most frequently mentioned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_domain: Option<String>,
}

/// Query parameters for `GET /nodes/:id`.
#[derive(Debug, Deserialize, ToSchema, IntoParams)]
pub struct GetNodeParams {
    /// When true, include confidence explanation breakdown.
    pub explain: Option<bool>,
}

/// Epistemic confidence explanation for a node.
///
/// Breaks down the Subjective Logic opinion tuple and
/// provenance statistics that contribute to the node's
/// confidence score.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NodeExplanation {
    /// Degree of positive evidence.
    pub belief: f64,
    /// Degree of negative evidence.
    pub disbelief: f64,
    /// Degree of ignorance.
    pub uncertainty: f64,
    /// Prior probability absent evidence.
    pub base_rate: f64,
    /// Projected probability: `belief + base_rate * uncertainty`.
    pub projected_probability: f64,
    /// Number of distinct sources contributing to this node.
    pub source_count: usize,
    /// Number of extraction records for this node.
    pub extraction_count: usize,
}

/// Detailed node response with optional confidence explanation.
#[derive(Debug, Serialize, ToSchema)]
pub struct NodeDetailResponse {
    /// Core node fields.
    #[serde(flatten)]
    pub node: NodeResponse,
    /// Confidence explanation (present when `?explain=true`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<NodeExplanation>,
}

/// Query parameters for neighborhood.
#[derive(Debug, Deserialize, ToSchema, IntoParams)]
pub struct NeighborhoodParams {
    /// Number of hops (default 1).
    pub hops: Option<usize>,
    /// Include nodes connected via invalidated edges (default false).
    pub include_invalidated: Option<bool>,
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

/// Request body for correcting a node.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CorrectNodeRequest {
    /// New canonical name (optional).
    pub canonical_name: Option<String>,
    /// New node type (optional).
    pub node_type: Option<String>,
    /// New description (optional).
    pub description: Option<String>,
    /// New confidence value 0.0–1.0 (optional).
    pub confidence: Option<f64>,
}

/// Request body for annotating a node.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AnnotateNodeRequest {
    /// Free-text annotation to append.
    pub text: String,
}

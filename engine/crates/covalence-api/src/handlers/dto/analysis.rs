//! Cross-domain analysis DTOs.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// Request body for alignment analysis.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AlignmentRequest {
    /// Which checks to run: "code_ahead", "spec_ahead",
    /// "design_contradicted", "stale_design". Empty = all.
    pub checks: Option<Vec<String>>,
    /// Minimum embedding similarity for matching (default 0.4).
    pub min_similarity: Option<f64>,
    /// Max items per check (default 20).
    pub limit: Option<i64>,
}

/// A single misalignment finding.
#[derive(Debug, Serialize, ToSchema)]
pub struct AlignmentItemResponse {
    /// Category: code_ahead, spec_ahead, design_contradicted, stale_design.
    pub check: String,
    /// Entity name.
    pub name: String,
    /// Entity domain.
    pub domain: String,
    /// Entity type.
    pub node_type: String,
    /// Similarity to closest match (0.0-1.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closest_match_score: Option<f64>,
    /// Closest match entity name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closest_match_name: Option<String>,
    /// Closest match domain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closest_match_domain: Option<String>,
    /// Explanation of the misalignment.
    pub reason: String,
}

impl From<covalence_core::services::analysis::AlignmentItem> for AlignmentItemResponse {
    fn from(item: covalence_core::services::analysis::AlignmentItem) -> Self {
        Self {
            check: item.check,
            name: item.name,
            domain: item.domain,
            node_type: item.node_type,
            closest_match_score: item.closest_match_score,
            closest_match_name: item.closest_match_name,
            closest_match_domain: item.closest_match_domain,
            reason: item.reason,
        }
    }
}

/// Full alignment report response.
#[derive(Debug, Serialize, ToSchema)]
pub struct AlignmentReportResponse {
    /// Code entities with no matching spec concept.
    pub code_ahead: Vec<AlignmentItemResponse>,
    /// Spec concepts with no implementing code.
    pub spec_ahead: Vec<AlignmentItemResponse>,
    /// Design decisions potentially contradicted by research.
    pub design_contradicted: Vec<AlignmentItemResponse>,
    /// Design docs diverging from code reality.
    pub stale_design: Vec<AlignmentItemResponse>,
}

/// Response for component bootstrapping.
#[derive(Debug, Serialize, ToSchema)]
pub struct BootstrapResponse {
    /// Components created as new nodes.
    pub components_created: u64,
    /// Components that already existed.
    pub components_existing: u64,
    /// Components that were embedded.
    pub components_embedded: u64,
}

/// Request body for cross-domain linking.
#[derive(Debug, Deserialize, ToSchema)]
pub struct LinkDomainsRequest {
    /// Minimum cosine similarity for semantic bridges (default 0.5).
    pub min_similarity: Option<f64>,
    /// Maximum bridge edges per component (default 100).
    pub max_edges_per_component: Option<i64>,
}

/// Response for cross-domain linking.
#[derive(Debug, Serialize, ToSchema)]
pub struct LinkDomainsResponse {
    /// PART_OF_COMPONENT edges created.
    pub part_of_edges: u64,
    /// IMPLEMENTS_INTENT edges created.
    pub intent_edges: u64,
    /// THEORETICAL_BASIS edges created.
    pub basis_edges: u64,
    /// Edges skipped (already exist).
    pub skipped_existing: u64,
}

/// A single coverage item in the response.
#[derive(Debug, Serialize, ToSchema)]
pub struct CoverageItemResponse {
    /// Node UUID.
    pub node_id: Uuid,
    /// Node name.
    pub name: String,
    /// Node type.
    pub node_type: String,
    /// File path (for code nodes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// Why this item is flagged.
    pub reason: String,
}

/// Response for coverage analysis.
#[derive(Debug, Serialize, ToSchema)]
pub struct CoverageResponse {
    /// Code nodes with no Component parent.
    pub orphan_code: Vec<CoverageItemResponse>,
    /// Spec concepts with no implementation edge.
    pub unimplemented_specs: Vec<CoverageItemResponse>,
    /// Fraction of spec topics with implementation coverage.
    pub coverage_score: f64,
}

/// Request body for erosion detection.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ErosionRequest {
    /// Drift threshold -- report components above this (default 0.3).
    pub threshold: Option<f64>,
}

/// A divergent code node in the erosion response.
#[derive(Debug, Serialize, ToSchema)]
pub struct DivergentNodeResponse {
    /// Code node UUID.
    pub node_id: Uuid,
    /// Code node name.
    pub name: String,
    /// Semantic summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Cosine distance from component embedding.
    pub distance: f64,
}

/// A single eroded component.
#[derive(Debug, Serialize, ToSchema)]
pub struct ErosionItemResponse {
    /// Component UUID.
    pub component_id: Uuid,
    /// Component name.
    pub component_name: String,
    /// Design intent description.
    pub spec_intent: String,
    /// Drift score.
    pub drift_score: f64,
    /// Most divergent code nodes.
    pub divergent_nodes: Vec<DivergentNodeResponse>,
}

/// Response for erosion detection.
#[derive(Debug, Serialize, ToSchema)]
pub struct ErosionResponse {
    /// Components with drift above the threshold.
    pub eroded_components: Vec<ErosionItemResponse>,
    /// Total components analyzed.
    pub total_components: u64,
}

/// Request body for blast-radius simulation.
#[derive(Debug, Deserialize, ToSchema)]
pub struct BlastRadiusRequest {
    /// Target node name or UUID.
    pub target: String,
    /// Maximum hops to traverse (default 2, max 10).
    pub max_hops: Option<usize>,
    /// Include nodes connected via invalidated edges (default false).
    pub include_invalidated: Option<bool>,
    /// Maximum affected nodes to return (default 50, max 500).
    /// The response sets `truncated: true` if BFS reached more nodes
    /// than this cap allows; `total_reachable` always reflects the
    /// full BFS frontier so the caller can tell what was hidden.
    pub node_limit: Option<usize>,
}

/// An affected node in the blast radius.
#[derive(Debug, Serialize, ToSchema)]
pub struct AffectedNodeResponse {
    /// Node UUID.
    pub node_id: Uuid,
    /// Node name.
    pub name: String,
    /// Node type.
    pub node_type: String,
    /// Relationship connecting to blast origin.
    pub relationship: String,
}

/// Nodes at a specific hop distance.
#[derive(Debug, Serialize, ToSchema)]
pub struct BlastRadiusHopResponse {
    /// Hop distance from target.
    pub hop_distance: usize,
    /// Nodes at this hop.
    pub nodes: Vec<AffectedNodeResponse>,
}

/// Response for blast-radius simulation.
#[derive(Debug, Serialize, ToSchema)]
pub struct BlastRadiusResponse {
    /// The target node ID.
    pub target_node_id: Uuid,
    /// The target node name.
    pub target_name: String,
    /// The target node type.
    pub target_node_type: String,
    /// Target's component (if assigned).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    /// Affected nodes by hop distance (capped by `node_limit_applied`).
    pub affected_by_hop: Vec<BlastRadiusHopResponse>,
    /// Number of affected nodes returned in `affected_by_hop`.
    pub total_affected: usize,
    /// Total nodes the BFS reached, regardless of cap. Equal to
    /// `total_affected` when no truncation happened.
    pub total_reachable: usize,
    /// True if BFS hit more nodes than the cap allowed.
    pub truncated: bool,
    /// The effective node cap applied to this request.
    pub node_limit_applied: usize,
}

/// Request body for whitespace roadmap analysis.
#[derive(Debug, Deserialize, ToSchema)]
pub struct WhitespaceRequest {
    /// Minimum entities per source to count as a gap (default 3).
    pub min_cluster_size: Option<usize>,
    /// Optional domain filter (matches source title/URI).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

/// A representative node in a whitespace gap.
#[derive(Debug, Serialize, ToSchema)]
pub struct WhitespaceNodeResponse {
    /// Node name.
    pub name: String,
    /// Node type.
    pub node_type: String,
}

/// A research cluster with no implementation bridges.
#[derive(Debug, Serialize, ToSchema)]
pub struct WhitespaceGapResponse {
    /// Source UUID.
    pub source_id: Uuid,
    /// Source title.
    pub title: String,
    /// Source URI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// Number of entities in this cluster.
    pub node_count: u64,
    /// Representative entities.
    pub representative_nodes: Vec<WhitespaceNodeResponse>,
    /// Connected Components (via bridge edges).
    pub connected_components: Vec<String>,
    /// Connected spec topics.
    pub connected_spec_topics: Vec<String>,
    /// Human-readable assessment.
    pub assessment: String,
}

/// Response for whitespace roadmap analysis.
#[derive(Debug, Serialize, ToSchema)]
pub struct WhitespaceResponse {
    /// Research gaps with no bridge edges.
    pub gaps: Vec<WhitespaceGapResponse>,
    /// Total research sources analyzed.
    pub total_research_sources: u64,
    /// Sources with zero bridge edges.
    pub unbridged_sources: u64,
    /// Fraction of research sources that are unbridged.
    pub whitespace_score: f64,
}

/// Request body for research-to-execution verification.
#[derive(Debug, Deserialize, ToSchema)]
pub struct VerifyRequest {
    /// Research topic to verify implementation of.
    pub research_query: String,
    /// Optional Component name filter.
    pub component: Option<String>,
}

/// A matched node in verification results.
#[derive(Debug, Serialize, ToSchema)]
pub struct VerificationMatchResponse {
    /// Node UUID.
    pub node_id: Uuid,
    /// Node name.
    pub name: String,
    /// Node type.
    pub node_type: String,
    /// Semantic summary or description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Cosine distance from the query.
    pub distance: f64,
    /// Domain: "research" or "code".
    pub domain: String,
}

/// Response for research-to-execution verification.
#[derive(Debug, Serialize, ToSchema)]
pub struct VerifyResponse {
    /// The research query searched for.
    pub research_query: String,
    /// Matched research-domain nodes.
    pub research_matches: Vec<VerificationMatchResponse>,
    /// Matched code-domain nodes.
    pub code_matches: Vec<VerificationMatchResponse>,
    /// Alignment score (mean cosine similarity).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alignment_score: Option<f64>,
    /// Bridging Component (if found).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
}

/// Request body for dialectical critique.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CritiqueRequest {
    /// Design proposal to critique.
    pub proposal: String,
}

/// A piece of evidence from the knowledge graph.
#[derive(Debug, Serialize, ToSchema)]
pub struct CritiqueEvidenceResponse {
    /// Node UUID.
    pub node_id: Uuid,
    /// Node name.
    pub name: String,
    /// Node type.
    pub node_type: String,
    /// Description or summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Cosine distance from the proposal embedding.
    pub distance: f64,
    /// Domain: "research", "spec", or "code".
    pub domain: String,
}

/// A counter-argument in the critique.
#[derive(Debug, Serialize, ToSchema)]
pub struct CounterArgumentResponse {
    /// The claim being made against the proposal.
    pub claim: String,
    /// Evidence supporting the counter-argument.
    pub evidence: Vec<String>,
    /// Strength: "strong", "moderate", or "weak".
    pub strength: String,
}

/// A supporting argument in the critique.
#[derive(Debug, Serialize, ToSchema)]
pub struct SupportingArgumentResponse {
    /// The claim supporting the proposal.
    pub claim: String,
    /// Evidence supporting this argument.
    pub evidence: Vec<String>,
}

/// LLM-synthesized dialectical critique.
#[derive(Debug, Serialize, ToSchema)]
pub struct CritiqueSynthesisResponse {
    /// Arguments against the proposal.
    pub counter_arguments: Vec<CounterArgumentResponse>,
    /// Arguments supporting the proposal.
    pub supporting_arguments: Vec<SupportingArgumentResponse>,
    /// Overall recommendation.
    pub recommendation: String,
}

/// Response for dialectical critique analysis.
#[derive(Debug, Serialize, ToSchema)]
pub struct CritiqueResponse {
    /// The original proposal text.
    pub proposal: String,
    /// Research-domain evidence.
    pub research_evidence: Vec<CritiqueEvidenceResponse>,
    /// Spec/design evidence.
    pub spec_evidence: Vec<CritiqueEvidenceResponse>,
    /// Code evidence.
    pub code_evidence: Vec<CritiqueEvidenceResponse>,
    /// LLM-synthesized critique (null if no chat backend available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synthesis: Option<CritiqueSynthesisResponse>,
}

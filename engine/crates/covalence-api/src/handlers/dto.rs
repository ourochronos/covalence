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
///
/// Supply either `content` (base64-encoded bytes) or `url` (fetched
/// by the server). When `url` is provided, `source_type` and `mime`
/// are auto-detected from the response if not explicitly set.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateSourceRequest {
    /// Base64-encoded content bytes. Required unless `url` is provided.
    pub content: Option<String>,
    /// URL to fetch content from. The server performs the HTTP GET,
    /// detects MIME type, auto-classifies source type from URL
    /// patterns, and extracts metadata (title, author, date) from
    /// the response.
    pub url: Option<String>,
    /// Source type (document, web_page, conversation, code, api,
    /// manual, tool_output, observation). Auto-detected from URL
    /// patterns when `url` is used and this field is omitted.
    pub source_type: Option<String>,
    /// MIME type of the content (e.g. "text/markdown", "text/plain").
    /// Auto-detected from Content-Type header when `url` is used.
    /// Defaults to "text/plain" for direct content upload.
    pub mime: Option<String>,
    /// Optional URI of the original material. When `url` is used,
    /// the URL is stored as the URI automatically.
    pub uri: Option<String>,
    /// Title for the source. Overrides auto-extracted title from
    /// HTML `<title>` or markdown `# heading`.
    pub title: Option<String>,
    /// Author of the source. Overrides auto-extracted author from
    /// HTML meta tags.
    pub author: Option<String>,
    /// Optional metadata.
    pub metadata: Option<serde_json::Value>,
    /// Original file format before conversion (e.g. "pdf", "html",
    /// "markdown", "docx"). Stored in metadata.format_origin.
    pub format_origin: Option<String>,
    /// List of authors. First entry is used as the primary author.
    /// Stored in metadata.authors.
    pub authors: Option<Vec<String>>,
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

/// Response for source reprocessing.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReprocessSourceResponse {
    /// Source ID that was reprocessed.
    pub source_id: Uuid,
    /// Number of old extractions marked as superseded.
    pub extractions_superseded: u64,
    /// Number of old chunks deleted.
    pub chunks_deleted: u64,
    /// Number of new chunks created.
    pub chunks_created: usize,
    /// New content version after reprocessing.
    pub content_version: i32,
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

/// Search delivery mode.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    /// Return individual ranked results (default).
    #[default]
    Results,
    /// Assemble results into a deduplicated, budget-trimmed
    /// context window suitable for LLM generation.
    Context,
}

/// Content granularity for search results.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchGranularity {
    /// Walk up to the parent section chunk (default). For
    /// paragraph-level chunks, the content field is replaced
    /// with the parent section's content.
    #[default]
    Section,
    /// Use the matched chunk content as-is.
    Paragraph,
    /// Use the full source `normalized_content`.
    Source,
}

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
    /// Delivery mode: `results` (default) or `context`.
    #[serde(default)]
    pub mode: SearchMode,
    /// Content granularity: `section` (default), `paragraph`,
    /// or `source`.
    #[serde(default)]
    pub granularity: SearchGranularity,
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
    /// Full content of the matched entity. For chunks this is
    /// the chunk text (or parent section / full source depending
    /// on `granularity`). For articles it is the body. For nodes
    /// it is the description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Source URI (for chunk results).
    pub source_uri: Option<String>,
    /// Per-dimension scores.
    pub dimension_scores: std::collections::HashMap<String, f64>,
    /// Per-dimension ranks.
    pub dimension_ranks: std::collections::HashMap<String, usize>,
}

/// A single item in an assembled context window.
#[derive(Debug, Serialize, ToSchema)]
pub struct ContextItemResponse {
    /// 1-indexed reference number for citation.
    pub ref_number: usize,
    /// The content text.
    pub content: String,
    /// Source title for attribution.
    pub source_title: Option<String>,
    /// Source identifier for provenance.
    pub source_id: Option<String>,
    /// Relevance score.
    pub score: f64,
    /// Token count of this item.
    pub token_count: usize,
}

/// Response for context assembly mode.
#[derive(Debug, Serialize, ToSchema)]
pub struct ContextResponse {
    /// Ordered context items with reference numbers.
    pub items: Vec<ContextItemResponse>,
    /// Total token count of assembled context.
    pub total_tokens: usize,
    /// Number of items dropped due to budget.
    pub items_dropped: usize,
    /// Number of duplicates removed.
    pub duplicates_removed: usize,
}

/// Unified search response supporting both result and context
/// delivery modes.
#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
pub enum SearchApiResponse {
    /// Standard ranked results.
    Results(Vec<SearchResultResponse>),
    /// Assembled context window.
    Context(ContextResponse),
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

// --- Knowledge Curation ---

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

/// Request body for correcting an edge.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CorrectEdgeRequest {
    /// New relationship type (optional).
    pub rel_type: Option<String>,
    /// New confidence value 0.0–1.0 (optional).
    pub confidence: Option<f64>,
}

/// Query parameter for edge deletion reason.
#[derive(Debug, Deserialize, ToSchema, IntoParams)]
pub struct DeleteEdgeParams {
    /// Reason for deleting the edge (required).
    pub reason: String,
}

/// Request body for annotating a node.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AnnotateNodeRequest {
    /// Free-text annotation to append.
    pub text: String,
}

/// Request body for search feedback.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SearchFeedbackRequest {
    /// The query text that was searched.
    pub query: String,
    /// The result entity ID being rated.
    pub result_id: Uuid,
    /// Relevance rating (0.0 to 1.0).
    pub relevance: f64,
    /// Optional free-text comment.
    pub comment: Option<String>,
}

/// Generic response for curation operations.
#[derive(Debug, Serialize, ToSchema)]
pub struct CurationResponse {
    /// Whether the operation succeeded.
    pub success: bool,
    /// The audit log entry ID for the operation.
    pub audit_log_id: Uuid,
}

/// Generic response for feedback submission.
#[derive(Debug, Serialize, ToSchema)]
pub struct FeedbackResponse {
    /// Whether the feedback was recorded.
    pub recorded: bool,
}

// --- Search Traces ---

/// Response for a search trace entry.
#[derive(Debug, Serialize, ToSchema)]
pub struct SearchTraceResponse {
    /// Trace ID.
    pub id: Uuid,
    /// The query text.
    pub query_text: String,
    /// The search strategy used.
    pub strategy: String,
    /// Per-dimension result counts.
    pub dimension_counts: serde_json::Value,
    /// Total results returned.
    pub result_count: i32,
    /// Execution time in milliseconds.
    pub execution_ms: i32,
    /// When the trace was recorded.
    pub created_at: String,
}

/// Response for a trace replay.
#[derive(Debug, Serialize, ToSchema)]
pub struct TraceReplayResponse {
    /// The trace that was replayed.
    pub trace_id: Uuid,
    /// New search results from replay.
    pub results: Vec<SearchResultResponse>,
}

// --- Ontology ---

/// Request body for ontology clustering.
#[derive(Debug, Deserialize, ToSchema)]
pub struct OntologyClusterRequest {
    /// Clustering level: `entity`, `entity_type`, `rel_type`, or
    /// null for all levels.
    pub level: Option<String>,
    /// Minimum number of labels to form a cluster (default 2).
    /// HDBSCAN finds natural density-based clusters; this
    /// controls the minimum group size.
    pub min_cluster_size: Option<usize>,
    /// If true, return clusters without writing them to the
    /// database (default true).
    pub dry_run: Option<bool>,
}

/// A single ontology cluster in the response.
#[derive(Debug, Serialize, ToSchema)]
pub struct OntologyClusterItem {
    /// Cluster ID.
    pub id: Uuid,
    /// Cluster level.
    pub level: String,
    /// Canonical (most frequent) label.
    pub canonical_label: String,
    /// All member labels in the cluster.
    pub member_labels: Vec<String>,
    /// Total mention count.
    pub member_count: usize,
}

/// Response for ontology clustering.
#[derive(Debug, Serialize, ToSchema)]
pub struct OntologyClusterResponse {
    /// Whether results were applied to the database.
    pub applied: bool,
    /// Number of clusters discovered.
    pub cluster_count: usize,
    /// The discovered clusters.
    pub clusters: Vec<OntologyClusterItem>,
    /// Labels that HDBSCAN identified as noise (unclustered).
    /// These are genuinely unique labels that don't belong to
    /// any density-based group.
    pub noise_labels: Vec<String>,
}

// --- Knowledge Gaps ---

/// Query parameters for knowledge gap detection.
#[derive(Debug, Deserialize, ToSchema, IntoParams)]
pub struct KnowledgeGapParams {
    /// Minimum in-degree to qualify as a gap (default 3).
    pub min_in_degree: Option<usize>,
    /// Minimum label length to filter noise (default 4).
    pub min_label_length: Option<usize>,
    /// Comma-separated node types to exclude.
    pub exclude_types: Option<String>,
    /// Maximum number of gaps to return (default 20).
    pub limit: Option<usize>,
}

/// A single knowledge gap entity.
#[derive(Debug, Serialize, ToSchema)]
pub struct KnowledgeGapItem {
    /// Node UUID.
    pub node_id: Uuid,
    /// Canonical entity name.
    pub canonical_name: String,
    /// Entity type.
    pub node_type: String,
    /// Number of incoming references.
    pub in_degree: usize,
    /// Number of outgoing edges.
    pub out_degree: usize,
    /// Gap score (in_degree - out_degree).
    pub gap_score: f64,
    /// Source URIs that reference this entity.
    pub referenced_by: Vec<String>,
}

/// Response for knowledge gap detection.
#[derive(Debug, Serialize, ToSchema)]
pub struct KnowledgeGapsResponse {
    /// Number of gaps found.
    pub gap_count: usize,
    /// The knowledge gaps, sorted by gap score descending.
    pub gaps: Vec<KnowledgeGapItem>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_mode_defaults_to_results() {
        let mode = SearchMode::default();
        assert!(matches!(mode, SearchMode::Results));
    }

    #[test]
    fn search_granularity_defaults_to_section() {
        let gran = SearchGranularity::default();
        assert!(matches!(gran, SearchGranularity::Section));
    }

    #[test]
    fn search_mode_serde_roundtrip() {
        let json = serde_json::to_string(&SearchMode::Context).unwrap();
        assert_eq!(json, "\"context\"");
        let mode: SearchMode = serde_json::from_str(&json).unwrap();
        assert!(matches!(mode, SearchMode::Context));
    }

    #[test]
    fn search_granularity_serde_roundtrip() {
        let json = serde_json::to_string(&SearchGranularity::Source).unwrap();
        assert_eq!(json, "\"source\"");
        let gran: SearchGranularity = serde_json::from_str(&json).unwrap();
        assert!(matches!(gran, SearchGranularity::Source));
    }

    #[test]
    fn search_request_defaults_mode_and_granularity() {
        // When mode and granularity are omitted from JSON, they
        // should default to Results and Section respectively.
        let json = serde_json::json!({
            "query": "test query"
        });
        let req: SearchRequest = serde_json::from_value(json).unwrap();
        assert!(matches!(req.mode, SearchMode::Results));
        assert!(matches!(req.granularity, SearchGranularity::Section));
    }

    #[test]
    fn search_request_with_mode_and_granularity() {
        let json = serde_json::json!({
            "query": "test query",
            "mode": "context",
            "granularity": "paragraph"
        });
        let req: SearchRequest = serde_json::from_value(json).unwrap();
        assert!(matches!(req.mode, SearchMode::Context));
        assert!(matches!(req.granularity, SearchGranularity::Paragraph));
    }

    #[test]
    fn search_result_response_omits_null_content() {
        let resp = SearchResultResponse {
            id: uuid::Uuid::new_v4(),
            fused_score: 0.5,
            confidence: None,
            entity_type: None,
            name: None,
            snippet: None,
            content: None,
            source_uri: None,
            dimension_scores: std::collections::HashMap::new(),
            dimension_ranks: std::collections::HashMap::new(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        // content should be omitted when None due to
        // skip_serializing_if.
        assert!(json.get("content").is_none());
    }

    #[test]
    fn search_result_response_includes_content_when_present() {
        let resp = SearchResultResponse {
            id: uuid::Uuid::new_v4(),
            fused_score: 0.5,
            confidence: None,
            entity_type: None,
            name: None,
            snippet: None,
            content: Some("full content here".to_string()),
            source_uri: None,
            dimension_scores: std::collections::HashMap::new(),
            dimension_ranks: std::collections::HashMap::new(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["content"], "full content here");
    }
}

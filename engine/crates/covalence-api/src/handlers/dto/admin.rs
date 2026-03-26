//! Admin, health, metrics, queue, ontology, and knowledge-gap DTOs.

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// Response for graph reload.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReloadResponse {
    pub node_count: usize,
    pub edge_count: usize,
    pub semantic_edge_count: usize,
    pub synthetic_edge_count: usize,
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

/// Response for RAPTOR recursive summarization.
#[derive(Debug, Serialize, ToSchema)]
pub struct RaptorResponse {
    /// Number of sources processed.
    pub sources_processed: usize,
    /// Number of sources skipped.
    pub sources_skipped: usize,
    /// Total summary chunks created.
    pub summaries_created: usize,
    /// Number of LLM calls made.
    pub llm_calls: usize,
    /// Number of embedding calls made.
    pub embed_calls: usize,
    /// Errors encountered (if any).
    pub errors: Vec<String>,
}

/// Response for garbage collection.
#[derive(Debug, Serialize, ToSchema)]
pub struct GcResponse {
    /// Number of ungrounded nodes evicted.
    pub nodes_evicted: u64,
    /// Number of edges removed (from evicted nodes).
    pub edges_removed: u64,
    /// Number of aliases removed (from evicted nodes).
    pub aliases_removed: u64,
}

/// Request body for noise entity cleanup.
#[derive(Debug, Deserialize, ToSchema)]
pub struct NoiseCleanupRequest {
    /// If true (default), only report what would be deleted.
    pub dry_run: Option<bool>,
}

/// A noise entity identified for cleanup.
#[derive(Debug, Serialize, ToSchema)]
pub struct NoiseEntityItem {
    /// Node UUID.
    pub node_id: uuid::Uuid,
    /// Canonical entity name.
    pub canonical_name: String,
    /// Entity type.
    pub node_type: String,
    /// Number of edges connected to this node.
    pub edge_count: u64,
}

/// Response for noise entity cleanup.
#[derive(Debug, Serialize, ToSchema)]
pub struct NoiseCleanupResponse {
    /// Number of noise nodes identified.
    pub nodes_identified: u64,
    /// Number of nodes actually deleted (0 in dry-run mode).
    pub nodes_deleted: u64,
    /// Number of edges removed (0 in dry-run mode).
    pub edges_removed: u64,
    /// Number of aliases removed (0 in dry-run mode).
    pub aliases_removed: u64,
    /// Whether this was a dry run.
    pub dry_run: bool,
    /// Details of identified noise entities.
    pub entities: Vec<NoiseEntityItem>,
}

/// Response for embedding backfill.
#[derive(Debug, Serialize, ToSchema)]
pub struct BackfillResponse {
    /// Total nodes found without embeddings.
    pub total_missing: u64,
    /// Nodes successfully embedded.
    pub embedded: u64,
    /// Nodes that failed to embed.
    pub failed: u64,
}

/// Query parameters for invalidated edge statistics.
#[derive(Debug, Deserialize, ToSchema, IntoParams)]
pub struct InvalidatedEdgeStatsParams {
    /// Maximum number of top relationship types to return (default 10).
    pub type_limit: Option<usize>,
    /// Maximum number of top nodes to return (default 20).
    pub node_limit: Option<usize>,
}

/// A relationship type with its count of invalidated edges.
#[derive(Debug, Serialize, ToSchema)]
pub struct InvalidatedEdgeTypeResponse {
    /// Relationship type (e.g. "RELATED_TO", "co_occurs").
    pub rel_type: String,
    /// Number of invalidated edges with this type.
    pub count: i64,
}

/// A node with a high count of invalidated edges (controversy indicator).
#[derive(Debug, Serialize, ToSchema)]
pub struct InvalidatedEdgeNodeResponse {
    /// Node UUID.
    pub node_id: Uuid,
    /// Canonical node name.
    pub canonical_name: String,
    /// Node type.
    pub node_type: String,
    /// Number of invalidated edges touching this node.
    pub invalidated_edge_count: i64,
}

/// Response for invalidated edge statistics.
#[derive(Debug, Serialize, ToSchema)]
pub struct InvalidatedEdgeStatsResponse {
    /// Total number of invalidated edges.
    pub total_invalidated: i64,
    /// Total number of valid (non-invalidated) edges.
    pub total_valid: i64,
    /// Top relationship types by invalidated edge count.
    pub top_types: Vec<InvalidatedEdgeTypeResponse>,
    /// Nodes with the highest invalidated-edge count (controversy indicators).
    pub top_nodes: Vec<InvalidatedEdgeNodeResponse>,
}

/// Response for opinion seeding.
#[derive(Debug, Serialize, ToSchema)]
pub struct SeedOpinionsResponse {
    /// Nodes that received computed opinions from extractions.
    pub nodes_seeded: u64,
    /// Nodes set to vacuous opinion (no extractions).
    pub nodes_vacuous: u64,
    /// Edges that received computed opinions from extractions.
    pub edges_seeded: u64,
    /// Edges set to vacuous opinion (no extractions).
    pub edges_vacuous: u64,
}

/// Request body for code-to-concept bridge creation.
#[derive(Debug, Deserialize, ToSchema)]
pub struct BridgeRequest {
    /// Minimum cosine similarity to create a bridge edge (default 0.6).
    pub min_similarity: Option<f64>,
    /// Maximum bridge edges per code node (default 3).
    pub max_edges_per_node: Option<i64>,
}

/// Response for code-to-concept bridge creation.
#[derive(Debug, Serialize, ToSchema)]
pub struct BridgeResponse {
    /// Code-type nodes checked for bridging.
    pub code_nodes_checked: u64,
    /// New bridge edges created.
    pub edges_created: u64,
    /// Pairs skipped because an edge already exists.
    pub skipped_existing: u64,
}

/// Request body for co-occurrence edge synthesis.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CooccurrenceRequest {
    /// Minimum co-occurrence count to create an edge (default 1).
    pub min_cooccurrences: Option<i64>,
    /// Only create edges for nodes with degree <= this value (default 2).
    pub max_degree: Option<i64>,
}

/// Response for LLM code node summarization.
#[derive(Debug, Serialize, ToSchema)]
pub struct CodeSummaryResponse {
    /// Code nodes found without semantic summaries.
    pub nodes_found: u64,
    /// Nodes successfully summarized and re-embedded.
    pub summarized: u64,
    /// Nodes where LLM summary failed.
    pub failed: u64,
}

/// Response for co-occurrence edge synthesis.
#[derive(Debug, Serialize, ToSchema)]
pub struct CooccurrenceResponse {
    /// Number of synthetic edges created.
    pub edges_created: u64,
    /// Number of candidate pairs evaluated.
    pub candidates_evaluated: u64,
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
    pub semantic_edge_count: usize,
    pub synthetic_edge_count: usize,
    pub component_count: usize,
    pub source_count: i64,
    pub chunk_count: i64,
    /// Number of RAPTOR summary chunks.
    pub summary_chunk_count: i64,
    pub article_count: i64,
    pub search_trace_count: i64,
}

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

/// Request body for Tier 5 HDBSCAN batch resolution.
#[derive(Debug, Deserialize, ToSchema)]
pub struct Tier5ResolveRequest {
    /// Minimum cluster size for HDBSCAN (default 2).
    pub min_cluster_size: Option<usize>,
}

/// Response for Tier 5 batch resolution.
#[derive(Debug, Serialize, ToSchema)]
pub struct Tier5ResolveResponse {
    /// Total entities processed from the pool.
    pub entities_processed: usize,
    /// Number of clusters formed by HDBSCAN.
    pub clusters_formed: usize,
    /// Number of entities resolved via clustering.
    pub clustered_resolved: usize,
    /// Number of noise entities promoted to individual nodes.
    pub noise_promoted: usize,
    /// Number of entities skipped (no embedding).
    pub skipped_no_embedding: usize,
}

/// Query parameters for knowledge gap detection.
#[derive(Debug, Deserialize, ToSchema, IntoParams)]
pub struct KnowledgeGapParams {
    /// Minimum in-degree to qualify as a gap (default 3).
    pub min_in_degree: Option<usize>,
    /// Minimum label length to filter noise (default 4).
    pub min_label_length: Option<usize>,
    /// Comma-separated node types to exclude. Defaults to
    /// "person,organization,event,location,publication,other" to
    /// filter bibliographic noise. Pass empty string for all types.
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

/// Health status of a single external service in the audit response.
#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceHealthResponse {
    /// Human-readable service name.
    pub name: String,
    /// Whether the service URL is configured.
    pub configured: bool,
    /// Whether the service responded to a health probe.
    pub reachable: bool,
    /// Description of fallback behavior when unreachable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
}

/// Response for the configuration audit endpoint.
#[derive(Debug, Serialize, ToSchema)]
pub struct ConfigAuditResponse {
    /// Current pipeline configuration summary.
    pub current_config: serde_json::Value,
    /// External service health status.
    pub services: Vec<ServiceHealthResponse>,
    /// Warnings about potential issues.
    pub warnings: Vec<String>,
}

/// A single row in the queue status summary.
#[derive(Debug, Serialize, ToSchema)]
pub struct QueueStatusRowResponse {
    /// Job kind (e.g. "reprocess_source").
    pub kind: String,
    /// Job status (e.g. "pending", "running", "dead").
    pub status: String,
    /// Count of jobs with this kind+status.
    pub count: i64,
}

/// Response for queue status.
#[derive(Debug, Serialize, ToSchema)]
pub struct QueueStatusResponse {
    /// Status summary grouped by kind and status.
    pub rows: Vec<QueueStatusRowResponse>,
}

/// Request body for retrying failed jobs.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RetryFailedRequest {
    /// Optional job kind filter. When null, retries all failed jobs.
    pub kind: Option<String>,
}

/// Response for retry-failed operation.
#[derive(Debug, Serialize, ToSchema)]
pub struct RetryFailedResponse {
    /// Number of jobs moved back to pending.
    pub retried: u64,
}

/// Query parameters for listing dead-letter jobs.
#[derive(Debug, Deserialize, ToSchema, IntoParams)]
pub struct ListDeadParams {
    /// Maximum number of dead jobs to return (default 20).
    pub limit: Option<i64>,
}

/// A dead-letter job in the response.
#[derive(Debug, Serialize, ToSchema)]
pub struct DeadJobResponse {
    /// Job UUID.
    pub id: Uuid,
    /// Job kind.
    pub kind: String,
    /// Number of attempts made.
    pub attempt: i32,
    /// Maximum attempts allowed.
    pub max_attempts: i32,
    /// Error message from the last attempt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Reason the job was moved to dead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dead_reason: Option<String>,
    /// Job payload.
    pub payload: serde_json::Value,
    /// When the job was created.
    pub created_at: String,
    /// When the job was last updated.
    pub updated_at: String,
}

/// Response for listing dead-letter jobs.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListDeadResponse {
    /// Dead-letter jobs.
    pub jobs: Vec<DeadJobResponse>,
}

/// Request body for clearing dead-letter jobs.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ClearDeadRequest {
    /// Optional job kind filter. When null, clears all dead jobs.
    pub kind: Option<String>,
}

/// Response for clearing dead-letter jobs.
#[derive(Debug, Serialize, ToSchema)]
pub struct ClearDeadResponse {
    /// Number of dead jobs deleted.
    pub deleted: u64,
}

/// Response for resurrecting dead-letter jobs.
#[derive(Debug, Serialize, ToSchema)]
pub struct ResurrectDeadResponse {
    /// Number of dead jobs resurrected to pending.
    pub resurrected: u64,
}

/// Health status of a single service in the registry.
#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceStatusResponse {
    /// Service name.
    pub name: String,
    /// Transport type: "http" or "stdio".
    pub transport_type: String,
    /// Whether the service passed its last health check.
    pub healthy: bool,
    /// When the service was last checked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checked: Option<String>,
    /// Error message from the last failed check.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Response for listing all registered services with health.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListServicesResponse {
    /// All registered services with current health status.
    pub services: Vec<ServiceStatusResponse>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gc_response_serializes_all_fields() {
        let resp = GcResponse {
            nodes_evicted: 3,
            edges_removed: 7,
            aliases_removed: 2,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["nodes_evicted"], 3);
        assert_eq!(json["edges_removed"], 7);
        assert_eq!(json["aliases_removed"], 2);
    }

    #[test]
    fn gc_response_zero_counts() {
        let resp = GcResponse {
            nodes_evicted: 0,
            edges_removed: 0,
            aliases_removed: 0,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["nodes_evicted"], 0);
        assert_eq!(json["edges_removed"], 0);
        assert_eq!(json["aliases_removed"], 0);
    }
}

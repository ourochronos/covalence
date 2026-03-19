//! Node handlers.

use axum::Json;
use axum::extract::{Path, Query, State};
use uuid::Uuid;

use crate::error::ApiError;
use crate::handlers::dto::{
    AnnotateNodeRequest, CorrectNodeRequest, CurationResponse, GetNodeParams, MergeNodesRequest,
    MergeNodesResponse, NeighborhoodParams, NodeDetailResponse, NodeExplanation, NodeResponse,
    ProvenanceResponse, ResolveNodeRequest, SplitNodeRequest, SplitNodeResponse,
};
use crate::state::AppState;

/// Get a node by ID, optionally with confidence explanation.
#[utoipa::path(
    get,
    path = "/nodes/{id}",
    params(
        ("id" = Uuid, Path, description = "Node ID"),
        GetNodeParams,
    ),
    responses(
        (status = 200, description = "Node found", body = NodeDetailResponse),
        (status = 404, description = "Node not found"),
    ),
    tag = "nodes"
)]
pub async fn get_node(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<GetNodeParams>,
) -> Result<Json<NodeDetailResponse>, ApiError> {
    let node =
        state
            .node_service
            .get(id.into())
            .await?
            .ok_or(covalence_core::error::Error::NotFound {
                entity_type: "node",
                id: id.to_string(),
            })?;

    let explanation = if params.explain.unwrap_or(false) {
        state
            .node_service
            .explain(id.into())
            .await?
            .map(|e| NodeExplanation {
                belief: e.belief,
                disbelief: e.disbelief,
                uncertainty: e.uncertainty,
                base_rate: e.base_rate,
                projected_probability: e.projected_probability,
                source_count: e.source_count,
                extraction_count: e.extraction_count,
            })
    } else {
        None
    };

    Ok(Json(NodeDetailResponse {
        node: node_to_response(node),
        explanation,
    }))
}

/// Get the neighborhood of a node.
#[utoipa::path(
    get,
    path = "/nodes/{id}/neighborhood",
    params(
        ("id" = Uuid, Path, description = "Node ID"),
        NeighborhoodParams,
    ),
    responses(
        (status = 200, description = "Neighbor nodes", body = Vec<NodeResponse>),
    ),
    tag = "nodes"
)]
pub async fn get_neighborhood(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<NeighborhoodParams>,
) -> Result<Json<Vec<NodeResponse>>, ApiError> {
    let hops = params.hops.unwrap_or(1).min(10);
    let include_invalidated = params.include_invalidated.unwrap_or(false);
    let nodes = state
        .node_service
        .neighborhood(id.into(), hops, include_invalidated)
        .await?;
    Ok(Json(nodes.into_iter().map(node_to_response).collect()))
}

/// Get the provenance chain for a node.
#[utoipa::path(
    get,
    path = "/nodes/{id}/provenance",
    params(("id" = Uuid, Path, description = "Node ID")),
    responses(
        (status = 200, description = "Provenance chain", body = ProvenanceResponse),
    ),
    tag = "nodes"
)]
pub async fn get_provenance(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ProvenanceResponse>, ApiError> {
    let chain = state.node_service.provenance(id.into()).await?;
    Ok(Json(ProvenanceResponse {
        node_id: chain.node_id.into_uuid(),
        extraction_count: chain.extractions.len(),
        chunk_count: chain.chunks.len(),
        source_count: chain.sources.len(),
    }))
}

/// Resolve a name to a node.
#[utoipa::path(
    post,
    path = "/nodes/resolve",
    request_body = ResolveNodeRequest,
    responses(
        (status = 200, description = "Resolved node", body = NodeResponse),
        (status = 404, description = "No matching node"),
    ),
    tag = "nodes"
)]
pub async fn resolve_node(
    State(state): State<AppState>,
    Json(req): Json<ResolveNodeRequest>,
) -> Result<Json<NodeResponse>, ApiError> {
    let node = state.node_service.resolve(&req.name).await?.ok_or(
        covalence_core::error::Error::NotFound {
            entity_type: "node",
            id: req.name,
        },
    )?;
    Ok(Json(node_to_response(node)))
}

/// Merge multiple nodes into a target.
#[utoipa::path(
    post,
    path = "/nodes/merge",
    request_body = MergeNodesRequest,
    responses(
        (status = 200, description = "Merge completed", body = MergeNodesResponse),
        (status = 400, description = "Not yet implemented"),
    ),
    tag = "nodes"
)]
pub async fn merge_nodes(
    State(state): State<AppState>,
    Json(req): Json<MergeNodesRequest>,
) -> Result<Json<MergeNodesResponse>, ApiError> {
    let source_ids = req.source_ids.into_iter().map(|id| id.into()).collect();
    let audit_id = state
        .node_service
        .merge(source_ids, req.target_id.into())
        .await?;
    Ok(Json(MergeNodesResponse {
        audit_log_id: audit_id.into_uuid(),
    }))
}

/// Split a node.
#[utoipa::path(
    post,
    path = "/nodes/{id}/split",
    params(("id" = Uuid, Path, description = "Node ID")),
    request_body = SplitNodeRequest,
    responses(
        (status = 200, description = "Split completed", body = SplitNodeResponse),
        (status = 404, description = "Node not found"),
    ),
    tag = "nodes"
)]
pub async fn split_node(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<SplitNodeRequest>,
) -> Result<Json<SplitNodeResponse>, ApiError> {
    let specs = req
        .specs
        .into_iter()
        .map(|s| covalence_core::services::node::SplitSpec {
            name: s.name,
            node_type: s.node_type,
            description: s.description,
            edge_ids: s.edge_ids.into_iter().map(|id| id.into()).collect(),
        })
        .collect();
    let ids = state.node_service.split(id.into(), specs).await?;
    Ok(Json(SplitNodeResponse {
        node_ids: ids.into_iter().map(|n| n.into_uuid()).collect(),
    }))
}

/// Correct a node's fields.
#[utoipa::path(
    post,
    path = "/nodes/{id}/correct",
    params(("id" = Uuid, Path, description = "Node ID")),
    request_body = CorrectNodeRequest,
    responses(
        (status = 200, description = "Node corrected", body = CurationResponse),
        (status = 404, description = "Node not found"),
    ),
    tag = "nodes"
)]
pub async fn correct_node(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<CorrectNodeRequest>,
) -> Result<Json<CurationResponse>, ApiError> {
    let audit_id = state
        .node_service
        .correct(
            id.into(),
            req.canonical_name,
            req.node_type,
            req.description,
            req.confidence,
        )
        .await?;
    Ok(Json(CurationResponse {
        success: true,
        audit_log_id: audit_id.into_uuid(),
    }))
}

/// Add a free-text annotation to a node.
#[utoipa::path(
    post,
    path = "/nodes/{id}/annotate",
    params(("id" = Uuid, Path, description = "Node ID")),
    request_body = AnnotateNodeRequest,
    responses(
        (status = 200, description = "Annotation added", body = CurationResponse),
        (status = 404, description = "Node not found"),
    ),
    tag = "nodes"
)]
pub async fn annotate_node(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AnnotateNodeRequest>,
) -> Result<Json<CurationResponse>, ApiError> {
    let audit_id = state.node_service.annotate(id.into(), req.text).await?;
    Ok(Json(CurationResponse {
        success: true,
        audit_log_id: audit_id.into_uuid(),
    }))
}

/// List landmark nodes (highest centrality).
#[utoipa::path(
    get,
    path = "/nodes/landmarks",
    params(
        ("limit" = Option<usize>, Query, description = "Max results"),
    ),
    responses(
        (status = 200, description = "Landmark nodes", body = Vec<NodeResponse>),
    ),
    tag = "nodes"
)]
pub async fn list_landmarks(
    State(state): State<AppState>,
    Query(params): Query<LandmarkParams>,
) -> Result<Json<Vec<NodeResponse>>, ApiError> {
    let limit = params.limit.unwrap_or(10).min(200);
    let nodes = state.node_service.list_landmarks(limit).await?;
    Ok(Json(nodes.into_iter().map(node_to_response).collect()))
}

/// Query parameters for landmark listing.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema, utoipa::IntoParams)]
pub struct LandmarkParams {
    /// Maximum number of landmark nodes to return.
    pub limit: Option<usize>,
}

fn node_to_response(node: covalence_core::models::node::Node) -> NodeResponse {
    NodeResponse {
        id: node.id.into_uuid(),
        canonical_name: node.canonical_name,
        node_type: node.node_type,
        entity_class: node.entity_class,
        description: node.description,
        properties: node.properties,
        clearance_level: node.clearance_level.as_i32(),
        first_seen: node.first_seen.to_rfc3339(),
        last_seen: node.last_seen.to_rfc3339(),
        mention_count: node.mention_count,
        domain_entropy: node.domain_entropy,
        primary_domain: node.primary_domain,
    }
}

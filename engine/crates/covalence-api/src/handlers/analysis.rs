//! Cross-domain analysis handlers.

use axum::Json;
use axum::extract::State;

use crate::error::ApiError;
use crate::handlers::dto::{
    AffectedNodeResponse, BlastRadiusHopResponse, BlastRadiusRequest, BlastRadiusResponse,
    BootstrapResponse, CoverageItemResponse, CoverageResponse, DivergentNodeResponse,
    ErosionItemResponse, ErosionRequest, ErosionResponse, LinkDomainsRequest, LinkDomainsResponse,
};
use crate::state::AppState;

/// Bootstrap Component nodes for the 9 known subsystems.
#[utoipa::path(
    post,
    path = "/analysis/bootstrap",
    responses(
        (status = 200, description = "Component bootstrap results",
         body = BootstrapResponse),
    ),
    tag = "analysis"
)]
pub async fn bootstrap_components(
    State(state): State<AppState>,
) -> Result<Json<BootstrapResponse>, ApiError> {
    let result = state.analysis_service.bootstrap_components().await?;
    Ok(Json(BootstrapResponse {
        components_created: result.components_created,
        components_existing: result.components_existing,
        components_embedded: result.components_embedded,
    }))
}

/// Create cross-domain bridge edges between Components and code/spec/research.
#[utoipa::path(
    post,
    path = "/analysis/link",
    request_body = LinkDomainsRequest,
    responses(
        (status = 200, description = "Cross-domain linking results",
         body = LinkDomainsResponse),
    ),
    tag = "analysis"
)]
pub async fn link_domains(
    State(state): State<AppState>,
    Json(req): Json<LinkDomainsRequest>,
) -> Result<Json<LinkDomainsResponse>, ApiError> {
    let min_sim = req.min_similarity.unwrap_or(0.5);
    let max_edges = req.max_edges_per_component.unwrap_or(5);
    let result = state
        .analysis_service
        .link_domains(min_sim, max_edges)
        .await?;
    Ok(Json(LinkDomainsResponse {
        part_of_edges: result.part_of_edges,
        intent_edges: result.intent_edges,
        basis_edges: result.basis_edges,
        skipped_existing: result.skipped_existing,
    }))
}

/// Detect orphaned code and unimplemented spec concepts.
#[utoipa::path(
    post,
    path = "/analysis/coverage",
    responses(
        (status = 200, description = "Coverage analysis results",
         body = CoverageResponse),
    ),
    tag = "analysis"
)]
pub async fn coverage_analysis(
    State(state): State<AppState>,
) -> Result<Json<CoverageResponse>, ApiError> {
    let result = state.analysis_service.coverage_analysis().await?;
    Ok(Json(CoverageResponse {
        orphan_code: result
            .orphan_code
            .into_iter()
            .map(|i| CoverageItemResponse {
                node_id: i.node_id,
                name: i.name,
                node_type: i.node_type,
                file_path: i.file_path,
                reason: i.reason,
            })
            .collect(),
        unimplemented_specs: result
            .unimplemented_specs
            .into_iter()
            .map(|i| CoverageItemResponse {
                node_id: i.node_id,
                name: i.name,
                node_type: i.node_type,
                file_path: i.file_path,
                reason: i.reason,
            })
            .collect(),
        coverage_score: result.coverage_score,
    }))
}

/// Detect architecture erosion — code drifting from design intent.
#[utoipa::path(
    post,
    path = "/analysis/erosion",
    request_body = ErosionRequest,
    responses(
        (status = 200, description = "Erosion detection results",
         body = ErosionResponse),
    ),
    tag = "analysis"
)]
pub async fn detect_erosion(
    State(state): State<AppState>,
    Json(req): Json<ErosionRequest>,
) -> Result<Json<ErosionResponse>, ApiError> {
    let threshold = req.threshold.unwrap_or(0.3);
    let result = state.analysis_service.detect_erosion(threshold).await?;
    Ok(Json(ErosionResponse {
        eroded_components: result
            .eroded_components
            .into_iter()
            .map(|e| ErosionItemResponse {
                component_id: e.component_id,
                component_name: e.component_name,
                spec_intent: e.spec_intent,
                drift_score: e.drift_score,
                divergent_nodes: e
                    .divergent_nodes
                    .into_iter()
                    .map(|d| DivergentNodeResponse {
                        node_id: d.node_id,
                        name: d.name,
                        summary: d.summary,
                        distance: d.distance,
                    })
                    .collect(),
            })
            .collect(),
        total_components: result.total_components,
    }))
}

/// Simulate the blast radius of changing a node.
#[utoipa::path(
    post,
    path = "/analysis/blast-radius",
    request_body = BlastRadiusRequest,
    responses(
        (status = 200, description = "Blast radius simulation",
         body = BlastRadiusResponse),
    ),
    tag = "analysis"
)]
pub async fn blast_radius(
    State(state): State<AppState>,
    Json(req): Json<BlastRadiusRequest>,
) -> Result<Json<BlastRadiusResponse>, ApiError> {
    let max_hops = req.max_hops.unwrap_or(2).min(10);
    let result = state
        .analysis_service
        .blast_radius(&req.target, max_hops)
        .await?;
    Ok(Json(BlastRadiusResponse {
        target_node_id: result.target.node_id,
        target_name: result.target.name,
        target_node_type: result.target.node_type,
        component: result.target.component,
        affected_by_hop: result
            .affected_by_hop
            .into_iter()
            .map(|h| BlastRadiusHopResponse {
                hop_distance: h.hop_distance,
                nodes: h
                    .nodes
                    .into_iter()
                    .map(|n| AffectedNodeResponse {
                        node_id: n.node_id,
                        name: n.name,
                        node_type: n.node_type,
                        relationship: n.relationship,
                    })
                    .collect(),
            })
            .collect(),
        total_affected: result.total_affected,
    }))
}

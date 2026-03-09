//! Admin, graph, and audit handlers.

use axum::Json;
use axum::extract::{Path, Query, State};
use uuid::Uuid;

use crate::error::ApiError;
use crate::handlers::dto::{
    AuditLogResponse, CommunityResponse, ConsolidateResponse, DomainLinkResponse, DomainResponse,
    GraphStatsResponse, HealthResponse, MetricsResponse, PaginationParams, PublishResponse,
    ReloadResponse, TopologyResponse,
};
use crate::state::AppState;

/// Get graph statistics.
#[utoipa::path(
    get,
    path = "/graph/stats",
    responses(
        (status = 200, description = "Graph statistics", body = GraphStatsResponse),
    ),
    tag = "graph"
)]
pub async fn graph_stats(State(state): State<AppState>) -> Json<GraphStatsResponse> {
    let stats = state.admin_service.graph_stats().await;
    Json(GraphStatsResponse {
        node_count: stats.node_count,
        edge_count: stats.edge_count,
        density: stats.density,
        component_count: stats.component_count,
    })
}

/// Get detected communities.
#[utoipa::path(
    get,
    path = "/graph/communities",
    responses(
        (status = 200, description = "Detected communities", body = Vec<CommunityResponse>),
    ),
    tag = "graph"
)]
pub async fn graph_communities(State(state): State<AppState>) -> Json<Vec<CommunityResponse>> {
    let graph = state.graph.read().await;
    let communities = covalence_core::graph::community::detect_communities(graph.graph());
    Json(
        communities
            .into_iter()
            .map(|c| CommunityResponse {
                id: c.id,
                node_ids: c.node_ids,
                label: c.label,
                coherence: c.coherence,
            })
            .collect(),
    )
}

/// Get the domain topology map.
#[utoipa::path(
    get,
    path = "/graph/topology",
    responses(
        (status = 200, description = "Domain topology map", body = TopologyResponse),
    ),
    tag = "graph"
)]
pub async fn graph_topology(State(state): State<AppState>) -> Json<TopologyResponse> {
    let graph = state.graph.read().await;
    let topo = covalence_core::graph::topology::build_topology(graph.graph());
    Json(TopologyResponse {
        domains: topo
            .domains
            .into_iter()
            .map(|d| DomainResponse {
                community_id: d.community_id,
                label: d.label,
                node_count: d.node_count,
                landmark_ids: d.landmark_ids,
                coherence: d.coherence,
                avg_pagerank: d.avg_pagerank,
            })
            .collect(),
        links: topo
            .links
            .into_iter()
            .map(|l| DomainLinkResponse {
                source_domain: l.source_domain,
                target_domain: l.target_domain,
                bridge_count: l.bridge_count,
                strongest_bridge: l.strongest_bridge,
            })
            .collect(),
        total_nodes: topo.total_nodes,
        total_edges: topo.total_edges,
    })
}

/// List recent audit log entries.
#[utoipa::path(
    get,
    path = "/audit",
    params(PaginationParams),
    responses(
        (status = 200, description = "Audit log entries", body = Vec<AuditLogResponse>),
    ),
    tag = "admin"
)]
pub async fn audit_log(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<Vec<AuditLogResponse>>, ApiError> {
    let entries = state.admin_service.audit_log(params.limit()).await?;
    Ok(Json(
        entries
            .into_iter()
            .map(|e| AuditLogResponse {
                id: e.id.into_uuid(),
                action: e.action,
                actor: e.actor,
                target_type: e.target_type,
                target_id: e.target_id,
                created_at: e.created_at.to_rfc3339(),
            })
            .collect(),
    ))
}

/// Reload the graph sidecar from PG.
#[utoipa::path(
    post,
    path = "/admin/graph/reload",
    responses(
        (status = 200, description = "Graph reloaded", body = ReloadResponse),
    ),
    tag = "admin"
)]
pub async fn reload_graph(State(state): State<AppState>) -> Result<Json<ReloadResponse>, ApiError> {
    let stats = state.admin_service.reload_graph().await?;
    Ok(Json(ReloadResponse {
        node_count: stats.node_count,
        edge_count: stats.edge_count,
        density: stats.density,
        component_count: stats.component_count,
    }))
}

/// Promote a source to a higher clearance level for federation.
#[utoipa::path(
    post,
    path = "/admin/publish/{source_id}",
    params(("source_id" = Uuid, Path, description = "Source ID to publish")),
    responses(
        (status = 200, description = "Source published", body = PublishResponse),
    ),
    tag = "admin"
)]
pub async fn publish_source(
    State(state): State<AppState>,
    Path(source_id): Path<Uuid>,
) -> Result<Json<PublishResponse>, ApiError> {
    let source_id = covalence_core::types::ids::SourceId::from_uuid(source_id);
    state.source_service.publish(source_id).await?;
    Ok(Json(PublishResponse { published: true }))
}

/// Trigger batch consolidation.
#[utoipa::path(
    post,
    path = "/admin/consolidate",
    responses(
        (status = 200, description = "Consolidation triggered", body = ConsolidateResponse),
    ),
    tag = "admin"
)]
pub async fn trigger_consolidation(
    State(state): State<AppState>,
) -> Result<Json<ConsolidateResponse>, ApiError> {
    state.admin_service.trigger_consolidation().await?;
    Ok(Json(ConsolidateResponse { triggered: true }))
}

/// Health check.
#[utoipa::path(
    get,
    path = "/admin/health",
    responses(
        (status = 200, description = "Service is healthy", body = HealthResponse),
    ),
    tag = "admin"
)]
pub async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    let health = state.admin_service.health().await;
    let status = if health.pg_healthy { "ok" } else { "degraded" };
    Json(HealthResponse {
        status: status.to_string(),
        service: "covalence".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// Get service metrics.
#[utoipa::path(
    get,
    path = "/admin/metrics",
    responses(
        (status = 200, description = "Service metrics", body = MetricsResponse),
    ),
    tag = "admin"
)]
pub async fn metrics(State(state): State<AppState>) -> Result<Json<MetricsResponse>, ApiError> {
    let m = state.admin_service.metrics().await?;
    Ok(Json(MetricsResponse {
        graph_nodes: m.graph_nodes,
        graph_edges: m.graph_edges,
        source_count: m.source_count,
    }))
}

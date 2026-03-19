//! Admin, graph, and audit handlers.

use axum::Json;
use axum::extract::{Path, Query, State};
use uuid::Uuid;

use crate::error::ApiError;
use crate::handlers::dto::{
    AuditLogResponse, BackfillResponse, BridgeRequest, BridgeResponse, ClearDeadRequest,
    ClearDeadResponse, CodeSummaryResponse, CommunityParams, CommunityResponse,
    ConfigAuditResponse, ConsolidateResponse, CooccurrenceRequest, CooccurrenceResponse,
    DeadJobResponse, DomainLinkResponse, DomainResponse, GcResponse, GraphStatsResponse,
    HealthResponse, InvalidatedEdgeNodeResponse, InvalidatedEdgeStatsParams,
    InvalidatedEdgeStatsResponse, InvalidatedEdgeTypeResponse, KnowledgeGapItem,
    KnowledgeGapParams, KnowledgeGapsResponse, ListDeadParams, ListDeadResponse, MetricsResponse,
    NoiseCleanupRequest, NoiseCleanupResponse, NoiseEntityItem, OntologyClusterItem,
    OntologyClusterRequest, OntologyClusterResponse, PaginationParams, PublishResponse,
    QueueStatusResponse, QueueStatusRowResponse, RaptorResponse, ReloadResponse,
    ResurrectDeadResponse, RetryFailedRequest, RetryFailedResponse, SearchTraceResponse,
    SeedOpinionsResponse, SidecarHealthResponse, Tier5ResolveRequest, Tier5ResolveResponse,
    TopologyResponse, TraceReplayResponse,
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
        semantic_edge_count: stats.semantic_edge_count,
        synthetic_edge_count: stats.synthetic_edge_count,
        density: stats.density,
        component_count: stats.component_count,
    })
}

/// Get detected communities.
#[utoipa::path(
    get,
    path = "/graph/communities",
    params(CommunityParams),
    responses(
        (status = 200, description = "Detected communities", body = Vec<CommunityResponse>),
    ),
    tag = "graph"
)]
pub async fn graph_communities(
    State(state): State<AppState>,
    Query(params): Query<CommunityParams>,
) -> Json<Vec<CommunityResponse>> {
    let min_size = params.min_size.unwrap_or(2);
    let communities = state
        .graph_engine
        .communities(min_size)
        .await
        .unwrap_or_default();
    Json(
        communities
            .into_iter()
            .map(|c| CommunityResponse {
                id: c.id,
                size: c.node_ids.len(),
                node_ids: c.node_ids,
                label: c.label,
                coherence: c.coherence,
                core_level: c.core_level,
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
    let topo = state.graph_engine.topology().await.unwrap_or_else(|_| {
        covalence_core::graph::TopologyMap {
            domains: Vec::new(),
            links: Vec::new(),
            total_nodes: 0,
            total_edges: 0,
        }
    });
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

/// Get statistics about invalidated edges.
///
/// Invalidated edges are normally invisible to the graph sidecar.
/// This endpoint exposes their distribution by relationship type
/// and highlights nodes with high invalidated-edge counts
/// (controversy indicators).
#[utoipa::path(
    get,
    path = "/admin/graph/invalidated-stats",
    params(InvalidatedEdgeStatsParams),
    responses(
        (status = 200, description = "Invalidated edge statistics",
         body = InvalidatedEdgeStatsResponse),
    ),
    tag = "admin"
)]
pub async fn invalidated_edge_stats(
    State(state): State<AppState>,
    Query(params): Query<InvalidatedEdgeStatsParams>,
) -> Result<Json<InvalidatedEdgeStatsResponse>, ApiError> {
    let type_limit = params.type_limit.unwrap_or(10).min(100);
    let node_limit = params.node_limit.unwrap_or(20).min(200);
    let stats = state
        .admin_service
        .invalidated_edge_stats(type_limit, node_limit)
        .await?;
    Ok(Json(InvalidatedEdgeStatsResponse {
        total_invalidated: stats.total_invalidated,
        total_valid: stats.total_valid,
        top_types: stats
            .top_types
            .into_iter()
            .map(|t| InvalidatedEdgeTypeResponse {
                rel_type: t.rel_type,
                count: t.count,
            })
            .collect(),
        top_nodes: stats
            .top_nodes
            .into_iter()
            .map(|n| InvalidatedEdgeNodeResponse {
                node_id: n.node_id,
                canonical_name: n.canonical_name,
                node_type: n.node_type,
                invalidated_edge_count: n.invalidated_edge_count,
            })
            .collect(),
    }))
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
        semantic_edge_count: stats.semantic_edge_count,
        synthetic_edge_count: stats.synthetic_edge_count,
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

/// Trigger RAPTOR recursive summarization.
///
/// Builds hierarchical summary chunks across all sources,
/// enabling multi-resolution retrieval.
#[utoipa::path(
    post,
    path = "/admin/raptor",
    responses(
        (status = 200, description = "RAPTOR summarization results", body = RaptorResponse),
    ),
    tag = "admin"
)]
pub async fn trigger_raptor(
    State(state): State<AppState>,
) -> Result<Json<RaptorResponse>, ApiError> {
    let report = state.admin_service.trigger_raptor().await?;
    Ok(Json(RaptorResponse {
        sources_processed: report.sources_processed,
        sources_skipped: report.sources_skipped,
        summaries_created: report.summaries_created,
        llm_calls: report.llm_calls,
        embed_calls: report.embed_calls,
        errors: report.errors,
    }))
}

/// Run provenance-based garbage collection.
///
/// Evicts nodes that have zero active (non-superseded) extractions,
/// along with their edges and aliases.
#[utoipa::path(
    post,
    path = "/admin/gc",
    responses(
        (status = 200, description = "Garbage collection results", body = GcResponse),
    ),
    tag = "admin"
)]
pub async fn garbage_collect(State(state): State<AppState>) -> Result<Json<GcResponse>, ApiError> {
    let result = state.admin_service.garbage_collect_nodes().await?;
    Ok(Json(GcResponse {
        nodes_evicted: result.nodes_evicted,
        edges_removed: result.edges_removed,
        aliases_removed: result.aliases_removed,
    }))
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
        semantic_edge_count: m.semantic_edge_count,
        synthetic_edge_count: m.synthetic_edge_count,
        component_count: m.component_count,
        source_count: m.source_count,
        chunk_count: m.chunk_count,
        summary_chunk_count: m.summary_chunk_count,
        article_count: m.article_count,
        search_trace_count: m.search_trace_count,
    }))
}

/// List recent search traces.
#[utoipa::path(
    get,
    path = "/admin/traces",
    params(PaginationParams),
    responses(
        (status = 200, description = "Search traces", body = Vec<SearchTraceResponse>),
    ),
    tag = "admin"
)]
pub async fn list_traces(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<Vec<SearchTraceResponse>>, ApiError> {
    let traces = state.admin_service.list_traces(params.limit()).await?;
    Ok(Json(
        traces
            .into_iter()
            .map(|t| SearchTraceResponse {
                id: t.id,
                query_text: t.query_text,
                strategy: t.strategy,
                dimension_counts: t.dimension_counts,
                result_count: t.result_count,
                execution_ms: t.execution_ms,
                created_at: t.created_at.to_rfc3339(),
            })
            .collect(),
    ))
}

/// Clear the semantic query cache.
#[utoipa::path(
    post,
    path = "/admin/cache/clear",
    responses(
        (status = 200, description = "Cache cleared", body = serde_json::Value),
    ),
    tag = "admin"
)]
pub async fn clear_cache(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cleared = state.search_service.clear_cache().await?;
    tracing::info!(entries_cleared = cleared, "query cache cleared");
    Ok(Json(serde_json::json!({
        "entries_cleared": cleared,
    })))
}

/// Run ontology clustering.
#[utoipa::path(
    post,
    path = "/admin/ontology/cluster",
    request_body = OntologyClusterRequest,
    responses(
        (status = 200, description = "Clustering results", body = OntologyClusterResponse),
    ),
    tag = "admin"
)]
pub async fn cluster_ontology(
    State(state): State<AppState>,
    Json(req): Json<OntologyClusterRequest>,
) -> Result<Json<OntologyClusterResponse>, ApiError> {
    let min_cluster_size = req.min_cluster_size.unwrap_or(2);
    let dry_run = req.dry_run.unwrap_or(true);
    let level = req.level.as_deref().and_then(|l| match l {
        "entity" => Some(covalence_core::consolidation::ClusterLevel::Entity),
        "entity_type" => Some(covalence_core::consolidation::ClusterLevel::EntityType),
        "rel_type" => Some(covalence_core::consolidation::ClusterLevel::RelationType),
        _ => None,
    });

    let result = state
        .admin_service
        .cluster_ontology(level, min_cluster_size, dry_run)
        .await?;

    let items: Vec<OntologyClusterItem> = result
        .clusters
        .iter()
        .map(|c| {
            let level_str = match c.level {
                covalence_core::consolidation::ClusterLevel::Entity => "entity",
                covalence_core::consolidation::ClusterLevel::EntityType => "entity_type",
                covalence_core::consolidation::ClusterLevel::RelationType => "rel_type",
            };
            OntologyClusterItem {
                id: c.id,
                level: level_str.to_string(),
                canonical_label: c.canonical_label.clone(),
                member_labels: c.member_labels.clone(),
                member_count: c.member_count,
            }
        })
        .collect();

    Ok(Json(OntologyClusterResponse {
        applied: !dry_run,
        cluster_count: items.len(),
        clusters: items,
        noise_labels: result.noise_labels,
    }))
}

/// Detect knowledge gaps in the graph.
#[utoipa::path(
    get,
    path = "/admin/knowledge-gaps",
    params(KnowledgeGapParams),
    responses(
        (status = 200, description = "Knowledge gaps", body = KnowledgeGapsResponse),
    ),
    tag = "admin"
)]
pub async fn knowledge_gaps(
    State(state): State<AppState>,
    Query(params): Query<KnowledgeGapParams>,
) -> Result<Json<KnowledgeGapsResponse>, ApiError> {
    let min_in_degree = params.min_in_degree.unwrap_or(3);
    let min_label_length = params.min_label_length.unwrap_or(4);
    let limit = params.limit.unwrap_or(20).min(200);
    // Default excludes bibliographic entity types which dominate
    // the gap metric with citation noise rather than real knowledge
    // gaps. Pass `exclude_types=` (empty) to include them.
    let exclude_types: Vec<String> = params.exclude_types.map_or_else(
        || {
            vec![
                "person".to_string(),
                "organization".to_string(),
                "event".to_string(),
                "location".to_string(),
                "publication".to_string(),
                "other".to_string(),
            ]
        },
        |s| {
            if s.is_empty() {
                Vec::new()
            } else {
                s.split(',').map(|t| t.trim().to_string()).collect()
            }
        },
    );

    let gaps = state
        .admin_service
        .knowledge_gaps(min_in_degree, min_label_length, &exclude_types, limit)
        .await?;

    let items: Vec<KnowledgeGapItem> = gaps
        .into_iter()
        .map(|g| KnowledgeGapItem {
            node_id: g.node_id,
            canonical_name: g.canonical_name,
            node_type: g.node_type,
            in_degree: g.in_degree,
            out_degree: g.out_degree,
            gap_score: g.gap_score,
            referenced_by: g.referenced_by,
        })
        .collect();

    Ok(Json(KnowledgeGapsResponse {
        gap_count: items.len(),
        gaps: items,
    }))
}

/// Replay a traced search query.
#[utoipa::path(
    post,
    path = "/admin/traces/{id}/replay",
    params(("id" = Uuid, Path, description = "Trace ID to replay")),
    responses(
        (status = 200, description = "Replay results", body = TraceReplayResponse),
        (status = 404, description = "Trace not found"),
    ),
    tag = "admin"
)]
pub async fn replay_trace(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<TraceReplayResponse>, ApiError> {
    let trace =
        state
            .admin_service
            .get_trace(id)
            .await?
            .ok_or(covalence_core::error::Error::NotFound {
                entity_type: "search_trace",
                id: id.to_string(),
            })?;

    let strategy = match trace.strategy.as_str() {
        "balanced" => covalence_core::search::strategy::SearchStrategy::Balanced,
        "precise" => covalence_core::search::strategy::SearchStrategy::Precise,
        "exploratory" => covalence_core::search::strategy::SearchStrategy::Exploratory,
        "recent" => covalence_core::search::strategy::SearchStrategy::Recent,
        "graph_first" => covalence_core::search::strategy::SearchStrategy::GraphFirst,
        "global" => covalence_core::search::strategy::SearchStrategy::Global,
        _ => covalence_core::search::strategy::SearchStrategy::Auto,
    };

    let results = state
        .search_service
        .search(&trace.query_text, strategy, 10, None)
        .await?;

    Ok(Json(TraceReplayResponse {
        trace_id: id,
        results: results
            .into_iter()
            .map(crate::handlers::dto::SearchResultResponse::from)
            .collect(),
    }))
}

/// Synthesize co-occurrence edges from extraction provenance.
///
/// Creates `co_occurs` edges between entities extracted from the
/// same chunk. Only targets poorly-connected nodes (degree ≤
/// `max_degree`) to avoid flooding the graph.
#[utoipa::path(
    post,
    path = "/admin/edges/synthesize",
    request_body = CooccurrenceRequest,
    responses(
        (status = 200, description = "Synthesis results", body = CooccurrenceResponse),
    ),
    tag = "admin"
)]
pub async fn synthesize_cooccurrence(
    State(state): State<AppState>,
    Json(req): Json<CooccurrenceRequest>,
) -> Result<Json<CooccurrenceResponse>, ApiError> {
    let min_cooccurrences = req.min_cooccurrences.unwrap_or(1);
    let max_degree = req.max_degree.unwrap_or(2);
    let result = state
        .admin_service
        .synthesize_cooccurrence_edges(min_cooccurrences, max_degree)
        .await?;
    Ok(Json(CooccurrenceResponse {
        edges_created: result.edges_created,
        candidates_evaluated: result.candidates_evaluated,
    }))
}

/// Run a configuration audit.
///
/// Checks sidecar health, summarizes current config, and generates
/// warnings for potential issues.
#[utoipa::path(
    post,
    path = "/admin/config-audit",
    responses(
        (status = 200, description = "Configuration audit result",
         body = ConfigAuditResponse),
    ),
    tag = "admin"
)]
pub async fn config_audit(
    State(state): State<AppState>,
) -> Result<Json<ConfigAuditResponse>, ApiError> {
    let audit = state.admin_service.config_audit().await?;

    let sidecars: Vec<SidecarHealthResponse> = audit
        .sidecars
        .into_iter()
        .map(|s| SidecarHealthResponse {
            name: s.name,
            configured: s.configured,
            reachable: s.reachable,
            fallback: s.fallback,
        })
        .collect();

    Ok(Json(ConfigAuditResponse {
        current_config: audit.current_config,
        sidecars,
        warnings: audit.warnings,
    }))
}

/// Trigger Tier 5 HDBSCAN batch entity resolution.
#[utoipa::path(
    post,
    path = "/admin/tier5/resolve",
    request_body = Tier5ResolveRequest,
    responses(
        (status = 200, description = "Tier 5 resolution report",
         body = Tier5ResolveResponse),
    ),
    tag = "admin"
)]
pub async fn resolve_tier5(
    State(state): State<AppState>,
    Json(req): Json<Tier5ResolveRequest>,
) -> Result<Json<Tier5ResolveResponse>, ApiError> {
    let report = state
        .admin_service
        .resolve_tier5(req.min_cluster_size)
        .await?;

    Ok(Json(Tier5ResolveResponse {
        entities_processed: report.entities_processed,
        clusters_formed: report.clusters_formed,
        clustered_resolved: report.clustered_resolved,
        noise_promoted: report.noise_promoted,
        skipped_no_embedding: report.skipped_no_embedding,
    }))
}

/// Retroactively clean noise entities from the graph.
///
/// Scans all nodes through the noise entity filter and optionally
/// removes matches along with their edges and aliases. Default mode
/// is dry-run (report only).
#[utoipa::path(
    post,
    path = "/admin/nodes/cleanup",
    request_body = NoiseCleanupRequest,
    responses(
        (status = 200, description = "Noise cleanup results",
         body = NoiseCleanupResponse),
    ),
    tag = "admin"
)]
pub async fn cleanup_noise_entities(
    State(state): State<AppState>,
    Json(req): Json<NoiseCleanupRequest>,
) -> Result<Json<NoiseCleanupResponse>, ApiError> {
    let dry_run = req.dry_run.unwrap_or(true);
    let result = state.admin_service.cleanup_noise_entities(dry_run).await?;

    let entities: Vec<NoiseEntityItem> = result
        .entities
        .into_iter()
        .map(|e| NoiseEntityItem {
            node_id: e.node_id,
            canonical_name: e.canonical_name,
            node_type: e.node_type,
            edge_count: e.edge_count,
        })
        .collect();

    Ok(Json(NoiseCleanupResponse {
        nodes_identified: result.nodes_identified,
        nodes_deleted: result.nodes_deleted,
        edges_removed: result.edges_removed,
        aliases_removed: result.aliases_removed,
        dry_run: result.dry_run,
        entities,
    }))
}

/// Backfill embeddings for nodes that are missing them.
#[utoipa::path(
    post,
    path = "/admin/nodes/backfill-embeddings",
    responses(
        (status = 200, description = "Backfill results",
         body = BackfillResponse),
    ),
    tag = "admin"
)]
pub async fn backfill_node_embeddings(
    State(state): State<AppState>,
) -> Result<Json<BackfillResponse>, ApiError> {
    let result = state.admin_service.backfill_node_embeddings().await?;
    Ok(Json(BackfillResponse {
        total_missing: result.total_missing,
        embedded: result.embedded,
        failed: result.failed,
    }))
}

/// Seed epistemic opinions on all nodes and edges.
#[utoipa::path(
    post,
    path = "/admin/opinions/seed",
    responses(
        (status = 200, description = "Seeding results",
         body = SeedOpinionsResponse),
    ),
    tag = "admin"
)]
pub async fn seed_opinions(
    State(state): State<AppState>,
) -> Result<Json<SeedOpinionsResponse>, ApiError> {
    let result = state.admin_service.seed_opinions().await?;
    Ok(Json(SeedOpinionsResponse {
        nodes_seeded: result.nodes_seeded,
        nodes_vacuous: result.nodes_vacuous,
        edges_seeded: result.edges_seeded,
        edges_vacuous: result.edges_vacuous,
    }))
}

/// Generate LLM semantic summaries for code nodes.
#[utoipa::path(
    post,
    path = "/admin/nodes/summarize-code",
    responses(
        (status = 200, description = "Summarization results",
         body = CodeSummaryResponse),
    ),
    tag = "admin"
)]
pub async fn summarize_code_nodes(
    State(state): State<AppState>,
) -> Result<Json<CodeSummaryResponse>, ApiError> {
    let result = state.admin_service.summarize_code_nodes().await?;
    Ok(Json(CodeSummaryResponse {
        nodes_found: result.nodes_found,
        summarized: result.summarized,
        failed: result.failed,
    }))
}

/// Create cross-domain bridge edges between code entities and concept nodes.
#[utoipa::path(
    post,
    path = "/admin/edges/bridge",
    request_body = BridgeRequest,
    responses(
        (status = 200, description = "Bridge results",
         body = BridgeResponse),
    ),
    tag = "admin"
)]
pub async fn bridge_code_to_concepts(
    State(state): State<AppState>,
    Json(req): Json<BridgeRequest>,
) -> Result<Json<BridgeResponse>, ApiError> {
    let min_sim = req.min_similarity.unwrap_or(0.6);
    let max_edges = req.max_edges_per_node.unwrap_or(3);
    let result = state
        .admin_service
        .bridge_code_to_concepts(min_sim, max_edges)
        .await?;
    Ok(Json(BridgeResponse {
        code_nodes_checked: result.code_nodes_checked,
        edges_created: result.edges_created,
        skipped_existing: result.skipped_existing,
    }))
}

// ------------------------------------------------------------------
// Retry queue
// ------------------------------------------------------------------

/// Get queue status summary.
#[utoipa::path(
    get,
    path = "/admin/queue/status",
    responses(
        (status = 200, description = "Queue status summary",
         body = QueueStatusResponse),
    ),
    tag = "admin"
)]
pub async fn queue_status_handler(
    State(state): State<AppState>,
) -> Result<Json<QueueStatusResponse>, ApiError> {
    let rows = state.queue_service.queue_status().await?;
    Ok(Json(QueueStatusResponse {
        rows: rows
            .into_iter()
            .map(|r| QueueStatusRowResponse {
                kind: r.kind,
                status: r.status,
                count: r.count,
            })
            .collect(),
    }))
}

/// Retry all failed/dead jobs, optionally filtered by kind.
#[utoipa::path(
    post,
    path = "/admin/queue/retry",
    request_body = RetryFailedRequest,
    responses(
        (status = 200, description = "Jobs retried",
         body = RetryFailedResponse),
    ),
    tag = "admin"
)]
pub async fn retry_failed_handler(
    State(state): State<AppState>,
    Json(req): Json<RetryFailedRequest>,
) -> Result<Json<RetryFailedResponse>, ApiError> {
    let kind = req
        .kind
        .as_deref()
        .and_then(covalence_core::models::retry_job::JobKind::from_pg_str);
    let retried = state.queue_service.retry_failed(kind).await?;
    Ok(Json(RetryFailedResponse { retried }))
}

/// List dead-letter jobs.
#[utoipa::path(
    get,
    path = "/admin/queue/dead",
    params(ListDeadParams),
    responses(
        (status = 200, description = "Dead-letter jobs",
         body = ListDeadResponse),
    ),
    tag = "admin"
)]
pub async fn list_dead_handler(
    State(state): State<AppState>,
    Query(params): Query<ListDeadParams>,
) -> Result<Json<ListDeadResponse>, ApiError> {
    let limit = params.limit.unwrap_or(20).clamp(1, 1000);
    let jobs = state.queue_service.list_dead(limit).await?;
    Ok(Json(ListDeadResponse {
        jobs: jobs
            .into_iter()
            .map(|j| DeadJobResponse {
                id: j.id.into_uuid(),
                kind: j.kind.as_pg_str().to_string(),
                attempt: j.attempt,
                max_attempts: j.max_attempts,
                last_error: j.last_error,
                dead_reason: j.dead_reason,
                payload: j.payload,
                created_at: j.created_at.to_rfc3339(),
                updated_at: j.updated_at.to_rfc3339(),
            })
            .collect(),
    }))
}

/// Comprehensive health report for the meta loop.
///
/// Aggregates metrics, coverage, erosion, pipeline progress, and
/// queue status into a single response. Designed to be the first
/// call in every meta loop iteration.
#[utoipa::path(
    get,
    path = "/admin/health-report",
    responses(
        (status = 200, description = "Meta loop health report",
         body = serde_json::Value),
    ),
    tag = "admin"
)]
pub async fn health_report(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Metrics
    let graph = state.graph_engine.stats().await.ok();
    let source_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sources")
        .fetch_one(state.repo.pool())
        .await
        .unwrap_or(0);

    // Domain distribution
    let domain_dist: Vec<(String, i64)> = sqlx::query_as(
        "SELECT COALESCE(domain, 'unknown'), COUNT(*) FROM sources GROUP BY 1 ORDER BY 2 DESC",
    )
    .fetch_all(state.repo.pool())
    .await
    .unwrap_or_default();

    // Entity class distribution
    let class_dist: Vec<(String, i64)> = sqlx::query_as(
        "SELECT COALESCE(entity_class, 'unknown'), COUNT(*) FROM nodes GROUP BY 1 ORDER BY 2 DESC",
    )
    .fetch_all(state.repo.pool())
    .await
    .unwrap_or_default();

    // Pipeline progress
    let entity_summaries: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM nodes WHERE properties->>'semantic_summary' IS NOT NULL AND entity_class = 'code'",
    )
    .fetch_one(state.repo.pool())
    .await
    .unwrap_or(0);

    let total_code_entities: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE entity_class = 'code'")
            .fetch_one(state.repo.pool())
            .await
            .unwrap_or(0);

    let file_summaries: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sources WHERE domain = 'code' AND summary IS NOT NULL",
    )
    .fetch_one(state.repo.pool())
    .await
    .unwrap_or(0);

    let total_code_files: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sources WHERE domain = 'code'")
            .fetch_one(state.repo.pool())
            .await
            .unwrap_or(0);

    // Queue status
    let queue_rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT kind::text, status::text, COUNT(*) FROM retry_jobs GROUP BY 1, 2 ORDER BY 1, 2",
    )
    .fetch_all(state.repo.pool())
    .await
    .unwrap_or_default();

    Ok(Json(serde_json::json!({
        "graph": {
            "nodes": graph.as_ref().map(|g| g.node_count).unwrap_or(0),
            "edges": graph.as_ref().map(|g| g.edge_count).unwrap_or(0),
            "components": graph.as_ref().map(|g| g.component_count).unwrap_or(0),
        },
        "sources": {
            "total": source_count,
            "domains": domain_dist.into_iter().collect::<std::collections::HashMap<_, _>>(),
        },
        "entities": {
            "total": class_dist.iter().map(|(_, c)| c).sum::<i64>(),
            "classes": class_dist.into_iter().collect::<std::collections::HashMap<_, _>>(),
        },
        "pipeline": {
            "entity_summaries": entity_summaries,
            "total_code_entities": total_code_entities,
            "entity_summary_pct": if total_code_entities > 0 {
                (entity_summaries as f64 / total_code_entities as f64 * 100.0).round()
            } else { 0.0 },
            "file_summaries": file_summaries,
            "total_code_files": total_code_files,
            "file_summary_pct": if total_code_files > 0 {
                (file_summaries as f64 / total_code_files as f64 * 100.0).round()
            } else { 0.0 },
        },
        "queue": queue_rows.iter().map(|(k, s, c)| {
            serde_json::json!({"kind": k, "status": s, "count": c})
        }).collect::<Vec<_>>(),
    })))
}

/// Enqueue semantic summary jobs for all unsummarized code entities.
#[utoipa::path(
    post,
    path = "/admin/queue/summarize-all",
    responses(
        (status = 200, description = "Summary jobs enqueued",
         body = serde_json::Value),
    ),
    tag = "admin"
)]
pub async fn enqueue_summarize_all(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let enqueued = state.queue_service.enqueue_summarize_all().await?;
    Ok(Json(serde_json::json!({ "enqueued": enqueued })))
}

/// Enqueue source summary composition for all code sources with entity summaries.
#[utoipa::path(
    post,
    path = "/admin/queue/compose-all",
    responses(
        (status = 200, description = "Compose jobs enqueued",
         body = serde_json::Value),
    ),
    tag = "admin"
)]
pub async fn enqueue_compose_all(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let enqueued = state.queue_service.enqueue_compose_all().await?;
    Ok(Json(serde_json::json!({ "enqueued": enqueued })))
}

/// Clear the dead-letter queue.
#[utoipa::path(
    post,
    path = "/admin/queue/dead/clear",
    request_body = ClearDeadRequest,
    responses(
        (status = 200, description = "Dead jobs cleared",
         body = ClearDeadResponse),
    ),
    tag = "admin"
)]
pub async fn clear_dead_handler(
    State(state): State<AppState>,
    Json(req): Json<ClearDeadRequest>,
) -> Result<Json<ClearDeadResponse>, ApiError> {
    let kind = req
        .kind
        .as_deref()
        .and_then(covalence_core::models::retry_job::JobKind::from_pg_str);
    let deleted = state.queue_service.clear_dead(kind).await?;
    Ok(Json(ClearDeadResponse { deleted }))
}

/// Resurrect dead jobs — reset to pending so they retry.
#[utoipa::path(
    post,
    path = "/admin/queue/dead/resurrect",
    request_body = ClearDeadRequest,
    responses(
        (status = 200, description = "Dead jobs resurrected",
         body = ResurrectDeadResponse),
    ),
    tag = "admin"
)]
pub async fn resurrect_dead_handler(
    State(state): State<AppState>,
    Json(req): Json<ClearDeadRequest>,
) -> Result<Json<ResurrectDeadResponse>, ApiError> {
    let kind = req
        .kind
        .as_deref()
        .and_then(covalence_core::models::retry_job::JobKind::from_pg_str);
    let resurrected = state.queue_service.resurrect_dead(kind).await?;
    Ok(Json(ResurrectDeadResponse { resurrected }))
}

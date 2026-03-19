//! Cross-domain analysis handlers.

use axum::Json;
use axum::extract::State;

use crate::error::ApiError;
use crate::handlers::dto::{
    AffectedNodeResponse, BlastRadiusHopResponse, BlastRadiusRequest, BlastRadiusResponse,
    BootstrapResponse, CounterArgumentResponse, CoverageItemResponse, CoverageResponse,
    CritiqueEvidenceResponse, CritiqueRequest, CritiqueResponse, CritiqueSynthesisResponse,
    DivergentNodeResponse, ErosionItemResponse, ErosionRequest, ErosionResponse,
    LinkDomainsRequest, LinkDomainsResponse, SupportingArgumentResponse, VerificationMatchResponse,
    VerifyRequest, VerifyResponse, WhitespaceGapResponse, WhitespaceNodeResponse,
    WhitespaceRequest, WhitespaceResponse,
};
use crate::handlers::dto::{AlignmentReportResponse, AlignmentRequest as AlignmentApiRequest};
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
    let max_edges = req.max_edges_per_component.unwrap_or(100).clamp(1, 500);
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
    let include_invalidated = req.include_invalidated.unwrap_or(false);
    let result = state
        .analysis_service
        .blast_radius(&req.target, max_hops, include_invalidated)
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

/// Detect research areas with no corresponding implementation.
#[utoipa::path(
    post,
    path = "/analysis/whitespace",
    request_body = WhitespaceRequest,
    responses(
        (status = 200, description = "Whitespace roadmap results",
         body = WhitespaceResponse),
    ),
    tag = "analysis"
)]
pub async fn whitespace_roadmap(
    State(state): State<AppState>,
    Json(req): Json<WhitespaceRequest>,
) -> Result<Json<WhitespaceResponse>, ApiError> {
    let min_cluster = req.min_cluster_size.unwrap_or(3);
    let result = state
        .analysis_service
        .whitespace_roadmap(min_cluster, req.domain.as_deref())
        .await?;
    Ok(Json(WhitespaceResponse {
        gaps: result
            .gaps
            .into_iter()
            .map(|g| WhitespaceGapResponse {
                source_id: g.source_id,
                title: g.title,
                uri: g.uri,
                node_count: g.node_count,
                representative_nodes: g
                    .representative_nodes
                    .into_iter()
                    .map(|n| WhitespaceNodeResponse {
                        name: n.name,
                        node_type: n.node_type,
                    })
                    .collect(),
                connected_components: g.connected_components,
                connected_spec_topics: g.connected_spec_topics,
                assessment: g.assessment,
            })
            .collect(),
        total_research_sources: result.total_research_sources,
        unbridged_sources: result.unbridged_sources,
        whitespace_score: result.whitespace_score,
    }))
}

/// Verify research-to-execution alignment through the Component bridge.
#[utoipa::path(
    post,
    path = "/analysis/verify",
    request_body = VerifyRequest,
    responses(
        (status = 200, description = "Verification results",
         body = VerifyResponse),
    ),
    tag = "analysis"
)]
pub async fn verify_implementation(
    State(state): State<AppState>,
    Json(req): Json<VerifyRequest>,
) -> Result<Json<VerifyResponse>, ApiError> {
    let result = state
        .analysis_service
        .verify_implementation(&req.research_query, req.component.as_deref())
        .await?;

    let to_match =
        |m: covalence_core::services::analysis::VerificationMatch| VerificationMatchResponse {
            node_id: m.node_id,
            name: m.name,
            node_type: m.node_type,
            summary: m.summary,
            distance: m.distance,
            domain: m.domain,
        };

    Ok(Json(VerifyResponse {
        research_query: result.research_query,
        research_matches: result.research_matches.into_iter().map(to_match).collect(),
        code_matches: result.code_matches.into_iter().map(to_match).collect(),
        alignment_score: result.alignment_score,
        component: result.component,
    }))
}

/// Generate a dialectical critique of a design proposal.
#[utoipa::path(
    post,
    path = "/analysis/critique",
    request_body = CritiqueRequest,
    responses(
        (status = 200, description = "Dialectical critique results",
         body = CritiqueResponse),
    ),
    tag = "analysis"
)]
pub async fn critique(
    State(state): State<AppState>,
    Json(req): Json<CritiqueRequest>,
) -> Result<Json<CritiqueResponse>, ApiError> {
    let result = state.analysis_service.critique(&req.proposal).await?;

    let to_evidence =
        |e: covalence_core::services::analysis::CritiqueEvidence| CritiqueEvidenceResponse {
            node_id: e.node_id,
            name: e.name,
            node_type: e.node_type,
            description: e.description,
            distance: e.distance,
            domain: e.domain,
        };

    Ok(Json(CritiqueResponse {
        proposal: result.proposal,
        research_evidence: result
            .research_evidence
            .into_iter()
            .map(to_evidence)
            .collect(),
        spec_evidence: result.spec_evidence.into_iter().map(to_evidence).collect(),
        code_evidence: result.code_evidence.into_iter().map(to_evidence).collect(),
        synthesis: result.synthesis.map(|s| CritiqueSynthesisResponse {
            counter_arguments: s
                .counter_arguments
                .into_iter()
                .map(|a| CounterArgumentResponse {
                    claim: a.claim,
                    evidence: a.evidence,
                    strength: a.strength,
                })
                .collect(),
            supporting_arguments: s
                .supporting_arguments
                .into_iter()
                .map(|a| SupportingArgumentResponse {
                    claim: a.claim,
                    evidence: a.evidence,
                })
                .collect(),
            recommendation: s.recommendation,
        }),
    }))
}

/// Cross-domain alignment report.
///
/// Compares entities across spec, design, code, and research domains
/// to surface misalignments.
#[utoipa::path(
    post,
    path = "/analysis/alignment",
    request_body = AlignmentApiRequest,
    responses(
        (status = 200, description = "Alignment report",
         body = AlignmentReportResponse),
    ),
    tag = "analysis"
)]
pub async fn alignment_report(
    State(state): State<AppState>,
    Json(req): Json<AlignmentApiRequest>,
) -> Result<Json<AlignmentReportResponse>, ApiError> {
    let report = state
        .analysis_service
        .alignment_report(&covalence_core::services::analysis::AlignmentRequest {
            checks: req.checks.unwrap_or_default(),
            min_similarity: req.min_similarity.unwrap_or(0.4),
            limit: req.limit.unwrap_or(20),
        })
        .await?;

    Ok(Json(AlignmentReportResponse {
        code_ahead: report.code_ahead.into_iter().map(Into::into).collect(),
        spec_ahead: report.spec_ahead.into_iter().map(Into::into).collect(),
        design_contradicted: report
            .design_contradicted
            .into_iter()
            .map(Into::into)
            .collect(),
        stale_design: report.stale_design.into_iter().map(Into::into).collect(),
    }))
}

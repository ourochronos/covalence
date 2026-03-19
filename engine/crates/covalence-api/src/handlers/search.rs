//! Search handlers.

use axum::Json;
use axum::extract::State;
use chrono::DateTime;

use crate::error::ApiError;
use crate::handlers::dto::{
    ContextItemResponse, ContextResponse, FeedbackResponse, SearchApiResponse,
    SearchFeedbackRequest, SearchGranularity, SearchMode, SearchRequest, SearchResultResponse,
};
use crate::state::AppState;

/// Execute a multi-dimensional fused search.
///
/// Supports two delivery modes via the `mode` field:
/// - `results` (default): returns ranked `SearchResultResponse`
///   items.
/// - `context`: assembles results into a deduplicated,
///   budget-trimmed context window.
///
/// The `granularity` field controls content resolution:
/// - `section` (default): paragraph chunks are promoted to
///   their parent section content.
/// - `paragraph`: matched chunk content as-is.
/// - `source`: full source `normalized_content`.
#[utoipa::path(
    post,
    path = "/search",
    request_body = SearchRequest,
    responses(
        (status = 200, description = "Search results or assembled context",
         body = SearchApiResponse),
    ),
    tag = "search"
)]
pub async fn search(
    State(state): State<AppState>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<SearchApiResponse>, ApiError> {
    if req.query.trim().is_empty() {
        return Err(ApiError::from(covalence_core::error::Error::InvalidInput(
            "query must not be empty".to_string(),
        )));
    }

    let strategy = match req.strategy.as_deref() {
        Some("balanced") => covalence_core::search::strategy::SearchStrategy::Balanced,
        Some("precise") => covalence_core::search::strategy::SearchStrategy::Precise,
        Some("exploratory") => covalence_core::search::strategy::SearchStrategy::Exploratory,
        Some("recent") => covalence_core::search::strategy::SearchStrategy::Recent,
        Some("graph_first") => covalence_core::search::strategy::SearchStrategy::GraphFirst,
        Some("global") => covalence_core::search::strategy::SearchStrategy::Global,
        Some("custom") => {
            let w = req.weights.as_ref().ok_or_else(|| {
                ApiError::from(covalence_core::error::Error::InvalidInput(
                    "strategy 'custom' requires a 'weights' object".to_string(),
                ))
            })?;
            covalence_core::search::strategy::SearchStrategy::Custom(
                covalence_core::search::strategy::DimensionWeights {
                    vector: w.vector,
                    lexical: w.lexical,
                    temporal: w.temporal,
                    graph: w.graph,
                    structural: w.structural,
                    global: w.global,
                },
            )
        }
        _ => covalence_core::search::strategy::SearchStrategy::Auto,
    };

    // Build optional filters from request fields.
    let date_range = match (&req.date_range_start, &req.date_range_end) {
        (Some(start), Some(end)) => {
            let s = DateTime::parse_from_rfc3339(start)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .map_err(|e| {
                    ApiError::from(covalence_core::error::Error::InvalidInput(format!(
                        "invalid date_range_start: {e}"
                    )))
                })?;
            let e = DateTime::parse_from_rfc3339(end)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .map_err(|e| {
                    ApiError::from(covalence_core::error::Error::InvalidInput(format!(
                        "invalid date_range_end: {e}"
                    )))
                })?;
            Some((s, e))
        }
        (Some(_), None) | (None, Some(_)) => {
            return Err(ApiError::from(covalence_core::error::Error::InvalidInput(
                "both date_range_start and date_range_end must be \
                     provided for date filtering"
                    .to_string(),
            )));
        }
        _ => None,
    };

    if let Some(mc) = req.min_confidence {
        if !mc.is_finite() || !(0.0..=1.0).contains(&mc) {
            return Err(ApiError::from(covalence_core::error::Error::InvalidInput(
                format!("min_confidence must be finite and in [0.0, 1.0], got {mc}"),
            )));
        }
    }

    // Parse optional graph_view parameter.
    let graph_view = if let Some(ref gv) = req.graph_view {
        let parsed =
            covalence_core::search::dimensions::GraphView::from_str_opt(gv).ok_or_else(|| {
                ApiError::from(covalence_core::error::Error::InvalidInput(format!(
                    "invalid graph_view: {gv}. \
                                 Expected: causal, temporal, \
                                 entity, structural, all"
                )))
            })?;
        Some(parsed)
    } else {
        None
    };

    let filters = if req.min_confidence.is_some()
        || req.node_types.is_some()
        || req.entity_classes.is_some()
        || req.source_types.is_some()
        || req.source_layers.is_some()
        || date_range.is_some()
        || graph_view.is_some()
    {
        Some(covalence_core::services::SearchFilters {
            min_confidence: req.min_confidence,
            node_types: req.node_types,
            entity_classes: req.entity_classes,
            date_range,
            source_types: req.source_types,
            source_layers: req.source_layers,
            graph_view,
        })
    } else {
        None
    };

    let limit = req.limit.unwrap_or(10).min(200);
    let mut results = if req.hierarchical {
        state
            .search_service
            .search_hierarchical(&req.query, strategy, limit, filters)
            .await?
    } else {
        state
            .search_service
            .search(&req.query, strategy, limit, filters)
            .await?
    };

    // --- Granularity adjustment ---
    apply_granularity(&state, &req.granularity, &mut results).await;

    // --- Post-granularity quality filter ---
    // Granularity promotion can replace paragraph content with its
    // parent section content (e.g., a bibliography section). Re-check
    // quality after promotion to catch these.  Only apply to chunk-type
    // results — sections, statements, and nodes have different content
    // characteristics and are already quality-filtered during ingestion.
    {
        use covalence_core::services::chunk_quality::{
            is_author_block, is_bibliography_entry, is_boilerplate_heavy, is_metadata_only,
            is_reference_section, is_title_only,
        };
        results.retain(|r| {
            let is_chunk = r.result_type.as_deref().is_none_or(|rt| rt == "chunk");
            if !is_chunk {
                return true;
            }
            let content = match r.content.as_deref() {
                Some(c) => c,
                None => return true,
            };
            !(is_bibliography_entry(content)
                || is_reference_section(content)
                || is_boilerplate_heavy(content)
                || is_metadata_only(content)
                || is_title_only(content)
                || is_author_block(content))
        });
    }

    // --- Delivery mode ---
    match req.mode {
        SearchMode::Context => {
            let assembled = state.search_service.assemble_context(&results, None).await;
            Ok(Json(SearchApiResponse::Context(ContextResponse {
                items: assembled
                    .items
                    .into_iter()
                    .map(|item| ContextItemResponse {
                        ref_number: item.ref_number,
                        content: item.content,
                        source_title: item.source_title,
                        source_id: item.source_id,
                        score: item.score,
                        token_count: item.token_count,
                    })
                    .collect(),
                total_tokens: assembled.total_tokens,
                items_dropped: assembled.items_dropped,
                duplicates_removed: assembled.duplicates_removed,
            })))
        }
        SearchMode::Results => Ok(Json(SearchApiResponse::Results(
            results
                .into_iter()
                .map(SearchResultResponse::from)
                .collect(),
        ))),
    }
}

/// Apply granularity adjustments to search result content.
///
/// - `Section`: for paragraph-level chunks, replace content with
///   the parent section's content.
/// - `Paragraph`: no change (default enrichment is paragraph).
/// - `Source`: replace content with full source normalized_content.
async fn apply_granularity(
    state: &AppState,
    granularity: &SearchGranularity,
    results: &mut [covalence_core::search::fusion::FusedResult],
) {
    use covalence_core::storage::traits::{ChunkRepo, SourceRepo};
    use covalence_core::types::ids::ChunkId;

    match granularity {
        SearchGranularity::Section => {
            // For paragraph-level chunks, walk up to parent
            // section and use its content.
            for result in results.iter_mut() {
                let is_chunk = result.result_type.as_deref().is_none_or(|rt| rt == "chunk");
                if !is_chunk {
                    continue;
                }
                if let Ok(Some(chunk)) =
                    ChunkRepo::get(&*state.repo, ChunkId::from_uuid(result.id)).await
                {
                    if chunk.level != "paragraph" {
                        continue;
                    }
                    if let Some(parent_id) = chunk.parent_chunk_id {
                        if let Ok(Some(parent)) = ChunkRepo::get(&*state.repo, parent_id).await {
                            result.content = Some(parent.content.clone());
                        }
                    }
                }
            }
        }
        SearchGranularity::Paragraph => {
            // Content is already at paragraph level from
            // enrichment — no adjustment needed.
        }
        SearchGranularity::Source => {
            // Replace content with full source normalized_content.
            for result in results.iter_mut() {
                let is_chunk = result.result_type.as_deref().is_none_or(|rt| rt == "chunk");
                if !is_chunk {
                    continue;
                }
                if let Ok(Some(chunk)) =
                    ChunkRepo::get(&*state.repo, ChunkId::from_uuid(result.id)).await
                {
                    if let Ok(Some(source)) = SourceRepo::get(&*state.repo, chunk.source_id).await {
                        if let Some(ref nc) = source.normalized_content {
                            result.content = Some(nc.clone());
                        }
                    }
                }
            }
        }
    }
}

/// Submit search feedback.
#[utoipa::path(
    post,
    path = "/search/feedback",
    request_body = SearchFeedbackRequest,
    responses(
        (status = 200, description = "Feedback recorded",
         body = FeedbackResponse),
    ),
    tag = "search"
)]
pub async fn search_feedback(
    State(state): State<AppState>,
    Json(req): Json<SearchFeedbackRequest>,
) -> Result<Json<FeedbackResponse>, ApiError> {
    if !req.relevance.is_finite() || !(0.0..=1.0).contains(&req.relevance) {
        return Err(ApiError::from(covalence_core::error::Error::InvalidInput(
            format!(
                "relevance must be finite and in [0.0, 1.0], got {}",
                req.relevance
            ),
        )));
    }

    let feedback = covalence_core::models::trace::SearchFeedback::new(
        req.query,
        req.result_id,
        req.relevance,
        req.comment,
    );
    state.admin_service.submit_feedback(feedback).await?;
    Ok(Json(FeedbackResponse { recorded: true }))
}

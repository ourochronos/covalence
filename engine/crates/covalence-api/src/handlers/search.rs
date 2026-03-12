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
    let strategy = match req.strategy.as_deref() {
        Some("balanced") => covalence_core::search::strategy::SearchStrategy::Balanced,
        Some("precise") => covalence_core::search::strategy::SearchStrategy::Precise,
        Some("exploratory") => covalence_core::search::strategy::SearchStrategy::Exploratory,
        Some("recent") => covalence_core::search::strategy::SearchStrategy::Recent,
        Some("graph_first") => covalence_core::search::strategy::SearchStrategy::GraphFirst,
        Some("global") => covalence_core::search::strategy::SearchStrategy::Global,
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
        _ => None,
    };

    let filters =
        if req.min_confidence.is_some() || req.node_types.is_some() || date_range.is_some() {
            Some(covalence_core::services::SearchFilters {
                min_confidence: req.min_confidence,
                node_types: req.node_types,
                date_range,
            })
        } else {
            None
        };

    let limit = req.limit.unwrap_or(10);
    let mut results = state
        .search_service
        .search(&req.query, strategy, limit, filters)
        .await?;

    // --- Granularity adjustment ---
    apply_granularity(&state, &req.granularity, &mut results).await;

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
                .map(|r| SearchResultResponse {
                    id: r.id,
                    fused_score: r.fused_score,
                    confidence: r.confidence,
                    entity_type: r.entity_type,
                    name: r.name,
                    snippet: r.snippet,
                    content: r.content,
                    source_uri: r.source_uri,
                    source_title: r.source_title,
                    dimension_scores: r.dimension_scores,
                    dimension_ranks: r.dimension_ranks,
                })
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
    let feedback = covalence_core::models::trace::SearchFeedback::new(
        req.query,
        req.result_id,
        req.relevance,
        req.comment,
    );
    state.admin_service.submit_feedback(feedback).await?;
    Ok(Json(FeedbackResponse { recorded: true }))
}

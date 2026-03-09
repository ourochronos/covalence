//! Search handlers.

use axum::Json;
use axum::extract::State;
use chrono::DateTime;

use crate::error::ApiError;
use crate::handlers::dto::{SearchRequest, SearchResultResponse};
use crate::state::AppState;

/// Execute a multi-dimensional fused search.
#[utoipa::path(
    post,
    path = "/search",
    request_body = SearchRequest,
    responses(
        (status = 200, description = "Search results", body = Vec<SearchResultResponse>),
    ),
    tag = "search"
)]
pub async fn search(
    State(state): State<AppState>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<Vec<SearchResultResponse>>, ApiError> {
    let strategy = match req.strategy.as_deref() {
        Some("precise") => covalence_core::search::strategy::SearchStrategy::Precise,
        Some("exploratory") => covalence_core::search::strategy::SearchStrategy::Exploratory,
        Some("recent") => covalence_core::search::strategy::SearchStrategy::Recent,
        Some("graph_first") => covalence_core::search::strategy::SearchStrategy::GraphFirst,
        _ => covalence_core::search::strategy::SearchStrategy::Balanced,
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
    let results = state
        .search_service
        .search(&req.query, strategy, limit, filters)
        .await?;

    Ok(Json(
        results
            .into_iter()
            .map(|r| SearchResultResponse {
                id: r.id,
                fused_score: r.fused_score,
                confidence: r.confidence,
                entity_type: r.entity_type,
                name: r.name,
                snippet: r.snippet,
                source_uri: r.source_uri,
                dimension_scores: r.dimension_scores,
                dimension_ranks: r.dimension_ranks,
            })
            .collect(),
    ))
}

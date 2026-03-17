//! Ask handler — LLM-powered knowledge synthesis endpoint.

use axum::Json;
use axum::extract::State;

use crate::error::ApiError;
use crate::handlers::dto::{AskApiRequest, AskApiResponse, CitationResponse};
use crate::state::AppState;

/// Synthesize an answer from the knowledge graph.
///
/// Searches across all dimensions, enriches context with provenance
/// and confidence metadata, and sends to an LLM for grounded
/// synthesis.
#[utoipa::path(
    post,
    path = "/ask",
    request_body = AskApiRequest,
    responses(
        (status = 200, description = "Synthesized answer with citations",
         body = AskApiResponse),
    ),
    tag = "search"
)]
pub async fn ask(
    State(state): State<AppState>,
    Json(req): Json<AskApiRequest>,
) -> Result<Json<AskApiResponse>, ApiError> {
    if req.question.trim().is_empty() {
        return Err(ApiError::from(covalence_core::error::Error::InvalidInput(
            "question must not be empty".to_string(),
        )));
    }

    let ask_service = state.ask_service.as_ref().ok_or_else(|| {
        ApiError::from(covalence_core::error::Error::Config(
            "no LLM backend configured — the /ask endpoint \
             requires a chat backend"
                .to_string(),
        ))
    })?;

    let options = covalence_core::services::ask::AskOptions {
        max_context: req.max_context.unwrap_or(15),
        strategy: req.strategy.clone(),
    };

    let response = ask_service.ask(&req.question, options).await?;

    Ok(Json(AskApiResponse {
        answer: response.answer,
        citations: response
            .citations
            .into_iter()
            .map(|c| CitationResponse {
                source: c.source,
                snippet: c.snippet,
                result_type: c.result_type,
                confidence: c.confidence,
            })
            .collect(),
        context_used: response.context_used,
    }))
}

//! Ask handler — LLM-powered knowledge synthesis endpoint.

use std::convert::Infallible;

use axum::Json;
use axum::extract::State;
use axum::response::sse::{Event, Sse};
use tokio_stream::StreamExt;

use crate::error::{ApiError, validate_request};
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
    validate_request(&req)?;

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

    let session_id = req
        .session_id
        .as_deref()
        .and_then(|s| s.parse::<uuid::Uuid>().ok());

    let options = covalence_core::services::ask::AskOptions {
        max_context: req.max_context.unwrap_or(15),
        strategy: req.strategy.clone(),
        model: req.model.clone(),
        session_id,
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

/// Stream an answer as Server-Sent Events.
///
/// Same search + enrichment as [`ask`], but the LLM synthesis is
/// streamed token-by-token. Events:
///   - `context`: emitted once with citations and context count
///   - `token`: emitted per LLM output chunk
///   - `done`: emitted when synthesis completes
///   - `error`: emitted if something goes wrong mid-stream
#[utoipa::path(
    post,
    path = "/ask/stream",
    request_body = AskApiRequest,
    responses(
        (status = 200, description = "Server-Sent Event stream"),
    ),
    tag = "search"
)]
pub async fn ask_stream(
    State(state): State<AppState>,
    Json(req): Json<AskApiRequest>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    validate_request(&req)?;

    if req.question.trim().is_empty() {
        return Err(ApiError::from(covalence_core::error::Error::InvalidInput(
            "question must not be empty".to_string(),
        )));
    }

    let ask_service = state.ask_service.as_ref().ok_or_else(|| {
        ApiError::from(covalence_core::error::Error::Config(
            "no LLM backend configured — the /ask/stream \
             endpoint requires a chat backend"
                .to_string(),
        ))
    })?;

    let session_id = req
        .session_id
        .as_deref()
        .and_then(|s| s.parse::<uuid::Uuid>().ok());

    let options = covalence_core::services::ask::AskOptions {
        max_context: req.max_context.unwrap_or(15),
        strategy: req.strategy.clone(),
        model: req.model.clone(),
        session_id,
    };

    let event_stream = ask_service.ask_stream(&req.question, options).await?;

    // Map AskStreamEvents to SSE Events.
    let sse_stream = event_stream.map(|event| {
        use covalence_core::services::ask::AskStreamEvent;

        let sse_event = match &event {
            AskStreamEvent::Context { .. } => Event::default()
                .event("context")
                .json_data(&event)
                .unwrap_or_else(|_| Event::default().event("error").data("serialization error")),
            AskStreamEvent::Token { .. } => Event::default()
                .event("token")
                .json_data(&event)
                .unwrap_or_else(|_| Event::default().event("error").data("serialization error")),
            AskStreamEvent::Done { .. } => Event::default()
                .event("done")
                .json_data(&event)
                .unwrap_or_else(|_| Event::default().event("error").data("serialization error")),
            AskStreamEvent::Error { .. } => Event::default()
                .event("error")
                .json_data(&event)
                .unwrap_or_else(|_| Event::default().event("error").data("serialization error")),
        };
        Ok(sse_event)
    });

    Ok(Sse::new(sse_stream))
}

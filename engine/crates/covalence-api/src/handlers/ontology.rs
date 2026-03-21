//! Ontology endpoints — view and manage the knowledge schema.

use axum::Json;
use axum::extract::State;

use crate::error::ApiError;
use crate::state::AppState;

/// Get the full ontology (categories, entity types, relationships, domains, views).
#[utoipa::path(
    get,
    path = "/admin/ontology",
    responses(
        (status = 200, description = "Full ontology schema",
         body = serde_json::Value),
    ),
    tag = "admin"
)]
pub async fn get_ontology(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cache = state.ontology_service.get().await;
    Ok(Json(serde_json::json!({
        "categories": cache.categories,
        "entity_types": cache.entity_types,
        "rel_universals": cache.rel_universals,
        "rel_types": cache.rel_types,
        "domains": cache.domains,
        "view_edges": cache.view_edges.iter()
            .map(|(k, v)| (k.clone(), v.iter().cloned().collect::<Vec<_>>()))
            .collect::<std::collections::HashMap<_, _>>(),
    })))
}

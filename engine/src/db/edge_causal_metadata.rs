//! Database access for `covalence.edge_causal_metadata` (covalence#116).
//!
//! Three async operations:
//! * [`get_by_edge_id`] — fetch the enrichment row for a given edge (if any).
//! * [`upsert`]         — insert or update a causal metadata row.
//! * [`delete_by_edge_id`] — remove a row (normally handled by FK CASCADE, but
//!   exposed so callers can strip metadata without deleting the underlying edge).

use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{
    CausalEvidenceType, CausalLevel, EdgeCausalMetadata, EdgeCausalMetadataUpsert,
};

// =============================================================================
// get_by_edge_id
// =============================================================================

/// Fetch the causal metadata row for `edge_id`, or `None` if no row exists.
pub async fn get_by_edge_id(
    pool: &PgPool,
    edge_id: Uuid,
) -> Result<Option<EdgeCausalMetadata>, sqlx::Error> {
    sqlx::query_as::<_, EdgeCausalMetadata>(
        "SELECT edge_id, causal_level, causal_strength, evidence_type,
                direction_conf, hidden_conf_risk, temporal_lag_ms,
                created_at, updated_at
         FROM covalence.edge_causal_metadata
         WHERE edge_id = $1",
    )
    .bind(edge_id)
    .fetch_optional(pool)
    .await
}

// =============================================================================
// upsert
// =============================================================================

/// Insert a new causal metadata row or update all mutable fields if a row
/// with the same `edge_id` already exists.  Returns the resulting row.
pub async fn upsert(
    pool: &PgPool,
    payload: &EdgeCausalMetadataUpsert,
) -> Result<EdgeCausalMetadata, sqlx::Error> {
    let causal_level = payload.causal_level.unwrap_or(CausalLevel::Association);
    let causal_strength = payload.causal_strength.unwrap_or(0.5);
    let evidence_type = payload
        .evidence_type
        .unwrap_or(CausalEvidenceType::StructuralPrior);
    let direction_conf = payload.direction_conf.unwrap_or(0.5);
    let hidden_conf_risk = payload.hidden_conf_risk.unwrap_or(0.5);
    let temporal_lag_ms = payload.temporal_lag_ms;

    sqlx::query_as::<_, EdgeCausalMetadata>(
        "INSERT INTO covalence.edge_causal_metadata
             (edge_id, causal_level, causal_strength, evidence_type,
              direction_conf, hidden_conf_risk, temporal_lag_ms)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (edge_id) DO UPDATE SET
             causal_level     = EXCLUDED.causal_level,
             causal_strength  = EXCLUDED.causal_strength,
             evidence_type    = EXCLUDED.evidence_type,
             direction_conf   = EXCLUDED.direction_conf,
             hidden_conf_risk = EXCLUDED.hidden_conf_risk,
             temporal_lag_ms  = EXCLUDED.temporal_lag_ms,
             updated_at       = NOW()
         RETURNING edge_id, causal_level, causal_strength, evidence_type,
                   direction_conf, hidden_conf_risk, temporal_lag_ms,
                   created_at, updated_at",
    )
    .bind(payload.edge_id)
    .bind(causal_level)
    .bind(causal_strength)
    .bind(evidence_type)
    .bind(direction_conf)
    .bind(hidden_conf_risk)
    .bind(temporal_lag_ms)
    .fetch_one(pool)
    .await
}

// =============================================================================
// delete_by_edge_id
// =============================================================================

/// Delete the causal metadata row for `edge_id`.
///
/// Returns the number of rows deleted (0 or 1).  In normal usage the FK
/// `ON DELETE CASCADE` handles deletion automatically when the parent edge is
/// deleted; this function is provided for cases where only the enrichment
/// should be stripped while the edge itself is retained.
pub async fn delete_by_edge_id(pool: &PgPool, edge_id: Uuid) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM covalence.edge_causal_metadata WHERE edge_id = $1")
        .bind(edge_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

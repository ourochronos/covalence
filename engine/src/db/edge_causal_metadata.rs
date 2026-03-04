//! Database access for `covalence.edge_causal_metadata` (covalence#116).
//!
//! Three async operations:
//! * [`get_by_edge_id`] — fetch the enrichment row for a given edge (if any).
//! * [`upsert`]         — partial-update upsert via `covalence.upsert_causal_metadata`
//!                        stored procedure (covalence#143, #145).
//! * [`delete_by_edge_id`] — remove a row (normally handled by FK CASCADE, but
//!   exposed so callers can strip metadata without deleting the underlying edge).

use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{EdgeCausalMetadata, EdgeCausalMetadataPatch};

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
                notes, created_at, updated_at
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

/// Insert a new causal metadata row or partially update an existing one.
///
/// Delegates to the `covalence.upsert_causal_metadata` stored procedure
/// (migration 035, covalence#143).  Fields present in `payload` are written;
/// `None` fields preserve the current database value via `COALESCE`, fixing
/// the silent-reset bug tracked as covalence#145.
///
/// Returns the resulting row.
pub async fn upsert(
    pool: &PgPool,
    payload: &EdgeCausalMetadataPatch,
) -> Result<EdgeCausalMetadata, sqlx::Error> {
    sqlx::query_as::<_, EdgeCausalMetadata>(
        "SELECT * FROM covalence.upsert_causal_metadata($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(payload.edge_id)
    .bind(payload.causal_level)
    .bind(payload.evidence_type)
    .bind(payload.causal_strength)
    .bind(payload.direction_conf)
    .bind(payload.hidden_conf_risk)
    .bind(payload.temporal_lag_ms)
    .bind(payload.notes.as_deref())
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

//! Temporal search dimension — recency and time-range scoring.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::{DimensionKind, SearchDimension, SearchQuery};
use crate::error::Result;
use crate::search::SearchResult;

/// Time-based scoring using recency decay or explicit time-range filtering.
///
/// When a `time_range` is provided, only chunks within the range are
/// returned and scored by their position within it. Otherwise, all
/// chunks are scored by recency: `score = 1.0 / (1.0 + days_old)`.
pub struct TemporalDimension {
    pool: PgPool,
}

impl TemporalDimension {
    /// Create a new temporal search dimension.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl SearchDimension for TemporalDimension {
    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        let now = Utc::now();

        let rows: Vec<(Uuid, DateTime<Utc>)> = if let Some((start, end)) = query.time_range {
            sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
                "SELECT id, created_at \
                     FROM chunks \
                     WHERE created_at >= $1 AND created_at <= $2 \
                     ORDER BY created_at DESC \
                     LIMIT $3",
            )
            .bind(start)
            .bind(end)
            .bind(query.limit as i64)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
                "SELECT id, created_at \
                     FROM chunks \
                     ORDER BY created_at DESC \
                     LIMIT $1",
            )
            .bind(query.limit as i64)
            .fetch_all(&self.pool)
            .await?
        };

        let results = rows
            .into_iter()
            .enumerate()
            .map(|(i, (id, created_at))| {
                let elapsed_secs = (now - created_at).num_seconds();
                let score = if elapsed_secs < -60 {
                    // Future timestamp (>60s ahead) — likely data
                    // corruption or clock skew. Demote rather than
                    // rewarding with max recency.
                    tracing::debug!(
                        chunk_id = %id,
                        seconds_in_future = -elapsed_secs,
                        "chunk has future timestamp, demoting"
                    );
                    0.1
                } else {
                    let days_old = elapsed_secs.max(0) as f64 / 86400.0;
                    1.0 / (1.0 + days_old)
                };
                SearchResult {
                    id,
                    score,
                    rank: i + 1,
                    dimension: "temporal".to_string(),
                    snippet: None,
                    result_type: Some("chunk".to_string()),
                }
            })
            .collect();

        Ok(results)
    }

    fn kind(&self) -> DimensionKind {
        DimensionKind::Temporal
    }
}

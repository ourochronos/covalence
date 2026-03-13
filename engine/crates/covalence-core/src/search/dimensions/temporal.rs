//! Temporal search dimension — recency and time-range scoring.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::{DimensionKind, SearchDimension, SearchQuery};
use crate::error::Result;
use crate::search::SearchResult;

/// Compute recency score from elapsed seconds.
///
/// Returns `0.1` for future timestamps (>60s ahead, likely clock skew),
/// and `1.0 / (1.0 + days_old)` otherwise. Result is in `(0, 1]`.
pub fn recency_score(elapsed_secs: i64) -> f64 {
    if elapsed_secs < -60 {
        0.1
    } else {
        let days_old = elapsed_secs.max(0) as f64 / 86400.0;
        1.0 / (1.0 + days_old)
    }
}

/// Time-based scoring using recency decay or explicit time-range
/// filtering.
///
/// Searches chunks and statements by recency. When a `time_range`
/// is provided, only items within the range are returned. Otherwise
/// all items are scored by recency: `score = 1.0 / (1.0 + days_old)`.
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
        let half = (query.limit as i64 / 2).max(2);

        // Query chunks and statements concurrently.
        let (chunk_rows, stmt_rows) = if let Some((start, end)) = query.time_range {
            tokio::join!(
                sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
                    "SELECT id, created_at FROM chunks \
                     WHERE created_at >= $1 AND created_at <= $2 \
                     ORDER BY created_at DESC LIMIT $3",
                )
                .bind(start)
                .bind(end)
                .bind(half)
                .fetch_all(&self.pool),
                sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
                    "SELECT id, created_at FROM statements \
                     WHERE NOT is_evicted \
                       AND created_at >= $1 AND created_at <= $2 \
                     ORDER BY created_at DESC LIMIT $3",
                )
                .bind(start)
                .bind(end)
                .bind(half)
                .fetch_all(&self.pool),
            )
        } else {
            tokio::join!(
                sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
                    "SELECT id, created_at FROM chunks \
                     ORDER BY created_at DESC LIMIT $1",
                )
                .bind(half)
                .fetch_all(&self.pool),
                sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
                    "SELECT id, created_at FROM statements \
                     WHERE NOT is_evicted \
                     ORDER BY created_at DESC LIMIT $1",
                )
                .bind(half)
                .fetch_all(&self.pool),
            )
        };

        let chunk_rows = chunk_rows?;
        let stmt_rows = stmt_rows?;

        // Merge chunk and statement rows, scoring by recency.
        let mut scored: Vec<(Uuid, f64, &str)> = Vec::new();
        for (id, created_at) in &chunk_rows {
            let elapsed_secs = (now - *created_at).num_seconds();
            let score = recency_score(elapsed_secs);
            if elapsed_secs < -60 {
                tracing::debug!(
                    chunk_id = %id,
                    seconds_in_future = -elapsed_secs,
                    "chunk has future timestamp, demoting"
                );
            }
            scored.push((*id, score, "chunk"));
        }
        for (id, created_at) in &stmt_rows {
            let elapsed_secs = (now - *created_at).num_seconds();
            scored.push((*id, recency_score(elapsed_secs), "statement"));
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(query.limit);

        let results = scored
            .into_iter()
            .enumerate()
            .map(|(i, (id, score, rtype))| SearchResult {
                id,
                score,
                rank: i + 1,
                dimension: "temporal".to_string(),
                snippet: None,
                result_type: Some(rtype.to_string()),
            })
            .collect();

        Ok(results)
    }

    fn kind(&self) -> DimensionKind {
        DimensionKind::Temporal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recency_score_now_is_one() {
        let score = recency_score(0);
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn recency_score_one_day_old() {
        let score = recency_score(86400);
        assert!((score - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn recency_score_seven_days_old() {
        let score = recency_score(7 * 86400);
        assert!((score - 0.125).abs() < f64::EPSILON);
    }

    #[test]
    fn recency_score_monotonically_decreases() {
        let s1 = recency_score(0);
        let s2 = recency_score(3600);
        let s3 = recency_score(86400);
        let s4 = recency_score(7 * 86400);
        assert!(s1 > s2);
        assert!(s2 > s3);
        assert!(s3 > s4);
        assert!(s4 > 0.0);
    }

    #[test]
    fn recency_score_future_within_tolerance() {
        // 30 seconds in the future — within the 60s tolerance
        let score = recency_score(-30);
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn recency_score_future_beyond_tolerance() {
        // 120 seconds in the future — demoted
        let score = recency_score(-120);
        assert!((score - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn recency_score_always_positive() {
        // Even very old content has a positive score.
        let score = recency_score(365 * 86400);
        assert!(score > 0.0);
        assert!(score < 0.01);
    }
}

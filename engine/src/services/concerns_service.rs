//! Standing concerns service — operational health signals for the dashboard.
//!
//! The heartbeat (jane-ops) writes concerns via `upsert_many`; the dashboard
//! reads them via `list`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};

use crate::errors::{AppError, AppResult};

// ── Public types ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, FromRow)]
pub struct ConcernStatus {
    pub name: String,
    pub status: String,
    pub notes: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct UpsertConcernRequest {
    pub name: String,
    pub status: String,
    pub notes: Option<String>,
}

impl UpsertConcernRequest {
    /// Validate that `status` is one of the allowed values.
    fn validate(&self) -> AppResult<()> {
        match self.status.as_str() {
            "green" | "yellow" | "red" => Ok(()),
            other => Err(AppError::BadRequest(format!(
                "invalid concern status '{other}'; must be 'green', 'yellow', or 'red'"
            ))),
        }
    }
}

// ── Service ──────────────────────────────────────────────────────────────────

pub struct ConcernsService {
    pool: PgPool,
}

impl ConcernsService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Upsert a batch of concerns (insert or update by name).
    ///
    /// Each concern is validated before any DB writes.  Returns the final
    /// state of all upserted concerns.
    pub async fn upsert_many(
        &self,
        concerns: Vec<UpsertConcernRequest>,
    ) -> AppResult<Vec<ConcernStatus>> {
        // Validate all items up-front so we fail before any writes.
        for c in &concerns {
            c.validate()?;
        }

        let mut results = Vec::with_capacity(concerns.len());

        for c in concerns {
            let row = sqlx::query_as::<_, ConcernStatus>(
                "INSERT INTO covalence.standing_concerns (name, status, notes, updated_at)
                 VALUES ($1, $2, $3, now())
                 ON CONFLICT (name) DO UPDATE
                     SET status     = EXCLUDED.status,
                         notes      = EXCLUDED.notes,
                         updated_at = now()
                 RETURNING name, status, notes, updated_at",
            )
            .bind(&c.name)
            .bind(&c.status)
            .bind(&c.notes)
            .fetch_one(&self.pool)
            .await?;

            results.push(row);
        }

        Ok(results)
    }

    /// Return all concerns ordered by name.
    pub async fn list(&self) -> AppResult<Vec<ConcernStatus>> {
        let rows = sqlx::query_as::<_, ConcernStatus>(
            "SELECT name, status, notes, updated_at
             FROM   covalence.standing_concerns
             ORDER  BY name",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }
}

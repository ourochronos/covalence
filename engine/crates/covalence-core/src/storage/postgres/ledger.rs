//! PostgreSQL implementation of [`LedgerRepo`].

use crate::error::Result;
use crate::models::projection::LedgerEntry;
use crate::storage::traits::LedgerRepo;
use crate::types::ids::SourceId;

use super::PgRepo;

impl LedgerRepo for PgRepo {
    async fn create_batch(&self, entries: &[LedgerEntry]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        // Batch insert using UNNEST for efficiency.
        let ids: Vec<uuid::Uuid> = entries.iter().map(|e| e.id).collect();
        let source_ids: Vec<uuid::Uuid> = entries.iter().map(|e| e.source_id.into_uuid()).collect();
        let c_starts: Vec<i32> = entries
            .iter()
            .map(|e| e.canonical_span_start as i32)
            .collect();
        let c_ends: Vec<i32> = entries
            .iter()
            .map(|e| e.canonical_span_end as i32)
            .collect();
        let c_tokens: Vec<String> = entries.iter().map(|e| e.canonical_token.clone()).collect();
        let m_starts: Vec<i32> = entries
            .iter()
            .map(|e| e.mutated_span_start as i32)
            .collect();
        let m_ends: Vec<i32> = entries.iter().map(|e| e.mutated_span_end as i32).collect();
        let m_tokens: Vec<String> = entries.iter().map(|e| e.mutated_token.clone()).collect();

        sqlx::query(
            "INSERT INTO offset_projection_ledgers
                (id, source_id, canonical_span_start, canonical_span_end,
                 canonical_token, mutated_span_start, mutated_span_end, mutated_token)
             SELECT * FROM UNNEST(
                $1::uuid[], $2::uuid[], $3::int[], $4::int[],
                $5::text[], $6::int[], $7::int[], $8::text[]
             )",
        )
        .bind(&ids)
        .bind(&source_ids)
        .bind(&c_starts)
        .bind(&c_ends)
        .bind(&c_tokens)
        .bind(&m_starts)
        .bind(&m_ends)
        .bind(&m_tokens)
        .execute(self.pool())
        .await?;

        Ok(())
    }

    async fn list_by_source(&self, source_id: SourceId) -> Result<Vec<LedgerEntry>> {
        let rows = sqlx::query_as::<_, LedgerRow>(
            "SELECT id, source_id, canonical_span_start, canonical_span_end,
                    canonical_token, mutated_span_start, mutated_span_end,
                    mutated_token, created_at
             FROM offset_projection_ledgers
             WHERE source_id = $1
             ORDER BY mutated_span_start",
        )
        .bind(source_id.into_uuid())
        .fetch_all(self.pool())
        .await?;

        Ok(rows.into_iter().map(|r| r.into()).collect())
    }

    async fn delete_by_source(&self, source_id: SourceId) -> Result<u64> {
        let result = sqlx::query("DELETE FROM offset_projection_ledgers WHERE source_id = $1")
            .bind(source_id.into_uuid())
            .execute(self.pool())
            .await?;

        Ok(result.rows_affected())
    }
}

/// Internal row type for sqlx mapping.
#[derive(sqlx::FromRow)]
struct LedgerRow {
    id: uuid::Uuid,
    source_id: uuid::Uuid,
    canonical_span_start: i32,
    canonical_span_end: i32,
    canonical_token: String,
    mutated_span_start: i32,
    mutated_span_end: i32,
    mutated_token: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl From<LedgerRow> for LedgerEntry {
    fn from(r: LedgerRow) -> Self {
        Self {
            id: r.id,
            source_id: SourceId::from(r.source_id),
            canonical_span_start: r.canonical_span_start as usize,
            canonical_span_end: r.canonical_span_end as usize,
            canonical_token: r.canonical_token,
            mutated_span_start: r.mutated_span_start as usize,
            mutated_span_end: r.mutated_span_end as usize,
            mutated_token: r.mutated_token,
            created_at: r.created_at,
        }
    }
}

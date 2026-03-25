//! QueueRepo implementation for PostgreSQL.
//!
//! Handles fan-in, watchdog, and pipeline queries that were
//! previously raw SQL in the queue service layer.

use crate::error::Result;
use crate::storage::traits::QueueRepo;
use crate::types::ids::SourceId;

use super::PgRepo;

impl QueueRepo for PgRepo {
    async fn get_chunks_by_source(&self, source_id: SourceId) -> Result<Vec<(uuid::Uuid,)>> {
        let rows: Vec<(uuid::Uuid,)> = sqlx::query_as("SELECT * FROM sp_get_chunks_by_source($1)")
            .bind(source_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    async fn list_unsummarized_entities_for_summary(
        &self,
        code_entity_class: &str,
    ) -> Result<Vec<(uuid::Uuid, uuid::Uuid)>> {
        let rows: Vec<(uuid::Uuid, uuid::Uuid)> = sqlx::query_as(
            "SELECT DISTINCT n.id, COALESCE( \
               (SELECT c.source_id FROM extractions ex \
                JOIN chunks c ON c.id = ex.chunk_id \
                WHERE ex.entity_id = n.id AND ex.entity_type = 'node' \
                LIMIT 1), \
               '00000000-0000-0000-0000-000000000000'::uuid \
             ) as source_id \
             FROM nodes n \
             WHERE n.entity_class = $1 \
               AND (n.properties->>'semantic_summary' IS NULL \
                    OR n.properties->>'semantic_summary' = '') \
               AND n.node_type != 'code_test' \
               AND n.canonical_name NOT LIKE 'test_%'",
        )
        .bind(code_entity_class)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn list_sources_needing_compose(
        &self,
        code_domain: &str,
        code_entity_class: &str,
    ) -> Result<Vec<(uuid::Uuid,)>> {
        let rows: Vec<(uuid::Uuid,)> = sqlx::query_as(
            "SELECT DISTINCT s.id \
             FROM sources s \
             WHERE s.domain = $1 \
               AND s.summary IS NULL \
               AND EXISTS ( \
                 SELECT 1 FROM nodes n \
                 JOIN extractions ex ON ex.entity_id = n.id AND ex.entity_type = 'node' \
                 JOIN chunks c ON c.id = ex.chunk_id \
                 WHERE c.source_id = s.id \
                   AND n.entity_class = $2 \
                   AND n.properties->>'semantic_summary' IS NOT NULL \
               )",
        )
        .bind(code_domain)
        .bind(code_entity_class)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn recover_orphaned_jobs(&self) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE retry_jobs
             SET status = 'pending'::job_status,
                 next_due = now(),
                 updated_at = now()
             WHERE status = 'running'::job_status",
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn list_stalled_sources(
        &self,
        code_domain: &str,
        code_entity_class: &str,
    ) -> Result<Vec<(uuid::Uuid,)>> {
        let rows: Vec<(uuid::Uuid,)> = sqlx::query_as(
            "SELECT s.id FROM sources s \
             WHERE s.domain = $1 \
               AND s.summary IS NULL \
               AND EXISTS ( \
                 SELECT 1 FROM nodes n \
                 JOIN extractions ex ON ex.entity_id = n.id \
                 JOIN chunks c ON c.id = ex.chunk_id \
                 WHERE c.source_id = s.id \
                   AND n.entity_class = $2 \
                   AND n.properties->>'semantic_summary' IS NOT NULL \
               ) \
               AND NOT EXISTS ( \
                 SELECT 1 FROM retry_jobs rj \
                 WHERE rj.kind = 'compose_source_summary' \
                   AND rj.payload->>'source_id' = s.id::text \
                   AND rj.status IN ('pending', 'running') \
               ) \
             LIMIT 20",
        )
        .bind(code_domain)
        .bind(code_entity_class)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn insert_retry_job_direct(
        &self,
        kind: &str,
        payload: &serde_json::Value,
        key: &str,
        max_attempts: i32,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO retry_jobs (kind, payload, idempotency_key, max_attempts) \
             VALUES ($1::job_kind, $2, $3, $4) \
             ON CONFLICT (idempotency_key) DO NOTHING",
        )
        .bind(kind)
        .bind(payload)
        .bind(key)
        .bind(max_attempts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn insert_retry_job_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        kind: &str,
        payload: &serde_json::Value,
        key: &str,
        max_attempts: i32,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO retry_jobs (kind, payload, idempotency_key, max_attempts) \
             VALUES ($1::job_kind, $2, $3, $4) \
             ON CONFLICT (idempotency_key) DO NOTHING",
        )
        .bind(kind)
        .bind(payload)
        .bind(key)
        .bind(max_attempts)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    async fn try_advisory_xact_lock(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        key: i64,
    ) -> Result<bool> {
        let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
            .bind(key)
            .fetch_one(&mut **tx)
            .await?;
        Ok(acquired)
    }

    async fn count_pending_jobs_for_source_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        kind: &str,
        source_id: &str,
    ) -> Result<i64> {
        let count: i64 = sqlx::query_scalar("SELECT sp_count_pending_jobs_for_source($1, $2)")
            .bind(kind)
            .bind(source_id)
            .fetch_one(&mut **tx)
            .await?;
        Ok(count)
    }

    async fn count_failed_jobs_for_source_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        kind: &str,
        source_id: &str,
    ) -> Result<i64> {
        let count: i64 = sqlx::query_scalar("SELECT sp_count_failed_jobs_for_source($1, $2)")
            .bind(kind)
            .bind(source_id)
            .fetch_one(&mut **tx)
            .await?;
        Ok(count)
    }

    async fn update_source_status_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        source_id: uuid::Uuid,
        status: &str,
    ) -> Result<()> {
        sqlx::query("SELECT sp_update_source_status($1, $2)")
            .bind(source_id)
            .bind(status)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    async fn source_has_summary_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        source_id: uuid::Uuid,
    ) -> Result<bool> {
        let has: bool = sqlx::query_scalar("SELECT sp_source_has_summary($1)")
            .bind(source_id)
            .fetch_one(&mut **tx)
            .await?;
        Ok(has)
    }

    async fn get_unsummarized_entities_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        source_id: uuid::Uuid,
    ) -> Result<Vec<(uuid::Uuid,)>> {
        let rows: Vec<(uuid::Uuid,)> =
            sqlx::query_as("SELECT * FROM sp_get_unsummarized_entities_by_source($1)")
                .bind(source_id)
                .fetch_all(&mut **tx)
                .await?;
        Ok(rows)
    }
}

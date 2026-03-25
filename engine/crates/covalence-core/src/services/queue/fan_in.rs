//! Fan-in completion tracking for the async pipeline.
//!
//! Advisory-lock-guarded fan-in triggers that detect when all jobs
//! of one stage are complete and enqueue the next stage.

use crate::types::ids::SourceId;

/// Fan-in trigger: check if all SummarizeEntity jobs for a source
/// are done. If so, enqueue a ComposeSourceSummary job.
///
/// Uses an advisory lock on the source_id to prevent two concurrent
/// completions from both triggering the compose stage.
pub(crate) async fn try_advance_to_compose(pool: &sqlx::PgPool, source_id: SourceId) {
    let sid = source_id.into_uuid();
    let lock_key = (sid.as_u128() & 0x7FFF_FFFF_FFFF_FFFF) as i64;

    // Use a transaction-scoped advisory lock. Unlike session-level locks,
    // these auto-release on commit/rollback/disconnect — no leak risk if
    // the async Future is dropped.
    let Ok(mut tx) = pool.begin().await else {
        return;
    };

    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(lock_key)
        .fetch_one(&mut *tx)
        .await
        .unwrap_or(false);

    if !acquired {
        return; // Another worker is handling this fan-in.
    }

    // Check: any pending/running summarize_entity jobs for this source?
    let remaining: i64 = sqlx::query_scalar("SELECT sp_count_pending_jobs_for_source($1, $2)")
        .bind("summarize_entity")
        .bind(sid.to_string())
        .fetch_one(&mut *tx)
        .await
        .unwrap_or(1);

    if remaining == 0 {
        // Check for failed summarization jobs (#170).
        let failed: i64 = sqlx::query_scalar("SELECT sp_count_failed_jobs_for_source($1, $2)")
            .bind("summarize_entity")
            .bind(sid.to_string())
            .fetch_one(&mut *tx)
            .await
            .unwrap_or(0);

        if failed > 0 {
            tracing::warn!(
                source_id = %sid,
                failed_summaries = failed,
                "fan-in: summarization partially failed — composing with available summaries"
            );
            let _ = sqlx::query("SELECT sp_update_source_status($1, $2)")
                .bind(sid)
                .bind("partial")
                .execute(&mut *tx)
                .await;
        }
        let has_summary: bool = sqlx::query_scalar("SELECT sp_source_has_summary($1)")
            .bind(sid)
            .fetch_one(&mut *tx)
            .await
            .unwrap_or(true);

        if !has_summary {
            let payload = serde_json::json!({ "source_id": sid.to_string() });
            let key = format!("compose:{sid}");
            let _ = sqlx::query(
                "INSERT INTO retry_jobs (kind, payload, idempotency_key, max_attempts) \
                 VALUES ('compose_source_summary', $1, $2, 5) \
                 ON CONFLICT (idempotency_key) DO NOTHING",
            )
            .bind(payload)
            .bind(&key)
            .execute(&mut *tx)
            .await;

            tracing::info!(
                source_id = %sid,
                "fan-in: all summaries complete, enqueued compose_source_summary"
            );
        }
    }

    // Lock auto-releases on commit.
    let _ = tx.commit().await;
}

/// Fan-in trigger: when all ExtractChunk jobs for a source complete,
/// enqueue SummarizeEntity jobs for the code entities extracted.
pub(crate) async fn try_advance_to_summarize(pool: &sqlx::PgPool, source_id: SourceId) {
    let sid = source_id.into_uuid();
    let lock_key = ((sid.as_u128() >> 1) & 0x7FFF_FFFF_FFFF_FFFF) as i64;

    let Ok(mut tx) = pool.begin().await else {
        return;
    };

    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(lock_key)
        .fetch_one(&mut *tx)
        .await
        .unwrap_or(false);

    if !acquired {
        return;
    }

    let remaining: i64 = sqlx::query_scalar("SELECT sp_count_pending_jobs_for_source($1, $2)")
        .bind("extract_chunk")
        .bind(sid.to_string())
        .fetch_one(&mut *tx)
        .await
        .unwrap_or(1);

    if remaining == 0 {
        // Check for failed/dead extraction jobs (#170).
        let failed: i64 = sqlx::query_scalar("SELECT sp_count_failed_jobs_for_source($1, $2)")
            .bind("extract_chunk")
            .bind(sid.to_string())
            .fetch_one(&mut *tx)
            .await
            .unwrap_or(0);

        if failed > 0 {
            tracing::warn!(
                source_id = %sid,
                failed_chunks = failed,
                "fan-in: extraction partially failed — proceeding with degraded coverage"
            );
            // Mark source as partial so downstream knows.
            let _ = sqlx::query("SELECT sp_update_source_status($1, $2)")
                .bind(sid)
                .bind("partial")
                .execute(&mut *tx)
                .await;
        }
        let entities: Vec<(uuid::Uuid,)> =
            sqlx::query_as("SELECT * FROM sp_get_unsummarized_entities_by_source($1)")
                .bind(sid)
                .fetch_all(&mut *tx)
                .await
                .unwrap_or_default();

        let mut enqueued = 0usize;
        for (node_id,) in &entities {
            let payload = serde_json::json!({
                "node_id": node_id.to_string(),
                "source_id": sid.to_string(),
            });
            let key = format!("summarize:{node_id}");
            if sqlx::query(
                "INSERT INTO retry_jobs (kind, payload, idempotency_key, max_attempts) \
                 VALUES ('summarize_entity', $1, $2, 5) \
                 ON CONFLICT (idempotency_key) DO NOTHING",
            )
            .bind(&payload)
            .bind(&key)
            .execute(&mut *tx)
            .await
            .is_ok()
            {
                enqueued += 1;
            }
        }

        if enqueued > 0 {
            tracing::info!(
                source_id = %sid,
                enqueued,
                "fan-in: all extractions complete, enqueued summarize_entity jobs"
            );
        }
    }

    let _ = tx.commit().await;
}

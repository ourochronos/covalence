//! Fan-in completion tracking for the async pipeline.
//!
//! Advisory-lock-guarded fan-in triggers that detect when all jobs
//! of one stage are complete and enqueue the next stage.

use crate::storage::postgres::PgRepo;
use crate::storage::traits::QueueRepo;
use crate::types::ids::SourceId;

/// Fan-in trigger: check if all SummarizeEntity jobs for a source
/// are done. If so, enqueue a ComposeSourceSummary job.
///
/// Uses an advisory lock on the source_id to prevent two concurrent
/// completions from both triggering the compose stage.
pub(crate) async fn try_advance_to_compose(repo: &PgRepo, source_id: SourceId) {
    let sid = source_id.into_uuid();
    let lock_key = (sid.as_u128() & 0x7FFF_FFFF_FFFF_FFFF) as i64;

    // Use a transaction-scoped advisory lock. Unlike session-level locks,
    // these auto-release on commit/rollback/disconnect — no leak risk if
    // the async Future is dropped.
    let Ok(mut tx) = repo.pool().begin().await else {
        return;
    };

    let acquired = QueueRepo::try_advisory_xact_lock(repo, &mut tx, lock_key)
        .await
        .unwrap_or(false);

    if !acquired {
        return; // Another worker is handling this fan-in.
    }

    // Check: any pending/running summarize_entity jobs for this source?
    let remaining = QueueRepo::count_pending_jobs_for_source_tx(
        repo,
        &mut tx,
        "summarize_entity",
        &sid.to_string(),
    )
    .await
    .unwrap_or(1);

    if remaining == 0 {
        // Check for failed summarization jobs (#170).
        let failed = QueueRepo::count_failed_jobs_for_source_tx(
            repo,
            &mut tx,
            "summarize_entity",
            &sid.to_string(),
        )
        .await
        .unwrap_or(0);

        if failed > 0 {
            tracing::warn!(
                source_id = %sid,
                failed_summaries = failed,
                "fan-in: summarization partially failed — composing with available summaries"
            );
            let _ = QueueRepo::update_source_status_tx(repo, &mut tx, sid, "partial").await;
        }
        let has_summary = QueueRepo::source_has_summary_tx(repo, &mut tx, sid)
            .await
            .unwrap_or(true);

        if !has_summary {
            let payload = serde_json::json!({ "source_id": sid.to_string() });
            let key = format!("compose:{sid}");
            let _ = QueueRepo::insert_retry_job_tx(
                repo,
                &mut tx,
                "compose_source_summary",
                &payload,
                &key,
                5,
            )
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
pub(crate) async fn try_advance_to_summarize(repo: &PgRepo, source_id: SourceId) {
    let sid = source_id.into_uuid();
    let lock_key = ((sid.as_u128() >> 1) & 0x7FFF_FFFF_FFFF_FFFF) as i64;

    let Ok(mut tx) = repo.pool().begin().await else {
        return;
    };

    let acquired = QueueRepo::try_advisory_xact_lock(repo, &mut tx, lock_key)
        .await
        .unwrap_or(false);

    if !acquired {
        return;
    }

    let remaining = QueueRepo::count_pending_jobs_for_source_tx(
        repo,
        &mut tx,
        "extract_chunk",
        &sid.to_string(),
    )
    .await
    .unwrap_or(1);

    if remaining == 0 {
        // Check for failed/dead extraction jobs (#170).
        let failed = QueueRepo::count_failed_jobs_for_source_tx(
            repo,
            &mut tx,
            "extract_chunk",
            &sid.to_string(),
        )
        .await
        .unwrap_or(0);

        if failed > 0 {
            tracing::warn!(
                source_id = %sid,
                failed_chunks = failed,
                "fan-in: extraction partially failed — proceeding with degraded coverage"
            );
            // Mark source as partial so downstream knows.
            let _ = QueueRepo::update_source_status_tx(repo, &mut tx, sid, "partial").await;
        }
        let entities = QueueRepo::get_unsummarized_entities_tx(repo, &mut tx, sid)
            .await
            .unwrap_or_default();

        let mut enqueued = 0usize;
        for (node_id,) in &entities {
            let payload = serde_json::json!({
                "node_id": node_id.to_string(),
                "source_id": sid.to_string(),
            });
            let key = format!("summarize:{node_id}");
            if QueueRepo::insert_retry_job_tx(repo, &mut tx, "summarize_entity", &payload, &key, 5)
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

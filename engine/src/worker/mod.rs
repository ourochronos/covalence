//! Slow-path background worker (Issue #4).
//!
//! Polls `covalence.slow_path_queue` for pending tasks and executes them
//! asynchronously without blocking the hot API path.
//!
//! # Task lifecycle
//! ```
//! pending → processing → complete
//!                      ↘ failed   (after 3 attempts)
//! ```
//!
//! # Retry strategy
//! Attempt count is tracked in the `result` JSONB column as
//! `{"attempts": N, ...}` since the table has no dedicated attempts column.

pub mod llm;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use llm::{LlmClient, StubLlmClient};

/// Maximum number of attempts before a task is marked `failed`.
const MAX_ATTEMPTS: i64 = 3;

/// How long we wait between poll cycles.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// A row fetched from `slow_path_queue`.
#[derive(Debug)]
struct QueueTask {
    id: Uuid,
    task_type: String,
    node_id: Option<Uuid>,
    payload: Value,
    result: Option<Value>,
}

/// Start the background worker loop.
/// Call this once at startup; it runs forever until the process exits.
pub async fn run(pool: PgPool) {
    let llm: Arc<dyn LlmClient> = Arc::new(StubLlmClient);
    tracing::info!("slow-path worker started (poll_interval={}s)", POLL_INTERVAL.as_secs());

    loop {
        match poll_and_execute(&pool, &llm).await {
            Ok(n) if n > 0 => tracing::debug!("worker processed {n} task(s)"),
            Ok(_) => {}
            Err(e) => tracing::error!("worker poll error: {e:#}"),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll for one pending task, execute it, and return how many tasks were processed.
async fn poll_and_execute(pool: &PgPool, llm: &Arc<dyn LlmClient>) -> anyhow::Result<usize> {
    // Claim the highest-priority pending task atomically using SKIP LOCKED
    // so multiple worker instances can run safely.
    let maybe_task = sqlx::query_as!(
        QueueTask,
        r#"
        UPDATE covalence.slow_path_queue
        SET    status     = 'processing',
               started_at = now()
        WHERE  id = (
            SELECT id
            FROM   covalence.slow_path_queue
            WHERE  status = 'pending'
            ORDER  BY priority DESC, created_at ASC
            LIMIT  1
            FOR UPDATE SKIP LOCKED
        )
        RETURNING
            id,
            task_type,
            node_id,
            payload,
            result
        "#,
    )
    .fetch_optional(pool)
    .await
    .context("failed to claim task from slow_path_queue")?;

    let task = match maybe_task {
        Some(t) => t,
        None => return Ok(0),
    };

    // Determine current attempt count from result JSONB
    let attempts: i64 = task
        .result
        .as_ref()
        .and_then(|v| v.get("attempts"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        + 1;

    tracing::info!(
        task_id  = %task.id,
        task_type = %task.task_type,
        node_id  = ?task.node_id,
        attempts,
        "worker: task started"
    );

    // Execute the task
    let exec_result = execute_task(pool, llm, &task).await;

    match exec_result {
        Ok(output) => {
            let result_json = json!({
                "attempts": attempts,
                "output": output,
            });
            sqlx::query!(
                r#"
                UPDATE covalence.slow_path_queue
                SET  status       = 'complete',
                     completed_at = now(),
                     result       = $1
                WHERE id = $2
                "#,
                result_json,
                task.id,
            )
            .execute(pool)
            .await
            .context("failed to mark task complete")?;

            tracing::info!(
                task_id   = %task.id,
                task_type = %task.task_type,
                "worker: task complete"
            );
        }
        Err(e) => {
            let error_msg = format!("{e:#}");
            tracing::warn!(
                task_id   = %task.id,
                task_type = %task.task_type,
                attempts,
                error     = %error_msg,
                "worker: task failed"
            );

            if attempts >= MAX_ATTEMPTS {
                // Permanently fail
                let result_json = json!({
                    "attempts": attempts,
                    "error": error_msg,
                    "final": true,
                });
                sqlx::query!(
                    r#"
                    UPDATE covalence.slow_path_queue
                    SET  status       = 'failed',
                         completed_at = now(),
                         result       = $1
                    WHERE id = $2
                    "#,
                    result_json,
                    task.id,
                )
                .execute(pool)
                .await
                .context("failed to mark task failed")?;

                tracing::error!(
                    task_id   = %task.id,
                    task_type = %task.task_type,
                    "worker: task permanently failed after {MAX_ATTEMPTS} attempts"
                );
            } else {
                // Requeue for retry by resetting to pending
                let result_json = json!({
                    "attempts": attempts,
                    "last_error": error_msg,
                });
                sqlx::query!(
                    r#"
                    UPDATE covalence.slow_path_queue
                    SET  status     = 'pending',
                         started_at = null,
                         result     = $1
                    WHERE id = $2
                    "#,
                    result_json,
                    task.id,
                )
                .execute(pool)
                .await
                .context("failed to requeue task")?;

                tracing::info!(
                    task_id   = %task.id,
                    task_type = %task.task_type,
                    attempts,
                    "worker: task requeued for retry"
                );
            }
        }
    }

    Ok(1)
}

/// Dispatch to the appropriate handler for each task type.
async fn execute_task(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    match task.task_type.as_str() {
        "embed"              => handle_embed(pool, llm, task).await,
        "contention_check"   => handle_contention_check(pool, task).await,
        "compile"            => handle_compile(pool, llm, task).await,
        "split"              => handle_split(pool, llm, task).await,
        "merge"              => handle_merge(pool, llm, task).await,
        "infer_edges"        => handle_infer_edges(pool, llm, task).await,
        "resolve_contention" => handle_resolve_contention(pool, llm, task).await,
        other => anyhow::bail!("unknown task_type: {other}"),
    }
}

// ─── Task handlers ────────────────────────────────────────────────────────────

/// `embed`: Generate and store a vector embedding for the node's content.
///
/// TODO: Integrate OpenAI text-embedding-3-small (or compatible) API.
/// The stub currently returns a zero vector and skips the DB upsert.
async fn handle_embed(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    let node_id = task
        .node_id
        .context("embed task requires node_id")?;

    // Fetch node content
    let row = sqlx::query!(
        "SELECT content FROM covalence.nodes WHERE id = $1",
        node_id
    )
    .fetch_optional(pool)
    .await?
    .context("node not found for embed")?;

    let content = row.content.unwrap_or_default();

    // TODO: Replace StubLlmClient with a real embedding client.
    // After obtaining a real vector, upsert into covalence.node_embeddings:
    //   INSERT INTO covalence.node_embeddings (node_id, embedding, model, dimensions)
    //   VALUES ($1, $2::vector, 'text-embedding-3-small', 1536)
    //   ON CONFLICT (node_id) DO UPDATE SET embedding = EXCLUDED.embedding, updated_at = now();
    let _embedding = llm.embed(&content).await?;

    tracing::debug!(node_id = %node_id, "embed: stub vector produced (TODO: store real embedding)");

    Ok(json!({
        "note": "stub embed — no real vector stored; integrate OpenAI in v1",
        "node_id": node_id,
        "content_len": content.len(),
    }))
}

/// `contention_check`: Find nodes with similar content; if similarity is high
/// but content differs meaningfully, insert a contention record.
async fn handle_contention_check(
    pool: &PgPool,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    let node_id = task
        .node_id
        .context("contention_check task requires node_id")?;

    // Full-text similarity threshold — ts_rank returns 0..1
    let similarity_threshold: f32 = task
        .payload
        .get("similarity_threshold")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(0.15);

    // Find candidate nodes ranked by ts_rank against this node's tsv
    let candidates = sqlx::query!(
        r#"
        SELECT
            candidate.id            AS candidate_id,
            candidate.content       AS candidate_content,
            source_node.content     AS source_content,
            ts_rank(
                candidate.content_tsv,
                to_tsquery('english', regexp_replace(
                    array_to_string(
                        tsvector_to_array(source_node.content_tsv), ' & '
                    ), '[^a-zA-Z0-9_ &]', '', 'g'
                ))
            )                       AS rank
        FROM covalence.nodes  AS source_node
        JOIN covalence.nodes  AS candidate
          ON candidate.id     != source_node.id
         AND candidate.status  = 'active'
         AND candidate.node_type = source_node.node_type
        WHERE source_node.id  = $1
          AND source_node.status = 'active'
        ORDER BY rank DESC
        LIMIT 10
        "#,
        node_id,
    )
    .fetch_all(pool)
    .await
    .context("failed to query contention candidates")?;

    let mut contention_count = 0_i32;

    for row in &candidates {
        let rank = row.rank.unwrap_or(0.0) as f32;
        if rank < similarity_threshold {
            continue;
        }

        // Check whether content is meaningfully different (simple heuristic:
        // if the content strings are identical, no contention needed).
        let source_content   = row.source_content.as_deref().unwrap_or("");
        let candidate_content = row.candidate_content.as_deref().unwrap_or("");

        if source_content.trim() == candidate_content.trim() {
            tracing::debug!(
                node_id = %node_id,
                candidate_id = %row.candidate_id,
                "contention_check: identical content, skipping"
            );
            continue;
        }

        // Check if a contention between these two nodes already exists
        let existing = sqlx::query!(
            r#"
            SELECT id FROM covalence.contentions
            WHERE ((node_id = $1 AND source_node_id = $2)
                OR (node_id = $2 AND source_node_id = $1))
              AND status != 'resolved'
            LIMIT 1
            "#,
            node_id,
            row.candidate_id,
        )
        .fetch_optional(pool)
        .await?;

        if existing.is_some() {
            tracing::debug!(
                node_id      = %node_id,
                candidate_id = %row.candidate_id,
                "contention_check: contention already exists, skipping"
            );
            continue;
        }

        // Insert new contention
        sqlx::query!(
            r#"
            INSERT INTO covalence.contentions
                (node_id, source_node_id, type, description, severity, status, materiality)
            VALUES
                ($1, $2, 'contends', $3, 'medium', 'detected', $4)
            "#,
            node_id,
            row.candidate_id,
            format!(
                "Content similarity detected (ts_rank={:.3}): nodes may be in conflict.",
                rank
            ),
            rank as f64,
        )
        .execute(pool)
        .await
        .context("failed to insert contention")?;

        contention_count += 1;

        tracing::info!(
            node_id      = %node_id,
            candidate_id = %row.candidate_id,
            rank,
            "contention_check: new contention inserted"
        );
    }

    Ok(json!({
        "node_id":          node_id,
        "candidates_found": candidates.len(),
        "contentions_created": contention_count,
        "threshold":        similarity_threshold,
    }))
}

/// `compile`: Synthesise a new article node from source nodes.
///
/// LLM integration is planned for v1. For now, marks the task complete
/// and logs the intent so the queue doesn't stall.
async fn handle_compile(
    _pool: &PgPool,
    _llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    tracing::info!(
        task_id = %task.id,
        payload = %task.payload,
        "compile: stub — LLM-driven article compilation planned for v1"
    );
    Ok(json!({
        "note": "stub — LLM article compilation not yet implemented (v1)",
        "node_id": task.node_id,
    }))
}

/// `split`: Split an oversized article node into two smaller nodes.
///
/// LLM integration planned for v1.
async fn handle_split(
    _pool: &PgPool,
    _llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    tracing::info!(
        task_id = %task.id,
        "split: stub — LLM-driven article split planned for v1"
    );
    Ok(json!({
        "note": "stub — LLM article split not yet implemented (v1)",
        "node_id": task.node_id,
    }))
}

/// `merge`: Merge two related article nodes into one.
///
/// LLM integration planned for v1.
async fn handle_merge(
    _pool: &PgPool,
    _llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    tracing::info!(
        task_id = %task.id,
        payload = %task.payload,
        "merge: stub — LLM-driven article merge planned for v1"
    );
    Ok(json!({
        "note": "stub — LLM article merge not yet implemented (v1)",
        "payload": task.payload,
    }))
}

/// `infer_edges`: Use LLM to infer semantic edges between nodes.
///
/// Requires LLM; planned for v1.
async fn handle_infer_edges(
    _pool: &PgPool,
    _llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    tracing::info!(
        task_id = %task.id,
        "infer_edges: stub — LLM-driven edge inference planned for v1"
    );
    Ok(json!({
        "note": "stub — LLM edge inference not yet implemented (v1)",
        "node_id": task.node_id,
    }))
}

/// `resolve_contention`: Attempt to automatically resolve a contention.
///
/// Requires LLM reasoning; planned for v1.
async fn handle_resolve_contention(
    _pool: &PgPool,
    _llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    tracing::info!(
        task_id = %task.id,
        payload = %task.payload,
        "resolve_contention: stub — LLM-driven resolution planned for v1"
    );
    Ok(json!({
        "note": "stub — LLM contention resolution not yet implemented (v1)",
        "payload": task.payload,
    }))
}

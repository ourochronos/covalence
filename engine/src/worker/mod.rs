//! Slow-path background worker (Issue #4).
//!
//! Polls `covalence.slow_path_queue` for pending tasks and executes them
//! asynchronously without blocking the hot API path.
//!
//! # Task lifecycle
//! ```text
//! pending → processing → complete
//!                      ↘ failed   (after 3 attempts)
//! ```
//!
//! # Retry strategy
//! Attempt count is tracked in the `result` JSONB column as
//! `{"attempts": N, ...}` since the table has no dedicated attempts column.

pub mod consolidation;
pub mod contention;
pub mod critique;
pub mod decay;
pub mod divergence;
pub mod infer_article_edges;
pub mod llm;
pub mod merge_edges;
pub mod navigation;
pub mod openai;
pub mod provenance_cap;
pub mod reconsolidation;
pub mod source_selection;
pub mod tree_index;

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::graph::{GraphRepository as _, SqlGraphRepository};
use crate::models::{EdgeType, NodeType};

use anyhow::Context;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use llm::{LlmClient, StubLlmClient};

/// Maximum number of attempts before a task is marked `failed`.
const MAX_ATTEMPTS: i64 = 3;

/// How long we wait between poll cycles.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Maximum number of concurrently in-flight LLM tasks.
///
/// Capping the JoinSet prevents 50-100+ simultaneous gpt-4.1-mini calls that
/// cascade into OpenAI rate-limit errors and cause every task to exhaust its
/// 3 retry attempts (covalence#141).
const MAX_CONCURRENT_LLM_TASKS: usize = 6;

/// Number of poll cycles between consolidation heartbeat scans (~60 s).
const CONSOLIDATION_HEARTBEAT_INTERVAL: u64 = 30;

/// A row fetched from `slow_path_queue`.
#[derive(Debug)]
pub struct QueueTask {
    pub id: Uuid,
    pub task_type: String,
    pub node_id: Option<Uuid>,
    pub payload: Value,
    pub result: Option<Value>,
}

/// Build the LLM client from environment variables.
fn build_llm_client() -> Arc<dyn LlmClient> {
    match std::env::var("OPENAI_API_KEY") {
        Ok(key) if !key.is_empty() => {
            let mut client = openai::OpenAiClient::new(key);
            if let Ok(url) = std::env::var("OPENAI_BASE_URL") {
                client = client.with_base_url(url);
            }
            if let Ok(model) = std::env::var("COVALENCE_EMBED_MODEL") {
                client = client.with_embed_model(model);
            }
            if let Ok(model) = std::env::var("COVALENCE_CHAT_MODEL") {
                client = client.with_chat_model(model);
            }
            tracing::info!("slow-path worker using OpenAI LLM client");
            Arc::new(client)
        }
        _ => {
            tracing::warn!(
                "OPENAI_API_KEY not set — using StubLlmClient (no real embeddings/completions)"
            );
            Arc::new(StubLlmClient)
        }
    }
}

/// Start the background worker loop.
/// Call this once at startup; it runs until the process exits.
pub async fn run(pool: PgPool) {
    let llm = build_llm_client();
    let token = CancellationToken::new();
    run_with_token(pool, llm, token).await;
}

/// Start the background worker loop with an explicit [`CancellationToken`].
///
/// When the token is cancelled, the worker stops claiming new tasks and drains
/// any in-flight tasks in its [`JoinSet`] before returning.  This ensures no
/// task is left in the `processing` state after shutdown.
///
/// # Graceful drain
/// 1. The outer poll loop races `POLL_INTERVAL` sleep against `token.cancelled()`.
/// 2. On cancellation, the loop exits — no new tasks are claimed.
/// 3. Any tasks already spawned into the [`JoinSet`] are allowed to run to
///    completion before this function returns.
pub async fn run_with_token(pool: PgPool, llm: Arc<dyn LlmClient>, token: CancellationToken) {
    tracing::info!(
        "slow-path worker started (poll_interval={}s)",
        POLL_INTERVAL.as_secs()
    );

    let mut join_set: JoinSet<()> = JoinSet::new();
    let mut heartbeat_tick: u64 = 0;

    loop {
        // Reap any tasks that finished since the last iteration.
        while let Some(res) = join_set.try_join_next() {
            if let Err(e) = res {
                tracing::error!("worker: spawned task panicked: {e}");
            }
        }

        heartbeat_tick += 1;

        // Periodically enqueue consolidation tasks for due articles.
        if heartbeat_tick % CONSOLIDATION_HEARTBEAT_INTERVAL == 0 {
            if let Err(e) = enqueue_due_article_consolidations(&pool).await {
                tracing::error!("consolidation heartbeat error: {e:#}");
            }
        }

        // Claim one pending task and spawn its execution into the JoinSet,
        // but only when we are below the concurrency cap (covalence#141).
        if join_set.len() >= MAX_CONCURRENT_LLM_TASKS {
            tracing::debug!(
                in_flight = join_set.len(),
                cap = MAX_CONCURRENT_LLM_TASKS,
                "worker: concurrency cap reached — skipping claim this cycle"
            );
        } else {
            match claim_task(&pool).await {
                Ok(Some(task)) => {
                    tracing::debug!(
                        task_id   = %task.id,
                        task_type = %task.task_type,
                        "worker: claimed task, spawning"
                    );
                    let pool_c = pool.clone();
                    let llm_c = Arc::clone(&llm);
                    join_set.spawn(async move {
                        execute_and_finalize(pool_c, llm_c, task).await;
                    });
                }
                Ok(None) => {} // no work available right now
                Err(e) => tracing::error!("worker poll error: {e:#}"),
            }
        }

        // Sleep until the next poll cycle, or exit immediately on cancellation.
        tokio::select! {
            _ = tokio::time::sleep(POLL_INTERVAL) => {}
            _ = token.cancelled() => {
                tracing::info!(
                    in_flight = join_set.len(),
                    "worker: cancellation requested — stopping new task intake"
                );
                break;
            }
        }
    }

    // Graceful drain: wait for all in-flight tasks to complete before returning.
    if !join_set.is_empty() {
        tracing::info!(count = join_set.len(), "worker: draining in-flight tasks");
        while let Some(res) = join_set.join_next().await {
            if let Err(e) = res {
                tracing::error!("worker: in-flight task panicked: {e:#}");
            }
        }
        tracing::info!("worker: graceful drain complete");
    }
}

/// Claim one pending task from `slow_path_queue` using `SKIP LOCKED`.
///
/// Returns `Ok(None)` when no task is currently available.
async fn claim_task(pool: &PgPool) -> anyhow::Result<Option<QueueTask>> {
    let maybe_row = sqlx::query(
        r#"
        UPDATE covalence.slow_path_queue
        SET    status     = 'processing',
               started_at = now()
        WHERE  id = (
            SELECT id
            FROM   covalence.slow_path_queue
            WHERE  status = 'pending'
              AND  (execute_after IS NULL OR execute_after <= now())
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

    Ok(maybe_row.map(|r| {
        use sqlx::Row;
        QueueTask {
            id: r.get("id"),
            task_type: r.get("task_type"),
            node_id: r.get("node_id"),
            payload: r.get("payload"),
            result: r.get("result"),
        }
    }))
}

/// Execute a claimed task and update its `slow_path_queue` status.
///
/// Owns the task (already marked `processing` by [`claim_task`]) and is
/// responsible for marking it `complete`, `failed`, or re-queueing for retry.
/// DB-finalisation errors are logged but not propagated so the JoinSet slot
/// is always released cleanly.
async fn execute_and_finalize(pool: PgPool, llm: Arc<dyn LlmClient>, task: QueueTask) {
    let attempts: i64 = task
        .result
        .as_ref()
        .and_then(|v| v.get("attempts"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        + 1;

    tracing::info!(
        task_id   = %task.id,
        task_type = %task.task_type,
        node_id   = ?task.node_id,
        attempts,
        "worker: task started"
    );

    let exec_result = execute_task(&pool, &llm, &task).await;

    match exec_result {
        Ok(output) => {
            let result_json = json!({ "attempts": attempts, "output": output });
            if let Err(e) = sqlx::query(
                r#"UPDATE covalence.slow_path_queue
                   SET  status       = 'complete',
                        completed_at = now(),
                        result       = $1
                   WHERE id = $2"#,
            )
            .bind(&result_json)
            .bind(task.id)
            .execute(&pool)
            .await
            {
                tracing::error!(
                    task_id = %task.id,
                    "worker: failed to mark task complete: {e:#}"
                );
            } else {
                tracing::info!(
                    task_id   = %task.id,
                    task_type = %task.task_type,
                    "worker: task complete"
                );
            }
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
                let result_json = json!({
                    "attempts": attempts,
                    "error":    error_msg,
                    "final":    true,
                });
                if let Err(e) = sqlx::query(
                    r#"UPDATE covalence.slow_path_queue
                       SET  status       = 'failed',
                            completed_at = now(),
                            result       = $1
                       WHERE id = $2"#,
                )
                .bind(&result_json)
                .bind(task.id)
                .execute(&pool)
                .await
                {
                    tracing::error!(
                        task_id = %task.id,
                        "worker: failed to mark task failed: {e:#}"
                    );
                } else {
                    tracing::error!(
                        task_id   = %task.id,
                        task_type = %task.task_type,
                        "worker: task permanently failed after {MAX_ATTEMPTS} attempts"
                    );
                }
            } else {
                let result_json = json!({
                    "attempts":   attempts,
                    "last_error": error_msg,
                });
                if let Err(e) = sqlx::query(
                    r#"UPDATE covalence.slow_path_queue
                       SET  status     = 'pending',
                            started_at = null,
                            result     = $1
                       WHERE id = $2"#,
                )
                .bind(&result_json)
                .bind(task.id)
                .execute(&pool)
                .await
                {
                    tracing::error!(
                        task_id = %task.id,
                        "worker: failed to requeue task: {e:#}"
                    );
                } else {
                    tracing::info!(
                        task_id   = %task.id,
                        task_type = %task.task_type,
                        attempts,
                        "worker: task requeued for retry"
                    );
                }
            }
        }
    }
}

/// Enqueue `consolidate_article` tasks for every active **article** node whose
/// `next_consolidation_at` has arrived and that has no pending/processing
/// consolidation task already.
///
/// The query explicitly filters `node_type = 'article'` so that future
/// `node_type = 'claim'` nodes are never swept into this heartbeat by accident
/// (covalence#173).
async fn enqueue_due_article_consolidations(pool: &PgPool) -> anyhow::Result<()> {
    use sqlx::Row as _;

    let rows = sqlx::query(
        "SELECT n.id, COALESCE(n.consolidation_count, 0) AS consolidation_count
         FROM   covalence.nodes n
         WHERE  n.node_type              = 'article'
           AND  n.status                 = 'active'
           AND  n.next_consolidation_at  IS NOT NULL
           AND  n.next_consolidation_at  <= now()
           AND  NOT EXISTS (
               SELECT 1
               FROM   covalence.slow_path_queue q
               WHERE  q.task_type               = 'consolidate_article'
                 AND  q.payload->>'article_id'  = n.id::text
                 AND  q.status IN ('pending', 'processing')
           )",
    )
    .fetch_all(pool)
    .await
    .context("enqueue_due_consolidations: query failed")?;

    for row in &rows {
        let article_id: Uuid = row.get("id");
        let count: i32 = row.get("consolidation_count");
        let next_pass = count + 1;

        enqueue_task(
            pool,
            "consolidate_article",
            None,
            serde_json::json!({
                "article_id": article_id.to_string(),
                "pass": next_pass,
            }),
            3,
        )
        .await
        .with_context(|| {
            format!("enqueue_due_consolidations: failed to enqueue for {article_id}")
        })?;

        tracing::debug!(
            article_id = %article_id,
            next_pass,
            "consolidation heartbeat: enqueued consolidate_article"
        );
    }

    Ok(())
}

/// Dispatch to the appropriate handler for each task type.
pub async fn execute_task(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    match task.task_type.as_str() {
        "embed" => handle_embed(pool, llm, task).await,
        "tree_index" => handle_tree_index(pool, llm, task).await,
        "tree_embed" => handle_tree_embed(pool, llm, task).await,
        "contention_check" => contention::handle_contention_check(pool, llm, task).await,
        "compile" => handle_compile(pool, llm, task).await,
        "split" => handle_split(pool, llm, task).await,
        "merge" => merge_edges::handle_merge(pool, llm, task).await,
        "infer_edges" => merge_edges::handle_infer_edges(pool, llm, task).await,
        "resolve_contention" => contention::handle_resolve_contention(pool, llm, task).await,
        "decay_check" => decay::handle_decay_check(pool, task).await,
        "divergence_scan" => divergence::handle_divergence_scan(pool, task).await,
        "reconsolidate" => reconsolidation::handle_reconsolidate(pool, llm, task).await,
        "consolidate_article" => consolidation::handle_consolidate_article(pool, llm, task).await,
        "critique_article" => critique::handle_critique_article(pool, llm, task).await,
        "infer_article_edges" => {
            infer_article_edges::handle_infer_article_edges(pool, llm, task).await
        }
        "recompute_graph_embeddings" => {
            let method = task
                .payload
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("both");
            recompute_graph_embeddings(pool, method).await
        }
        "auto_split" => provenance_cap::handle_auto_split(pool, llm, task).await,
        other => anyhow::bail!("unknown task_type: {other}"),
    }
}

// ─── Graph embedding recomputation ───────────────────────────────────────────

/// Rebuild a `CovalenceGraph` from the SQL edges table.
async fn load_graph_from_db(pool: &PgPool) -> anyhow::Result<crate::graph::CovalenceGraph> {
    use sqlx::Row as _;
    let rows = sqlx::query("SELECT source_node_id, target_node_id, edge_type FROM covalence.edges")
        .fetch_all(pool)
        .await
        .context("recompute_graph_embeddings: failed to load edges")?;

    let mut graph = crate::graph::CovalenceGraph::new();
    for row in &rows {
        let src: uuid::Uuid = row.try_get("source_node_id")?;
        let tgt: uuid::Uuid = row.try_get("target_node_id")?;
        let etype: String = row.try_get("edge_type")?;
        graph.add_edge(src, tgt, etype);
    }
    Ok(graph)
}

/// Recompute Node2Vec and/or Spectral embeddings and persist them to DB.
///
/// Skipped unless the `COVALENCE_GRAPH_EMBEDDINGS` env var is set to `"true"`.
async fn recompute_graph_embeddings(pool: &PgPool, method: &str) -> anyhow::Result<Value> {
    let enabled = std::env::var("COVALENCE_GRAPH_EMBEDDINGS").unwrap_or_default();
    if enabled != "true" {
        return Ok(json!({
            "skipped": true,
            "reason": "COVALENCE_GRAPH_EMBEDDINGS not enabled"
        }));
    }

    let graph = load_graph_from_db(pool).await?;
    let mut results = json!({});

    if method == "node2vec" || method == "both" {
        let config = crate::embeddings::node2vec::Node2VecConfig::default();
        let embs = crate::embeddings::node2vec::compute_node2vec(&graph, config)?;
        let count = crate::embeddings::store::store_embeddings(pool, embs, "node2vec").await?;
        results["node2vec"] = json!(count);
    }

    if method == "spectral" || method == "both" {
        let config = crate::embeddings::spectral::SpectralConfig::default();
        let embs = crate::embeddings::spectral::compute_spectral(&graph, config)?;
        let count = crate::embeddings::store::store_embeddings(pool, embs, "spectral").await?;
        results["spectral"] = json!(count);
    }

    Ok(json!({"stored": results}))
}

// ─── Task handlers ────────────────────────────────────────────────────────────

/// `embed`: Generate and store a vector embedding for the node's content.
///
/// TODO: Integrate OpenAI text-embedding-3-small (or compatible) API.
/// The stub currently returns a zero vector and skips the DB upsert.
pub async fn handle_embed(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    let node_id = task.node_id.context("embed task requires node_id")?;

    // Fetch node content and metadata needed for the contextual preamble.
    let row = sqlx::query(
        "SELECT content, title, source_type, node_type, domain_path \
         FROM covalence.nodes WHERE id = $1",
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await?
    .context("node not found for embed")?;

    use sqlx::Row as _;
    let title: Option<String> = row.get("title");
    let content: Option<String> = row.get("content");
    let source_type: Option<String> = row.get("source_type");
    let node_type: String = row.get("node_type");
    let domain_path: Option<Vec<String>> = row.get("domain_path");
    let title = title.unwrap_or_default();
    let content = content.unwrap_or_default();

    // Contextual preamble (covalence#73): use payload value when present
    // (written by the service layer at ingest/create time); fall back to
    // deriving from DB metadata for tasks enqueued without a preamble
    // (e.g. compile, legacy re-embeds).
    let preamble: String = task
        .payload
        .get("embed_preamble")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let type_str = source_type.as_deref().unwrap_or(node_type.as_str());
            let domain = domain_path.as_deref().unwrap_or(&[]).join("/");
            format!(
                "[Context: {}. Source type: {}. Domain: {}.]",
                title, type_str, domain,
            )
        });

    // For large sources, delegate to tree_index pipeline (no truncation)
    if content.len() > tree_index::TRIVIAL_THRESHOLD_CHARS {
        tracing::info!(
            node_id = %node_id,
            content_len = content.len(),
            "embed: large source — delegating to tree_index pipeline"
        );

        let overlap = tree_index::DEFAULT_OVERLAP_FRACTION;
        // Build tree index if missing
        let has_tree = {
            let row =
                sqlx::query_as::<_, (Value,)>("SELECT metadata FROM covalence.nodes WHERE id = $1")
                    .bind(node_id)
                    .fetch_optional(pool)
                    .await?;
            row.and_then(|r| r.0.get("tree_index").cloned())
                .map(|v| !v.is_null())
                .unwrap_or(false)
        };

        if !has_tree {
            tree_index::build_tree_index(pool, llm, node_id, overlap, false).await?;
        }

        // Embed sections + compose node embedding
        return tree_index::embed_sections(pool, llm, node_id).await;
    }

    // Small sources: direct embedding (no tree overhead).
    // Prepend the contextual preamble (covalence#73) before the content so
    // the embedding model has domain context.  The preamble already names the
    // title, so we only re-include the raw content to keep the text concise.
    let embed_input = if preamble.is_empty() {
        if title.is_empty() {
            content.clone()
        } else {
            format!("{title}\n\n{content}")
        }
    } else {
        format!("{preamble}\n\n{content}")
    };

    let embedding = llm.embed(&embed_input).await?;
    let dims = embedding.len() as i32;

    // Format as pgvector literal: [0.1,0.2,...]
    let vec_literal = format!(
        "[{}]",
        embedding
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );

    // Determine model name from env or default
    let model =
        std::env::var("COVALENCE_EMBED_MODEL").unwrap_or_else(|_| "text-embedding-3-small".into());

    // Upsert into node_embeddings using halfvec cast
    sqlx::query(&format!(
        "INSERT INTO covalence.node_embeddings (node_id, embedding, model)
         VALUES ($1, \'{vec_literal}\'::halfvec({dims}), $2)
         ON CONFLICT (node_id) DO UPDATE
           SET embedding = EXCLUDED.embedding,
               model = EXCLUDED.model"
    ))
    .bind(node_id)
    .bind(&model)
    .execute(pool)
    .await
    .context("failed to upsert embedding")?;

    tracing::info!(node_id = %node_id, dims, model = %model, "embed: vector stored");

    Ok(json!({
        "node_id": node_id,
        "dimensions": dims,
        "model": model,
        "content_len": content.len(),
    }))
}

/// `contention_check`: Find nodes with similar content; if similarity is high
/// but content differs meaningfully, insert a contention record.
#[allow(dead_code)]
async fn handle_contention_check(pool: &PgPool, task: &QueueTask) -> anyhow::Result<Value> {
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
    let candidates = sqlx::query(
        r#"SELECT
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
        LIMIT 10"#,
    )
    .bind(node_id)
    .fetch_all(pool)
    .await
    .context("failed to query contention candidates")?;

    let mut contention_count = 0_i32;

    for row in &candidates {
        use sqlx::Row as _;
        let rank: Option<f32> = row.try_get("rank").ok();
        let rank = rank.unwrap_or(0.0);
        if rank < similarity_threshold {
            continue;
        }

        let candidate_id: Uuid = row.get("candidate_id");

        // Check whether content is meaningfully different (simple heuristic:
        // if the content strings are identical, no contention needed).
        let source_content: Option<String> = row.get("source_content");
        let candidate_content: Option<String> = row.get("candidate_content");
        let source_content = source_content.as_deref().unwrap_or("");
        let candidate_content = candidate_content.as_deref().unwrap_or("");

        if source_content.trim() == candidate_content.trim() {
            tracing::debug!(
                node_id = %node_id,
                candidate_id = %candidate_id,
                "contention_check: identical content, skipping"
            );
            continue;
        }

        // Check if a contention between these two nodes already exists
        let existing = sqlx::query(
            r#"SELECT id FROM covalence.contentions
            WHERE ((node_id = $1 AND source_node_id = $2)
                OR (node_id = $2 AND source_node_id = $1))
              AND status != 'resolved'
            LIMIT 1"#,
        )
        .bind(node_id)
        .bind(candidate_id)
        .fetch_optional(pool)
        .await?;

        if existing.is_some() {
            tracing::debug!(
                node_id      = %node_id,
                candidate_id = %candidate_id,
                "contention_check: contention already exists, skipping"
            );
            continue;
        }

        // Insert new contention
        sqlx::query(
            r#"INSERT INTO covalence.contentions
                (node_id, source_node_id, type, description, severity, status, materiality)
            VALUES
                ($1, $2, 'contends', $3, 'medium', 'detected', $4)"#,
        )
        .bind(node_id)
        .bind(candidate_id)
        .bind(format!(
            "Content similarity detected (ts_rank={:.3}): nodes may be in conflict.",
            rank
        ))
        .bind(rank as f64)
        .execute(pool)
        .await
        .context("failed to insert contention")?;

        contention_count += 1;

        tracing::info!(
            node_id      = %node_id,
            candidate_id = %candidate_id,
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

/// Strip markdown code fences from an LLM response and parse it as JSON.
///
/// Returns `Some(value)` on success, `None` if the response cannot be parsed
/// as JSON after stripping fences.  The fallback concatenation / midpoint logic
/// at each callsite is responsible for handling the `None` case.
pub(crate) fn parse_llm_json(raw: &str) -> Option<serde_json::Value> {
    let text = raw.trim();
    let json_str = if text.starts_with("```") {
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() >= 3 {
            lines[1..lines.len() - 1].join("\n")
        } else {
            text.to_string()
        }
    } else {
        text.to_string()
    };
    serde_json::from_str(&json_str).ok()
}

/// Enqueue a slow-path task with an optional node_id and JSON payload.
pub async fn enqueue_task(
    pool: &PgPool,
    task_type: &str,
    node_id: Option<Uuid>,
    payload: Value,
    priority: i32,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO covalence.slow_path_queue \
             (id, task_type, node_id, payload, status, priority) \
         VALUES ($1, $2, $3, $4, 'pending', $5)",
    )
    .bind(Uuid::new_v4())
    .bind(task_type)
    .bind(node_id)
    .bind(&payload)
    .bind(priority)
    .execute(pool)
    .await
    .with_context(|| format!("failed to enqueue {task_type} task"))?;
    Ok(())
}

/// Enqueue a slow-path task that should not be processed until `execute_after`.
///
/// When `execute_after` is `None`, the task is eligible for processing
/// immediately (equivalent to [`enqueue_task`]).
pub async fn enqueue_task_at(
    pool: &PgPool,
    task_type: &str,
    node_id: Option<Uuid>,
    payload: Value,
    priority: i32,
    execute_after: Option<chrono::DateTime<chrono::Utc>>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO covalence.slow_path_queue \
             (id, task_type, node_id, payload, status, priority, execute_after) \
         VALUES ($1, $2, $3, $4, 'pending', $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(task_type)
    .bind(node_id)
    .bind(&payload)
    .bind(priority)
    .bind(execute_after)
    .execute(pool)
    .await
    .with_context(|| format!("failed to enqueue {task_type} task (at)"))?;
    Ok(())
}

/// Write an inference log entry after a successful LLM completion.
/// Errors are logged as warnings but not propagated — inference logging is
/// best-effort and must never fail a handler.
#[allow(clippy::too_many_arguments)]
pub(super) async fn log_inference(
    pool: &PgPool,
    operation: &str,
    input_node_ids: &[Uuid],
    input_summary: &str,
    output_decision: &str,
    output_confidence: Option<f64>,
    output_rationale: &str,
    model: &str,
    latency_ms: i32,
) -> anyhow::Result<()> {
    let inputs = json!({
        "input_node_ids": input_node_ids,
        "input_summary":  input_summary,
    });
    let output = json!({
        "decision":   output_decision,
        "confidence": output_confidence,
        "rationale":  output_rationale,
    });

    sqlx::query(
        "INSERT INTO covalence.inference_log
             (operation, inputs, output, input_node_ids, input_summary,
              output_decision, output_confidence, output_rationale, model, latency_ms)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(operation)
    .bind(&inputs)
    .bind(&output)
    .bind(input_node_ids)
    .bind(input_summary)
    .bind(output_decision)
    .bind(output_confidence)
    .bind(output_rationale)
    .bind(model)
    .bind(latency_ms)
    .execute(pool)
    .await
    .with_context(|| format!("log_inference: failed to insert row for operation={operation}"))?;

    Ok(())
}

/// Idempotency guard: returns `true` if a *different* task row with the same
/// `task_type` and `node_id` (or same payload when `node_id` is NULL) has
/// already reached `status = 'complete'`.
pub(super) async fn already_completed(pool: &PgPool, task: &QueueTask) -> anyhow::Result<bool> {
    let row = if let Some(nid) = task.node_id {
        sqlx::query(
            "SELECT 1 FROM covalence.slow_path_queue
             WHERE task_type = $1
               AND status    = 'complete'
               AND id       != $2
               AND node_id   = $3
             LIMIT 1",
        )
        .bind(&task.task_type)
        .bind(task.id)
        .bind(nid)
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query(
            "SELECT 1 FROM covalence.slow_path_queue
             WHERE task_type = $1
               AND status    = 'complete'
               AND id       != $2
               AND node_id  IS NULL
               AND payload   = $3
             LIMIT 1",
        )
        .bind(&task.task_type)
        .bind(task.id)
        .bind(&task.payload)
        .fetch_optional(pool)
        .await?
    };
    Ok(row.is_some())
}

/// `compile`: Synthesise a new article node from source nodes.
pub async fn handle_compile(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    use sqlx::Row as _;

    // ── 0. Idempotency guard ────────────────────────────────────────────────
    if already_completed(pool, task).await? {
        tracing::info!(task_id = %task.id, "compile: idempotency guard — already complete, skipping");
        return Ok(json!({"skipped": true, "reason": "already_complete"}));
    }

    // ── 1. Parse payload ────────────────────────────────────────────────────
    let source_ids_val = task
        .payload
        .get("source_ids")
        .context("compile: missing source_ids in payload")?;
    let source_ids: Vec<Uuid> = source_ids_val
        .as_array()
        .context("compile: source_ids must be an array")?
        .iter()
        .enumerate()
        .map(|(i, v)| {
            v.as_str()
                .with_context(|| format!("compile: source_ids[{i}] is not a string"))
                .and_then(|s| {
                    Uuid::parse_str(s)
                        .with_context(|| format!("compile: invalid UUID in source_ids[{i}]"))
                })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let title_hint: Option<String> = task
        .payload
        .get("title_hint")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let compilation_focus: Option<String> = task
        .payload
        .get("compilation_focus")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // ── 1b. Source cap (covalence#85) ───────────────────────────────────────
    // "Lost in the middle" degradation: faithfulness drops when relevant
    // content is buried in the centre of a long context window.  Capping at
    // MAX_COMPILATION_SOURCES keeps all source material near the context edges
    // where attention is strongest.  When more sources exist we keep the
    // MAX_COMPILATION_SOURCES with the highest reliability score so the most
    // trustworthy content is always included.
    const MAX_COMPILATION_SOURCES: usize = 7;
    let original_source_count = source_ids.len();
    let source_ids = if original_source_count > MAX_COMPILATION_SOURCES {
        let capped =
            source_selection::select_by_reliability(pool, &source_ids, MAX_COMPILATION_SOURCES)
                .await
                .context("compile: failed to score sources for cap")?;
        tracing::info!(
            task_id   = %task.id,
            original  = original_source_count,
            capped_to = capped.len(),
            "compile: source cap applied (covalence#85)"
        );
        capped
    } else {
        source_ids
    };

    // ── 2. Fetch source content + facets ────────────────────────────────────
    let rows = sqlx::query(
        "SELECT id, title, content, facet_function, facet_scope \
         FROM covalence.nodes WHERE id = ANY($1)",
    )
    .bind(&source_ids)
    .fetch_all(pool)
    .await
    .context("compile: failed to fetch source nodes")?;

    if rows.is_empty() {
        anyhow::bail!("compile: no source nodes found for {:?}", source_ids);
    }

    struct SourceDoc {
        id: Uuid,
        title: String,
        content: String,
        facet_function: Option<Vec<String>>,
        facet_scope: Option<Vec<String>>,
    }

    let sources: Vec<SourceDoc> = rows
        .iter()
        .map(|r| SourceDoc {
            id: r.get("id"),
            title: r.get::<Option<String>, _>("title").unwrap_or_default(),
            content: r.get::<Option<String>, _>("content").unwrap_or_default(),
            facet_function: r.get("facet_function"),
            facet_scope: r.get("facet_scope"),
        })
        .collect();

    // ── 2b. Union source facets for the compiled article ────────────────────
    // Merge facet_function and facet_scope from all contributing sources.
    // Result is deduped and sorted for determinism (covalence#103 Phase 2).
    let compiled_facet_function: Option<Vec<String>> = {
        let mut set: Vec<String> = sources
            .iter()
            .filter_map(|s| s.facet_function.as_ref())
            .flat_map(|v| v.iter().cloned())
            .collect();
        set.sort_unstable();
        set.dedup();
        if set.is_empty() { None } else { Some(set) }
    };
    let compiled_facet_scope: Option<Vec<String>> = {
        let mut set: Vec<String> = sources
            .iter()
            .filter_map(|s| s.facet_scope.as_ref())
            .flat_map(|v| v.iter().cloned())
            .collect();
        set.sort_unstable();
        set.dedup();
        if set.is_empty() { None } else { Some(set) }
    };

    // ── 3. Build prompt ─────────────────────────────────────────────────────
    // Prompt caching (covalence#85): Anthropic's cache_control header requires
    // a structured system-message object with {"type": "ephemeral"} on the
    // system turn.  The current LlmClient trait exposes only
    // `complete(prompt: &str, max_tokens: u32)` — a single undifferentiated
    // string — with no facility for per-message metadata or provider-specific
    // headers.  Until the trait is extended to support system/user message
    // separation and a `cache_control` field, prompt caching cannot be enabled
    // here without coupling this module directly to the Anthropic HTTP client
    // (undesirable).  Track progress in covalence#85 (next-2-weeks backlog).
    let mut sources_block = String::new();
    for s in &sources {
        sources_block.push_str(&format!(
            "=== SOURCE {} ===\nTitle: {}\n\n{}\n\n",
            s.id, s.title, s.content
        ));
    }

    let title_hint_line = title_hint
        .as_deref()
        .map(|h| format!("Suggested title: {h}\n"))
        .unwrap_or_default();

    // Optional focus instruction injected when the caller supplies one.
    let focus_line = compilation_focus
        .as_deref()
        .map(|f| format!("\nCompilation focus: {f}\n"))
        .unwrap_or_default();

    let prompt = format!(
        "You are a knowledge synthesizer. Read the following source documents and \
produce a well-structured article that synthesizes their information.\n\
\n\
{title_hint_line}\
{focus_line}\
Target length: ~2000 tokens (minimum 200, maximum 4000 tokens).\n\
\n\
CRITICAL — Preserve with HIGH FIDELITY:\n\
- Decisions and their rationale: write \"We chose X over Y because Z\", not just \"X was chosen\".\n\
- Rejected alternatives: explicitly note what was considered but not done, and why.\n\
- Open questions: capture unresolved issues and uncertainties flagged in the sources.\n\
- Reasoning chains: when sources explain WHY something works, keep that explanation.\n\
\n\
DO NOT compress decisions into bare facts. Reasoning is knowledge.\n\
\n\
When the source material contains distinct decisions, findings, open questions, or \
rejected approaches, reflect that structure using Markdown headers such as \
## Key Decisions, ## Findings, ## Open Questions, ## Rejected Approaches — \
but only when the content warrants it; do not force headers onto homogeneous material.\n\
\n\
Respond ONLY with valid JSON (no markdown fences), exactly:\n\
{{\n\
  \"title\": \"...\",\n\
  \"content\": \"...\",\n\
  \"epistemic_type\": \"episodic|semantic|procedural\",\n\
  \"source_relationships\": [\n\
    {{\"source_id\": \"<uuid>\", \"relationship\": \"originates|confirms|supersedes|contradicts|contends\"}}\n\
  ]\n\
}}\n\
\n\
Think step by step when synthesizing these sources.\n\
\n\
SOURCE DOCUMENTS:\n\
{sources_block}"
    );

    // ── 4. LLM completion with timing + fallback ─────────────────────────────
    let chat_model = std::env::var("COVALENCE_CHAT_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
    let t0 = Instant::now();
    let llm_result = llm.complete(&prompt, 4096).await;
    let llm_latency_ms = t0.elapsed().as_millis() as i32;

    let (llm_json, degraded) = match llm_result {
        Ok(raw) => match parse_llm_json(&raw) {
            Some(v) => (v, false),
            None => {
                tracing::warn!(
                    task_id = %task.id,
                    "compile: JSON parse failed, falling back to concatenation"
                );
                (Value::Null, true)
            }
        },
        Err(e) => {
            tracing::warn!(
                task_id = %task.id,
                "compile: LLM error ({e}), falling back to concatenation"
            );
            (Value::Null, true)
        }
    };

    let (article_title, article_content, epistemic_type, source_relationships): (
        String,
        String,
        String,
        Vec<(Uuid, String)>,
    ) = if degraded {
        let mut concat = String::new();
        for s in &sources {
            if !s.title.is_empty() {
                concat.push_str(&format!("## {}\n\n", s.title));
            }
            concat.push_str(&s.content);
            concat.push_str("\n\n");
        }
        let title = title_hint
            .clone()
            .unwrap_or_else(|| "Compiled Article".to_string());
        (title, concat, "semantic".to_string(), vec![])
    } else {
        let title = llm_json
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Compiled Article")
            .to_string();
        let content = llm_json
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let etype = llm_json
            .get("epistemic_type")
            .and_then(|v| v.as_str())
            .unwrap_or("semantic")
            .to_string();
        let rels: Vec<(Uuid, String)> = llm_json
            .get("source_relationships")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let sid = item
                            .get("source_id")
                            .and_then(|v| v.as_str())
                            .and_then(|s| Uuid::parse_str(s).ok())?;
                        let rel = item
                            .get("relationship")
                            .and_then(|v| v.as_str())
                            .unwrap_or("originates")
                            .to_string();
                        Some((sid, rel))
                    })
                    .collect()
            })
            .unwrap_or_default();
        (title, content, etype, rels)
    };

    // ── 4a. Log inference ────────────────────────────────────────────────────
    if !degraded {
        let input_summary = format!(
            "source_ids=[{}], title_hint={:?}",
            source_ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(","),
            title_hint
        );
        let output_decision = format!("{} / {}", article_title, epistemic_type);
        if let Err(e) = log_inference(
            pool,
            "compile",
            &source_ids,
            &input_summary,
            &output_decision,
            None,
            "",
            &chat_model,
            llm_latency_ms,
        )
        .await
        {
            tracing::warn!(task_id = %task.id, "compile: log_inference failed: {e:#}");
        }
    }

    // ── 5. Dedup check via vector similarity ────────────────────────────────
    let embed_result = llm.embed(&article_content).await.ok();
    let existing_article_id: Option<Uuid> = if let Some(ref emb) = embed_result {
        let dims = emb.len();
        let vec_literal = format!(
            "[{}]",
            emb.iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        sqlx::query(&format!(
            "SELECT ne.node_id \
             FROM covalence.node_embeddings ne \
             JOIN covalence.nodes n ON n.id = ne.node_id \
             WHERE n.node_type = 'article' AND n.status = 'active' \
               AND (n.metadata->>'split_from') IS NULL \
               AND (ne.embedding::halfvec({dims}) <=> '{vec_literal}'::halfvec({dims})) < 0.15 \
             ORDER BY (ne.embedding::halfvec({dims}) <=> '{vec_literal}'::halfvec({dims})) ASC \
             LIMIT 1"
        ))
        .fetch_optional(pool)
        .await
        .unwrap_or(None)
        .map(|r| r.get::<Uuid, _>("node_id"))
    } else {
        None
    };

    // ── 6. Insert or update article node (transactional) ────────────────────
    // Steps 6, 6b, and the mutation log are wrapped in a single transaction so
    // a mid-operation cancellation cannot leave the article row in a partial state.
    let meta = json!({
        "epistemic_type": epistemic_type,
        "degraded":       degraded,
    });

    let article_id: Uuid = {
        let mut tx = pool
            .begin()
            .await
            .context("compile: failed to begin transaction")?;

        let id = if let Some(existing_id) = existing_article_id {
            tracing::info!(
                existing_id = %existing_id,
                "compile: dedup hit — updating existing article"
            );
            sqlx::query(
                "UPDATE covalence.nodes \
                 SET title = $1, content = $2, metadata = $3, \
                     facet_function = COALESCE($5, facet_function), \
                     facet_scope    = COALESCE($6, facet_scope), \
                     modified_at = now() \
                 WHERE id = $4",
            )
            .bind(&article_title)
            .bind(&article_content)
            .bind(&meta)
            .bind(existing_id)
            .bind(&compiled_facet_function)
            .bind(&compiled_facet_scope)
            .execute(&mut *tx)
            .await
            .context("compile: failed to update existing article")?;
            existing_id
        } else {
            let new_id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO covalence.nodes \
                     (id, node_type, status, title, content, metadata, \
                      facet_function, facet_scope, \
                      created_at, modified_at) \
                 VALUES ($1, 'article', 'active', $2, $3, $4, $5, $6, now(), now())",
            )
            .bind(new_id)
            .bind(&article_title)
            .bind(&article_content)
            .bind(&meta)
            .bind(&compiled_facet_function)
            .bind(&compiled_facet_scope)
            .execute(&mut *tx)
            .await
            .context("compile: failed to insert article node")?;
            new_id
        };

        // ── 6b. Schedule first consolidation pass ────────────────────────────
        // Set next_consolidation_at = now() + SCHEDULE_PASS_1_HOURS so the
        // heartbeat can trigger pass 1.  Reset consolidation_count to 0.
        let first_pass_delay =
            chrono::Duration::hours(crate::worker::consolidation::SCHEDULE_PASS_1_HOURS);
        let first_consolidation_at = chrono::Utc::now() + first_pass_delay;
        sqlx::query(
            "UPDATE covalence.nodes \
             SET next_consolidation_at = $1, \
                 consolidation_count   = 0 \
             WHERE id = $2",
        )
        .bind(first_consolidation_at)
        .bind(id)
        .execute(&mut *tx)
        .await
        .context("compile: failed to schedule consolidation")?;

        // ── Mutation log (within transaction) ────────────────────────────────
        let mutation_entry = serde_json::json!([{
            "type": "created",
            "summary": format!("Compiled from {} source(s)", sources.len()),
            "recorded_at": chrono::Utc::now().to_rfc3339(),
        }]);
        sqlx::query(
            r#"UPDATE covalence.nodes
                  SET metadata = jsonb_set(
                                     coalesce(metadata, '{}'::jsonb),
                                     '{mutation_log}',
                                     coalesce(metadata->'mutation_log', '[]'::jsonb) || $1::jsonb,
                                     true
                                 ),
                      modified_at = now()
                WHERE id = $2"#,
        )
        .bind(&mutation_entry)
        .bind(id)
        .execute(&mut *tx)
        .await
        .context("compile: failed to record mutation in metadata")?;

        tx.commit()
            .await
            .context("compile: failed to commit article transaction")?;

        id
    };

    // ── 7. Create AGE vertex + insert provenance edges via GraphRepository ───
    // All writes go through GraphRepository so both AGE and SQL are updated.
    // These are outside the transaction; failures are non-fatal (logged as warnings).
    let graph = SqlGraphRepository::new(pool.clone());

    // Ensure the new article has an AGE vertex before creating edges.
    if let Err(e) = graph
        .create_vertex(article_id, NodeType::Article, serde_json::json!({}))
        .await
    {
        tracing::warn!(
            article_id = %article_id,
            "compile: failed to create AGE vertex (edges will still be written to SQL): {e}"
        );
    }

    let rel_map: std::collections::HashMap<Uuid, String> =
        source_relationships.into_iter().collect();

    for src in &sources {
        let rel = rel_map
            .get(&src.id)
            .map(|s| s.as_str())
            .unwrap_or("originates");
        // Map LLM relationship name to EdgeType enum value.
        let edge_type = match rel {
            "confirms" => EdgeType::Confirms,
            "supersedes" => EdgeType::Supersedes,
            "contradicts" => EdgeType::Contradicts,
            "contends" => EdgeType::Contends,
            _ => EdgeType::Originates,
        };
        if let Err(e) = graph
            .create_edge(
                src.id,
                article_id,
                edge_type,
                1.0,
                "compile",
                serde_json::json!({}),
            )
            .await
        {
            tracing::warn!(
                src_id = %src.id,
                article_id = %article_id,
                "compile: failed to create provenance edge via GraphRepository: {e}"
            );
        }
    }

    // ── 7b. Mirror provenance edges into article_sources + recompute confidence ─
    // Insert one row per source into covalence.article_sources so that the
    // Phase-1 confidence pipeline (covalence#137) can query them.
    // Uses ON CONFLICT DO UPDATE so re-compiling an article refreshes weights
    // rather than leaving stale causal_weight / confidence values.
    for src in &sources {
        let rel_str = rel_map
            .get(&src.id)
            .map(|s| s.as_str())
            .unwrap_or("originates");
        // Derive causal_weight from EdgeType::causal_weight() so insertion
        // weights stay in sync with the recompute pipeline (fix #1).
        let causal_w: f32 = rel_str
            .to_uppercase()
            .parse::<crate::models::EdgeType>()
            .map(|et| et.causal_weight())
            .unwrap_or(1.0); // originates / unknown → 1.0
        if let Err(e) = sqlx::query(
            "INSERT INTO covalence.article_sources \
             (article_id, source_id, relationship, causal_weight, confidence) \
             VALUES ($1, $2, $3, $4, 1.0) \
             ON CONFLICT (article_id, source_id, relationship) DO UPDATE \
             SET causal_weight  = EXCLUDED.causal_weight, \
                 confidence     = EXCLUDED.confidence, \
                 superseded_at  = EXCLUDED.superseded_at",
        )
        .bind(article_id)
        .bind(src.id)
        .bind(rel_str)
        .bind(causal_w)
        .execute(pool)
        .await
        {
            tracing::warn!(
                article_id = %article_id,
                source_id  = %src.id,
                "compile: failed to mirror edge into article_sources (non-fatal): {e}"
            );
        }
    }

    // Trigger confidence recompute now that provenance links are written.
    match pool.acquire().await {
        Ok(mut conn) => {
            if let Err(e) =
                crate::confidence::recompute_article_confidence(article_id, &mut conn).await
            {
                tracing::warn!(
                    article_id = %article_id,
                    "compile: confidence recompute failed (non-fatal): {e:#}"
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                article_id = %article_id,
                "compile: could not acquire conn for confidence recompute: {e:#}"
            );
        }
    }

    // ── 8. Queue follow-up tasks ────────────────────────────────────────────
    let source_ids_for_tasks: Vec<Uuid> = sources.iter().map(|s| s.id).collect();
    schedule_post_article_compile_tasks(
        pool,
        article_id,
        &source_ids_for_tasks,
        article_content.len(),
    )
    .await?;

    tracing::info!(
        article_id  = %article_id,
        title       = %article_title,
        content_len = article_content.len(),
        degraded,
        "compile: done"
    );

    Ok(json!({
        "article_id":     article_id,
        "title":          article_title,
        "content_len":    article_content.len(),
        "epistemic_type": epistemic_type,
        "degraded":       degraded,
        "source_count":   sources.len(),
    }))
}

/// Enqueue the standard set of follow-up tasks after an article node has been
/// compiled or updated by [`handle_compile`].
///
/// Extracted from the inline block at the end of `handle_compile` so that the
/// claims compilation handler (covalence#173) can use a *different* post-task
/// sequence without accidentally inheriting this one.  Naming it
/// `_article_compile_tasks` makes the scope explicit.
///
/// Tasks queued (in order):
/// 1. `embed` — generate/refresh the article's vector embedding.
/// 2. `contention_check` — check each source for contradictions.
/// 3. `split` — only when `content_len > 14 000` chars (structural split).
/// 4. `critique_article` — deferred 1 h (Reflexion loop, covalence#105).
/// 5. `infer_article_edges` — article-to-article semantic edges (covalence#160).
/// 6. `auto_split` — provenance-overflow guard (covalence#161), enqueued only
///    when the ORIGINATES edge count exceeds the configured threshold and no
///    such task is already pending/processing.
async fn schedule_post_article_compile_tasks(
    pool: &PgPool,
    article_id: Uuid,
    source_ids: &[Uuid],
    content_len: usize,
) -> anyhow::Result<()> {
    enqueue_task(pool, "embed", Some(article_id), json!({}), 5).await?;
    for &src_id in source_ids {
        enqueue_task(pool, "contention_check", Some(src_id), json!({}), 3).await?;
    }
    if content_len > 14_000 {
        tracing::info!(
            article_id  = %article_id,
            content_len,
            "compile: content large, queuing split task"
        );
        enqueue_task(pool, "split", Some(article_id), json!({}), 4).await?;
    }

    // Queue a deferred critique_article task (covalence#105 — Reflexion loop).
    // Delayed 1 hour to allow embedding and other follow-up work to settle first.
    let critique_after = chrono::Utc::now() + chrono::Duration::hours(1);
    enqueue_task_at(
        pool,
        "critique_article",
        Some(article_id),
        json!({}),
        2,
        Some(critique_after),
    )
    .await?;

    // Queue article-to-article semantic edge inference (covalence#160).
    // Enqueued *after* the compile transaction commits so it never holds a
    // compile lock.  Priority 3 — higher than critique (2) so edges are
    // available quickly but lower than embed (5) so embedding lands first.
    enqueue_task(pool, "infer_article_edges", Some(article_id), json!({}), 3).await?;

    // Auto-split trigger (covalence#161): count ORIGINATES edges accumulated
    // on this article across all compile calls.  If the count exceeds
    // PROVENANCE_SPLIT_THRESHOLD, enqueue an auto_split task (idempotent:
    // skips if one is already pending/running).
    {
        let split_threshold = provenance_cap::provenance_split_threshold() as i64;
        let originates_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM covalence.edges
             WHERE  target_node_id = $1
               AND  edge_type      = 'ORIGINATES'",
        )
        .bind(article_id)
        .fetch_one(pool)
        .await
        .unwrap_or(0);

        if originates_count > split_threshold {
            let already_queued: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                     SELECT 1 FROM covalence.slow_path_queue
                     WHERE  task_type              = 'auto_split'
                       AND  status                 IN ('pending', 'processing')
                       AND  payload->>'article_id' = $1
                 )",
            )
            .bind(article_id.to_string())
            .fetch_one(pool)
            .await
            .unwrap_or(false);

            if !already_queued {
                if let Err(e) = enqueue_task(
                    pool,
                    "auto_split",
                    None,
                    json!({
                        "article_id": article_id.to_string(),
                        "reason": "originates_overflow",
                        "originates_count_at_trigger": originates_count,
                    }),
                    1,
                )
                .await
                {
                    tracing::warn!(
                        article_id = %article_id,
                        "compile: failed to enqueue auto_split (non-fatal): {e:#}"
                    );
                } else {
                    tracing::info!(
                        article_id        = %article_id,
                        originates_count,
                        "compile: provenance overflow — auto_split enqueued (covalence#161)"
                    );
                }
            }
        }
    }

    Ok(())
}

/// `split`: Split an oversized article node into two smaller nodes.
#[allow(deprecated)]
pub async fn handle_split(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    use sqlx::Row as _;

    // ── 0. Idempotency guard ────────────────────────────────────────────────
    if already_completed(pool, task).await? {
        tracing::info!(task_id = %task.id, "split: idempotency guard — already complete, skipping");
        return Ok(json!({"skipped": true, "reason": "already_complete"}));
    }

    // ── 1. Fetch article ────────────────────────────────────────────────────
    let node_id = task.node_id.context("split: task requires node_id")?;

    let row = sqlx::query("SELECT title, content, metadata FROM covalence.nodes WHERE id = $1")
        .bind(node_id)
        .fetch_optional(pool)
        .await?
        .with_context(|| format!("split: article {node_id} not found"))?;

    let orig_title: String = row.get::<Option<String>, _>("title").unwrap_or_default();
    let orig_content: String = row.get::<Option<String>, _>("content").unwrap_or_default();
    let metadata: Value = row
        .get::<Option<Value>, _>("metadata")
        .unwrap_or(Value::Null);

    // ── 2. Find split point ─────────────────────────────────────────────────
    let midpoint = orig_content.len() / 2;
    let _chat_model =
        std::env::var("COVALENCE_CHAT_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
    let split_index: usize = 'tree: {
        if let Some(tree) = metadata.get("tree_index").and_then(|v| v.as_array()) {
            let mut best: Option<usize> = None;
            let mut best_dist = usize::MAX;
            for node in tree {
                let end = node
                    .get("end_char")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                if let Some(e) = end {
                    let dist = e.abs_diff(midpoint);
                    if dist < best_dist {
                        best_dist = dist;
                        best = Some(e);
                    }
                }
            }
            if let Some(idx) = best {
                tracing::info!(
                    node_id     = %node_id,
                    split_index = idx,
                    "split: using tree_index split point"
                );
                break 'tree idx;
            }
        }

        // No tree_index — ask LLM
        let prompt = format!(
            "Find the best point to split this article into two coherent parts.\n\
Return ONLY valid JSON (no markdown fences):\n\
{{\"split_index\": <char_index>, \"part_a_title\": \"...\", \
\"part_b_title\": \"...\", \"reasoning\": \"...\"}}\n\
\nARTICLE ({} chars):\n{orig_content}",
            orig_content.len()
        );

        match llm.complete(&prompt, 512).await {
            Ok(raw) => match parse_llm_json(&raw) {
                Some(v) => {
                    let idx = v
                        .get("split_index")
                        .and_then(|x| x.as_u64())
                        .map(|x| x as usize)
                        .unwrap_or(midpoint);
                    tracing::info!(
                        node_id     = %node_id,
                        split_index = idx,
                        "split: LLM provided split point"
                    );
                    idx
                }
                None => {
                    tracing::warn!(
                        node_id = %node_id,
                        "split: LLM JSON parse failed, using midpoint"
                    );
                    midpoint
                }
            },
            Err(e) => {
                tracing::warn!(
                    node_id = %node_id,
                    "split: LLM error ({e}), using midpoint"
                );
                midpoint
            }
        }
    };

    // Clamp and align to UTF-8 boundary
    let split_index = split_index.clamp(1, orig_content.len().saturating_sub(1));
    let split_at = orig_content
        .char_indices()
        .map(|(i, _)| i)
        .find(|&i| i >= split_index)
        .unwrap_or(orig_content.len());

    let content_a = orig_content[..split_at].to_string();
    let content_b = orig_content[split_at..].to_string();
    let title_a = format!("{orig_title} (Part 1)");
    let title_b = format!("{orig_title} (Part 2)");

    // ── 3. Create two new article nodes ─────────────────────────────────────
    let id_a = Uuid::new_v4();
    let id_b = Uuid::new_v4();

    for (new_id, new_title, new_content) in
        [(id_a, &title_a, &content_a), (id_b, &title_b, &content_b)]
    {
        sqlx::query(
            "INSERT INTO covalence.nodes \
                 (id, node_type, status, title, content, metadata, created_at, modified_at) \
             VALUES ($1, 'article', 'active', $2, $3, $4, now(), now())",
        )
        .bind(new_id)
        .bind(new_title)
        .bind(new_content)
        .bind(json!({"split_from": node_id}))
        .execute(pool)
        .await
        .with_context(|| format!("split: failed to insert new article {new_id}"))?;
    }

    // ── 4. Archive original + clean up AGE vertex ───────────────────────────
    sqlx::query(
        "UPDATE covalence.nodes \
         SET status = 'archived', modified_at = now() WHERE id = $1",
    )
    .bind(node_id)
    .execute(pool)
    .await
    .context("split: failed to archive original article")?;

    let graph = SqlGraphRepository::new(pool.clone());

    // Remove the archived node from the live AGE graph (SQL edges are kept).
    if let Err(e) = graph.archive_vertex(node_id).await {
        tracing::warn!(
            node_id = %node_id,
            "split: archive_vertex failed for original (non-fatal): {e}"
        );
    }

    // ── 5. Create AGE vertices for new articles ─────────────────────────────
    for new_id in [id_a, id_b] {
        if let Err(e) = graph
            .create_vertex(new_id, NodeType::Article, serde_json::json!({}))
            .await
        {
            tracing::warn!(
                new_id = %new_id,
                "split: failed to create AGE vertex (non-fatal): {e}"
            );
        }
    }

    // ── 6. Create SPLIT_INTO edges via GraphRepository ──────────────────────
    for target in [id_a, id_b] {
        if let Err(e) = graph
            .create_edge(
                node_id,
                target,
                EdgeType::SplitInto,
                1.0,
                "split",
                serde_json::json!({}),
            )
            .await
        {
            tracing::warn!(
                node_id = %node_id,
                target  = %target,
                "split: failed to create SPLIT_INTO edge via GraphRepository: {e}"
            );
        }
    }

    // ── 7. Copy provenance edges from original to both new articles ────────
    let prov_rows = sqlx::query(&format!(
        "SELECT source_node_id, edge_type \
             FROM covalence.edges \
             WHERE target_node_id = $1 \
               AND edge_type IN ({})",
        EdgeType::provenance_sql_labels()
    ))
    .bind(node_id)
    .fetch_all(pool)
    .await
    .context("split: failed to fetch provenance edges")?;

    for prow in &prov_rows {
        let src_id: Uuid = prow.get("source_node_id");
        let edge_type_str: String = prow.get("edge_type");
        // Parse the string label to a typed EdgeType.
        let edge_type: EdgeType = match edge_type_str.parse() {
            Ok(et) => et,
            Err(e) => {
                tracing::warn!(
                    edge_type = %edge_type_str,
                    "split: unknown edge_type in provenance copy, skipping: {e}"
                );
                continue;
            }
        };
        for &new_article in &[id_a, id_b] {
            if let Err(e) = graph
                .create_edge(
                    src_id,
                    new_article,
                    edge_type,
                    1.0,
                    "split_inherit",
                    serde_json::json!({}),
                )
                .await
            {
                tracing::warn!(
                    src_id      = %src_id,
                    new_article = %new_article,
                    "split: failed to copy provenance edge via GraphRepository: {e}"
                );
            }
        }
    }

    // ── 7. Record mutations in node metadata ──────────────────────────
    let _now_str = chrono::Utc::now().to_rfc3339();
    let _split_note = serde_json::json!([{
        "type": "split",
        "summary": format!("Split into {id_a} and {id_b}"),
        "recorded_at": &_now_str,
    }]);
    sqlx::query(
        r#"UPDATE covalence.nodes
              SET metadata = jsonb_set(coalesce(metadata, '{}'::jsonb), '{mutation_log}',
                                 coalesce(metadata->'mutation_log', '[]'::jsonb) || $1::jsonb, true),
                  modified_at = now()
            WHERE id = $2"#,
    )
    .bind(&_split_note)
    .bind(node_id)
    .execute(pool)
    .await
    .context("split: failed to record split mutation")?;

    for (new_id, part) in [(id_a, "Part 1"), (id_b, "Part 2")] {
        let _create_note = serde_json::json!([{
            "type": "created",
            "summary": format!("{part} of split from {node_id}"),
            "recorded_at": &_now_str,
        }]);
        sqlx::query(
            r#"UPDATE covalence.nodes
                  SET metadata = jsonb_set(coalesce(metadata, '{}' ::jsonb), '{mutation_log}',
                                     coalesce(metadata->'mutation_log', '[]'::jsonb) || $1::jsonb, true),
                      modified_at = now()
                WHERE id = $2"#,
        )
        .bind(&_create_note)
        .bind(new_id)
        .execute(pool)
        .await
        .context("split: failed to record created mutation")?;
    }

    // ── 8. Queue embed + tree_embed tasks for both new articles (Item 8) ───
    enqueue_task(pool, "embed", Some(id_a), json!({}), 5).await?;
    enqueue_task(pool, "embed", Some(id_b), json!({}), 5).await?;
    enqueue_task(pool, "tree_embed", Some(id_a), json!({}), 4).await?;
    enqueue_task(pool, "tree_embed", Some(id_b), json!({}), 4).await?;

    tracing::info!(
        original_id = %node_id,
        part_a      = %id_a,
        part_b      = %id_b,
        split_at,
        "split: done"
    );

    Ok(json!({
        "original_id":  node_id,
        "part_a_id":    id_a,
        "part_b_id":    id_b,
        "part_a_title": title_a,
        "part_b_title": title_b,
        "part_a_len":   content_a.len(),
        "part_b_len":   content_b.len(),
        "split_at":     split_at,
    }))
}

/// `merge`: Merge two related article nodes into one.
///
/// LLM integration planned for v1.
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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

// ---------------------------------------------------------------------------
// Tree index + section embedding handlers
// ---------------------------------------------------------------------------

/// `tree_index`: Build a tree index for a node using LLM decomposition.
/// Payload: { "overlap": 0.20, "force": false }
pub async fn handle_tree_index(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    let node_id = task.node_id.context("tree_index task requires node_id")?;

    let overlap = task
        .payload
        .get("overlap")
        .and_then(|v| v.as_f64())
        .unwrap_or(tree_index::DEFAULT_OVERLAP_FRACTION);

    let force = task
        .payload
        .get("force")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    tree_index::build_tree_index(pool, llm, node_id, overlap, force).await
}

/// `tree_embed`: Embed all sections of a tree-indexed node + compose node embedding.
pub async fn handle_tree_embed(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    let node_id = task.node_id.context("tree_embed task requires node_id")?;
    tree_index::embed_sections(pool, llm, node_id).await
}

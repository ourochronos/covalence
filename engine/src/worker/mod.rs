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

pub mod contention;
pub mod llm;
pub mod merge_edges;
pub mod openai;
pub mod tree_index;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use llm::{LlmClient, StubLlmClient};

/// Maximum number of attempts before a task is marked `failed`.
const MAX_ATTEMPTS: i64 = 3;

/// How long we wait between poll cycles.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// A row fetched from `slow_path_queue`.
#[derive(Debug)]
pub struct QueueTask {
    pub id: Uuid,
    pub task_type: String,
    pub node_id: Option<Uuid>,
    pub payload: Value,
    pub result: Option<Value>,
}

/// Start the background worker loop.
/// Call this once at startup; it runs forever until the process exits.
pub async fn run(pool: PgPool) {
    let llm: Arc<dyn LlmClient> = match std::env::var("OPENAI_API_KEY") {
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
    };
    tracing::info!(
        "slow-path worker started (poll_interval={}s)",
        POLL_INTERVAL.as_secs()
    );

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
    let maybe_row = sqlx::query(
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

    let maybe_task = maybe_row.map(|r| {
        use sqlx::Row;
        QueueTask {
            id: r.get("id"),
            task_type: r.get("task_type"),
            node_id: r.get("node_id"),
            payload: r.get("payload"),
            result: r.get("result"),
        }
    });

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
            sqlx::query(
                r#"UPDATE covalence.slow_path_queue
                SET  status       = 'complete',
                     completed_at = now(),
                     result       = $1
                WHERE id = $2"#,
            )
            .bind(&result_json)
            .bind(task.id)
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
                sqlx::query(
                    r#"UPDATE covalence.slow_path_queue
                    SET  status       = 'failed',
                         completed_at = now(),
                         result       = $1
                    WHERE id = $2"#,
                )
                .bind(&result_json)
                .bind(task.id)
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
                sqlx::query(
                    r#"UPDATE covalence.slow_path_queue
                    SET  status     = 'pending',
                         started_at = null,
                         result     = $1
                    WHERE id = $2"#,
                )
                .bind(&result_json)
                .bind(task.id)
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
        other => anyhow::bail!("unknown task_type: {other}"),
    }
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

    // Fetch node content
    let row = sqlx::query("SELECT content, title FROM covalence.nodes WHERE id = $1")
        .bind(node_id)
        .fetch_optional(pool)
        .await?
        .context("node not found for embed")?;

    use sqlx::Row as _;
    let title: Option<String> = row.get("title");
    let content: Option<String> = row.get("content");
    let title = title.unwrap_or_default();
    let content = content.unwrap_or_default();

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

    // Small sources: direct embedding (no tree overhead)
    let embed_input = if title.is_empty() {
        content.clone()
    } else {
        format!("{title}\n\n{content}")
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

/// Strip markdown code fences from an LLM response, returning the inner text.
fn strip_fences(text: &str) -> String {
    let text = text.trim();
    if text.starts_with("```") {
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() >= 3 {
            return lines[1..lines.len() - 1].join("\n");
        }
    }
    text.to_string()
}

/// Enqueue a slow-path task with an optional node_id and JSON payload.
async fn enqueue_task(
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

/// `compile`: Synthesise a new article node from source nodes.
pub async fn handle_compile(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    use sqlx::Row as _;

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

    // ── 2. Fetch source content ─────────────────────────────────────────────
    let rows = sqlx::query("SELECT id, title, content FROM covalence.nodes WHERE id = ANY($1)")
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
    }

    let sources: Vec<SourceDoc> = rows
        .iter()
        .map(|r| SourceDoc {
            id: r.get("id"),
            title: r.get::<Option<String>, _>("title").unwrap_or_default(),
            content: r.get::<Option<String>, _>("content").unwrap_or_default(),
        })
        .collect();

    // ── 3. Build prompt ─────────────────────────────────────────────────────
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

    let prompt = format!(
        "You are a knowledge synthesizer. Read the following source documents and \
produce a well-structured article that synthesizes their information.\n\
\n\
{title_hint_line}\
Target length: ~2000 tokens (minimum 200, maximum 4000 tokens).\n\
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
SOURCE DOCUMENTS:\n\
{sources_block}"
    );

    // ── 4. LLM completion with fallback ─────────────────────────────────────
    let (llm_json, degraded) = match llm.complete(&prompt, 4096).await {
        Ok(raw) => match serde_json::from_str::<Value>(&strip_fences(&raw)) {
            Ok(v) => (v, false),
            Err(e) => {
                tracing::warn!(
                    task_id = %task.id,
                    "compile: JSON parse error ({e}), falling back to concatenation"
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

    // ── 6. Insert or update article node ────────────────────────────────────
    let meta = json!({
        "epistemic_type": epistemic_type,
        "degraded":       degraded,
    });

    let article_id: Uuid = if let Some(existing_id) = existing_article_id {
        tracing::info!(
            existing_id = %existing_id,
            "compile: dedup hit — updating existing article"
        );
        sqlx::query(
            "UPDATE covalence.nodes \
             SET title = $1, content = $2, metadata = $3, modified_at = now() \
             WHERE id = $4",
        )
        .bind(&article_title)
        .bind(&article_content)
        .bind(&meta)
        .bind(existing_id)
        .execute(pool)
        .await
        .context("compile: failed to update existing article")?;
        existing_id
    } else {
        let new_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO covalence.nodes \
                 (id, node_type, status, title, content, metadata, created_at, modified_at) \
             VALUES ($1, 'article', 'active', $2, $3, $4, now(), now())",
        )
        .bind(new_id)
        .bind(&article_title)
        .bind(&article_content)
        .bind(&meta)
        .execute(pool)
        .await
        .context("compile: failed to insert article node")?;
        new_id
    };

    // ── 7. Insert provenance links via edges table ─────────────────────────
    let rel_map: std::collections::HashMap<Uuid, String> =
        source_relationships.into_iter().collect();

    for src in &sources {
        let rel = rel_map
            .get(&src.id)
            .map(|s| s.as_str())
            .unwrap_or("originates");
        // Map LLM relationship name to valid edge_type enum value
        let edge_type = match rel {
            "confirms" => "CONFIRMS",
            "supersedes" => "SUPERSEDES",
            "contradicts" => "CONTRADICTS",
            "contends" => "CONTENDS",
            _ => "ORIGINATES",
        };
        sqlx::query(
            "INSERT INTO covalence.edges \
                 (id, source_node_id, target_node_id, edge_type, weight, created_by) \
             VALUES ($1, $2, $3, $4, 1.0, 'compile')",
        )
        .bind(Uuid::new_v4())
        .bind(src.id)
        .bind(article_id)
        .bind(edge_type)
        .execute(pool)
        .await
        .context("compile: failed to insert provenance edge")?;
    }

    // ── 8. Record mutation in node metadata ────────────────────────────────
    let _mutation_entry = serde_json::json!([{
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
    .bind(&_mutation_entry)
    .bind(article_id)
    .execute(pool)
    .await
    .context("compile: failed to record mutation in metadata")?;

    // ── 9. Queue follow-up tasks ────────────────────────────────────────────
    enqueue_task(pool, "embed", Some(article_id), json!({}), 5).await?;
    for src in &sources {
        enqueue_task(pool, "contention_check", Some(src.id), json!({}), 3).await?;
    }
    if article_content.len() > 14_000 {
        tracing::info!(
            article_id  = %article_id,
            content_len = article_content.len(),
            "compile: content large, queuing split task"
        );
        enqueue_task(pool, "split", Some(article_id), json!({}), 4).await?;
    }

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

/// `split`: Split an oversized article node into two smaller nodes.
pub async fn handle_split(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    use sqlx::Row as _;

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
            Ok(raw) => match serde_json::from_str::<Value>(&strip_fences(&raw)) {
                Ok(v) => {
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
                Err(e) => {
                    tracing::warn!(
                        node_id = %node_id,
                        "split: LLM JSON parse error ({e}), using midpoint"
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
        .filter(|&i| i >= split_index)
        .next()
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

    // ── 4. Archive original ─────────────────────────────────────────────────
    sqlx::query(
        "UPDATE covalence.nodes \
         SET status = 'archived', modified_at = now() WHERE id = $1",
    )
    .bind(node_id)
    .execute(pool)
    .await
    .context("split: failed to archive original article")?;

    // ── 5. Create SPLIT_INTO edges ──────────────────────────────────────────
    for target in [id_a, id_b] {
        sqlx::query(
            "INSERT INTO covalence.edges \
                 (id, source_node_id, target_node_id, edge_type, weight, metadata) \
             VALUES ($1, $2, $3, 'SPLIT_INTO', 1.0, '{}'::jsonb)",
        )
        .bind(Uuid::new_v4())
        .bind(node_id)
        .bind(target)
        .execute(pool)
        .await
        .context("split: failed to insert SPLIT_INTO edge")?;
    }

    // ── 6. Copy provenance edges from original to both new articles ────────
    let prov_rows = sqlx::query(
        "SELECT source_node_id, edge_type \
         FROM covalence.edges \
         WHERE target_node_id = $1 \
           AND edge_type IN ('ORIGINATES','COMPILED_FROM','CONFIRMS','SUPERSEDES')",
    )
    .bind(node_id)
    .fetch_all(pool)
    .await
    .context("split: failed to fetch provenance edges")?;

    for prow in &prov_rows {
        let src_id: Uuid = prow.get("source_node_id");
        let edge_type: String = prow.get("edge_type");
        for &new_article in &[id_a, id_b] {
            sqlx::query(
                "INSERT INTO covalence.edges \
                     (id, source_node_id, target_node_id, edge_type, weight, created_by) \
                 VALUES ($1, $2, $3, $4, 1.0, 'split_inherit')",
            )
            .bind(Uuid::new_v4())
            .bind(src_id)
            .bind(new_article)
            .bind(&edge_type)
            .execute(pool)
            .await
            .context("split: failed to copy provenance edge")?;
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

    // ── 8. Queue embed tasks for new articles ───────────────────────────────
    enqueue_task(pool, "embed", Some(id_a), json!({}), 5).await?;
    enqueue_task(pool, "embed", Some(id_b), json!({}), 5).await?;

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

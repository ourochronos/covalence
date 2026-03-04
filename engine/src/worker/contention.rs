//! Contention detection and resolution handlers for the slow-path worker.
//!
//! Two entry points:
//!
//! * [`handle_contention_check`] — triggered after source ingest; uses vector
//!   similarity to find article candidates and asks the LLM whether a real
//!   contention exists.
//!
//! * [`handle_resolve_contention`] — picks up a detected contention
//!   (`contentions` row with `status = 'detected'`) and
//!   asks the LLM to decide how to resolve it, then applies the resolution.

use std::sync::Arc;
use std::time::Instant;

use anyhow::Context as _;
use serde_json::{Value, json};
use sqlx::{PgPool, Row as _};
use uuid::Uuid;

use super::QueueTask;
use super::llm::LlmClient;
use crate::graph::{GraphRepository as _, SqlGraphRepository};
use crate::models::EdgeType;

// ─── JSON helpers ─────────────────────────────────────────────────────────────

/// Strip markdown code-fences from an LLM response and parse as [`Value`].
fn extract_json_value(text: &str) -> anyhow::Result<Value> {
    let text = text.trim();
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
    serde_json::from_str(&json_str).context("failed to parse JSON from LLM response")
}

// ─── handle_resolve_contention ────────────────────────────────────────────────

/// `resolve_contention`: LLM-driven resolution of a single contention record.
///
/// The contention is represented as a row in `covalence.contentions`
/// with `status = 'detected'`.
///
/// Payload: `{ "contention_id": "<uuid>" }`
pub async fn handle_resolve_contention(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    // ── 1. Read contention_id from payload ──────────────────────────────────
    let contention_id: Uuid = task
        .payload
        .get("contention_id")
        .and_then(|v| v.as_str())
        .context("resolve_contention: missing payload.contention_id")?
        .parse()
        .context("resolve_contention: contention_id is not a valid UUID")?;

    // ── 0. Idempotency guard ──────────────────────────────────────────────
    if super::already_completed(pool, task).await? {
        tracing::info!(task_id = %task.id, "resolve_contention: idempotency guard — already complete, skipping");
        return Ok(serde_json::json!({"skipped": true, "reason": "already_complete"}));
    }

    tracing::info!(
        task_id        = %task.id,
        contention_id  = %contention_id,
        "resolve_contention: starting"
    );

    // ── 2. Fetch the contentions row ───────────────────────────────────────
    let contention_row = sqlx::query(
        r#"SELECT id, node_id, source_node_id, type
           FROM   covalence.contentions
           WHERE  id     = $1
             AND  status = 'detected'"#,
    )
    .bind(contention_id)
    .fetch_optional(pool)
    .await
    .context("failed to fetch contention row")?
    .with_context(|| format!("contention {contention_id} not found (or already resolved)"))?;

    let article_id: Uuid = contention_row.get("node_id");
    let source_id: Uuid = contention_row.get("source_node_id");
    let relationship: String = contention_row
        .get::<Option<String>, _>("type")
        .unwrap_or_else(|| "contends".to_string());

    // ── 3. Fetch article and source content ─────────────────────────────────
    let article_row = sqlx::query("SELECT title, content FROM covalence.nodes WHERE id = $1")
        .bind(article_id)
        .fetch_optional(pool)
        .await
        .context("failed to fetch article")?
        .with_context(|| format!("article {article_id} not found"))?;

    let article_title: String = article_row.try_get("title").unwrap_or_default();
    let article_content: String = article_row.try_get("content").unwrap_or_default();

    let source_row = sqlx::query("SELECT title, content FROM covalence.nodes WHERE id = $1")
        .bind(source_id)
        .fetch_optional(pool)
        .await
        .context("failed to fetch source")?
        .with_context(|| format!("source {source_id} not found"))?;

    let source_title: String = source_row.try_get("title").unwrap_or_default();
    let source_content: String = source_row.try_get("content").unwrap_or_default();

    // ── 4. Ask LLM to evaluate the contention ───────────────────────────────
    let prompt = format!(
        r#"You are a knowledge-base curator resolving a content contention.

## Article (id: {article_id})
Title: {article_title}

{article_content}

## Source (id: {source_id})
Title: {source_title}

{source_content}

## Contention relationship
The source was flagged as "{relationship}" with respect to the article.

Evaluate whether the source actually contradicts or supersedes the article,
and choose the most appropriate resolution:

- **supersede_a**: The article is correct; the source does not materially
  change it. Source is noted but article stays unchanged.
- **supersede_b**: The source contains more accurate/recent information.
  The article content should be replaced. Provide `updated_content`.
- **accept_both**: Both perspectives are valid (e.g. different contexts,
  time periods, or framings). Annotate the article to acknowledge the tension.
- **dismiss**: The contention is not material (e.g. trivial wording
  differences, out-of-scope source). Ignore it.

Respond ONLY with valid JSON matching this schema (no prose, no fences):
{{
  "resolution":      "supersede_a|supersede_b|accept_both|dismiss",
  "materiality":     "high|medium|low",
  "reasoning":       "concise explanation (1-3 sentences)",
  "updated_content": "full replacement article text (ONLY required when resolution=supersede_b)"
}}"#,
    );

    let chat_model = std::env::var("COVALENCE_CHAT_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
    let t0 = Instant::now();
    let raw = llm
        .complete(&prompt, 2048)
        .await
        .context("LLM call failed for resolve_contention")?;
    let llm_latency_ms = t0.elapsed().as_millis() as i32;

    tracing::debug!(task_id = %task.id, raw_response = %raw, "resolve_contention: LLM response received");

    let llm_json =
        extract_json_value(&raw).context("resolve_contention: could not parse LLM JSON")?;

    let resolution = llm_json
        .get("resolution")
        .and_then(|v| v.as_str())
        .unwrap_or("dismiss");
    let materiality = llm_json
        .get("materiality")
        .and_then(|v| v.as_str())
        .unwrap_or("low");
    let reasoning = llm_json
        .get("reasoning")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let updated_content = llm_json.get("updated_content").and_then(|v| v.as_str());

    tracing::info!(
        task_id       = %task.id,
        contention_id = %contention_id,
        resolution,
        materiality,
        "resolve_contention: applying LLM resolution"
    );

    // ── 4a. Log inference ──────────────────────────────────────────────────
    {
        let input_node_ids = [article_id, source_id];
        let input_summary = format!(
            "contention_id={contention_id}, article_id={article_id}, source_id={source_id}, relationship={relationship}"
        );
        let output_decision = format!("resolution={resolution}, materiality={materiality}");
        if let Err(e) = super::log_inference(
            pool,
            "resolve_contention",
            &input_node_ids,
            &input_summary,
            &output_decision,
            None,
            reasoning,
            &chat_model,
            llm_latency_ms,
        )
        .await
        {
            tracing::warn!(task_id = %task.id, "resolve_contention: log_inference failed: {e:#}");
        }
    }

    // ── 5. Apply the resolution ──────────────────────────────────────────────
    match resolution {
        // Article wins — mark relationship resolved, leave article content alone.
        "supersede_a" => {
            sqlx::query(
                "UPDATE covalence.contentions
                    SET status = 'resolved', resolution = 'supersede_a', resolved_at = now()
                  WHERE id = $1",
            )
            .bind(contention_id)
            .execute(pool)
            .await
            .context("supersede_a: failed to update contentions")?;

            record_mutation(
                pool,
                article_id,
                "contention_resolved",
                &format!(
                    "Contention {contention_id} resolved as supersede_a (article wins). \
                     Materiality: {materiality}. {reasoning}"
                ),
            )
            .await?;
        }

        // Source wins — replace article content, update relationship, re-embed.
        "supersede_b" => {
            let new_content = updated_content
                .context("supersede_b: LLM did not provide updated_content")?;

            sqlx::query(
                "UPDATE covalence.nodes
                    SET content     = $1,
                        modified_at = now()
                  WHERE id = $2",
            )
            .bind(new_content)
            .bind(article_id)
            .execute(pool)
            .await
            .context("supersede_b: failed to update article content")?;

            // Increment version (Item 7)
            sqlx::query(
                "UPDATE covalence.nodes SET version = version + 1 WHERE id = $1",
            )
            .bind(article_id)
            .execute(pool)
            .await
            .context("supersede_b: failed to increment article version")?;

            // Create CONFIRMS edge: source → article via GraphRepository (dual-writes AGE + SQL).
            {
                let graph = SqlGraphRepository::new(pool.clone());
                if let Err(e) = graph
                    .create_edge(
                        source_id,
                        article_id,
                        EdgeType::Confirms,
                        1.0,
                        "resolve_contention",
                        serde_json::json!({}),
                    )
                    .await
                {
                    tracing::warn!(
                        source_id  = %source_id,
                        article_id = %article_id,
                        "supersede_b: failed to create CONFIRMS edge via GraphRepository: {e}"
                    );
                }
            }

            sqlx::query(
                "UPDATE covalence.contentions
                    SET status = 'resolved', resolution = 'supersede_b', resolved_at = now()
                  WHERE id = $1",
            )
            .bind(contention_id)
            .execute(pool)
            .await
            .context("supersede_b: failed to update contentions")?;

            // Queue re-embed so the new content gets a fresh vector
            sqlx::query(
                "INSERT INTO covalence.slow_path_queue
                     (task_type, node_id, payload, priority, status)
                 VALUES ('embed', $1, '{}', 5, 'pending')",
            )
            .bind(article_id)
            .execute(pool)
            .await
            .context("supersede_b: failed to queue embed task")?;

            // Queue tree_embed to invalidate stale section embeddings (Item 8)
            sqlx::query(
                "INSERT INTO covalence.slow_path_queue
                     (task_type, node_id, payload, priority, status)
                 VALUES ('tree_embed', $1, '{}', 4, 'pending')",
            )
            .bind(article_id)
            .execute(pool)
            .await
            .context("supersede_b: failed to queue tree_embed task")?;

            record_mutation(
                pool,
                article_id,
                "content_superseded",
                &format!(
                    "Contention {contention_id} resolved as supersede_b (source wins). \
                     Article content replaced. Materiality: {materiality}. {reasoning}"
                ),
            )
            .await?;
        }

        // Both valid — annotate article metadata.contention_notes.
        "accept_both" => {
            let note = json!({
                "contention_id": contention_id.to_string(),
                "source_id":     source_id.to_string(),
                "materiality":   materiality,
                "note":          reasoning,
            });

            // Append to metadata.contention_notes (initialise to [] if absent)
            sqlx::query(
                r#"UPDATE covalence.nodes
                      SET metadata   = jsonb_set(
                                           coalesce(metadata, '{}'::jsonb),
                                           '{contention_notes}',
                                           coalesce(metadata->'contention_notes', '[]'::jsonb)
                                           || $1::jsonb,
                                           true
                                       ),
                          modified_at = now()
                    WHERE id = $2"#,
            )
            .bind(json!([note]))
            .bind(article_id)
            .execute(pool)
            .await
            .context("accept_both: failed to update article metadata")?;

            sqlx::query(
                "UPDATE covalence.contentions
                    SET status = 'resolved', resolution = 'accept_both', resolved_at = now()
                  WHERE id = $1",
            )
            .bind(contention_id)
            .execute(pool)
            .await
            .context("accept_both: failed to update contentions")?;

            record_mutation(
                pool,
                article_id,
                "contention_acknowledged",
                &format!(
                    "Contention {contention_id} acknowledged (accept_both). \
                     Materiality: {materiality}. {reasoning}"
                ),
            )
            .await?;
        }

        // Dismiss — not material, mark and move on.
        _ /* "dismiss" */ => {
            sqlx::query(
                "UPDATE covalence.contentions
                    SET status = 'dismissed', resolution = 'dismiss', resolved_at = now()
                  WHERE id = $1",
            )
            .bind(contention_id)
            .execute(pool)
            .await
            .context("dismiss: failed to update contentions")?;

            record_mutation(
                pool,
                article_id,
                "contention_dismissed",
                &format!(
                    "Contention {contention_id} dismissed (not material). \
                     Materiality: {materiality}. {reasoning}"
                ),
            )
            .await?;
        }
    }

    // ── 6. Return resolution summary ─────────────────────────────────────────
    Ok(json!({
        "contention_id": contention_id,
        "article_id":    article_id,
        "source_id":     source_id,
        "resolution":    resolution,
        "materiality":   materiality,
        "reasoning":     reasoning,
    }))
}

// ─── handle_contention_check ─────────────────────────────────────────────────

/// `contention_check`: Detect new contentions after a source is ingested.
///
/// Uses vector similarity (cosine distance via pgvector `<=>`) to find the
/// top-5 most similar articles, then asks the LLM for each whether a real
/// contention exists.  For each confirmed, non-low-materiality contention:
///
/// * inserts a `contentions` row with appropriate type
/// * queues a `resolve_contention` task
///
/// Requires `task.node_id` (the newly ingested source node).
pub async fn handle_contention_check(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    let source_id = task
        .node_id
        .context("contention_check: task requires node_id")?;

    // ── 0. Idempotency guard ──────────────────────────────────────────────
    if super::already_completed(pool, task).await? {
        tracing::info!(task_id = %task.id, "contention_check: idempotency guard — already complete, skipping");
        return Ok(serde_json::json!({"skipped": true, "reason": "already_complete"}));
    }

    tracing::info!(
        task_id   = %task.id,
        source_id = %source_id,
        "contention_check: starting"
    );

    // ── 1. Fetch source content ──────────────────────────────────────────────
    let source_row = sqlx::query("SELECT title, content FROM covalence.nodes WHERE id = $1")
        .bind(source_id)
        .fetch_optional(pool)
        .await
        .context("contention_check: failed to fetch source node")?
        .with_context(|| format!("contention_check: source node {source_id} not found"))?;

    let source_title: String = source_row.try_get("title").unwrap_or_default();
    let source_content: String = source_row.try_get("content").unwrap_or_default();

    // ── 2. Guard: source must already have an embedding ─────────────────────
    let has_embedding = sqlx::query("SELECT 1 FROM covalence.node_embeddings WHERE node_id = $1")
        .bind(source_id)
        .fetch_optional(pool)
        .await?
        .is_some();

    if !has_embedding {
        tracing::warn!(
            source_id = %source_id,
            "contention_check: source has no embedding yet — skipping vector search"
        );
        return Ok(json!({
            "source_id":           source_id,
            "candidates_checked":  0,
            "contentions_created": 0,
            "note": "source embedding not yet available; rerun after embed task completes",
        }));
    }

    // ── 3. Vector search: top-5 most similar articles ───────────────────────
    // Self-join on node_embeddings; filter target to kind='article'.
    let candidates = sqlx::query(
        r#"SELECT
               n.id                                      AS article_id,
               n.title                                   AS article_title,
               n.content                                 AS article_content,
               ne.embedding <=> src_emb.embedding        AS distance
           FROM       covalence.node_embeddings AS src_emb
           JOIN       covalence.node_embeddings AS ne
             ON       ne.node_id != src_emb.node_id
           JOIN       covalence.nodes           AS n
             ON       n.id     = ne.node_id
            AND       n.node_type = 'article'
            AND       n.status = 'active'
           WHERE      src_emb.node_id = $1
           ORDER BY   distance ASC
           LIMIT 5"#,
    )
    .bind(source_id)
    .fetch_all(pool)
    .await
    .context("contention_check: vector similarity search failed")?;

    tracing::debug!(
        source_id  = %source_id,
        found      = candidates.len(),
        "contention_check: vector search returned candidates"
    );

    let mut contentions_created = 0_i32;

    // ── 4. LLM contention check for each candidate ───────────────────────────
    for row in &candidates {
        let article_id: Uuid = row.get("article_id");
        let article_title: String = row.try_get("article_title").unwrap_or_default();
        let article_content: String = row.try_get("article_content").unwrap_or_default();
        let distance: f64 = row
            .try_get::<f32, _>("distance")
            .map(|d| d as f64)
            .unwrap_or(1.0);

        // Skip very distant results (cosine distance close to 1 = unrelated)
        if distance > 0.80 {
            tracing::debug!(
                source_id  = %source_id,
                article_id = %article_id,
                distance,
                "contention_check: candidate too dissimilar, skipping"
            );
            continue;
        }

        // Skip if a contention link already exists between these two
        let already_linked = sqlx::query(
            r#"SELECT 1 FROM covalence.contentions
               WHERE  node_id        = $1
                 AND  source_node_id = $2
                 AND  status != 'dismissed'
               LIMIT 1"#,
        )
        .bind(article_id)
        .bind(source_id)
        .fetch_optional(pool)
        .await?
        .is_some();

        if already_linked {
            tracing::debug!(
                source_id  = %source_id,
                article_id = %article_id,
                "contention_check: contention already recorded, skipping"
            );
            continue;
        }

        // Ask the LLM
        let prompt = format!(
            r#"You are a knowledge-base curator checking for content conflicts.

## Existing Article (id: {article_id})
Title: {article_title}

{article_content}

## Newly Ingested Source (id: {source_id})
Title: {source_title}

{source_content}

Does the source CONTRADICT or CONTEND with the article?

- "contradicts": the source states something directly opposite or factually
  incompatible with the article.
- "contends": the source presents a competing perspective, alternative
  framing, or significantly different emphasis that creates meaningful tension
  (but is not an outright factual contradiction).
- Not a contention if the two are merely on different sub-topics or
  complementary.

Respond ONLY with valid JSON (no prose, no fences).

If a contention:
{{
  "is_contention": true,
  "relationship":  "contradicts|contends",
  "materiality":   "high|medium|low",
  "explanation":   "one-sentence reason"
}}

If NOT a contention:
{{
  "is_contention": false,
  "materiality":   "low",
  "explanation":   "one-sentence reason"
}}"#,
        );

        let chat_model =
            std::env::var("COVALENCE_CHAT_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
        let t0 = Instant::now();
        let raw = match llm.complete(&prompt, 512).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    source_id  = %source_id,
                    article_id = %article_id,
                    "contention_check: LLM error: {e:#}"
                );
                continue;
            }
        };
        let llm_latency_ms = t0.elapsed().as_millis() as i32;

        let llm_json = match extract_json_value(&raw) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    source_id  = %source_id,
                    article_id = %article_id,
                    "contention_check: could not parse LLM JSON: {e:#}"
                );
                continue;
            }
        };

        let is_contention = llm_json
            .get("is_contention")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let materiality = llm_json
            .get("materiality")
            .and_then(|v| v.as_str())
            .unwrap_or("low");
        let rel_str = llm_json
            .get("relationship")
            .and_then(|v| v.as_str())
            .unwrap_or("contends");
        let explanation = llm_json
            .get("explanation")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Log inference for this LLM call
        {
            let input_nodes = [source_id, article_id];
            let input_summary =
                format!("source_id={source_id}, article_id={article_id}, distance={distance:.4}");
            let explanation_str = llm_json
                .get("explanation")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let output_decision =
                format!("is_contention={is_contention}, materiality={materiality}");
            if let Err(e) = super::log_inference(
                pool,
                "contention_check",
                &input_nodes,
                &input_summary,
                &output_decision,
                None,
                explanation_str,
                &chat_model,
                llm_latency_ms,
            )
            .await
            {
                tracing::warn!(
                    source_id = %source_id,
                    article_id = %article_id,
                    "contention_check: log_inference failed: {e:#}"
                );
            }
        }

        // ── 5. Skip if not a contention or materiality is low ───────────────
        if !is_contention || materiality == "low" {
            tracing::debug!(
                source_id   = %source_id,
                article_id  = %article_id,
                is_contention,
                materiality,
                "contention_check: no actionable contention"
            );
            continue;
        }

        // Normalise relationship value and map to ASPIC+ contention_type
        let (relationship, contention_type) = if rel_str == "contradicts" {
            ("contradicts", "rebuttal")
        } else {
            ("contends", "rebuttal")
        };

        // ── 6. Insert contentions row + queue resolve_contention ─────────────
        let mat_score: f64 = match materiality {
            "high" => 0.9,
            "medium" => 0.5,
            _ => 0.2,
        };
        let contention_id: Uuid = sqlx::query_scalar(
            r#"INSERT INTO covalence.contentions
                   (node_id, source_node_id, type, description, severity, status, materiality, contention_type)
               VALUES ($1, $2, $3, $4, $5, 'detected', $6, $7)
               RETURNING id"#,
        )
        .bind(article_id)
        .bind(source_id)
        .bind(relationship)
        .bind(format!("Source {source_id} flagged as '{relationship}' against article {article_id}: {explanation}"))
        .bind(materiality)
        .bind(mat_score)
        .bind(contention_type)
        .fetch_one(pool)
        .await
        .context("contention_check: failed to insert contention")?;

        contentions_created += 1;

        tracing::info!(
            source_id     = %source_id,
            article_id    = %article_id,
            contention_id = %contention_id,
            relationship,
            materiality,
            "contention_check: contention recorded"
        );

        // Queue the resolver
        sqlx::query(
            r#"INSERT INTO covalence.slow_path_queue
                   (task_type, node_id, payload, priority, status)
               VALUES (
                   'resolve_contention',
                   $1,
                   jsonb_build_object('contention_id', $2::text),
                   6,
                   'pending'
               )"#,
        )
        .bind(article_id)
        .bind(contention_id)
        .execute(pool)
        .await
        .context("contention_check: failed to queue resolve_contention task")?;

        tracing::info!(
            contention_id = %contention_id,
            "contention_check: resolve_contention task queued"
        );

        record_mutation(
            pool,
            article_id,
            "contention_detected",
            &format!(
                "Source {source_id} flagged as '{relationship}' against this article \
                 (materiality={materiality}). {explanation}"
            ),
        )
        .await?;
    }

    Ok(json!({
        "source_id":           source_id,
        "candidates_checked":  candidates.len(),
        "contentions_created": contentions_created,
    }))
}

// ─── Shared helpers ───────────────────────────────────────────────────────────

/// Record a mutation event in node metadata (no dedicated mutations table).
async fn record_mutation(
    pool: &PgPool,
    article_id: Uuid,
    mutation_type: &str,
    summary: &str,
) -> anyhow::Result<()> {
    let entry = serde_json::json!([{
        "type": mutation_type,
        "summary": summary,
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
    .bind(&entry)
    .bind(article_id)
    .execute(pool)
    .await
    .with_context(|| {
        format!("record_mutation: failed to log {mutation_type} mutation for article {article_id}")
    })?;
    Ok(())
}

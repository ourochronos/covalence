//! `critique_article` slow-path task handler (covalence#105).
//!
//! Implements a Reflexion-style critique loop: after an article is compiled,
//! a deferred critique task evaluates its quality using an LLM and records the
//! result as an observation source linked to the article via a `CRITIQUES` edge.
//!
//! # Task payload
//! The `node_id` field on the queue row holds the `article_id` to evaluate.
//! The `payload` field is currently unused (reserved for future configuration).
//!
//! # LLM output schema
//! ```json
//! {
//!   "overall_quality": 0.85,
//!   "issues": ["Gap in coverage of X", "Unsupported claim Y"],
//!   "recommendation": "accept"
//! }
//! ```
//! `recommendation` is one of `"accept"`, `"flag"`, or `"recompile"`.
//!
//! # Recompile trigger
//! When `recommendation == "recompile"` AND `overall_quality < 0.6` AND
//! `consolidation_count < 5`, a new `consolidate_article` task is enqueued
//! immediately (no delay), so the article is synthesised again.

use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::QueueTask;
use super::llm::LlmClient;

/// Quality threshold below which a "recompile" recommendation triggers
/// an immediate reconsolidation.
const QUALITY_RECOMPILE_THRESHOLD: f64 = 0.6;

/// Maximum consolidation passes before the critique loop stops triggering
/// automatic recompiles (prevents runaway LLM spend).
const MAX_AUTO_RECOMPILE_COUNT: i32 = 5;

/// Strip markdown code fences from an LLM response.
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

/// Handle a `critique_article` task.
///
/// Steps:
/// 1. Resolve `article_id` from `task.node_id`.
/// 2. Fetch article content, title, and `consolidation_count`.
/// 3. Fetch titles of linked source nodes (for context in the prompt).
/// 4. Call LLM for a structured critique (JSON: overall_quality, issues, recommendation).
/// 5. Store critique result as a new `observation` source node.
/// 6. Create a `CRITIQUES` edge from the critique source → article.
/// 7. If conditions met, re-queue `consolidate_article`.
pub async fn handle_critique_article(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    use sqlx::Row as _;

    // ── 1. Resolve article_id ────────────────────────────────────────────────
    let article_id = task
        .node_id
        .context("critique_article: task requires node_id (article_id)")?;

    tracing::info!(
        task_id    = %task.id,
        article_id = %article_id,
        "critique_article: starting"
    );

    // ── 2. Fetch article ─────────────────────────────────────────────────────
    let article_row = sqlx::query(
        "SELECT title, content, COALESCE(consolidation_count, 0) AS consolidation_count
         FROM   covalence.nodes
         WHERE  id = $1 AND node_type = 'article' AND status = 'active'",
    )
    .bind(article_id)
    .fetch_optional(pool)
    .await
    .context("critique_article: failed to fetch article")?;

    let article_row = match article_row {
        Some(r) => r,
        None => {
            tracing::info!(
                task_id    = %task.id,
                article_id = %article_id,
                "critique_article: article not found or inactive, skipping"
            );
            return Ok(json!({"skipped": true, "reason": "article_not_found"}));
        }
    };

    let article_title: String = article_row
        .get::<Option<String>, _>("title")
        .unwrap_or_default();
    let article_content: String = article_row
        .get::<Option<String>, _>("content")
        .unwrap_or_default();
    let consolidation_count: i32 = article_row.get("consolidation_count");

    // ── 3. Fetch linked source titles ────────────────────────────────────────
    let source_titles: Vec<String> = sqlx::query_scalar(
        "SELECT COALESCE(n.title, '(untitled)')
         FROM   covalence.edges e
         JOIN   covalence.nodes n ON n.id = e.source_node_id
         WHERE  e.target_node_id = $1
           AND  e.edge_type IN ('ORIGINATES', 'COMPILED_FROM', 'CONFIRMS')
           AND  n.node_type = 'source'
           AND  n.status    = 'active'
         LIMIT  20",
    )
    .bind(article_id)
    .fetch_all(pool)
    .await
    .context("critique_article: failed to fetch source titles")?;

    let sources_summary = if source_titles.is_empty() {
        "(no linked sources)".to_string()
    } else {
        source_titles.join(", ")
    };

    // ── 4. LLM critique ──────────────────────────────────────────────────────
    let prompt = format!(
        "You are a knowledge-quality critic. Evaluate the following article and \
return ONLY valid JSON (no markdown fences), exactly:\n\
{{\n\
  \"overall_quality\": <float 0.0–1.0>,\n\
  \"issues\": [\"<gap or problem>\", ...],\n\
  \"recommendation\": \"accept|flag|recompile\"\n\
}}\n\
\n\
Use these criteria:\n\
- overall_quality: 1.0 = comprehensive, well-supported, no gaps; \
0.0 = incoherent, unsupported, or severely incomplete.\n\
- issues: list specific gaps, unsupported claims, internal contradictions, \
or missing context. Empty array if none.\n\
- recommendation:\n\
  * \"accept\"    — quality is sufficient; no action needed.\n\
  * \"flag\"      — notable issues but not worth recompiling (e.g. style, minor gaps).\n\
  * \"recompile\" — significant issues; article should be re-synthesised.\n\
\n\
Article title: {article_title}\n\
Source documents used: {sources_summary}\n\
\n\
ARTICLE CONTENT:\n\
{article_content}"
    );

    let chat_model = std::env::var("COVALENCE_CHAT_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
    let t0 = Instant::now();
    let llm_result = llm.complete(&prompt, 1024).await;
    let llm_latency_ms = t0.elapsed().as_millis() as i32;

    // Parse the LLM response — degrade gracefully on failure.
    let (overall_quality, issues, recommendation) = match llm_result {
        Ok(raw) => match serde_json::from_str::<Value>(&strip_fences(&raw)) {
            Ok(v) => {
                let quality = v
                    .get("overall_quality")
                    .and_then(|x| x.as_f64())
                    .unwrap_or(0.5)
                    .clamp(0.0, 1.0);
                let issues: Vec<String> = v
                    .get("issues")
                    .and_then(|x| x.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|s| s.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let rec = v
                    .get("recommendation")
                    .and_then(|x| x.as_str())
                    .unwrap_or("accept")
                    .to_string();
                (quality, issues, rec)
            }
            Err(e) => {
                tracing::warn!(
                    task_id    = %task.id,
                    article_id = %article_id,
                    "critique_article: JSON parse error ({e}), defaulting to accept"
                );
                (0.5, vec![], "accept".to_string())
            }
        },
        Err(e) => {
            tracing::warn!(
                task_id    = %task.id,
                article_id = %article_id,
                "critique_article: LLM error ({e}), defaulting to accept"
            );
            (0.5, vec![], "accept".to_string())
        }
    };

    tracing::info!(
        task_id        = %task.id,
        article_id     = %article_id,
        overall_quality,
        recommendation = %recommendation,
        issues_count   = issues.len(),
        llm_latency_ms,
        "critique_article: LLM critique complete"
    );

    // ── 5. Store critique as an observation source node ──────────────────────
    let critique_title = format!("Critique: {article_title}");
    let critique_content = if issues.is_empty() {
        format!(
            "Quality score: {overall_quality:.2}\nRecommendation: {recommendation}\n\nNo issues identified."
        )
    } else {
        format!(
            "Quality score: {overall_quality:.2}\nRecommendation: {recommendation}\n\nIssues:\n{}",
            issues
                .iter()
                .map(|i| format!("- {i}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    let critique_metadata = json!({
        "critique":        true,
        "article_id":      article_id.to_string(),
        "overall_quality": overall_quality,
        "recommendation":  recommendation,
        "issues":          issues,
        "model":           chat_model,
        "latency_ms":      llm_latency_ms,
    });

    let critique_source_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes \
             (id, node_type, source_type, status, title, content, metadata, reliability) \
         VALUES ($1, 'source', 'observation', 'active', $2, $3, $4, $5)",
    )
    .bind(critique_source_id)
    .bind(&critique_title)
    .bind(&critique_content)
    .bind(&critique_metadata)
    .bind(overall_quality)
    .execute(pool)
    .await
    .context("critique_article: failed to insert critique source node")?;

    tracing::debug!(
        task_id            = %task.id,
        critique_source_id = %critique_source_id,
        "critique_article: critique source node created"
    );

    // ── 6. Create CRITIQUES edge: critique_source → article ──────────────────
    sqlx::query(
        "INSERT INTO covalence.edges \
             (source_node_id, target_node_id, edge_type, weight, confidence, created_by) \
         VALUES ($1, $2, 'CRITIQUES', 1.0, $3, 'critique_article') \
         ON CONFLICT (source_node_id, target_node_id, edge_type) DO NOTHING",
    )
    .bind(critique_source_id)
    .bind(article_id)
    .bind(overall_quality)
    .execute(pool)
    .await
    .context("critique_article: failed to insert CRITIQUES edge")?;

    tracing::info!(
        task_id            = %task.id,
        article_id         = %article_id,
        critique_source_id = %critique_source_id,
        "critique_article: CRITIQUES edge created"
    );

    // ── 7. Optionally re-queue consolidation ─────────────────────────────────
    let should_recompile = recommendation == "recompile"
        && overall_quality < QUALITY_RECOMPILE_THRESHOLD
        && consolidation_count < MAX_AUTO_RECOMPILE_COUNT;

    if should_recompile {
        let next_pass = consolidation_count + 1;
        super::enqueue_task(
            pool,
            "consolidate_article",
            None,
            json!({
                "article_id": article_id.to_string(),
                "pass": next_pass,
            }),
            4, // higher priority than the scheduled pass
        )
        .await
        .context("critique_article: failed to enqueue consolidate_article")?;

        tracing::info!(
            task_id    = %task.id,
            article_id = %article_id,
            next_pass,
            overall_quality,
            "critique_article: low quality — re-queued consolidate_article"
        );
    }

    Ok(json!({
        "article_id":          article_id,
        "critique_source_id":  critique_source_id,
        "overall_quality":     overall_quality,
        "recommendation":      recommendation,
        "issues_count":        issues.len(),
        "triggered_recompile": should_recompile,
    }))
}

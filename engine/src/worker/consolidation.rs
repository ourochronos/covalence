//! `consolidate_article` slow-path task handler (covalence#67).
//!
//! Implements a spacing-effect (expanding-interval) recompilation schedule
//! for articles, inspired by the neuroscience spacing effect (Smolen et al. 2017).
//!
//! | consolidation_count (before pass) | next_consolidation_at delay |
//! |-----------------------------------|-----------------------------|
//! | 0 (initial compile)               | +1 hour  (→ pass 1)         |
//! | 1 (after pass 1)                  | +12 hours (→ pass 2)        |
//! | 2 (after pass 2)                  | +3 days  (→ pass 3)         |
//! | 3 (after pass 3)                  | +1 week  (→ pass 4)         |
//! | 4+ (after pass 4+)                | +1 month (→ pass N+1)       |
//!
//! Orphan articles (those with no linked sources) are **skipped** — they have
//! no source material to synthesise and advancing them serves no purpose.
//!
//! # Configurable schedule constants
//! All interval values are defined as named constants (`SCHEDULE_*`) at the top
//! of this module so they can be adjusted without touching handler logic.
//!
//! # Payload shape
//! ```json
//! { "article_id": "<uuid>", "pass": 1 }
//! ```

use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use chrono::Utc;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::QueueTask;
use super::llm::LlmClient;

// ─── schedule constants ───────────────────────────────────────────────────────
//
// These are the only places to change timing — handler logic derives all
// intervals from these values.

/// Delay before the first consolidation pass (set immediately after compile).
pub const SCHEDULE_PASS_1_HOURS: i64 = 1;

/// Delay after pass 1 completes before pass 2 is eligible.
pub const SCHEDULE_PASS_2_HOURS: i64 = 12;

/// Delay after pass 2 completes before pass 3 is eligible (3 days).
pub const SCHEDULE_PASS_3_DAYS: i64 = 3;

/// Delay after pass 3 completes before pass 4 is eligible (1 week).
pub const SCHEDULE_PASS_4_WEEKS: i64 = 1;

/// Delay after pass 4+ completes before the next pass is eligible (1 month ≈ 30 days).
pub const SCHEDULE_PASS_N_DAYS: i64 = 30;

// ─── internal helpers ─────────────────────────────────────────────────────────

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

struct SourceDoc {
    id: Uuid,
    title: String,
    content: String,
}

/// Concatenate source documents into a plain-text fallback article body.
fn concatenate_sources(sources: &[SourceDoc]) -> String {
    let mut out = String::new();
    for s in sources {
        if !s.title.is_empty() {
            out.push_str(&format!("## {}\n\n", s.title));
        }
        out.push_str(&s.content);
        out.push_str("\n\n");
    }
    out
}

/// Compute the delay until the *next* consolidation pass, given the pass number
/// that just completed.
///
/// The schedule never terminates — after pass 4 every subsequent pass recurs
/// on a monthly interval.  All timing is driven by the `SCHEDULE_*` constants
/// so the values are visible at a glance and easy to adjust.
pub fn next_pass_delay(completed_pass: i32) -> chrono::Duration {
    match completed_pass {
        1 => chrono::Duration::hours(SCHEDULE_PASS_2_HOURS),
        2 => chrono::Duration::days(SCHEDULE_PASS_3_DAYS),
        3 => chrono::Duration::weeks(SCHEDULE_PASS_4_WEEKS),
        _ => chrono::Duration::days(SCHEDULE_PASS_N_DAYS),
    }
}

// ─── handler ──────────────────────────────────────────────────────────────────

/// Handle a `consolidate_article` task.
///
/// Steps:
/// 1. Parse `article_id` and `pass` from the payload.
/// 2. Fetch the article — skip if not found or archived.
/// 3. **Orphan guard** — skip if the article has zero linked sources.  An
///    article with no sources has nothing to synthesise; advancing it wastes
///    LLM capacity and does not converge toward a better article.
/// 4. Check for new sources ingested since `modified_at` that are not yet
///    linked to the article (mirrors the orphan-detection logic from
///    reconsolidation.rs).
/// 5. If new sources exist: re-compile using the same LLM synthesis prompt
///    as reconsolidation, then update article content + create provenance edges.
/// 6. Increment `consolidation_count` and set `next_consolidation_at` using
///    the expanding-interval schedule defined by [`next_pass_delay`].
pub async fn handle_consolidate_article(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    use sqlx::Row as _;

    // ── 1. Parse payload ────────────────────────────────────────────────────
    let article_id: Uuid = task
        .payload
        .get("article_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .context("consolidate_article: missing or invalid article_id in payload")?;

    let pass: i32 = task
        .payload
        .get("pass")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .unwrap_or(1);

    tracing::info!(
        task_id    = %task.id,
        article_id = %article_id,
        pass,
        "consolidate_article: starting"
    );

    // ── 2. Fetch article ────────────────────────────────────────────────────
    let article_row = sqlx::query(
        "SELECT title, content, modified_at \
         FROM   covalence.nodes \
         WHERE  id = $1 AND status = 'active' AND node_type = 'article'",
    )
    .bind(article_id)
    .fetch_optional(pool)
    .await
    .context("consolidate_article: failed to fetch article")?;

    let article_row = match article_row {
        Some(r) => r,
        None => {
            tracing::info!(
                task_id    = %task.id,
                article_id = %article_id,
                "consolidate_article: article not found or inactive, skipping"
            );
            return Ok(json!({"skipped": true, "reason": "article_not_found"}));
        }
    };

    let article_title: String = article_row
        .get::<Option<String>, _>("title")
        .unwrap_or_default();
    let article_modified_at: chrono::DateTime<Utc> = article_row.get("modified_at");

    // ── 3. Orphan guard ─────────────────────────────────────────────────────
    // An article with zero linked sources has no material to synthesise.
    // Advancing such an article wastes LLM capacity and produces no value.
    // We return early *without* updating consolidation_count or schedule so
    // the task remains logically due — if sources are linked later the next
    // heartbeat will pick it up.
    let linked_source_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM   covalence.edges e
         JOIN   covalence.nodes n ON n.id = e.source_node_id
         WHERE  e.target_node_id = $1
           AND  e.edge_type IN ('ORIGINATES', 'COMPILED_FROM', 'CONFIRMS')
           AND  n.node_type = 'source'
           AND  n.status    = 'active'",
    )
    .bind(article_id)
    .fetch_one(pool)
    .await
    .context("consolidate_article: failed to count linked sources")?;

    if linked_source_count == 0 {
        tracing::info!(
            task_id    = %task.id,
            article_id = %article_id,
            pass,
            "consolidate_article: orphan article (no linked sources), skipping"
        );
        return Ok(json!({
            "skipped":        true,
            "reason":         "orphan_article",
            "article_id":     article_id,
            "pass":           pass,
        }));
    }

    // ── 4. Check for new sources since article's modified_at ────────────────
    // Find active source nodes that:
    //  - were created after the article's modified_at
    //  - are not already linked to this article via any edge
    let new_source_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT s.id
         FROM   covalence.nodes s
         WHERE  s.node_type = 'source'
           AND  s.status    = 'active'
           AND  s.created_at > $1
           AND  NOT EXISTS (
               SELECT 1
               FROM   covalence.edges e
               WHERE  e.source_node_id = s.id
                 AND  e.target_node_id = $2
           )",
    )
    .bind(article_modified_at)
    .bind(article_id)
    .fetch_all(pool)
    .await
    .context("consolidate_article: failed to query new sources")?;

    let has_new_sources = !new_source_ids.is_empty();
    let mut new_source_count = 0usize;

    if has_new_sources {
        tracing::info!(
            task_id    = %task.id,
            article_id = %article_id,
            pass,
            new_count  = new_source_ids.len(),
            "consolidate_article: new sources found, recompiling"
        );

        // ── 5. Collect all linked sources + new sources ─────────────────────
        let existing_source_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT e.source_node_id
             FROM   covalence.edges e
             JOIN   covalence.nodes n ON n.id = e.source_node_id
             WHERE  e.target_node_id = $1
               AND  e.edge_type IN ('ORIGINATES', 'COMPILED_FROM', 'CONFIRMS')
               AND  n.node_type = 'source'
               AND  n.status    = 'active'",
        )
        .bind(article_id)
        .fetch_all(pool)
        .await
        .context("consolidate_article: failed to fetch existing source IDs")?;

        let mut all_source_ids = existing_source_ids;
        all_source_ids.extend_from_slice(&new_source_ids);

        // ── 6. Fetch all source content ──────────────────────────────────────
        let source_rows =
            sqlx::query("SELECT id, title, content FROM covalence.nodes WHERE id = ANY($1)")
                .bind(&all_source_ids)
                .fetch_all(pool)
                .await
                .context("consolidate_article: failed to fetch source nodes")?;

        let sources: Vec<SourceDoc> = source_rows
            .iter()
            .map(|r| SourceDoc {
                id: r.get("id"),
                title: r.get::<Option<String>, _>("title").unwrap_or_default(),
                content: r.get::<Option<String>, _>("content").unwrap_or_default(),
            })
            .collect();

        // ── 7. Build LLM synthesis prompt (identical to reconsolidation) ─────
        // Source content is wrapped in XML tags so the LLM treats it as
        // structured data rather than instructions (Fix #84 — prompt injection
        // defence for RAG pipelines).
        let mut sources_block = String::new();
        for s in &sources {
            sources_block.push_str(&format!(
                "<source id=\"{}\">\n<title>{}</title>\n<content>\n{}\n</content>\n</source>\n\n",
                s.id, s.title, s.content
            ));
        }

        let prompt = format!(
            "You are a knowledge synthesizer. Read the following source documents and \
produce a well-structured article that synthesizes their information.\n\
\n\
Suggested title: {article_title}\n\
\n\
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

        // ── 8. LLM completion (with timeout — Fix #84) ──────────────────────
        // Wrap the LLM call in a 60-second timeout to prevent indefinite
        // blocking on slow or hung API endpoints.
        let t0 = Instant::now();
        let llm_result = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            llm.complete(&prompt, 4096),
        )
        .await
        .unwrap_or_else(|_elapsed| {
            tracing::warn!(
                task_id    = %task.id,
                article_id = %article_id,
                "consolidate_article: LLM completion timed out after 60s"
            );
            Err(anyhow::anyhow!("LLM completion timed out after 60s"))
        });
        let _llm_latency_ms = t0.elapsed().as_millis() as i32;

        let (new_title, new_content): (String, String) = match llm_result {
            Ok(raw) => match serde_json::from_str::<Value>(&strip_fences(&raw)) {
                Ok(v) => {
                    let title = v
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&article_title)
                        .to_string();
                    let content = v
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    (title, content)
                }
                Err(e) => {
                    tracing::warn!(
                        task_id    = %task.id,
                        article_id = %article_id,
                        "consolidate_article: JSON parse error ({e}), falling back to concatenation"
                    );
                    (article_title.clone(), concatenate_sources(&sources))
                }
            },
            Err(e) => {
                tracing::warn!(
                    task_id    = %task.id,
                    article_id = %article_id,
                    "consolidate_article: LLM error ({e}), falling back to concatenation"
                );
                (article_title.clone(), concatenate_sources(&sources))
            }
        };

        // ── 9. Update article in-place ───────────────────────────────────────
        sqlx::query(
            "UPDATE covalence.nodes
             SET    title       = $1,
                    content     = $2,
                    modified_at = now()
             WHERE  id = $3",
        )
        .bind(&new_title)
        .bind(&new_content)
        .bind(article_id)
        .execute(pool)
        .await
        .context("consolidate_article: failed to update article content")?;

        // ── 10. Create provenance edges for newly-linked sources ─────────────
        new_source_count = new_source_ids.len();
        for src_id in &new_source_ids {
            sqlx::query(
                "INSERT INTO covalence.edges \
                 (source_node_id, target_node_id, edge_type, weight, confidence, created_by) \
                 VALUES ($1, $2, 'ORIGINATES', 1.0, 1.0, 'consolidate_article') \
                 ON CONFLICT (source_node_id, target_node_id, edge_type) DO NOTHING",
            )
            .bind(src_id)
            .bind(article_id)
            .execute(pool)
            .await
            .context("consolidate_article: failed to insert provenance edge")?;
        }

        // ── 11. Queue re-embed for the updated article ───────────────────────
        super::enqueue_task(pool, "embed", Some(article_id), json!({}), 5).await?;
    } else {
        tracing::info!(
            task_id    = %task.id,
            article_id = %article_id,
            pass,
            "consolidate_article: no new sources, advancing pass without recompile"
        );
    }

    // ── 12. Increment consolidation_count and schedule next pass ────────────
    // consolidation_count is set to the pass number just completed.
    // The schedule never terminates: after pass 4 every subsequent pass uses
    // the monthly interval defined by SCHEDULE_PASS_N_DAYS.
    let new_count = pass;
    let delay = next_pass_delay(pass);
    let execute_after = Utc::now() + delay;
    let next_pass = pass + 1;

    // Update node: bump count, record when the next pass is due.
    sqlx::query(
        "UPDATE covalence.nodes
         SET consolidation_count   = $1,
             next_consolidation_at = $2
         WHERE id = $3",
    )
    .bind(new_count)
    .bind(execute_after)
    .bind(article_id)
    .execute(pool)
    .await
    .context("consolidate_article: failed to update consolidation state")?;

    // Insert a delayed task for the next pass.
    super::enqueue_task_at(
        pool,
        "consolidate_article",
        None,
        json!({
            "article_id": article_id.to_string(),
            "pass": next_pass,
        }),
        3,
        Some(execute_after),
    )
    .await?;

    tracing::info!(
        task_id       = %task.id,
        article_id    = %article_id,
        pass,
        next_pass,
        execute_after = %execute_after,
        delay_secs    = delay.num_seconds(),
        "consolidate_article: pass complete, next pass scheduled"
    );

    Ok(json!({
        "article_id":       article_id,
        "pass":             pass,
        "had_new_sources":  has_new_sources,
        "new_source_count": new_source_count,
        "new_count":        new_count,
        "next_pass":        next_pass,
        "execute_after":    execute_after.to_rfc3339(),
    }))
}

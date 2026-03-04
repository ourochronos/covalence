//! `reconsolidate` slow-path task handler (covalence#66).
//!
//! Triggered by retrieval-triggered reconsolidation when the search engine
//! discovers orphan source nodes that are related to a returned article but
//! not yet linked to it.  The handler re-compiles the article in-place,
//! incorporating the new sources into the synthesis.
//!
//! # Payload shape
//! ```json
//! {
//!   "article_id": "<uuid>",
//!   "source_ids": ["<uuid>", ...]
//! }
//! ```

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::QueueTask;
use super::llm::LlmClient;
use crate::graph::{GraphRepository as _, SqlGraphRepository};
use crate::models::{EdgeType, NodeType};

// ─── internal helpers ─────────────────────────────────────────────────────────

struct ReconSourceDoc {
    id: Uuid,
    title: String,
    content: String,
}

/// Concatenate source documents into a plain-text fallback article body.
fn concatenate_sources(sources: &[ReconSourceDoc]) -> String {
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

// ─── handler ──────────────────────────────────────────────────────────────────

/// Handle a `reconsolidate` task.
///
/// Steps:
/// 1. Parse `article_id` and new `source_ids` from the payload.
/// 2. Fetch the article's existing linked source IDs and combine with the
///    incoming orphan IDs (deduped).
/// 3. Fetch all combined source content.
/// 4. Run the LLM and update the article content in-place.
/// 5. Create provenance edges for the newly-linked orphan sources.
/// 6. Update `last_reconsolidated_at` so the 6-hour cooldown is enforced.
pub async fn handle_reconsolidate(
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
        .context("reconsolidate: missing or invalid article_id in payload")?;

    let new_source_ids: Vec<Uuid> = task
        .payload
        .get("source_ids")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().and_then(|s| Uuid::parse_str(s).ok()))
                .collect()
        })
        .unwrap_or_default();

    if new_source_ids.is_empty() {
        tracing::info!(
            task_id    = %task.id,
            article_id = %article_id,
            "reconsolidate: no new source_ids in payload, skipping"
        );
        return Ok(json!({"skipped": true, "reason": "no_new_sources"}));
    }

    // ── 2. Verify article exists and is active ──────────────────────────────
    let article_title: String = {
        let row = sqlx::query(
            "SELECT title FROM covalence.nodes \
             WHERE id = $1 AND status = 'active' AND node_type = 'article'",
        )
        .bind(article_id)
        .fetch_optional(pool)
        .await?;

        match row {
            Some(r) => r.get::<Option<String>, _>("title").unwrap_or_default(),
            None => {
                tracing::info!(
                    task_id    = %task.id,
                    article_id = %article_id,
                    "reconsolidate: article not found or inactive, skipping"
                );
                return Ok(json!({"skipped": true, "reason": "article_not_found"}));
            }
        }
    };

    // ── 3. Collect existing linked sources and identify truly-new orphans ───
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
    .context("reconsolidate: failed to fetch existing source IDs")?;

    let existing_set: HashSet<Uuid> = existing_source_ids.iter().cloned().collect();
    let truly_new: Vec<Uuid> = new_source_ids
        .into_iter()
        .filter(|id| !existing_set.contains(id))
        .collect();

    if truly_new.is_empty() {
        tracing::info!(
            task_id    = %task.id,
            article_id = %article_id,
            "reconsolidate: all source_ids already linked, skipping"
        );
        return Ok(json!({"skipped": true, "reason": "already_linked"}));
    }

    let mut all_source_ids = existing_source_ids.clone();
    all_source_ids.extend_from_slice(&truly_new);

    // ── 3b. Source cap (covalence#85) ────────────────────────────────────────
    // "Lost in the middle" degradation: faithfulness drops when relevant
    // content is buried in the centre of a long context window.  Capping at
    // MAX_COMPILATION_SOURCES keeps all source material near the context edges
    // where attention is strongest.  When more sources exist we keep the
    // MAX_COMPILATION_SOURCES with the highest reliability score so the most
    // trustworthy content is always included.
    const MAX_COMPILATION_SOURCES: usize = 7;
    let original_source_count = all_source_ids.len();
    if original_source_count > MAX_COMPILATION_SOURCES {
        let capped: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM covalence.nodes \
             WHERE  id = ANY($1) \
             ORDER BY COALESCE(reliability, 0.5) DESC \
             LIMIT  $2",
        )
        .bind(&all_source_ids)
        .bind(MAX_COMPILATION_SOURCES as i64)
        .fetch_all(pool)
        .await
        .context("reconsolidate: failed to score sources for cap")?;
        tracing::info!(
            task_id    = %task.id,
            article_id = %article_id,
            original   = original_source_count,
            capped_to  = capped.len(),
            "reconsolidate: source cap applied (covalence#85)"
        );
        all_source_ids = capped;
    }

    tracing::info!(
        task_id        = %task.id,
        article_id     = %article_id,
        existing_count = existing_source_ids.len(),
        new_count      = truly_new.len(),
        total_count    = all_source_ids.len(),
        "reconsolidate: starting"
    );

    // ── 4. Fetch all combined source content ────────────────────────────────
    let source_rows =
        sqlx::query("SELECT id, title, content FROM covalence.nodes WHERE id = ANY($1)")
            .bind(&all_source_ids)
            .fetch_all(pool)
            .await
            .context("reconsolidate: failed to fetch source nodes")?;

    if source_rows.is_empty() {
        anyhow::bail!(
            "reconsolidate: no source nodes found for {:?}",
            all_source_ids
        );
    }

    let sources: Vec<ReconSourceDoc> = source_rows
        .iter()
        .map(|r| ReconSourceDoc {
            id: r.get("id"),
            title: r.get::<Option<String>, _>("title").unwrap_or_default(),
            content: r.get::<Option<String>, _>("content").unwrap_or_default(),
        })
        .collect();

    // ── 5. Build the LLM compilation prompt ─────────────────────────────────
    // Source content is wrapped in XML tags so the LLM treats it as
    // structured data rather than instructions (Fix #84 — prompt injection
    // defence for RAG pipelines).
    //
    // Prompt caching (covalence#85): Anthropic's cache_control header
    // requires the API request to carry a structured system-message object
    // with {"type": "ephemeral"} on the system turn.  The current
    // LlmClient trait exposes only `complete(prompt: &str, max_tokens: u32)`
    // — a single undifferentiated string — with no facility for per-message
    // metadata or provider-specific headers.  Until the trait is extended to
    // support system/user message separation and a `cache_control` field,
    // prompt caching cannot be enabled here without coupling this module
    // directly to the Anthropic HTTP client (undesirable).  Track progress
    // in covalence#85 (next-2-weeks backlog item).
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
Think step by step when synthesizing these sources.\n\
\n\
SOURCE DOCUMENTS:\n\
{sources_block}"
    );

    // ── 6. LLM completion (with timeout — Fix #84) ──────────────────────────
    // Wrap the LLM call in a 120-second timeout to prevent indefinite
    // blocking on slow or hung API endpoints.
    let t0 = Instant::now();
    let llm_result = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        llm.complete(&prompt, 4096),
    )
    .await
    .unwrap_or_else(|_elapsed| {
        tracing::warn!(
            task_id    = %task.id,
            article_id = %article_id,
            "reconsolidate: LLM completion timed out after 120s"
        );
        Err(anyhow::anyhow!("LLM completion timed out after 120s"))
    });
    let _llm_latency_ms = t0.elapsed().as_millis() as i32;

    let (new_title, new_content, source_relationships): (String, String, Vec<(Uuid, String)>) =
        match llm_result {
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
                    let rels: Vec<(Uuid, String)> = v
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
                    (title, content, rels)
                }
                Err(e) => {
                    tracing::warn!(
                        task_id    = %task.id,
                        article_id = %article_id,
                        "reconsolidate: JSON parse error ({e}), concatenating sources"
                    );
                    (article_title.clone(), concatenate_sources(&sources), vec![])
                }
            },
            Err(e) => {
                tracing::warn!(
                    task_id    = %task.id,
                    article_id = %article_id,
                    "reconsolidate: LLM error ({e}), concatenating sources"
                );
                (article_title.clone(), concatenate_sources(&sources), vec![])
            }
        };

    // ── 7 + 9. Update article content and mutation log (transactional) ────────
    // Both writes are wrapped in a single transaction so that a mid-operation
    // cancellation cannot leave the article partially updated.
    {
        let mut tx = pool
            .begin()
            .await
            .context("reconsolidate: failed to begin transaction")?;

        sqlx::query(
            "UPDATE covalence.nodes
             SET    title                  = $1,
                    content                = $2,
                    modified_at            = now(),
                    last_reconsolidated_at = now()
             WHERE  id = $3",
        )
        .bind(&new_title)
        .bind(&new_content)
        .bind(article_id)
        .execute(&mut *tx)
        .await
        .context("reconsolidate: failed to update article")?;

        let mutation_entry = json!([{
            "type": "reconsolidated",
            "summary": format!(
                "Reconsolidated with {} new source(s): {}",
                truly_new.len(),
                truly_new
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
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
        .bind(article_id)
        .execute(&mut *tx)
        .await
        .context("reconsolidate: failed to record mutation log")?;

        tx.commit()
            .await
            .context("reconsolidate: failed to commit transaction")?;
    }

    // ── 8. Create provenance edges for newly-linked orphan sources ───────────
    // Outside the transaction — non-fatal (failures are logged as warnings).
    let graph = SqlGraphRepository::new(pool.clone());

    // Ensure an AGE vertex exists for the article (idempotent, non-fatal).
    if let Err(e) = graph
        .create_vertex(article_id, NodeType::Article, json!({}))
        .await
    {
        tracing::warn!(
            article_id = %article_id,
            "reconsolidate: create_vertex non-fatal: {e}"
        );
    }

    let rel_map: std::collections::HashMap<Uuid, String> =
        source_relationships.into_iter().collect();

    for src_id in &truly_new {
        let rel = rel_map
            .get(src_id)
            .map(|s| s.as_str())
            .unwrap_or("originates");
        let edge_type = match rel {
            "confirms" => EdgeType::Confirms,
            "supersedes" => EdgeType::Supersedes,
            "contradicts" => EdgeType::Contradicts,
            "contends" => EdgeType::Contends,
            _ => EdgeType::Originates,
        };
        if let Err(e) = graph
            .create_edge(
                *src_id,
                article_id,
                edge_type,
                1.0,
                "reconsolidate",
                json!({}),
            )
            .await
        {
            tracing::warn!(
                src_id     = %src_id,
                article_id = %article_id,
                "reconsolidate: create_edge non-fatal: {e}"
            );
        }
    }

    // ── 10. Queue embed for the updated article ─────────────────────────────
    super::enqueue_task(pool, "embed", Some(article_id), json!({}), 5).await?;

    tracing::info!(
        task_id       = %task.id,
        article_id    = %article_id,
        new_title     = %new_title,
        new_sources   = truly_new.len(),
        total_sources = all_source_ids.len(),
        "reconsolidate: done"
    );

    Ok(json!({
        "article_id":     article_id,
        "new_title":      new_title,
        "new_source_ids": truly_new.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        "total_sources":  all_source_ids.len(),
        "content_len":    new_content.len(),
    }))
}

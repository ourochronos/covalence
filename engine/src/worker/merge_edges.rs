//! Implementations for the `merge` and `infer_edges` slow-path task handlers.
//!
//! Both functions are `pub` so that `mod.rs` can call them directly:
//!
//! ```rust,ignore
//! "merge"       => merge_edges::handle_merge(pool, llm, task).await,
//! "infer_edges" => merge_edges::handle_infer_edges(pool, llm, task).await,
//! ```

use std::sync::Arc;

use anyhow::Context;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::QueueTask;
use super::llm::LlmClient;

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Strip markdown code fences from an LLM response and parse it as JSON.
fn parse_json_response(text: &str) -> anyhow::Result<Value> {
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

/// Queue a follow-up task for a node at default priority.
async fn queue_task(pool: &PgPool, task_type: &str, node_id: Uuid) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT INTO covalence.slow_path_queue
               (task_type, node_id, payload, priority, status)
           VALUES ($1, $2, '{}'::jsonb, 5, 'pending')"#,
    )
    .bind(task_type)
    .bind(node_id)
    .execute(pool)
    .await
    .with_context(|| format!("failed to queue {task_type} task for {node_id}"))?;
    Ok(())
}

// ─── handle_merge ─────────────────────────────────────────────────────────────

/// `merge`: Merge two article nodes into a new synthesised article.
///
/// Payload keys
/// - `article_id_a` (UUID string) — first article
/// - `article_id_b` (UUID string) — second article
pub async fn handle_merge(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    // ── 1. Parse article IDs ──────────────────────────────────────────────────
    let id_a: Uuid = task
        .payload
        .get("article_id_a")
        .and_then(|v| v.as_str())
        .context("merge task payload missing article_id_a")?
        .parse()
        .context("article_id_a is not a valid UUID")?;

    let id_b: Uuid = task
        .payload
        .get("article_id_b")
        .and_then(|v| v.as_str())
        .context("merge task payload missing article_id_b")?
        .parse()
        .context("article_id_b is not a valid UUID")?;

    tracing::info!(
        task_id      = %task.id,
        article_id_a = %id_a,
        article_id_b = %id_b,
        "merge: starting"
    );

    // ── 2. Fetch both articles ────────────────────────────────────────────────
    use sqlx::Row as _;

    let row_a = sqlx::query("SELECT title, content FROM covalence.nodes WHERE id = $1")
        .bind(id_a)
        .fetch_optional(pool)
        .await?
        .with_context(|| format!("article not found: {id_a}"))?;

    let row_b = sqlx::query("SELECT title, content FROM covalence.nodes WHERE id = $1")
        .bind(id_b)
        .fetch_optional(pool)
        .await?
        .with_context(|| format!("article not found: {id_b}"))?;

    let title_a: String = row_a.get::<Option<String>, _>("title").unwrap_or_default();
    let content_a: String = row_a
        .get::<Option<String>, _>("content")
        .unwrap_or_default();
    let title_b: String = row_b.get::<Option<String>, _>("title").unwrap_or_default();
    let content_b: String = row_b
        .get::<Option<String>, _>("content")
        .unwrap_or_default();

    // ── 3. Ask LLM to produce a merged article ────────────────────────────────
    let prompt = format!(
        "You are a knowledge synthesis engine. Merge the two articles below into a single, \
         coherent article.\n\n\
         Return ONLY valid JSON (no markdown fences) in this exact shape:\n\
         {{\"title\": \"...\", \"content\": \"...\", \"reasoning\": \"...\"}}\n\n\
         Rules:\n\
         - Target ~2000 tokens in the content field; keep it between 200 and 4000 tokens.\n\
         - Preserve all distinct facts from both articles.\n\
         - Eliminate duplicate information.\n\
         - Write clear, encyclopaedic prose.\n\n\
         --- ARTICLE A (id: {id_a}) ---\n\
         Title: {title_a}\n\
         {content_a}\n\n\
         --- ARTICLE B (id: {id_b}) ---\n\
         Title: {title_b}\n\
         {content_b}\n"
    );

    let (merged_title, merged_content, degraded) = match llm.complete(&prompt, 4096).await {
        Ok(resp) => match parse_json_response(&resp) {
            Ok(v) => {
                let title = v
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("Merged Article")
                    .to_string();
                let content = v
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                (title, content, false)
            }
            Err(e) => {
                tracing::warn!(
                    task_id = %task.id,
                    error   = %e,
                    "merge: LLM JSON parse failed — using concatenation fallback"
                );
                (
                    format!("{title_a} / {title_b}"),
                    format!("{content_a}\n\n---\n\n{content_b}"),
                    true,
                )
            }
        },
        Err(e) => {
            tracing::warn!(
                task_id = %task.id,
                error   = %e,
                "merge: LLM call failed — using concatenation fallback"
            );
            (
                format!("{title_a} / {title_b}"),
                format!("{content_a}\n\n---\n\n{content_b}"),
                true,
            )
        }
    };

    // ── 4. Create new merged article node ────────────────────────────────────
    let new_id = Uuid::new_v4();
    let new_metadata = json!({
        "merged_from": [id_a, id_b],
        "degraded":    degraded,
    });

    sqlx::query(
        r#"INSERT INTO covalence.nodes
               (id, node_type, status, title, content, metadata)
           VALUES ($1, 'article', 'active', $2, $3, $4)"#,
    )
    .bind(new_id)
    .bind(&merged_title)
    .bind(&merged_content)
    .bind(&new_metadata)
    .execute(pool)
    .await
    .context("failed to insert merged article node")?;

    tracing::info!(
        task_id  = %task.id,
        new_id   = %new_id,
        degraded,
        "merge: new article node created"
    );

    // ── 5. Archive both originals ─────────────────────────────────────────────
    for id in [id_a, id_b] {
        sqlx::query(
            "UPDATE covalence.nodes SET status = 'archived', modified_at = now() WHERE id = $1",
        )
        .bind(id)
        .execute(pool)
        .await
        .with_context(|| format!("failed to archive article {id}"))?;
    }

    // ── 6. MERGED_FROM edges: new_article → each original ────────────────────
    for original_id in [id_a, id_b] {
        sqlx::query(
            r#"INSERT INTO covalence.edges
                   (id, source_node_id, target_node_id, edge_type, weight, metadata)
               VALUES ($1, $2, $3, 'MERGED_FROM', 1.0, '{"inferred":false}'::jsonb)"#,
        )
        .bind(Uuid::new_v4())
        .bind(new_id)
        .bind(original_id)
        .execute(pool)
        .await
        .with_context(|| format!("failed to insert MERGED_FROM edge to {original_id}"))?;
    }

    // ── 7. Union provenance — copy inbound edges from both originals to new node ──
    // ON CONFLICT (node_id, source_id) absorbs duplicates that appear in both.
    sqlx::query(
        r#"INSERT INTO covalence.edges
               (id, source_node_id, target_node_id, edge_type, weight, created_by)
           SELECT gen_random_uuid(), source_node_id, $1, edge_type, weight, 'merge_inherit'
           FROM   covalence.edges
           WHERE  target_node_id IN ($2, $3)
             AND  edge_type IN ('ORIGINATES','COMPILED_FROM','CONFIRMS','SUPERSEDES','CONTRADICTS','CONTENDS')"#,
    )
    .bind(new_id)
    .bind(id_a)
    .bind(id_b)
    .execute(pool)
    .await
    .context("failed to copy provenance edges to merged node")?;

    // ── 8. Record mutations ───────────────────────────────────────────────────
    let _now_str = chrono::Utc::now().to_rfc3339();
    let mutations: &[(Uuid, &str, String)] = &[
        (id_a, "merged", format!("Archived: merged into {new_id}")),
        (id_b, "merged", format!("Archived: merged into {new_id}")),
        (
            new_id,
            "created",
            format!("Created by merging {id_a} and {id_b}"),
        ),
    ];
    for (article_id, mutation_type, summary) in mutations {
        let _entry = serde_json::json!([{"type": mutation_type, "summary": summary, "recorded_at": &_now_str}]);
        sqlx::query(
            r#"UPDATE covalence.nodes
                  SET metadata = jsonb_set(coalesce(metadata, '{}'::jsonb), '{mutation_log}',
                                     coalesce(metadata->'mutation_log', '[]'::jsonb) || $1::jsonb, true),
                      modified_at = now()
                WHERE id = $2"#,
        )
        .bind(&_entry)
        .bind(article_id)
        .execute(pool)
        .await
        .with_context(|| format!("failed to record mutation '{mutation_type}' for {article_id}"))?;
    }

    // ── 9. Queue embed task for new article ───────────────────────────────────
    queue_task(pool, "embed", new_id).await?;

    // ── 10. Return result ─────────────────────────────────────────────────────
    Ok(json!({
        "new_article_id": new_id,
        "merged_from":    [id_a, id_b],
        "title":          merged_title,
        "content_len":    merged_content.len(),
        "degraded":       degraded,
    }))
}

// ─── handle_infer_edges ───────────────────────────────────────────────────────

/// `infer_edges`: Use vector similarity + LLM classification to infer semantic
/// edges between the task's node and its nearest neighbours.
///
/// Edge types (spec §9.1):
/// `EXTENDS | CONTRADICTS | CONFIRMS | SUPERSEDES | RELATES_TO | DERIVED_FROM | CONCURRENT_WITH`
pub async fn handle_infer_edges(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    // ── 1. Resolve node ───────────────────────────────────────────────────────
    let node_id = task.node_id.context("infer_edges task requires node_id")?;

    tracing::info!(task_id = %task.id, node_id = %node_id, "infer_edges: starting");

    use sqlx::Row as _;

    // ── 2. Fetch node content ─────────────────────────────────────────────────
    let node_row = sqlx::query(
        "SELECT title, content FROM covalence.nodes WHERE id = $1 AND status = 'active'",
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await?
    .with_context(|| format!("active node not found: {node_id}"))?;

    let node_title: String = node_row
        .get::<Option<String>, _>("title")
        .unwrap_or_default();
    let node_content: String = node_row
        .get::<Option<String>, _>("content")
        .unwrap_or_default();

    // ── 3. Check that an embedding exists ────────────────────────────────────
    let emb_exists =
        sqlx::query("SELECT 1 FROM covalence.node_embeddings WHERE node_id = $1 LIMIT 1")
            .bind(node_id)
            .fetch_optional(pool)
            .await?;

    if emb_exists.is_none() {
        tracing::warn!(
            node_id = %node_id,
            "infer_edges: no embedding found — queuing embed then re-queuing self"
        );
        queue_task(pool, "embed", node_id).await?;
        // Re-queue infer_edges at lower priority so embed runs first.
        sqlx::query(
            r#"INSERT INTO covalence.slow_path_queue
                   (task_type, node_id, payload, priority, status)
               VALUES ('infer_edges', $1, '{}'::jsonb, 3, 'pending')"#,
        )
        .bind(node_id)
        .execute(pool)
        .await
        .context("failed to re-queue infer_edges")?;

        return Ok(json!({
            "node_id": node_id,
            "status":  "deferred — embedding not yet available",
        }));
    }

    // ── 4. Vector search: top-10 neighbours with cosine distance < 0.3 ───────
    let candidates = sqlx::query(
        r#"SELECT
               ne.node_id                                   AS candidate_id,
               n.title                                      AS candidate_title,
               n.content                                    AS candidate_content,
               (ne.embedding <=> src.embedding)::float4     AS cosine_distance
           FROM   covalence.node_embeddings AS ne
           JOIN   covalence.nodes           AS n
                  ON n.id = ne.node_id AND n.status = 'active'
           CROSS JOIN LATERAL (
               SELECT embedding
               FROM   covalence.node_embeddings
               WHERE  node_id = $1
           ) AS src
           WHERE  ne.node_id != $1
             AND  (ne.embedding <=> src.embedding) < 0.3
           ORDER  BY cosine_distance ASC
           LIMIT  10"#,
    )
    .bind(node_id)
    .fetch_all(pool)
    .await
    .context("failed to query nearest-neighbour candidates")?;

    tracing::debug!(
        node_id    = %node_id,
        candidates = candidates.len(),
        "infer_edges: candidates retrieved"
    );

    let mut edges_created: u32 = 0;
    let mut edges_skipped: u32 = 0;

    // ── 5. Classify each candidate ────────────────────────────────────────────
    for row in &candidates {
        let candidate_id: Uuid = row.get("candidate_id");
        let candidate_title: String = row
            .get::<Option<String>, _>("candidate_title")
            .unwrap_or_default();
        let candidate_content: String = row
            .get::<Option<String>, _>("candidate_content")
            .unwrap_or_default();
        let cosine_distance: f32 = row.get("cosine_distance");

        // Truncate snippets so the prompt stays within the context window.
        let src_snippet = if node_content.len() > 1200 {
            &node_content[..1200]
        } else {
            &node_content
        };
        let cand_snippet = if candidate_content.len() > 1200 {
            &candidate_content[..1200]
        } else {
            &candidate_content
        };

        let prompt = format!(
            "You are a knowledge-graph edge classifier. Analyse the relationship between the \
             two knowledge items below.\n\n\
             Return ONLY valid JSON (no markdown, no extra text):\n\
             {{\"relationship\": \"<TYPE>\", \"confidence\": <0.0-1.0>, \"reasoning\": \"<one sentence>\"}}\n\n\
             Valid relationship types (choose exactly one):\n\
               EXTENDS         — Item B extends or elaborates on Item A\n\
               CONTRADICTS     — The items make incompatible claims\n\
               CONFIRMS        — Item B independently validates Item A\n\
               SUPERSEDES      — Item B replaces Item A (newer / more complete)\n\
               RELATES_TO      — Topically related but no clear direction\n\
               DERIVED_FROM    — Item B was derived from Item A\n\
               CONCURRENT_WITH — The items reference overlapping time periods\n\n\
             --- ITEM A (id: {node_id}) ---\n\
             Title: {node_title}\n\
             {src_snippet}\n\n\
             --- ITEM B (id: {candidate_id}) ---\n\
             Title: {candidate_title}\n\
             {cand_snippet}\n"
        );

        let (relationship, confidence, reasoning) = match llm.complete(&prompt, 512).await {
            Ok(resp) => match parse_json_response(&resp) {
                Ok(v) => {
                    let rel = v
                        .get("relationship")
                        .and_then(|r| r.as_str())
                        .unwrap_or("RELATES_TO")
                        .to_uppercase();
                    // Normalise aliases to valid edge_type constraint values
                    let rel = match rel.as_str() {
                        "DERIVED_FROM" => "DERIVES_FROM".to_string(),
                        _ => rel,
                    };
                    let conf = v.get("confidence").and_then(|c| c.as_f64()).unwrap_or(0.0) as f32;
                    let reason = v
                        .get("reasoning")
                        .and_then(|r| r.as_str())
                        .unwrap_or("")
                        .to_string();
                    (rel, conf, reason)
                }
                Err(e) => {
                    tracing::warn!(
                        node_id      = %node_id,
                        candidate_id = %candidate_id,
                        error        = %e,
                        "infer_edges: JSON parse failed — skipping candidate"
                    );
                    edges_skipped += 1;
                    continue;
                }
            },
            Err(e) => {
                tracing::warn!(
                    node_id      = %node_id,
                    candidate_id = %candidate_id,
                    error        = %e,
                    "infer_edges: LLM call failed — skipping candidate"
                );
                edges_skipped += 1;
                continue;
            }
        };

        // Confidence gate
        if confidence < 0.5 {
            tracing::debug!(
                node_id      = %node_id,
                candidate_id = %candidate_id,
                confidence,
                "infer_edges: confidence below 0.5 threshold — skipping"
            );
            edges_skipped += 1;
            continue;
        }

        // ── 6. Skip if edge already exists ────────────────────────────────────
        let existing = sqlx::query(
            "SELECT id FROM covalence.edges WHERE source_node_id = $1 AND target_node_id = $2 LIMIT 1",
        )
        .bind(node_id)
        .bind(candidate_id)
        .fetch_optional(pool)
        .await
        .context("failed to check for existing edge")?;

        if existing.is_some() {
            tracing::debug!(
                node_id      = %node_id,
                candidate_id = %candidate_id,
                "infer_edges: edge already exists — skipping"
            );
            edges_skipped += 1;
            continue;
        }

        // ── 7. Upsert inferred edge ───────────────────────────────────────────
        let edge_metadata = json!({
            "inferred":        true,
            "reasoning":       reasoning,
            "cosine_distance": cosine_distance,
        });

        sqlx::query(
            r#"INSERT INTO covalence.edges
                   (id, source_node_id, target_node_id, edge_type, weight, metadata)
               VALUES ($1, $2, $3, $4, $5, $6)"#,
        )
        .bind(Uuid::new_v4())
        .bind(node_id)
        .bind(candidate_id)
        .bind(&relationship)
        .bind(confidence as f64)
        .bind(&edge_metadata)
        .execute(pool)
        .await
        .with_context(|| {
            format!("failed to upsert edge {node_id} -{relationship}-> {candidate_id}")
        })?;

        tracing::info!(
            node_id      = %node_id,
            candidate_id = %candidate_id,
            relationship = %relationship,
            confidence,
            "infer_edges: edge upserted"
        );
        edges_created += 1;
    }

    Ok(json!({
        "node_id":          node_id,
        "candidates_found": candidates.len(),
        "edges_created":    edges_created,
        "edges_skipped":    edges_skipped,
    }))
}

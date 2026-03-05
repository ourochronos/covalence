//! Implementations for the `merge` and `infer_edges` slow-path task handlers.
//!
//! Both functions are `pub` so that `mod.rs` can call them directly:
//!
//! ```rust,ignore
//! "merge"       => merge_edges::handle_merge(pool, llm, task).await,
//! "infer_edges" => merge_edges::handle_infer_edges(pool, llm, task).await,
//! ```

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

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Truncate `s` to at most `max_bytes` bytes, walking back to a valid UTF-8
/// char boundary when necessary.
///
/// This avoids the classic `byte index N is not a char boundary` panic that
/// occurs when a multi-byte character (e.g. em-dash U+2014, 3 bytes) straddles
/// a hard byte limit.  The returned slice is always a valid sub-slice of `s`.
fn safe_truncate(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
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
#[allow(deprecated)]
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

    // ── 0. Idempotency guard ─────────────────────────────────────────────────
    if super::already_completed(pool, task).await? {
        tracing::info!(task_id = %task.id, "merge: idempotency guard — already complete, skipping");
        return Ok(json!({"skipped": true, "reason": "already_complete"}));
    }

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

    let chat_model = std::env::var("COVALENCE_CHAT_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
    let t0 = Instant::now();
    let (merged_title, merged_content, degraded) = match llm.complete(&prompt, 4096).await {
        Ok(resp) => match super::parse_llm_json(&resp) {
            Some(v) => {
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
            None => {
                tracing::warn!(
                    task_id = %task.id,
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

    let llm_latency_ms = t0.elapsed().as_millis() as i32;

    // Log inference for merge LLM call
    if !degraded {
        let input_nodes = [id_a, id_b];
        let input_summary = format!(
            "article_id_a={id_a}, article_id_b={id_b}, title_a={title_a:?}, title_b={title_b:?}"
        );
        let output_decision = format!("merged_title={merged_title}");
        if let Err(e) = super::log_inference(
            pool,
            "merge",
            &input_nodes,
            &input_summary,
            &output_decision,
            None,
            "",
            &chat_model,
            llm_latency_ms,
        )
        .await
        {
            tracing::warn!(task_id = %task.id, "merge: log_inference failed: {e:#}");
        }
    }

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

    // ── 5. Create AGE vertex for the new merged article ───────────────────────
    let graph = SqlGraphRepository::new(pool.clone());
    if let Err(e) = graph
        .create_vertex(new_id, NodeType::Article, serde_json::json!({}))
        .await
    {
        tracing::warn!(
            new_id = %new_id,
            "merge: failed to create AGE vertex for merged article (non-fatal): {e}"
        );
    }

    // ── 6. Archive both originals + remove from AGE live graph ───────────────
    for id in [id_a, id_b] {
        sqlx::query(
            "UPDATE covalence.nodes SET status = 'archived', modified_at = now() WHERE id = $1",
        )
        .bind(id)
        .execute(pool)
        .await
        .with_context(|| format!("failed to archive article {id}"))?;

        // Remove archived vertex from AGE (SQL edges are preserved as history).
        if let Err(e) = graph.archive_vertex(id).await {
            tracing::warn!(
                id = %id,
                "merge: archive_vertex failed for original article (non-fatal): {e}"
            );
        }
    }

    // ── 7. MERGED_FROM edges via GraphRepository ──────────────────────────────
    for original_id in [id_a, id_b] {
        if let Err(e) = graph
            .create_edge(
                new_id,
                original_id,
                EdgeType::MergedFrom,
                1.0,
                "merge",
                serde_json::json!({"inferred": false}),
            )
            .await
        {
            tracing::warn!(
                new_id      = %new_id,
                original_id = %original_id,
                "merge: failed to create MERGED_FROM edge via GraphRepository: {e}"
            );
        }
    }

    // ── 8. Union provenance — copy inbound edges from both originals ──────────
    // Fetch the candidate rows from SQL, then write each via GraphRepository
    // so both AGE and SQL are kept in sync.
    let prov_rows = sqlx::query(&format!(
        "SELECT DISTINCT source_node_id, edge_type, weight \
             FROM   covalence.edges \
             WHERE  target_node_id IN ($1, $2) \
               AND  edge_type IN ({})",
        EdgeType::provenance_sql_labels()
    ))
    .bind(id_a)
    .bind(id_b)
    .fetch_all(pool)
    .await
    .context("merge: failed to fetch provenance edges for union")?;

    for prow in &prov_rows {
        let src_id: Uuid = prow.get("source_node_id");
        let edge_type_str: String = prow.get("edge_type");
        let weight: f64 = prow.try_get("weight").unwrap_or(1.0);

        let edge_type: EdgeType = match edge_type_str.parse() {
            Ok(et) => et,
            Err(e) => {
                tracing::warn!(
                    edge_type = %edge_type_str,
                    "merge: unknown provenance edge_type, skipping: {e}"
                );
                continue;
            }
        };

        if let Err(e) = graph
            .create_edge(
                src_id,
                new_id,
                edge_type,
                weight as f32,
                "merge_inherit",
                serde_json::json!({}),
            )
            .await
        {
            tracing::warn!(
                src_id     = %src_id,
                new_id     = %new_id,
                edge_type  = %edge_type_str,
                "merge: failed to copy provenance edge via GraphRepository: {e}"
            );
        }
    }

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

    // ── 9. Queue embed + tree_embed tasks for new article (Item 8) ──────────
    queue_task(pool, "embed", new_id).await?;
    queue_task(pool, "tree_embed", new_id).await?;

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

    // ── 0. Idempotency guard ─────────────────────────────────────────────────
    if super::already_completed(pool, task).await? {
        tracing::info!(task_id = %task.id, "infer_edges: idempotency guard — already complete, skipping");
        return Ok(json!({"skipped": true, "reason": "already_complete"}));
    }

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

    // ── 2b. Keyword / tag extraction (idempotent, non-fatal) ─────────────────
    {
        let kw_exists: bool = sqlx::query_scalar(
            "SELECT (metadata->'keywords') IS NOT NULL \
             FROM covalence.nodes WHERE id = $1",
        )
        .bind(node_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(false);

        if !kw_exists {
            let snippet = {
                let combined = format!("{node_title}\n\n{node_content}");
                safe_truncate(&combined, 800).to_string()
            };
            let kw_prompt = format!(
                "You are a knowledge indexing assistant. Extract keywords and topic tags \
                 from the text below.\n\n\
                 Return ONLY valid JSON (no markdown, no extra text):\n\
                 {{\"keywords\": [\"k1\",\"k2\",...], \"tags\": [\"t1\",\"t2\"]}}\n\n\
                 Rules:\n\
                 - keywords: 5–8 specific terms or phrases that best represent the content\n\
                 - tags: 2–3 broad topic categories\n\n\
                 TEXT:\n{snippet}"
            );

            match llm.complete(&kw_prompt, 256).await {
                Ok(resp) => match super::parse_llm_json(&resp) {
                    Some(kw_val) => {
                        let has_keywords = kw_val.get("keywords").is_some();
                        let has_tags = kw_val.get("tags").is_some();
                        if has_keywords || has_tags {
                            let patch = json!({
                                "keywords": kw_val.get("keywords").cloned().unwrap_or(json!([])),
                                "tags": kw_val.get("tags").cloned().unwrap_or(json!([])),
                            });
                            if let Err(e) = sqlx::query(
                                "UPDATE covalence.nodes \
                                 SET metadata = metadata || $1 \
                                 WHERE id = $2",
                            )
                            .bind(&patch)
                            .bind(node_id)
                            .execute(pool)
                            .await
                            {
                                tracing::warn!(
                                    node_id = %node_id,
                                    error   = %e,
                                    "infer_edges: failed to store keywords in metadata"
                                );
                            } else {
                                tracing::debug!(
                                    node_id = %node_id,
                                    "infer_edges: stored keywords/tags in node metadata"
                                );
                            }
                        }
                    }
                    None => {
                        tracing::warn!(
                            node_id = %node_id,
                            "infer_edges: keyword extraction JSON parse failed — continuing"
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        node_id = %node_id,
                        error   = %e,
                        "infer_edges: keyword extraction LLM call failed — continuing"
                    );
                }
            }
        }
    }

    // ── 2c. Auto-facet annotation from keywords (covalence#103 Phase 2) ──────
    // If the node does not yet have facet_function or facet_scope set, try to
    // infer them from the extracted keywords (stored in metadata["keywords"]).
    // Uses a lightweight PMEST-inspired lookup table (opt-in, non-destructive).
    {
        let facet_row: Option<(Option<Vec<String>>, Option<Vec<String>>, serde_json::Value)> =
            sqlx::query_as(
                "SELECT facet_function, facet_scope, COALESCE(metadata, '{}'::jsonb) \
                 FROM covalence.nodes WHERE id = $1",
            )
            .bind(node_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

        if let Some((existing_ff, existing_fs, metadata)) = facet_row {
            // Only auto-annotate if at least one facet dimension is not already set.
            let needs_ff = existing_ff.is_none();
            let needs_fs = existing_fs.is_none();

            if needs_ff || needs_fs {
                // Extract keywords from metadata["keywords"] array.
                let keywords: Vec<String> = metadata
                    .get("keywords")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .map(|s| s.to_lowercase())
                            .collect()
                    })
                    .unwrap_or_default();

                if !keywords.is_empty() {
                    // PMEST-inspired lookup table: keyword → facet dimension.
                    // facet_function: what does this knowledge DO?
                    const FF_TERMS: &[&str] = &[
                        "process",
                        "method",
                        "algorithm",
                        "function",
                        "operation",
                        "retrieval",
                        "storage",
                        "evaluation",
                        "analysis",
                        "inference",
                        "procedure",
                        "workflow",
                        "design",
                        "generation",
                    ];
                    // facet_scope: at what abstraction level?
                    const FS_TERMS: &[&str] = &[
                        "location",
                        "region",
                        "domain",
                        "scope",
                        "theoretical",
                        "practical",
                        "operational",
                        "historical",
                        "context",
                        "environment",
                        "field",
                        "framework",
                        "architecture",
                    ];

                    let inferred_ff: Vec<String> = FF_TERMS
                        .iter()
                        .filter(|term| keywords.iter().any(|kw| kw.contains(*term)))
                        .map(|s| s.to_string())
                        .collect();

                    let inferred_fs: Vec<String> = FS_TERMS
                        .iter()
                        .filter(|term| keywords.iter().any(|kw| kw.contains(*term)))
                        .map(|s| s.to_string())
                        .collect();

                    let new_ff: Option<Vec<String>> = if needs_ff && !inferred_ff.is_empty() {
                        Some(inferred_ff)
                    } else {
                        None
                    };
                    let new_fs: Option<Vec<String>> = if needs_fs && !inferred_fs.is_empty() {
                        Some(inferred_fs)
                    } else {
                        None
                    };

                    if new_ff.is_some() || new_fs.is_some() {
                        // Only set columns that were inferred (don't overwrite existing).
                        let update_result = sqlx::query(
                            "UPDATE covalence.nodes \
                             SET facet_function = CASE WHEN facet_function IS NULL THEN $2 ELSE facet_function END, \
                                 facet_scope    = CASE WHEN facet_scope    IS NULL THEN $3 ELSE facet_scope    END \
                             WHERE id = $1",
                        )
                        .bind(node_id)
                        .bind(&new_ff)
                        .bind(&new_fs)
                        .execute(pool)
                        .await;

                        match update_result {
                            Ok(_) => tracing::debug!(
                                node_id = %node_id,
                                ff      = ?new_ff,
                                fs      = ?new_fs,
                                "infer_edges: auto-facet annotation applied"
                            ),
                            Err(e) => tracing::warn!(
                                node_id = %node_id,
                                error   = %e,
                                "infer_edges: auto-facet annotation failed (non-fatal)"
                            ),
                        }
                    }
                }
            }
        }
    }

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
        // safe_truncate walks back to a char boundary, preventing panics on
        // multi-byte characters (e.g. em-dash U+2014) at the cut point.
        let src_snippet = safe_truncate(&node_content, 1200);
        let cand_snippet = safe_truncate(&candidate_content, 1200);

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

        let chat_model =
            std::env::var("COVALENCE_CHAT_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
        let t0 = Instant::now();
        let (relationship, confidence, reasoning) = match llm.complete(&prompt, 512).await {
            Ok(resp) => match super::parse_llm_json(&resp) {
                Some(v) => {
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
                None => {
                    tracing::warn!(
                        node_id      = %node_id,
                        candidate_id = %candidate_id,
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

        let llm_latency_ms = t0.elapsed().as_millis() as i32;

        // Log inference for this candidate
        {
            let input_nodes = [node_id, candidate_id];
            let input_summary = format!(
                "node_id={node_id}, candidate_id={candidate_id}, cosine_distance={cosine_distance:.4}"
            );
            let output_decision =
                format!("relationship={relationship}, confidence={confidence:.3}");
            if let Err(e) = super::log_inference(
                pool,
                "infer_edge",
                &input_nodes,
                &input_summary,
                &output_decision,
                Some(confidence as f64),
                &reasoning,
                &chat_model,
                llm_latency_ms,
            )
            .await
            {
                tracing::warn!(
                    node_id = %node_id,
                    candidate_id = %candidate_id,
                    "infer_edges: log_inference failed: {e:#}"
                );
            }
        }

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

        // ── 7. Create inferred edge via GraphRepository (dual-writes AGE + SQL) ──
        let edge_metadata = json!({
            "inferred":        true,
            "reasoning":       reasoning,
            "cosine_distance": cosine_distance,
        });

        let infer_edge_type: EdgeType = match relationship.parse() {
            Ok(et) => et,
            Err(e) => {
                tracing::warn!(
                    node_id      = %node_id,
                    candidate_id = %candidate_id,
                    relationship = %relationship,
                    "infer_edges: unknown relationship type, skipping: {e}"
                );
                edges_skipped += 1;
                continue;
            }
        };

        let graph = SqlGraphRepository::new(pool.clone());
        if let Err(e) = graph
            .create_edge(
                node_id,
                candidate_id,
                infer_edge_type,
                confidence,
                "infer_edges",
                edge_metadata,
            )
            .await
        {
            tracing::warn!(
                node_id      = %node_id,
                candidate_id = %candidate_id,
                relationship = %relationship,
                "infer_edges: failed to create edge via GraphRepository: {e}"
            );
            edges_skipped += 1;
            continue;
        }

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

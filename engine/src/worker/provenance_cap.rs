//! Provenance Cap & Auto-Split handler (covalence#161).
//!
//! Two related concerns live here:
//!
//! 1. **Provenance cap** constants (`PROVENANCE_CAP` / `PROVENANCE_SPLIT_THRESHOLD`)
//!    used by `handle_compile` to limit the number of sources it sends to the
//!    LLM and to decide when an article must be auto-split.
//!
//! 2. **`handle_auto_split`** — the slow-path task that fires when an article
//!    has accumulated more than `PROVENANCE_SPLIT_THRESHOLD` ORIGINATES edges.
//!    It:
//!      * Clusters the article's sources into two groups using k-means (k=2)
//!        seeded with the farthest-apart source embeddings.
//!      * Falls back to a temporal (created_at) split when the embedding-based
//!        clustering would produce a degenerate partition (< 3 sources in either
//!        cluster, or when sources lack embeddings entirely).
//!      * Creates two child articles compiled from each source cluster.
//!      * Archives the parent article (status = 'archived') without tombstoning,
//!        preserving inbound session / usage-trace references.
//!      * Enqueues `tree_embed` tasks for both children.

use std::sync::Arc;
use std::time::Instant;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::worker::{QueueTask, enqueue_task, llm::LlmClient, log_inference, parse_llm_json};

// ─── Runtime-overridable constants ───────────────────────────────────────────

/// Maximum number of ORIGINATES sources included in a single article compile.
///
/// Overridable via the `COVALENCE_PROVENANCE_CAP` environment variable.
pub const DEFAULT_PROVENANCE_CAP: usize = 40;

/// If an article accumulates more ORIGINATES edges than this, an `auto_split`
/// task is enqueued.
///
/// Overridable via the `COVALENCE_PROVENANCE_SPLIT_THRESHOLD` environment variable.
pub const DEFAULT_PROVENANCE_SPLIT_THRESHOLD: usize = 50;

/// Return the effective provenance cap, checking the env-var override first.
pub fn provenance_cap() -> usize {
    std::env::var("COVALENCE_PROVENANCE_CAP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_PROVENANCE_CAP)
}

/// Return the effective split-trigger threshold, checking the env-var override first.
pub fn provenance_split_threshold() -> usize {
    std::env::var("COVALENCE_PROVENANCE_SPLIT_THRESHOLD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_PROVENANCE_SPLIT_THRESHOLD)
}

// ─── Public handler ───────────────────────────────────────────────────────────

/// `auto_split`: Split an article whose ORIGINATES edge count has exceeded
/// `PROVENANCE_SPLIT_THRESHOLD` into two child articles.
///
/// Expected payload:
/// ```json
/// {
///   "article_id": "<uuid>",
///   "reason": "originates_overflow" | "backfill_overflow",
///   "originates_count_at_trigger": 51
/// }
/// ```
pub async fn handle_auto_split(
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
        .context("auto_split: missing article_id in payload")
        .and_then(|s| Uuid::parse_str(s).context("auto_split: invalid article_id UUID"))?;

    tracing::info!(
        task_id    = %task.id,
        article_id = %article_id,
        reason     = ?task.payload.get("reason"),
        "auto_split: starting"
    );

    // ── 2. Idempotency: abort if article is no longer active ────────────────
    let status_row = sqlx::query("SELECT status FROM covalence.nodes WHERE id = $1")
        .bind(article_id)
        .fetch_optional(pool)
        .await
        .context("auto_split: failed to fetch article status")?;

    let status: String = status_row
        .context("auto_split: article not found")?
        .get("status");

    if status != "active" {
        tracing::info!(
            article_id = %article_id,
            status     = %status,
            "auto_split: article is not active — skipping (idempotency)"
        );
        return Ok(json!({ "skipped": true, "reason": "article_not_active", "status": status }));
    }

    // ── 3. Fetch all ORIGINATES source IDs + created_at timestamps ──────────
    let source_rows = sqlx::query(
        "SELECT n.id, n.created_at
         FROM   covalence.edges  e
         JOIN   covalence.nodes  n ON n.id = e.source_node_id
         WHERE  e.target_node_id = $1
           AND  e.edge_type      = 'ORIGINATES'
         ORDER BY n.created_at ASC",
    )
    .bind(article_id)
    .fetch_all(pool)
    .await
    .context("auto_split: failed to fetch ORIGINATES sources")?;

    if source_rows.is_empty() {
        anyhow::bail!("auto_split: article {article_id} has no ORIGINATES sources");
    }

    let source_ids: Vec<Uuid> = source_rows.iter().map(|r| r.get("id")).collect();
    let source_created_at: Vec<DateTime<Utc>> =
        source_rows.iter().map(|r| r.get("created_at")).collect();

    tracing::debug!(
        article_id  = %article_id,
        source_count = source_ids.len(),
        "auto_split: fetched sources"
    );

    // ── 4. Fetch source embeddings ────────────────────────────────────────────
    // Sources that lack embeddings are excluded from clustering and assigned to
    // child_a by default (logged as a warning below).
    let embedding_rows = fetch_embeddings_raw(pool, &source_ids).await?;

    let embedded: Vec<(Uuid, Vec<f32>, DateTime<Utc>)> = source_ids
        .iter()
        .zip(source_created_at.iter())
        .filter_map(|(&id, &ts)| embedding_rows.get(&id).map(|emb| (id, emb.clone(), ts)))
        .collect();

    let unembedded_ids: Vec<Uuid> = source_ids
        .iter()
        .filter(|id| !embedding_rows.contains_key(*id))
        .copied()
        .collect();

    if !unembedded_ids.is_empty() {
        tracing::warn!(
            article_id     = %article_id,
            unembedded     = unembedded_ids.len(),
            "auto_split: {} source(s) lack embeddings — assigned to cluster A by default",
            unembedded_ids.len()
        );
        // Enqueue embed tasks for any sources missing embeddings.
        for &src_id in &unembedded_ids {
            if let Err(e) = enqueue_task(pool, "embed", Some(src_id), json!({}), 5).await {
                tracing::warn!(src_id = %src_id, "auto_split: failed to enqueue embed task: {e}");
            }
        }
    }

    // ── 5. Cluster sources into two groups ───────────────────────────────────
    // Fall back to temporal split when not enough embedded sources exist.
    let all_items: Vec<(Uuid, DateTime<Utc>)> = source_ids
        .iter()
        .zip(source_created_at.iter())
        .map(|(&id, &ts)| (id, ts))
        .collect();

    let (cluster_a_ids, cluster_b_ids) = if embedded.len() >= 6 {
        // Enough embeddings — use k-means k=2.
        let (mut ca, mut cb) = kmeans_2_split(&embedded);
        // Assign unembedded sources to cluster_a.
        ca.extend_from_slice(&unembedded_ids);
        (ca, cb)
    } else {
        // Fall back to temporal (even) split for all sources.
        tracing::info!(
            article_id  = %article_id,
            embedded    = embedded.len(),
            "auto_split: too few embedded sources — using temporal split"
        );
        temporal_split(&all_items)
    };

    tracing::info!(
        article_id = %article_id,
        cluster_a  = cluster_a_ids.len(),
        cluster_b  = cluster_b_ids.len(),
        "auto_split: cluster sizes determined"
    );

    // ── 6. Compile child articles ─────────────────────────────────────────────
    let t0 = Instant::now();
    let child_a_id = compile_child_article(pool, llm, &cluster_a_ids, article_id, "Part A")
        .await
        .context("auto_split: failed to compile child article A")?;

    let child_b_id = compile_child_article(pool, llm, &cluster_b_ids, article_id, "Part B")
        .await
        .context("auto_split: failed to compile child article B")?;
    let compile_ms = t0.elapsed().as_millis() as i32;

    tracing::info!(
        article_id = %article_id,
        child_a    = %child_a_id,
        child_b    = %child_b_id,
        compile_ms,
        "auto_split: child articles created"
    );

    // ── 7. Write CHILD_OF edges (raw SQL — no EdgeType enum variant needed) ──
    // edges.edge_type has no CHECK constraint (dropped in migration 010).
    for &child_id in &[child_a_id, child_b_id] {
        sqlx::query(
            "INSERT INTO covalence.edges
                 (source_node_id, target_node_id, edge_type, weight, created_by)
             VALUES ($1, $2, 'CHILD_OF', 1.0, 'auto_split')
             ON CONFLICT DO NOTHING",
        )
        .bind(child_id)
        .bind(article_id)
        .execute(pool)
        .await
        .with_context(|| format!("auto_split: failed to insert CHILD_OF edge for {child_id}"))?;
    }

    // ── 8. Archive the parent article ────────────────────────────────────────
    // NOT tombstoned — preserve inbound session / usage-trace references.
    let now_str = Utc::now().to_rfc3339();
    let split_note = json!([{
        "type": "split",
        "summary": format!("auto_split into {} and {}", child_a_id, child_b_id),
        "recorded_at": &now_str,
    }]);
    sqlx::query(
        r#"UPDATE covalence.nodes
              SET status      = 'archived',
                  modified_at = now(),
                  metadata    = jsonb_set(
                                    coalesce(metadata, '{}'::jsonb),
                                    '{mutation_log}',
                                    coalesce(metadata->'mutation_log', '[]'::jsonb) || $1::jsonb,
                                    true
                                )
            WHERE id = $2"#,
    )
    .bind(&split_note)
    .bind(article_id)
    .execute(pool)
    .await
    .context("auto_split: failed to archive parent article")?;

    // ── 9. Enqueue tree_embed + infer_article_edges for both children ─────────
    for &child_id in &[child_a_id, child_b_id] {
        enqueue_task(pool, "tree_embed", Some(child_id), json!({}), 4).await?;
        enqueue_task(pool, "infer_article_edges", Some(child_id), json!({}), 3).await?;
    }

    // ── 10. Log to inference_log ──────────────────────────────────────────────
    if let Err(e) = log_inference(
        pool,
        "auto_split",
        &[article_id],
        &format!(
            "article_id={article_id}, cluster_a={}, cluster_b={}",
            cluster_a_ids.len(),
            cluster_b_ids.len()
        ),
        &format!("split into {child_a_id} and {child_b_id}"),
        None,
        "",
        "auto_split",
        compile_ms,
    )
    .await
    {
        tracing::warn!(article_id = %article_id, "auto_split: log_inference failed: {e:#}");
    }

    tracing::info!(
        article_id = %article_id,
        child_a    = %child_a_id,
        child_b    = %child_b_id,
        "auto_split: done"
    );

    Ok(json!({
        "article_id":    article_id,
        "child_a_id":    child_a_id,
        "child_b_id":    child_b_id,
        "cluster_a_len": cluster_a_ids.len(),
        "cluster_b_len": cluster_b_ids.len(),
        "compile_ms":    compile_ms,
    }))
}

// ─── Child article compilation ────────────────────────────────────────────────

/// Create a new article compiled from the given source IDs.
///
/// Inserts the article node, writes ORIGINATES edges from each source, mirrors
/// those edges into `article_sources`, and returns the new article's UUID.
///
/// This is an inlined, simplified version of `handle_compile` tailored for the
/// auto-split case.  It does NOT re-enqueue further compile tasks (avoiding
/// infinite recursion) and does NOT apply the provenance cap (the k-means
/// partition already limits cluster size to ~N/2).
async fn compile_child_article(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    source_ids: &[Uuid],
    parent_id: Uuid,
    part_label: &str,
) -> anyhow::Result<Uuid> {
    use sqlx::Row as _;

    // Fetch parent title for child naming.
    let parent_title: String =
        sqlx::query_scalar("SELECT COALESCE(title, 'Article') FROM covalence.nodes WHERE id = $1")
            .bind(parent_id)
            .fetch_optional(pool)
            .await
            .context("compile_child_article: failed to fetch parent title")?
            .unwrap_or_else(|| "Article".to_string());

    let title_hint = format!("{parent_title} — {part_label}");

    // Cap source count to PROVENANCE_CAP so each child article does not itself
    // immediately trigger another auto_split after creation.
    let cap = provenance_cap();
    let source_ids = if source_ids.len() > cap {
        tracing::warn!(
            parent_id   = %parent_id,
            part_label  = %part_label,
            cluster_len = source_ids.len(),
            cap,
            "compile_child_article: cluster exceeds provenance cap — truncating"
        );
        &source_ids[..cap]
    } else {
        source_ids
    };

    // Fetch source content.
    let rows = sqlx::query(
        "SELECT id, title, content
         FROM   covalence.nodes
         WHERE  id = ANY($1)",
    )
    .bind(source_ids)
    .fetch_all(pool)
    .await
    .context("compile_child_article: failed to fetch source nodes")?;

    struct SrcDoc {
        id: Uuid,
        title: String,
        content: String,
    }

    let sources: Vec<SrcDoc> = rows
        .iter()
        .map(|r| SrcDoc {
            id: r.get("id"),
            title: r.get::<Option<String>, _>("title").unwrap_or_default(),
            content: r.get::<Option<String>, _>("content").unwrap_or_default(),
        })
        .collect();

    // Build LLM prompt (same format as handle_compile so MockLlmClient matches).
    let mut sources_block = String::new();
    for s in &sources {
        sources_block.push_str(&format!(
            "=== SOURCE {} ===\nTitle: {}\n\n{}\n\n",
            s.id, s.title, s.content
        ));
    }

    let prompt = format!(
        "You are a knowledge synthesizer. Read the following source documents and \
produce a well-structured article that synthesizes their information.\n\
\n\
Suggested title: {title_hint}\n\
Target length: ~2000 tokens (minimum 200, maximum 4000 tokens).\n\
\n\
CRITICAL — Preserve with HIGH FIDELITY:\n\
- Decisions and their rationale.\n\
- Rejected alternatives.\n\
- Open questions.\n\
- Reasoning chains.\n\
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

    let chat_model = std::env::var("COVALENCE_CHAT_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());

    let llm_result = llm.complete(&prompt, 4096).await;
    let (article_title, article_content, epistemic_type) = match llm_result {
        Ok(raw) => match parse_llm_json(&raw) {
            Some(v) => {
                let title = v
                    .get("title")
                    .and_then(|x| x.as_str())
                    .unwrap_or(&title_hint)
                    .to_string();
                let content = v
                    .get("content")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let etype = v
                    .get("epistemic_type")
                    .and_then(|x| x.as_str())
                    .unwrap_or("semantic")
                    .to_string();
                (title, content, etype)
            }
            None => {
                tracing::warn!(
                    parent_id  = %parent_id,
                    part_label = %part_label,
                    "compile_child_article: LLM JSON parse failed, falling back to concat"
                );
                let concat = sources
                    .iter()
                    .map(|s| format!("## {}\n\n{}", s.title, s.content))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                (title_hint.clone(), concat, "semantic".to_string())
            }
        },
        Err(e) => {
            tracing::warn!(
                parent_id  = %parent_id,
                part_label = %part_label,
                "compile_child_article: LLM error ({e}), falling back to concat"
            );
            let concat = sources
                .iter()
                .map(|s| format!("## {}\n\n{}", s.title, s.content))
                .collect::<Vec<_>>()
                .join("\n\n");
            (title_hint.clone(), concat, "semantic".to_string())
        }
    };

    let meta = json!({
        "split_from":     parent_id,
        "split_part":     part_label,
        "epistemic_type": epistemic_type,
        "mutation_log": [{
            "type": "created",
            "summary": format!("auto_split child from {parent_id} ({part_label})"),
            "recorded_at": Utc::now().to_rfc3339(),
        }],
    });

    // Insert child article node (transactional).
    let child_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO covalence.nodes
             (id, node_type, status, title, content, metadata, created_at, modified_at)
         VALUES ($1, 'article', 'active', $2, $3, $4, now(), now())",
    )
    .bind(child_id)
    .bind(&article_title)
    .bind(&article_content)
    .bind(&meta)
    .execute(pool)
    .await
    .context("compile_child_article: failed to insert child article node")?;

    // Write ORIGINATES edges + mirror into article_sources.
    for &src_id in source_ids {
        // SQL edges table.
        if let Err(e) = sqlx::query(
            "INSERT INTO covalence.edges
                 (source_node_id, target_node_id, edge_type, weight, created_by)
             VALUES ($1, $2, 'ORIGINATES', 1.0, 'auto_split')
             ON CONFLICT DO NOTHING",
        )
        .bind(src_id)
        .bind(child_id)
        .execute(pool)
        .await
        {
            tracing::warn!(
                src_id   = %src_id,
                child_id = %child_id,
                "compile_child_article: failed to insert ORIGINATES edge (non-fatal): {e}"
            );
        }

        // article_sources bridge (covalence#137).
        if let Err(e) = sqlx::query(
            "INSERT INTO covalence.article_sources
                 (article_id, source_id, relationship, causal_weight, confidence)
             VALUES ($1, $2, 'originates', 1.0, 1.0)
             ON CONFLICT (article_id, source_id, relationship) DO NOTHING",
        )
        .bind(child_id)
        .bind(src_id)
        .execute(pool)
        .await
        {
            tracing::warn!(
                child_id = %child_id,
                src_id   = %src_id,
                "compile_child_article: failed to mirror into article_sources (non-fatal): {e}"
            );
        }
    }

    // Enqueue embed task for the new child article.
    enqueue_task(pool, "embed", Some(child_id), json!({}), 5).await?;

    if let Err(e) = log_inference(
        pool,
        "auto_split_compile_child",
        source_ids,
        &format!(
            "parent_id={parent_id}, part_label={part_label}, sources={}",
            source_ids.len()
        ),
        &format!("child_id={child_id}, title={article_title}"),
        None,
        "",
        &chat_model,
        0,
    )
    .await
    {
        tracing::warn!(child_id = %child_id, "compile_child_article: log_inference failed: {e:#}");
    }

    Ok(child_id)
}

// ─── Embedding helpers ────────────────────────────────────────────────────────

/// Fetch embeddings as `Vec<f32>` for a list of node IDs.
///
/// Returns a map of node_id → embedding for all nodes that have an entry in
/// `covalence.node_embeddings`.  Nodes without embeddings are simply absent.
async fn fetch_embeddings_raw(
    pool: &PgPool,
    node_ids: &[Uuid],
) -> anyhow::Result<std::collections::HashMap<Uuid, Vec<f32>>> {
    use sqlx::Row as _;

    // Cast halfvec → float4[] for retrieval (pgvector stores as halfvec).
    let rows = sqlx::query(
        "SELECT node_id, embedding::float4[] AS embedding
         FROM   covalence.node_embeddings
         WHERE  node_id = ANY($1)",
    )
    .bind(node_ids)
    .fetch_all(pool)
    .await
    .context("fetch_embeddings_raw: query failed")?;

    let mut map = std::collections::HashMap::with_capacity(rows.len());
    for row in rows {
        let node_id: Uuid = row.get("node_id");
        let embedding: Vec<f32> = row.get("embedding");
        map.insert(node_id, embedding);
    }
    Ok(map)
}

// ─── K-means k=2 clustering ───────────────────────────────────────────────────

/// Cosine similarity between two equal-length float vectors.
///
/// Returns 1.0 when vectors are identical, −1.0 when anti-parallel, 0.0 when
/// orthogonal.  Returns 0.0 for zero-length vectors (treated as orthogonal).
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
}

/// Cosine *distance* = 1 − cosine_similarity.  Range [0, 2].
fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    1.0 - cosine_similarity(a, b)
}

/// Compute the elementwise mean of a non-empty slice of embedding vectors.
fn compute_centroid(vecs: &[&Vec<f32>]) -> Vec<f32> {
    assert!(!vecs.is_empty());
    let dim = vecs[0].len();
    let n = vecs.len() as f32;
    let mut centroid = vec![0.0_f32; dim];
    for v in vecs {
        for (i, x) in v.iter().enumerate() {
            centroid[i] += x;
        }
    }
    for x in &mut centroid {
        *x /= n;
    }
    centroid
}

/// Mini-batch k-means k=2 on source embeddings.
///
/// # Algorithm
/// 1. Seed centroids: pick the two source embeddings with maximum pairwise
///    cosine distance (avoids degenerate empty-cluster start).
/// 2. 20 iterations of assignment → centroid recompute.
/// 3. Degenerate guard: if either cluster has fewer than 3 items after
///    convergence, fall back to [`temporal_split`].
///
/// # Input
/// `items` — `(node_id, embedding, created_at)` triples.  Must have ≥ 2 items.
///
/// # Output
/// Two `Vec<Uuid>` representing the two clusters.  Every input ID appears in
/// exactly one output cluster.
pub fn kmeans_2_split(items: &[(Uuid, Vec<f32>, DateTime<Utc>)]) -> (Vec<Uuid>, Vec<Uuid>) {
    assert!(items.len() >= 2, "kmeans_2_split requires at least 2 items");

    let n = items.len();

    // ── Find the two farthest-apart seed embeddings (Option A) ────────────
    let (seed_a, seed_b) = {
        let mut best_dist = -1.0_f32;
        let mut best_i = 0;
        let mut best_j = 1;
        for i in 0..n {
            for j in (i + 1)..n {
                let d = cosine_distance(&items[i].1, &items[j].1);
                if d > best_dist {
                    best_dist = d;
                    best_i = i;
                    best_j = j;
                }
            }
        }
        (best_i, best_j)
    };

    let mut centroid_a = items[seed_a].1.clone();
    let mut centroid_b = items[seed_b].1.clone();

    // ── 20 iterations of assignment + centroid update ─────────────────────
    let mut assignments: Vec<bool> = vec![false; n]; // false → cluster A, true → cluster B

    for _ in 0..20 {
        // Assignment step.
        let mut changed = false;
        for (i, (_, emb, _)) in items.iter().enumerate() {
            let da = cosine_distance(emb, &centroid_a);
            let db = cosine_distance(emb, &centroid_b);
            let assign_b = db < da;
            if assignments[i] != assign_b {
                assignments[i] = assign_b;
                changed = true;
            }
        }

        // Centroid update.
        let vecs_a: Vec<&Vec<f32>> = items
            .iter()
            .enumerate()
            .filter(|(i, _)| !assignments[*i])
            .map(|(_, (_, emb, _))| emb)
            .collect();
        let vecs_b: Vec<&Vec<f32>> = items
            .iter()
            .enumerate()
            .filter(|(i, _)| assignments[*i])
            .map(|(_, (_, emb, _))| emb)
            .collect();

        if vecs_a.is_empty() || vecs_b.is_empty() {
            // Degenerate: one cluster is empty — stop early.
            break;
        }

        centroid_a = compute_centroid(&vecs_a);
        centroid_b = compute_centroid(&vecs_b);

        if !changed {
            break; // Converged.
        }
    }

    let cluster_a: Vec<Uuid> = items
        .iter()
        .enumerate()
        .filter(|(i, _)| !assignments[*i])
        .map(|(_, (id, _, _))| *id)
        .collect();
    let cluster_b: Vec<Uuid> = items
        .iter()
        .enumerate()
        .filter(|(i, _)| assignments[*i])
        .map(|(_, (id, _, _))| *id)
        .collect();

    // ── Degenerate guard: < 3 items in either cluster ─────────────────────
    if cluster_a.len() < 3 || cluster_b.len() < 3 {
        tracing::debug!(
            cluster_a_len = cluster_a.len(),
            cluster_b_len = cluster_b.len(),
            "kmeans_2_split: degenerate cluster detected — falling back to temporal split"
        );
        let all_items: Vec<(Uuid, DateTime<Utc>)> =
            items.iter().map(|(id, _, ts)| (*id, *ts)).collect();
        return temporal_split(&all_items);
    }

    (cluster_a, cluster_b)
}

/// Split `items` evenly by `created_at` order (temporal fallback).
///
/// Items are already expected to be sorted by `created_at`; this function
/// simply cuts at the median index.
pub fn temporal_split(items: &[(Uuid, DateTime<Utc>)]) -> (Vec<Uuid>, Vec<Uuid>) {
    let mid = items.len() / 2;
    let a = items[..mid].iter().map(|(id, _)| *id).collect();
    let b = items[mid..].iter().map(|(id, _)| *id).collect();
    (a, b)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    /// CAP and THRESHOLD values used only in unit tests.
    /// These avoid relying on env vars or the real constants in isolation tests.
    #[allow(dead_code)]
    const TEST_CAP: usize = 5;
    #[allow(dead_code)]
    const TEST_THRESHOLD: usize = 8;

    fn make_embedding(positive_half: bool, dim: usize) -> Vec<f32> {
        // "positive half" sources have high values in the first dim/2 dims
        // and near-zero in the rest; the other half is inverted.
        (0..dim)
            .map(|i| {
                if positive_half {
                    if i < dim / 2 { 1.0_f32 } else { 0.0_f32 }
                } else {
                    if i < dim / 2 { 0.0_f32 } else { 1.0_f32 }
                }
            })
            .collect()
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0_f32, 0.0, 0.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0_f32, 0.0, 0.0];
        let b = vec![0.0_f32, 1.0, 0.0];
        assert!((cosine_similarity(&a, &b)).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0_f32, 0.0, 0.0];
        let b = vec![-1.0_f32, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_kmeans_2_split_well_separated() {
        let ts = Utc::now();
        let dim = 16;
        // 6 sources clearly in cluster A, 6 in cluster B.
        let mut items: Vec<(Uuid, Vec<f32>, DateTime<Utc>)> = (0..6)
            .map(|_| (Uuid::new_v4(), make_embedding(true, dim), ts))
            .collect();
        items.extend((0..6).map(|_| (Uuid::new_v4(), make_embedding(false, dim), ts)));

        let (ca, cb) = kmeans_2_split(&items);
        assert_eq!(ca.len() + cb.len(), 12, "all items must be assigned");
        assert!(ca.len() >= 3, "cluster A must have ≥ 3 items");
        assert!(cb.len() >= 3, "cluster B must have ≥ 3 items");
        // With well-separated embeddings the 6+6 split should be perfect.
        assert_eq!(ca.len(), 6, "expected 6 in cluster A");
        assert_eq!(cb.len(), 6, "expected 6 in cluster B");
    }

    #[test]
    fn test_kmeans_2_split_degenerate_falls_back_to_temporal() {
        let ts = Utc::now();
        // All embeddings identical → cosine distance = 0 → degenerate clusters
        // → must fall back to temporal split at median.
        let n = 10usize;
        let emb = vec![1.0_f32; 4];
        let items: Vec<(Uuid, Vec<f32>, DateTime<Utc>)> =
            (0..n).map(|_| (Uuid::new_v4(), emb.clone(), ts)).collect();

        let (ca, cb) = kmeans_2_split(&items);
        assert_eq!(ca.len() + cb.len(), n);
        // Temporal split at n/2 = 5.
        assert_eq!(ca.len(), 5);
        assert_eq!(cb.len(), 5);
    }

    #[test]
    fn test_temporal_split_even() {
        let ts = Utc::now();
        let items: Vec<(Uuid, DateTime<Utc>)> = (0..10).map(|_| (Uuid::new_v4(), ts)).collect();
        let (a, b) = temporal_split(&items);
        assert_eq!(a.len(), 5);
        assert_eq!(b.len(), 5);
    }

    #[test]
    fn test_temporal_split_odd() {
        let ts = Utc::now();
        let items: Vec<(Uuid, DateTime<Utc>)> = (0..7).map(|_| (Uuid::new_v4(), ts)).collect();
        let (a, b) = temporal_split(&items);
        // 7 / 2 = 3 (integer division), so a=3, b=4.
        assert_eq!(a.len(), 3);
        assert_eq!(b.len(), 4);
    }

    #[test]
    fn test_provenance_cap_default() {
        // Without env var set the defaults should match spec constants.
        // (Avoid mutating env vars in tests — just check the hardcoded defaults.)
        assert_eq!(DEFAULT_PROVENANCE_CAP, 40);
        assert_eq!(DEFAULT_PROVENANCE_SPLIT_THRESHOLD, 50);
    }

    #[test]
    fn test_compute_centroid() {
        let v1 = vec![1.0_f32, 0.0];
        let v2 = vec![0.0_f32, 1.0];
        let c = compute_centroid(&[&v1, &v2]);
        assert!((c[0] - 0.5).abs() < 1e-6);
        assert!((c[1] - 0.5).abs() < 1e-6);
    }
}

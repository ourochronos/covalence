//! `infer_article_edges` slow-path handler (covalence#160).
//!
//! Populates direct article-to-article semantic edges using a four-tier
//! cheapest-first inference cascade:
//!
//! | Tier | Signal                        | Cost       | Edge type             | Conf   |
//! |------|-------------------------------|------------|-----------------------|--------|
//! | 1    | Jaccard on `article_sources`  | near-zero  | `RELATES_TO`          | 0.90   |
//! | 2    | `domain_path` array overlap   | near-zero  | `RELATES_TO`/`EXTENDS`| 0.80   |
//! | 3    | pgvector ANN cosine similarity| sub-ms     | `RELATES_TO`          | 0.7+sim|
//! | 4    | LLM directionality (optional) | 1 LLM call | any                   | LLM    |
//!
//! # Fanout discipline
//! Each task processes **one subject article** against all existing articles
//! (O(N) work per new article, not O(N²)).  Existing articles are never
//! re-queued as a side-effect of a new article's edge inference.
//!
//! # Idempotency
//! Before inserting any edge, the handler checks whether that exact
//! `(source, target, edge_type)` triple already exists in `covalence.edges`.
//! If it does, insertion is skipped.  Re-running the task for the same article
//! is therefore a safe no-op.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::QueueTask;
use super::llm::LlmClient;

// ─── configuration ────────────────────────────────────────────────────────────

/// Runtime-tunable thresholds for the edge-inference pipeline.
pub struct InferenceConfig {
    /// Minimum Jaccard coefficient (Tier 1) to insert a `RELATES_TO` edge.
    pub tier1_jaccard_threshold: f32,
    /// Minimum number of shared `domain_path` tags (Tier 2) to insert an edge.
    pub tier2_min_shared_tags: usize,
    /// Minimum cosine similarity (Tier 3) to insert a `RELATES_TO` edge.
    pub tier3_cosine_threshold: f32,
    /// ANN neighbour limit for Tier 3.
    pub tier3_ann_limit: usize,
    /// Combined-score threshold above which Tier 4 (LLM) is invoked.
    pub llm_threshold: f32,
    /// When `false`, Tier 4 is never invoked regardless of score.
    pub llm_enabled: bool,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            tier1_jaccard_threshold: 0.3,
            tier2_min_shared_tags: 2,
            tier3_cosine_threshold: 0.65,
            tier3_ann_limit: 20,
            llm_threshold: 0.85,
            llm_enabled: true,
        }
    }
}

// ─── internal types ───────────────────────────────────────────────────────────

/// All we need to know about the subject article.
struct SubjectArticle {
    id: Uuid,
    domain_path: Vec<String>,
    /// Embedding dimension count; `None` if the article has not been embedded.
    embedding_dims: Option<usize>,
    embedding_literal: Option<String>,
}

/// A candidate article with per-tier scores.
#[derive(Debug, Clone)]
struct Candidate {
    id: Uuid,
    domain_path: Vec<String>,
    /// Jaccard coefficient from Tier 1 (0 if not found via Tier 1).
    tier1_jaccard: f32,
    /// Cosine similarity from Tier 3 (0 if not found via Tier 3).
    tier3_sim: f32,
}

impl Candidate {
    /// Number of domain_path tags shared with `subject`.
    fn shared_tags(&self, subject: &SubjectArticle) -> usize {
        self.domain_path
            .iter()
            .filter(|t| subject.domain_path.contains(t))
            .count()
    }

    /// Returns `true` when `self.domain_path ⊂ subject.domain_path` (strict subset).
    fn is_subset_of(&self, subject: &SubjectArticle) -> bool {
        !self.domain_path.is_empty()
            && self
                .domain_path
                .iter()
                .all(|t| subject.domain_path.contains(t))
            && self.domain_path.len() < subject.domain_path.len()
    }

    /// Returns `true` when `subject.domain_path ⊂ self.domain_path` (strict subset).
    fn subject_is_subset_of(&self, subject: &SubjectArticle) -> bool {
        !subject.domain_path.is_empty()
            && subject
                .domain_path
                .iter()
                .all(|t| self.domain_path.contains(t))
            && subject.domain_path.len() < self.domain_path.len()
    }

    /// Combined score across all signal tiers.
    fn combined_score(&self, subject: &SubjectArticle, min_shared_tags: usize) -> f32 {
        let tier1 = self.tier1_jaccard * 0.9;
        let tier3 = self.tier3_sim * (0.7 + self.tier3_sim);
        let shared = self.shared_tags(subject);
        let tier2 = if shared >= min_shared_tags {
            0.8_f32
        } else {
            0.0
        };
        tier1.max(tier3).max(tier2)
    }

    /// Infer edge type from structural signals alone (before LLM).
    fn structural_edge_type(
        &self,
        subject: &SubjectArticle,
        min_shared_tags: usize,
    ) -> &'static str {
        let shared = self.shared_tags(subject);
        if shared >= min_shared_tags {
            // Narrower article EXTENDS the broader one.
            if self.is_subset_of(subject) || self.subject_is_subset_of(subject) {
                return "EXTENDS";
            }
        }
        "RELATES_TO"
    }
}

// ─── handler ─────────────────────────────────────────────────────────────────

/// Handle an `infer_article_edges` slow-path queue task.
///
/// # Steps
/// 1. Load the subject article's `domain_path` and embedding.
/// 2. Tier 1 — Jaccard query over `article_sources`.
/// 3. Tier 2 — domain_path overlap query (independent of Tier 1).
/// 4. Tier 3 — ANN query over `node_embeddings` (skipped if no embedding).
/// 5. Merge candidate sets; bulk-fetch missing domain_paths.
/// 6. Compute combined score; determine edge type.
/// 7. Optionally call LLM (Tier 4) to refine directionality.
/// 8. Insert edges through the output-equality firewall.
pub async fn handle_infer_article_edges(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> anyhow::Result<Value> {
    use sqlx::Row as _;

    let config = InferenceConfig::default();
    let subject_id = task
        .node_id
        .context("infer_article_edges: task requires node_id")?;

    let t_start = Instant::now();

    // ── 1. Load subject article ───────────────────────────────────────────────
    let subject = load_subject(pool, subject_id).await?;

    // Early exit if the article has been archived or doesn't exist.
    let subject = match subject {
        Some(s) => s,
        None => {
            tracing::info!(
                task_id    = %task.id,
                subject_id = %subject_id,
                "infer_article_edges: subject article not found or archived — skipping"
            );
            return Ok(json!({ "skipped": true, "reason": "article_not_found" }));
        }
    };

    // ── 2. Tier 1 — Jaccard on article_sources ────────────────────────────────
    let tier1_candidates = tier1_jaccard_candidates(pool, &subject, config.tier1_jaccard_threshold)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(
                subject_id = %subject_id,
                "infer_article_edges: Tier 1 query failed (non-fatal): {e:#}"
            );
            vec![]
        });

    // ── 3. Tier 2 — domain_path tag overlap ──────────────────────────────────
    // Runs independently so articles with overlapping tags but no shared
    // sources are still discovered.
    let tier2_candidates =
        tier2_domain_path_candidates(pool, &subject, config.tier2_min_shared_tags)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(
                    subject_id = %subject_id,
                    "infer_article_edges: Tier 2 query failed (non-fatal): {e:#}"
                );
                vec![]
            });

    // ── 4. Tier 3 — ANN embedding similarity ─────────────────────────────────
    let tier3_candidates = if subject.embedding_dims.is_some() {
        tier3_ann_candidates(
            pool,
            &subject,
            config.tier3_cosine_threshold,
            config.tier3_ann_limit,
        )
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(
                subject_id = %subject_id,
                "infer_article_edges: Tier 3 ANN query failed (non-fatal): {e:#}"
            );
            vec![]
        })
    } else {
        tracing::debug!(
            subject_id = %subject_id,
            "infer_article_edges: no embedding found — skipping Tier 3"
        );
        vec![]
    };

    // ── 5. Merge candidate sets ───────────────────────────────────────────────
    // Tier 2 candidates already carry their `domain_path` populated; Tier 1/3
    // candidates receive theirs in the bulk-fetch below.
    let mut candidates: HashMap<Uuid, Candidate> = HashMap::new();

    for c in tier1_candidates {
        candidates.insert(c.id, c);
    }
    for c in tier2_candidates {
        candidates
            .entry(c.id)
            .and_modify(|existing| {
                // Preserve domain_path from Tier 2 on merge.
                if existing.domain_path.is_empty() {
                    existing.domain_path = c.domain_path.clone();
                }
            })
            .or_insert(c);
    }
    for c in tier3_candidates {
        candidates
            .entry(c.id)
            .and_modify(|existing| {
                existing.tier3_sim = c.tier3_sim;
            })
            .or_insert(c);
    }

    if candidates.is_empty() {
        tracing::debug!(
            subject_id = %subject_id,
            "infer_article_edges: no candidates from any tier"
        );
    }

    // Bulk-fetch domain_paths for candidates that don't have them yet
    // (Tier 1 and Tier 3 entries without a Tier 2 hit).
    let missing_dp_ids: Vec<Uuid> = candidates
        .iter()
        .filter(|(_, c)| c.domain_path.is_empty())
        .map(|(id, _)| *id)
        .collect();

    if !missing_dp_ids.is_empty() {
        let domain_rows =
            sqlx::query("SELECT id, domain_path FROM covalence.nodes WHERE id = ANY($1)")
                .bind(&missing_dp_ids)
                .fetch_all(pool)
                .await
                .context("infer_article_edges: failed to fetch candidate domain_paths")?;

        for row in &domain_rows {
            let id: Uuid = row.get("id");
            let dp: Vec<String> = row
                .get::<Option<Vec<String>>, _>("domain_path")
                .unwrap_or_default();
            if let Some(c) = candidates.get_mut(&id) {
                c.domain_path = dp;
            }
        }
    }

    // ── 6–8. Score each candidate, optionally call LLM, firewall-insert ──────
    let mut edges_inserted = 0u32;
    let mut edges_skipped = 0u32;

    for candidate in candidates.values() {
        let combined_score = candidate.combined_score(&subject, config.tier2_min_shared_tags);

        // At least one tier must produce a positive signal.
        let tier1_ok = candidate.tier1_jaccard >= config.tier1_jaccard_threshold;
        let tier3_ok = candidate.tier3_sim > config.tier3_cosine_threshold;
        let shared_tags = candidate.shared_tags(&subject);
        let tier2_ok = shared_tags >= config.tier2_min_shared_tags;

        if !tier1_ok && !tier2_ok && !tier3_ok {
            continue;
        }

        // Determine edge type from structural signals.
        let mut edge_type = candidate
            .structural_edge_type(&subject, config.tier2_min_shared_tags)
            .to_string();

        // Determine confidence.
        let mut confidence: f32 = if tier1_ok {
            0.9_f32.max(combined_score).min(0.95)
        } else if tier2_ok {
            0.8_f32.max(combined_score).min(0.95)
        } else {
            // Tier 3 only.
            (0.7 + candidate.tier3_sim).min(0.95)
        };

        // Identify the first firing tier (for observability metadata).
        let tier: u8 = if tier1_ok {
            1
        } else if tier2_ok {
            2
        } else {
            3
        };

        // ── Tier 4: LLM directionality (selective) ────────────────────────────
        if config.llm_enabled && combined_score >= config.llm_threshold {
            match llm_infer_directionality(pool, llm, subject_id, candidate.id).await {
                Ok((llm_edge_type, llm_conf)) => {
                    edge_type = llm_edge_type;
                    confidence = llm_conf;
                    tracing::debug!(
                        subject_id   = %subject_id,
                        candidate_id = %candidate.id,
                        edge_type    = %edge_type,
                        confidence   = confidence,
                        "infer_article_edges: Tier 4 LLM refined edge"
                    );
                }
                Err(e) => {
                    // Graceful degradation: keep RELATES_TO with Tier 3 confidence.
                    tracing::warn!(
                        subject_id   = %subject_id,
                        candidate_id = %candidate.id,
                        "infer_article_edges: Tier 4 LLM failed, keeping RELATES_TO \
                         (non-fatal): {e:#}"
                    );
                }
            }
        }

        // ── Output equality firewall ──────────────────────────────────────────
        let already_exists: Option<_> = sqlx::query(
            "SELECT 1 FROM covalence.edges \
             WHERE source_node_id = $1 \
               AND target_node_id = $2 \
               AND edge_type      = $3 \
             LIMIT 1",
        )
        .bind(subject_id)
        .bind(candidate.id)
        .bind(&edge_type)
        .fetch_optional(pool)
        .await
        .context("infer_article_edges: firewall check failed")?;

        if already_exists.is_some() {
            tracing::debug!(
                subject_id   = %subject_id,
                candidate_id = %candidate.id,
                edge_type    = %edge_type,
                "infer_article_edges: edge already exists — skipping (idempotent)"
            );
            edges_skipped += 1;
            continue;
        }

        // ── Insert the edge ───────────────────────────────────────────────────
        let causal_weight: f32 = edge_type
            .parse::<crate::models::EdgeType>()
            .map(|et| et.causal_weight())
            .unwrap_or(0.15);

        sqlx::query(
            "INSERT INTO covalence.edges \
             (source_node_id, target_node_id, edge_type, \
              weight, confidence, causal_weight, metadata, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'infer_article_edges')",
        )
        .bind(subject_id)
        .bind(candidate.id)
        .bind(&edge_type)
        .bind(1.0_f32)
        .bind(confidence)
        .bind(causal_weight)
        .bind(json!({ "inferred_by": "infer_article_edges", "tier": tier }))
        .execute(pool)
        .await
        .with_context(|| {
            format!(
                "infer_article_edges: failed to insert {} edge {} → {}",
                edge_type, subject_id, candidate.id
            )
        })?;

        edges_inserted += 1;

        tracing::debug!(
            subject    = %subject_id,
            target     = %candidate.id,
            edge_type  = %edge_type,
            confidence = confidence,
            tier,
            "[infer_article_edges] edge inserted (tier={tier}, conf={confidence:.3})"
        );
    }

    let elapsed_ms = t_start.elapsed().as_millis() as u64;

    tracing::info!(
        subject_id     = %subject_id,
        edges_inserted,
        edges_skipped,
        elapsed_ms,
        "infer_article_edges: done"
    );

    Ok(json!({
        "subject_id":     subject_id,
        "edges_inserted": edges_inserted,
        "edges_skipped":  edges_skipped,
        "elapsed_ms":     elapsed_ms,
    }))
}

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Load the subject article's metadata and embedding from the DB.
///
/// Returns `None` when the article does not exist or is not `active`.
async fn load_subject(pool: &PgPool, subject_id: Uuid) -> anyhow::Result<Option<SubjectArticle>> {
    use sqlx::Row as _;

    let row = sqlx::query(
        "SELECT n.id, n.domain_path, ne.embedding::text AS emb_text \
         FROM   covalence.nodes n \
         LEFT JOIN covalence.node_embeddings ne ON ne.node_id = n.id \
         WHERE  n.id        = $1 \
           AND  n.node_type = 'article' \
           AND  n.status    = 'active'",
    )
    .bind(subject_id)
    .fetch_optional(pool)
    .await
    .context("infer_article_edges: failed to fetch subject article")?;

    let row = match row {
        Some(r) => r,
        None => return Ok(None),
    };

    let domain_path: Vec<String> = row
        .get::<Option<Vec<String>>, _>("domain_path")
        .unwrap_or_default();

    // Parse the pgvector text representation back into a float vector so we
    // can reuse it for the ANN query without a round-trip.
    let emb_text: Option<String> = row.get("emb_text");
    let (embedding_dims, embedding_literal) = match emb_text {
        Some(text) => {
            // pgvector text format: "[f1,f2,...,fn]"
            let dims = text
                .trim_matches(|c| c == '[' || c == ']')
                .split(',')
                .count();
            (Some(dims), Some(text))
        }
        None => (None, None),
    };

    Ok(Some(SubjectArticle {
        id: subject_id,
        domain_path,
        embedding_dims,
        embedding_literal,
    }))
}

/// Tier 1: Return articles whose Jaccard coefficient with the subject
/// (over shared `article_sources` source sets) meets the threshold.
async fn tier1_jaccard_candidates(
    pool: &PgPool,
    subject: &SubjectArticle,
    threshold: f32,
) -> anyhow::Result<Vec<Candidate>> {
    use sqlx::Row as _;

    // Guard: if subject has no sources there is nothing to compare.
    let subject_source_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM covalence.article_sources WHERE article_id = $1")
            .bind(subject.id)
            .fetch_one(pool)
            .await
            .unwrap_or(0);

    if subject_source_count == 0 {
        return Ok(vec![]);
    }

    // Cast to float8 explicitly so sqlx can decode the division result.
    let rows = sqlx::query(
        r#"SELECT
               b.article_id,
               (COUNT(*) * 1.0 / NULLIF(
                   (SELECT COUNT(*) FROM covalence.article_sources WHERE article_id = $1)
                   + COUNT(DISTINCT b.source_id)
                   - COUNT(*),
                   0
               ))::float8 AS jaccard
           FROM covalence.article_sources a
           JOIN covalence.article_sources b
             ON a.source_id = b.source_id
            AND b.article_id != $1
           WHERE a.article_id = $1
           GROUP BY b.article_id
           HAVING (COUNT(*) * 1.0 / NULLIF(
               (SELECT COUNT(*) FROM covalence.article_sources WHERE article_id = $1)
               + COUNT(DISTINCT b.source_id)
               - COUNT(*),
               0
           ))::float8 >= $2"#,
    )
    .bind(subject.id)
    .bind(threshold as f64)
    .fetch_all(pool)
    .await
    .context("Tier 1 Jaccard query failed")?;

    Ok(rows
        .iter()
        .map(|r| {
            let id: Uuid = r.get("article_id");
            let jaccard: f64 = r.get::<Option<f64>, _>("jaccard").unwrap_or(0.0);
            Candidate {
                id,
                domain_path: vec![],
                tier1_jaccard: jaccard as f32,
                tier3_sim: 0.0,
            }
        })
        .collect())
}

/// Tier 2: Return articles with ≥ `min_shared_tags` overlapping `domain_path`
/// entries.  The SQL `&&` array operator is used to pre-filter, then Rust
/// counts the actual intersection size.
async fn tier2_domain_path_candidates(
    pool: &PgPool,
    subject: &SubjectArticle,
    min_shared_tags: usize,
) -> anyhow::Result<Vec<Candidate>> {
    use sqlx::Row as _;

    if subject.domain_path.is_empty() {
        return Ok(vec![]);
    }

    // Fetch all articles that share *any* tag (quick index scan via `&&`),
    // then filter the intersection count in Rust.
    let rows = sqlx::query(
        "SELECT id, domain_path \
         FROM   covalence.nodes \
         WHERE  node_type   = 'article' \
           AND  status      = 'active' \
           AND  id         != $1 \
           AND  domain_path && $2",
    )
    .bind(subject.id)
    .bind(&subject.domain_path)
    .fetch_all(pool)
    .await
    .context("Tier 2 domain_path query failed")?;

    let candidates = rows
        .iter()
        .filter_map(|r| {
            let id: Uuid = r.get("id");
            let dp: Vec<String> = r
                .get::<Option<Vec<String>>, _>("domain_path")
                .unwrap_or_default();

            // Count shared tags in Rust.
            let shared = dp
                .iter()
                .filter(|t| subject.domain_path.contains(t))
                .count();

            if shared >= min_shared_tags {
                Some(Candidate {
                    id,
                    domain_path: dp,
                    tier1_jaccard: 0.0,
                    tier3_sim: 0.0,
                })
            } else {
                None
            }
        })
        .collect();

    Ok(candidates)
}

/// Tier 3: Return up to `limit` articles ordered by embedding cosine similarity,
/// filtered to those whose similarity exceeds `threshold`.
async fn tier3_ann_candidates(
    pool: &PgPool,
    subject: &SubjectArticle,
    threshold: f32,
    limit: usize,
) -> anyhow::Result<Vec<Candidate>> {
    use sqlx::Row as _;

    let dims = match subject.embedding_dims {
        Some(d) => d,
        None => return Ok(vec![]),
    };
    let vec_literal = match &subject.embedding_literal {
        Some(l) => l,
        None => return Ok(vec![]),
    };

    // Build the ANN query with the literal embedding dimensions baked in,
    // since halfvec casts require a compile-time dimension parameter in pgvector.
    // The vec_literal value itself is passed as a bound $3 parameter to avoid
    // SQL injection (Coding Standard §2).
    let query = format!(
        "SELECT ne.node_id, \
                (1.0 - (ne.embedding::halfvec({dims}) <=> $3::halfvec({dims})))::float8 \
                    AS similarity \
         FROM   covalence.node_embeddings ne \
         JOIN   covalence.nodes n ON n.id = ne.node_id \
         WHERE  n.node_type = 'article' \
           AND  n.status    = 'active' \
           AND  ne.node_id != $1 \
         ORDER  BY ne.embedding::halfvec({dims}) <=> $3::halfvec({dims}) \
         LIMIT  $2",
    );

    let rows = sqlx::query(&query)
        .bind(subject.id)
        .bind(limit as i64)
        .bind(vec_literal.as_str())
        .fetch_all(pool)
        .await
        .context("Tier 3 ANN query failed")?;

    Ok(rows
        .iter()
        .filter_map(|r| {
            let similarity: f64 = r.get::<f64, _>("similarity");
            if (similarity as f32) <= threshold {
                return None; // below threshold
            }
            let id: Uuid = r.get("node_id");
            Some(Candidate {
                id,
                domain_path: vec![],
                tier1_jaccard: 0.0,
                tier3_sim: similarity as f32,
            })
        })
        .collect())
}

/// Tier 4: Ask the LLM to determine the directionality of the relationship
/// between two articles.
///
/// Returns `(edge_type, confidence)` where `edge_type` is one of
/// `EXTENDS`, `CONFIRMS`, `CONTRADICTS`, `CONTENDS`, or `RELATES_TO`.
/// Confidence is clamped to `[0.5, 0.95]`.
async fn llm_infer_directionality(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    subject_id: Uuid,
    candidate_id: Uuid,
) -> anyhow::Result<(String, f32)> {
    use sqlx::Row as _;

    // Fetch excerpts for both articles.  Skip the first 150 chars (injected
    // boilerplate on split articles per covalence#186) and take up to 2000
    // chars of real content.
    let rows = sqlx::query(
        "SELECT id, title, SUBSTRING(content, 150, 2000) AS excerpt \
         FROM   covalence.nodes \
         WHERE  id = ANY($1)",
    )
    .bind(&[subject_id, candidate_id])
    .fetch_all(pool)
    .await
    .context("Tier 4: failed to fetch article excerpts")?;

    let mut subject_title = String::new();
    let mut subject_excerpt = String::new();
    let mut candidate_title = String::new();
    let mut candidate_excerpt = String::new();

    for row in &rows {
        let id: Uuid = row.get("id");
        let title: String = row.get::<Option<String>, _>("title").unwrap_or_default();
        let excerpt: String = row.get::<Option<String>, _>("excerpt").unwrap_or_default();
        if id == subject_id {
            subject_title = title;
            subject_excerpt = excerpt;
        } else {
            candidate_title = title;
            candidate_excerpt = excerpt;
        }
    }

    let prompt = format!(
        "You are a knowledge-graph edge classifier. \
         Determine the semantic relationship between Article A and Article B.\n\n\
         Article A:\nTitle: {subject_title}\nExcerpt: {subject_excerpt}\n\n\
         Article B:\nTitle: {candidate_title}\nExcerpt: {candidate_excerpt}\n\n\
         Return ONLY valid JSON (no markdown fences):\n\
         {{\"relationship\": \"EXTENDS|CONFIRMS|CONTRADICTS|CONTENDS|RELATES_TO\", \
           \"confidence\": 0.0..1.0, \
           \"reasoning\": \"...\"}}\n\
         Where:\n\
         - EXTENDS: A elaborates/specializes B or vice-versa\n\
         - CONFIRMS: A and B corroborate each other\n\
         - CONTRADICTS: A and B make incompatible, mutually exclusive claims\n\
         - CONTENDS: one source contends (disputes without fully contradicting) a claim in the other\n\
         - RELATES_TO: topically related but no stronger relationship"
    );

    let raw = llm
        .complete(&prompt, 256)
        .await
        .context("Tier 4 LLM call failed")?;

    // Strip fences and parse JSON.
    let text = raw.trim();
    let text = if text.starts_with("```") {
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() >= 3 {
            lines[1..lines.len() - 1].join("\n")
        } else {
            text.to_string()
        }
    } else {
        text.to_string()
    };

    let v: serde_json::Value =
        serde_json::from_str(&text).context("Tier 4: failed to parse LLM JSON response")?;

    let rel = v
        .get("relationship")
        .and_then(|r| r.as_str())
        .unwrap_or("RELATES_TO");

    let edge_type = match rel {
        "EXTENDS" | "CONFIRMS" | "CONTRADICTS" | "CONTENDS" => rel.to_string(),
        _ => "RELATES_TO".to_string(),
    };

    let confidence: f32 = v
        .get("confidence")
        .and_then(|c| c.as_f64())
        .map(|c| c as f32)
        .unwrap_or(0.75)
        .clamp(0.5, 0.95);

    Ok((edge_type, confidence))
}

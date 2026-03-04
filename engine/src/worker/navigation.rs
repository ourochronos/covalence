//! KB Navigation — 3-layer topology-map and domain-landmark generation (covalence#112).
//!
//! # Layers
//! 1. **Topology Map** (`generate_topology_map`) — a single auto-generated article
//!    that describes the KB's overall structure: domain facets, hub articles,
//!    recent modifications, active contentions, and cross-domain bridges.
//! 2. **Domain Landmarks** (`generate_domain_landmarks`) — one "Domain Overview"
//!    article per major domain (≥ 5 articles), pinned and flagged as landmarks
//!    so they are never evicted.
//! 3. **Bridge Detection** (`detect_bridge_articles`) — identifies articles whose
//!    `domain_path` spans two or more distinct top-level domains.

use anyhow::Context as _;
use chrono::Utc;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

// ─── Types ────────────────────────────────────────────────────────────────────

/// A domain facet: top-level domain name and count of active articles.
#[derive(Debug, Clone)]
pub struct DomainFacet {
    pub domain: String,
    pub article_count: i64,
}

/// Summary of a hub article.
#[derive(Debug, Clone)]
pub struct HubArticle {
    pub id: Uuid,
    pub title: String,
    pub structural_importance: f64,
}

/// A recently-modified article.
#[derive(Debug, Clone)]
pub struct RecentArticle {
    pub id: Uuid,
    pub title: String,
    pub modified_at: chrono::DateTime<Utc>,
}

/// An article that bridges multiple top-level domains.
#[derive(Debug, Clone)]
pub struct BridgeArticle {
    pub id: Uuid,
    pub title: String,
    /// Top-level domain names this article spans.
    pub domains: Vec<String>,
}

/// Full result returned by [`generate_topology_map`].
#[derive(Debug)]
pub struct TopologyMapResult {
    /// UUID of the upserted topology-map article.
    pub article_id: Uuid,
    pub domain_count: usize,
    pub bridge_count: usize,
    pub hub_count: usize,
}

/// Result returned by [`generate_domain_landmarks`].
#[derive(Debug)]
pub struct DomainLandmarkResult {
    /// Number of domain landmark articles created or refreshed.
    pub upserted: usize,
    /// Names of the domains that were processed.
    pub domains: Vec<String>,
}

// ─── Layer 3: Bridge detection ────────────────────────────────────────────────

/// Identify articles whose `domain_path` spans ≥ 2 distinct top-level domains.
///
/// A "top-level domain" is `domain_path[1]` in Postgres array notation
/// (i.e. the first element of the `TEXT[]` column).  Articles with fewer than
/// two elements in `domain_path` cannot be bridges.
pub async fn detect_bridge_articles(pool: &PgPool) -> anyhow::Result<Vec<BridgeArticle>> {
    use sqlx::Row as _;

    // Fetch all active articles that have at least two domain_path elements.
    let rows = sqlx::query(
        "SELECT id, title, domain_path
         FROM   covalence.nodes
         WHERE  node_type  = 'article'
           AND  status     = 'active'
           AND  array_length(domain_path, 1) >= 2
         ORDER  BY id",
    )
    .fetch_all(pool)
    .await
    .context("detect_bridge_articles: query failed")?;

    let mut bridges = Vec::new();

    for row in &rows {
        let id: Uuid = row.get("id");
        let title: String = row.get::<Option<String>, _>("title").unwrap_or_default();
        let domain_path: Vec<String> = row
            .get::<Option<Vec<String>>, _>("domain_path")
            .unwrap_or_default();

        // Collect distinct top-level domains (first path element of each entry).
        // e.g. ["rust/stdlib", "python/io"] → ["rust", "python"]
        let mut top_domains: Vec<String> = domain_path
            .iter()
            .map(|entry| {
                entry
                    .split('/')
                    .next()
                    .unwrap_or(entry.as_str())
                    .to_lowercase()
            })
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        top_domains.sort_unstable();

        if top_domains.len() >= 2 {
            bridges.push(BridgeArticle {
                id,
                title,
                domains: top_domains,
            });
        }
    }

    Ok(bridges)
}

// ─── Layer 2: Domain landmark articles ───────────────────────────────────────

/// Generate (or refresh) a "Domain Overview: {domain}" landmark article for
/// each top-level domain that has ≥ `min_articles` active articles.
///
/// Landmark articles are:
/// - `pinned = true`  — protected from organic eviction by the eviction query
/// - `is_landmark = true` — protected by the eviction guard added in #112
///
/// A domain is considered to need refreshing if:
/// - No landmark article exists yet, **or**
/// - The article count for the domain has changed by > 20 % since the last
///   generation (stored in the article's metadata as `landmark_article_count`).
pub async fn generate_domain_landmarks(
    pool: &PgPool,
    min_articles: i64,
) -> anyhow::Result<DomainLandmarkResult> {
    use sqlx::Row as _;

    // ── 1. Facet active articles by top-level domain ──────────────────────
    let facet_rows = sqlx::query(
        "SELECT domain_path[1] AS top_domain, COUNT(*) AS cnt
         FROM   covalence.nodes
         WHERE  node_type  = 'article'
           AND  status     = 'active'
           AND  domain_path IS NOT NULL
           AND  array_length(domain_path, 1) >= 1
         GROUP  BY domain_path[1]
         HAVING COUNT(*) >= $1
         ORDER  BY cnt DESC",
    )
    .bind(min_articles)
    .fetch_all(pool)
    .await
    .context("generate_domain_landmarks: facet query failed")?;

    let facets: Vec<DomainFacet> = facet_rows
        .iter()
        .map(|r| DomainFacet {
            domain: r.get::<Option<String>, _>("top_domain").unwrap_or_default(),
            article_count: r.get("cnt"),
        })
        .filter(|f| !f.domain.is_empty())
        .collect();

    let mut upserted = 0usize;
    let mut processed_domains = Vec::new();

    for facet in &facets {
        // ── 2. Check for existing landmark article ────────────────────────
        let landmark_title = format!("Domain Overview: {}", facet.domain);

        let existing: Option<(Uuid, serde_json::Value)> = sqlx::query_as(
            "SELECT id, COALESCE(metadata, '{}'::jsonb)
             FROM   covalence.nodes
             WHERE  node_type   = 'article'
               AND  status      = 'active'
               AND  is_landmark = true
               AND  title       = $1
             LIMIT  1",
        )
        .bind(&landmark_title)
        .fetch_optional(pool)
        .await
        .context("generate_domain_landmarks: existing landmark query failed")?;

        // ── 3. Decide whether to (re)generate ────────────────────────────
        let needs_refresh = match &existing {
            None => true,
            Some((_, meta)) => {
                let prev_count = meta
                    .get("landmark_article_count")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let delta = (facet.article_count - prev_count).unsigned_abs() as f64;
                let threshold = (prev_count as f64 * 0.20).max(1.0);
                delta > threshold
            }
        };

        if !needs_refresh {
            processed_domains.push(facet.domain.clone());
            continue;
        }

        // ── 4. Fetch key articles for this domain (top 20 by structural_importance) ─
        let article_rows = sqlx::query(
            "SELECT id, title, structural_importance
             FROM   covalence.nodes
             WHERE  node_type   = 'article'
               AND  status      = 'active'
               AND  domain_path[1] = $1
             ORDER  BY structural_importance DESC NULLS LAST, modified_at DESC
             LIMIT  20",
        )
        .bind(&facet.domain)
        .fetch_all(pool)
        .await
        .context("generate_domain_landmarks: article fetch failed")?;

        // ── 5. Build landmark article content ─────────────────────────────
        let mut content = format!(
            "# Domain Overview: {}\n\n\
             This landmark article provides an orientation to the **{}** domain \
             in the knowledge base.\n\n\
             **Article count:** {}\n\n\
             ## Key Articles\n\n",
            facet.domain, facet.domain, facet.article_count
        );

        for row in &article_rows {
            let article_id: Uuid = row.get("id");
            let article_title: String = row.get::<Option<String>, _>("title").unwrap_or_default();
            let si: f64 = row
                .get::<Option<f64>, _>("structural_importance")
                .unwrap_or(0.0);
            content.push_str(&format!(
                "- **{}** (id: `{}`, importance: {:.2})\n",
                article_title, article_id, si
            ));
        }

        content.push_str(&format!(
            "\n---\n*Auto-generated by KB navigation maintenance on {}.*\n",
            Utc::now().format("%Y-%m-%d")
        ));

        let meta = json!({
            "landmark_article_count": facet.article_count,
            "landmark_domain": facet.domain,
            "generated_at": Utc::now().to_rfc3339(),
        });

        // ── 6. Upsert landmark article ────────────────────────────────────
        match existing {
            Some((existing_id, _)) => {
                // Refresh existing landmark
                sqlx::query(
                    "UPDATE covalence.nodes
                     SET    title       = $1,
                            content     = $2,
                            metadata    = $3,
                            pinned      = true,
                            is_landmark = true,
                            modified_at = now()
                     WHERE  id = $4",
                )
                .bind(&landmark_title)
                .bind(&content)
                .bind(&meta)
                .bind(existing_id)
                .execute(pool)
                .await
                .context("generate_domain_landmarks: update failed")?;
            }
            None => {
                // Create new landmark
                sqlx::query(
                    "INSERT INTO covalence.nodes
                         (id, node_type, status, title, content, metadata,
                          pinned, is_landmark, domain_path, created_at, modified_at)
                     VALUES
                         (gen_random_uuid(), 'article', 'active', $1, $2, $3,
                          true, true, ARRAY[$4], now(), now())",
                )
                .bind(&landmark_title)
                .bind(&content)
                .bind(&meta)
                .bind(&facet.domain)
                .execute(pool)
                .await
                .context("generate_domain_landmarks: insert failed")?;
            }
        }

        upserted += 1;
        processed_domains.push(facet.domain.clone());
    }

    Ok(DomainLandmarkResult {
        upserted,
        domains: processed_domains,
    })
}

// ─── Layer 1: Topology map article ───────────────────────────────────────────

/// Generate (or refresh) the single "KB Topology Map — {date}" article.
///
/// Collects:
/// - Domain facets (top-level domain → article count)
/// - Total articles, sources, edge count
/// - Top-5 hub articles by `structural_importance DESC`
/// - Articles modified in the last 7 days
/// - Active contention count
/// - Bridge articles (spanning multiple domains)
///
/// The resulting article is inserted or updated in place (matched on title
/// prefix `"KB Topology Map"`).  It is `pinned = true` and `is_landmark = true`
/// so it is never evicted.
pub async fn generate_topology_map(pool: &PgPool) -> anyhow::Result<TopologyMapResult> {
    use sqlx::Row as _;

    // ── 1. Aggregate counts ───────────────────────────────────────────────
    let count_row = sqlx::query(
        "SELECT
             COUNT(*) FILTER (WHERE node_type = 'article' AND status = 'active') AS article_count,
             COUNT(*) FILTER (WHERE node_type = 'source'  AND status = 'active') AS source_count
         FROM covalence.nodes",
    )
    .fetch_one(pool)
    .await
    .context("generate_topology_map: count query failed")?;

    let article_count: i64 = count_row.get("article_count");
    let source_count: i64 = count_row.get("source_count");

    let edge_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM covalence.edges")
        .fetch_one(pool)
        .await
        .context("generate_topology_map: edge count failed")?;

    let active_contentions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM covalence.contentions WHERE status != 'resolved'")
            .fetch_one(pool)
            .await
            .context("generate_topology_map: contention count failed")?;

    // ── 2. Domain facets ──────────────────────────────────────────────────
    let facet_rows = sqlx::query(
        "SELECT domain_path[1] AS top_domain, COUNT(*) AS cnt
         FROM   covalence.nodes
         WHERE  node_type  = 'article'
           AND  status     = 'active'
           AND  domain_path IS NOT NULL
           AND  array_length(domain_path, 1) >= 1
         GROUP  BY domain_path[1]
         ORDER  BY cnt DESC",
    )
    .fetch_all(pool)
    .await
    .context("generate_topology_map: domain facet query failed")?;

    let domain_facets: Vec<DomainFacet> = facet_rows
        .iter()
        .map(|r| DomainFacet {
            domain: r.get::<Option<String>, _>("top_domain").unwrap_or_default(),
            article_count: r.get("cnt"),
        })
        .filter(|f| !f.domain.is_empty())
        .collect();

    // ── 3. Hub articles (top-5 by structural_importance) ─────────────────
    let hub_rows = sqlx::query(
        "SELECT id, title, structural_importance
         FROM   covalence.nodes
         WHERE  node_type = 'article'
           AND  status    = 'active'
         ORDER  BY structural_importance DESC NULLS LAST
         LIMIT  5",
    )
    .fetch_all(pool)
    .await
    .context("generate_topology_map: hub article query failed")?;

    let hub_articles: Vec<HubArticle> = hub_rows
        .iter()
        .map(|r| HubArticle {
            id: r.get("id"),
            title: r.get::<Option<String>, _>("title").unwrap_or_default(),
            structural_importance: r
                .get::<Option<f64>, _>("structural_importance")
                .unwrap_or(0.0),
        })
        .collect();

    // ── 4. Recently modified articles (last 7 days) ───────────────────────
    let recent_rows = sqlx::query(
        "SELECT id, title, modified_at
         FROM   covalence.nodes
         WHERE  node_type   = 'article'
           AND  status      = 'active'
           AND  is_landmark = false
           AND  modified_at >= now() - interval '7 days'
         ORDER  BY modified_at DESC
         LIMIT  10",
    )
    .fetch_all(pool)
    .await
    .context("generate_topology_map: recent articles query failed")?;

    let recent_articles: Vec<RecentArticle> = recent_rows
        .iter()
        .map(|r| RecentArticle {
            id: r.get("id"),
            title: r.get::<Option<String>, _>("title").unwrap_or_default(),
            modified_at: r.get("modified_at"),
        })
        .collect();

    // ── 5. Bridge articles ────────────────────────────────────────────────
    let bridge_articles = detect_bridge_articles(pool).await?;

    // ── 6. Build article content ──────────────────────────────────────────
    let date_str = Utc::now().format("%Y-%m-%d").to_string();
    let title = format!("KB Topology Map — {date_str}");

    let mut content = format!(
        "# KB Topology Map — {date_str}\n\n\
         This article provides a structural overview of the knowledge base.\n\n\
         ## Summary\n\n\
         | Metric | Count |\n\
         |--------|-------|\n\
         | Active articles | {article_count} |\n\
         | Active sources  | {source_count} |\n\
         | Edges           | {edge_count} |\n\
         | Active contentions | {active_contentions} |\n\n"
    );

    // Domain facets
    if !domain_facets.is_empty() {
        content.push_str("## Domains\n\n");
        content.push_str("| Domain | Articles |\n|--------|----------|\n");
        for f in &domain_facets {
            content.push_str(&format!("| {} | {} |\n", f.domain, f.article_count));
        }
        content.push('\n');
    } else {
        content.push_str("## Domains\n\n*No domain-tagged articles yet.*\n\n");
    }

    // Hub articles
    content.push_str("## Hub Articles (by Structural Importance)\n\n");
    if hub_articles.is_empty() {
        content.push_str("*No articles yet.*\n\n");
    } else {
        for h in &hub_articles {
            content.push_str(&format!(
                "- **{}** (id: `{}`, importance: {:.3})\n",
                h.title, h.id, h.structural_importance
            ));
        }
        content.push('\n');
    }

    // Recently modified
    content.push_str("## Recently Modified (last 7 days)\n\n");
    if recent_articles.is_empty() {
        content.push_str("*No articles modified in the last 7 days.*\n\n");
    } else {
        for r in &recent_articles {
            content.push_str(&format!(
                "- **{}** — modified {}\n",
                r.title,
                r.modified_at.format("%Y-%m-%d %H:%M UTC")
            ));
        }
        content.push('\n');
    }

    // Bridge articles
    content.push_str("## Bridge Articles (cross-domain)\n\n");
    if bridge_articles.is_empty() {
        content.push_str("*No cross-domain bridge articles detected.*\n\n");
    } else {
        for b in &bridge_articles {
            content.push_str(&format!(
                "- **{}** (id: `{}`) — spans: {}\n",
                b.title,
                b.id,
                b.domains.join(", ")
            ));
        }
        content.push('\n');
    }

    content.push_str(&format!(
        "---\n*Auto-generated by KB navigation maintenance on {date_str}.*\n"
    ));

    let meta = json!({
        "topology_map": true,
        "generated_at": Utc::now().to_rfc3339(),
        "article_count": article_count,
        "source_count":  source_count,
        "edge_count":    edge_count,
    });

    // ── 7. Upsert topology map article ────────────────────────────────────
    let existing_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM covalence.nodes
         WHERE  node_type   = 'article'
           AND  status      = 'active'
           AND  is_landmark = true
           AND  title LIKE 'KB Topology Map%'
         ORDER  BY created_at DESC
         LIMIT  1",
    )
    .fetch_optional(pool)
    .await
    .context("generate_topology_map: existing topology map query failed")?;

    let article_id: Uuid = match existing_id {
        Some(id) => {
            sqlx::query(
                "UPDATE covalence.nodes
                 SET    title       = $1,
                        content     = $2,
                        metadata    = $3,
                        modified_at = now()
                 WHERE  id = $4",
            )
            .bind(&title)
            .bind(&content)
            .bind(&meta)
            .bind(id)
            .execute(pool)
            .await
            .context("generate_topology_map: update failed")?;
            id
        }
        None => {
            let new_id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO covalence.nodes
                     (id, node_type, status, title, content, metadata,
                      pinned, is_landmark, created_at, modified_at)
                 VALUES
                     ($1, 'article', 'active', $2, $3, $4,
                      true, true, now(), now())",
            )
            .bind(new_id)
            .bind(&title)
            .bind(&content)
            .bind(&meta)
            .execute(pool)
            .await
            .context("generate_topology_map: insert failed")?;
            new_id
        }
    };

    tracing::info!(
        article_id   = %article_id,
        article_count,
        source_count,
        edge_count,
        domains      = domain_facets.len(),
        bridges      = bridge_articles.len(),
        hubs         = hub_articles.len(),
        "generate_topology_map: done"
    );

    Ok(TopologyMapResult {
        article_id,
        domain_count: domain_facets.len(),
        bridge_count: bridge_articles.len(),
        hub_count: hub_articles.len(),
    })
}

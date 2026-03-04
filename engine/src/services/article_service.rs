//! Article lifecycle — create, get, update, delete, split, merge, provenance (SPEC §5.4).

use chrono::Utc;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

use crate::errors::*;
use crate::graph::{GraphRepository, SqlGraphRepository};
use crate::models::article_state::{Active, Archived, TypedArticle};
use crate::models::*;

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct CreateArticleRequest {
    pub content: String,
    pub title: Option<String>,
    pub domain_path: Option<Vec<String>>,
    pub epistemic_type: Option<String>,
    #[schema(value_type = Option<Vec<String>>)]
    pub source_ids: Option<Vec<Uuid>>,
    #[schema(value_type = Object)]
    pub metadata: Option<serde_json::Value>,
    /// Functional facet — what does this knowledge DO? (covalence#92)
    pub facet_function: Option<Vec<String>>,
    /// Scope facet — abstraction level. (covalence#92)
    pub facet_scope: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct UpdateArticleRequest {
    pub content: Option<String>,
    pub title: Option<String>,
    pub domain_path: Option<Vec<String>>,
    #[allow(dead_code)]
    #[schema(value_type = Object)]
    pub metadata: Option<serde_json::Value>,
    pub pinned: Option<bool>,
    /// Functional facet — what does this knowledge DO? (covalence#92)
    pub facet_function: Option<Vec<String>>,
    /// Scope facet — abstraction level. (covalence#92)
    pub facet_scope: Option<Vec<String>>,
    /// OCC guard (covalence#98): when provided, the update is rejected with
    /// HTTP 409 if the article's current `version` does not match.  Omit to
    /// skip the version check (backwards-compatible default).
    pub expected_version: Option<i32>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct CompileRequest {
    #[schema(value_type = Vec<String>)]
    pub source_ids: Vec<Uuid>,
    pub title_hint: Option<String>,
    /// Optional compilation focus hint injected into the LLM prompt.
    /// Example: "focus on architectural decisions and trade-offs".
    pub compilation_focus: Option<String>,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct MergeRequest {
    #[schema(value_type = String)]
    pub article_id_a: Uuid,
    #[schema(value_type = String)]
    pub article_id_b: Uuid,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct ArticleResponse {
    #[schema(value_type = String)]
    pub id: Uuid,
    pub node_type: String,
    pub title: Option<String>,
    pub content: Option<String>,
    pub status: String,
    pub confidence: f32,
    pub epistemic_type: Option<String>,
    pub domain_path: Vec<String>,
    #[schema(value_type = Object)]
    pub metadata: serde_json::Value,
    pub version: i32,
    pub pinned: bool,
    pub usage_score: f32,
    pub contention_count: i64,
    /// Hex-encoded SHA-256 digest of `content` — computed at create/update time.
    /// Used for tamper detection and federation trust verification (covalence#78).
    pub content_hash: Option<String>,
    #[schema(value_type = String)]
    pub created_at: chrono::DateTime<Utc>,
    #[schema(value_type = String)]
    pub modified_at: chrono::DateTime<Utc>,
    /// `true` when newer unlinked sources exist in the same domain — the
    /// article may benefit from recompilation.  `None` when not computed
    /// (e.g. in list responses).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale: Option<bool>,
    /// Functional facet — what does this knowledge DO? (covalence#92)
    pub facet_function: Option<Vec<String>>,
    /// Scope facet — abstraction level. (covalence#92)
    pub facet_scope: Option<Vec<String>>,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct SplitResponse {
    #[schema(value_type = String)]
    pub original_id: Uuid,
    pub part_a: ArticleResponse,
    pub part_b: ArticleResponse,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct CompileJobResponse {
    #[schema(value_type = String)]
    pub job_id: Uuid,
    pub status: String,
}

pub struct ArticleService {
    pool: PgPool,
    graph: SqlGraphRepository,
    namespace: String,
}

impl ArticleService {
    pub fn new(pool: PgPool) -> Self {
        let graph = SqlGraphRepository::new(pool.clone());
        Self {
            pool,
            graph,
            namespace: "default".into(),
        }
    }

    /// Set the namespace for this service instance.
    pub fn with_namespace(mut self, ns: String) -> Self {
        self.namespace = ns;
        self
    }

    /// Create an article directly (agent-authored).
    pub async fn create(&self, req: CreateArticleRequest) -> AppResult<ArticleResponse> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let content_hash = hex::encode(Sha256::digest(req.content.as_bytes()));
        let size_tokens = (req.content.split_whitespace().count() as f64 / 0.75) as i32;
        let domain_path = req.domain_path.unwrap_or_default();
        let epistemic_type = req.epistemic_type.unwrap_or_else(|| "semantic".into());
        let metadata = req.metadata.unwrap_or(serde_json::json!({}));

        let first_consolidation_at =
            now + chrono::Duration::hours(crate::worker::consolidation::SCHEDULE_PASS_1_HOURS);
        sqlx::query(
            "INSERT INTO covalence.nodes \
             (id, node_type, title, content, status, epistemic_type, domain_path, \
              content_hash, size_tokens, metadata, confidence, namespace, \
              facet_function, facet_scope, \
              created_at, modified_at, accessed_at, \
              next_consolidation_at, consolidation_count) \
             VALUES ($1, 'article', $2, $3, 'active', $4, $5, $6, $7, $8, 0.5, $9, $10, $11, $12, $12, $12, \
                     $13, 0)",
        )
        .bind(id)
        .bind(&req.title)
        .bind(&req.content)
        .bind(&epistemic_type)
        .bind(&domain_path)
        .bind(&content_hash)
        .bind(size_tokens)
        .bind(&metadata)
        .bind(&self.namespace)
        .bind(&req.facet_function)
        .bind(&req.facet_scope)
        .bind(now)
        .bind(first_consolidation_at)
        .execute(&self.pool)
        .await?;

        // Create AGE vertex
        if let Err(e) = self
            .graph
            .create_vertex(id, NodeType::Article, serde_json::json!({}))
            .await
        {
            tracing::warn!(error = %e, "failed to create AGE vertex for article {id}");
        }

        // Create ORIGINATES edges if source_ids provided
        if let Some(source_ids) = req.source_ids {
            for source_id in source_ids {
                if let Err(e) = self
                    .graph
                    .create_edge(
                        source_id,
                        id,
                        EdgeType::Originates,
                        1.0,
                        "agent_explicit",
                        serde_json::json!({}),
                    )
                    .await
                {
                    tracing::warn!(error = %e, "failed to create ORIGINATES edge {source_id} → {id}");
                }
            }
        }

        // Queue embedding with contextual preamble (covalence#73).
        let preamble = format!(
            "[Context: {}. Source type: article. Domain: {}.]",
            req.title.as_deref().unwrap_or(""),
            domain_path.join("/"),
        );
        let embed_payload = serde_json::json!({ "embed_preamble": preamble });
        sqlx::query(
            "INSERT INTO covalence.slow_path_queue \
             (id, task_type, node_id, payload, priority, status) \
             VALUES ($1, 'embed', $2, $3, 3, 'pending')",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(&embed_payload)
        .execute(&self.pool)
        .await?;

        self.get(id).await
    }

    /// Get an article by ID.
    pub async fn get(&self, id: Uuid) -> AppResult<ArticleResponse> {
        let row = sqlx::query(
            "SELECT n.id, n.title, n.content, n.status, n.confidence, \
             n.epistemic_type, n.domain_path, n.metadata, n.version, n.pinned, n.usage_score, \
             n.content_hash, n.facet_function, n.facet_scope, n.created_at, n.modified_at, \
             (SELECT COUNT(*) FROM covalence.edges e \
              WHERE (e.source_node_id = n.id OR e.target_node_id = n.id) \
              AND e.edge_type IN ('CONTRADICTS', 'CONTENDS')) AS contention_count \
             FROM covalence.nodes n \
             WHERE n.id = $1 AND n.node_type = 'article' AND n.namespace = $2",
        )
        .bind(id)
        .bind(&self.namespace)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("article {id}")))?;

        let mut article = article_from_row(&row);

        // Populate staleness hint: true when a newer unlinked source shares a
        // domain_path element with this article.
        let is_stale: bool = sqlx::query_scalar(
            "SELECT EXISTS (
               SELECT 1
               FROM   covalence.nodes s
               WHERE  s.node_type    = 'source'
                 AND  s.status       = 'active'
                 AND  s.created_at   > $1
                 AND  s.domain_path IS NOT NULL
                 AND  EXISTS (
                       SELECT 1
                       FROM   covalence.nodes a
                       WHERE  a.id          = $2
                         AND  a.domain_path IS NOT NULL
                         AND  a.domain_path && s.domain_path
                 )
                 AND NOT EXISTS (
                       SELECT 1
                       FROM   covalence.edges e
                       WHERE  e.source_node_id = s.id
                         AND  e.target_node_id = $2
                         AND  e.edge_type      = 'ORIGINATES'
                 )
             )",
        )
        .bind(article.modified_at)
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false);

        article.stale = Some(is_stale);
        Ok(article)
    }

    /// Update an article — bumps version, updates modified_at.
    pub async fn update(&self, id: Uuid, req: UpdateArticleRequest) -> AppResult<ArticleResponse> {
        // Verify it exists; capture current state for preamble construction below.
        let existing = self.get(id).await?;

        let mut sets = vec![
            "modified_at = now()".to_string(),
            "version = version + 1".to_string(),
        ];
        #[allow(unused_assignments)]
        let mut bind_idx = 2u32; // $1 is the id

        if let Some(ref content) = req.content {
            let hash = hex::encode(Sha256::digest(content.as_bytes()));
            let tokens = (content.split_whitespace().count() as f64 / 0.75) as i32;
            sets.push(format!("content = ${bind_idx}"));
            bind_idx += 1;
            sets.push(format!("content_hash = '{hash}'"));
            sets.push(format!("size_tokens = {tokens}"));
        }
        if req.title.is_some() {
            sets.push(format!("title = ${bind_idx}"));
            bind_idx += 1;
        }
        if req.domain_path.is_some() {
            sets.push(format!("domain_path = ${bind_idx}"));
            bind_idx += 1;
            let _ = bind_idx; // consumed to suppress unused_assignments warning
        }
        if let Some(pinned) = req.pinned {
            sets.push(format!("pinned = {pinned}"));
        }

        // Build and execute dynamic UPDATE
        // For simplicity, use a straightforward approach with individual updates
        if let Some(ref content) = req.content {
            let hash = hex::encode(Sha256::digest(content.as_bytes()));
            let tokens = (content.split_whitespace().count() as f64 / 0.75) as i32;

            // OCC guard (covalence#98): when the caller supplies expected_version
            // we add it to the WHERE clause so a concurrent write that already
            // bumped the version causes rows_affected == 0 → 409 Conflict.
            let rows_affected = if let Some(expected) = req.expected_version {
                sqlx::query(
                    "UPDATE covalence.nodes \
                     SET content = $2, content_hash = $3, size_tokens = $4, \
                         version = version + 1, modified_at = now() \
                     WHERE id = $1 AND version = $5",
                )
                .bind(id)
                .bind(content)
                .bind(&hash)
                .bind(tokens)
                .bind(expected)
                .execute(&self.pool)
                .await?
                .rows_affected()
            } else {
                sqlx::query(
                    "UPDATE covalence.nodes \
                     SET content = $2, content_hash = $3, size_tokens = $4, \
                         version = version + 1, modified_at = now() \
                     WHERE id = $1",
                )
                .bind(id)
                .bind(content)
                .bind(&hash)
                .bind(tokens)
                .execute(&self.pool)
                .await?
                .rows_affected()
            };

            if rows_affected == 0 {
                return Err(AppError::Conflict(format!(
                    "article {id}: version conflict — expected version {}, \
                     article has been concurrently modified",
                    req.expected_version.unwrap(),
                )));
            }
        }
        if let Some(ref title) = req.title {
            sqlx::query("UPDATE covalence.nodes SET title = $2, modified_at = now() WHERE id = $1")
                .bind(id)
                .bind(title)
                .execute(&self.pool)
                .await?;
        }
        if let Some(ref domain_path) = req.domain_path {
            sqlx::query(
                "UPDATE covalence.nodes SET domain_path = $2, modified_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(domain_path)
            .execute(&self.pool)
            .await?;
        }
        if let Some(pinned) = req.pinned {
            sqlx::query(
                "UPDATE covalence.nodes SET pinned = $2, modified_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(pinned)
            .execute(&self.pool)
            .await?;
        }
        if let Some(ref facet_function) = req.facet_function {
            sqlx::query(
                "UPDATE covalence.nodes SET facet_function = $2, modified_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(facet_function)
            .execute(&self.pool)
            .await?;
        }
        if let Some(ref facet_scope) = req.facet_scope {
            sqlx::query(
                "UPDATE covalence.nodes SET facet_scope = $2, modified_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(facet_scope)
            .execute(&self.pool)
            .await?;
        }

        // Re-queue embedding if content changed (with contextual preamble, covalence#73).
        // Use updated field values if available, falling back to the pre-update snapshot.
        if req.content.is_some() {
            let effective_title = req
                .title
                .as_deref()
                .or(existing.title.as_deref())
                .unwrap_or("");
            let effective_domain = req
                .domain_path
                .as_ref()
                .unwrap_or(&existing.domain_path)
                .join("/");
            let preamble = format!(
                "[Context: {}. Source type: article. Domain: {}.]",
                effective_title, effective_domain,
            );
            let embed_payload = serde_json::json!({ "embed_preamble": preamble });
            sqlx::query(
                "INSERT INTO covalence.slow_path_queue \
                 (id, task_type, node_id, payload, priority, status) \
                 VALUES ($1, 'embed', $2, $3, 3, 'pending')",
            )
            .bind(Uuid::new_v4())
            .bind(id)
            .bind(&embed_payload)
            .execute(&self.pool)
            .await?;
        }

        self.get(id).await
    }

    /// Delete (archive) an article.
    #[allow(deprecated)]
    pub async fn delete(&self, id: Uuid) -> AppResult<()> {
        let result = sqlx::query(
            "UPDATE covalence.nodes SET status = 'archived', archived_at = now() \
             WHERE id = $1 AND node_type = 'article' AND status = 'active'",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(AppError::NotFound(format!("article {id}")));
        }

        // Remove from the live AGE graph — SQL edges are retained as history.
        if let Err(e) = self.graph.archive_vertex(id).await {
            tracing::warn!(
                article_id = %id,
                "article delete: archive_vertex failed (non-fatal): {e}"
            );
        }

        Ok(())
    }

    /// **Type-safe archive** — the typestate analogue of [`Self::delete`].
    ///
    /// Fetches the article, enforces at compile time that only an `Active`
    /// article can be passed to the archive path, persists the state change,
    /// and returns a `TypedArticle<Archived>`.
    ///
    /// # Errors
    /// * [`AppError::NotFound`] — article does not exist.
    /// * [`AppError::Conflict`] — article is already archived (status ≠ "active").
    ///
    /// # Example
    /// ```ignore
    /// let archived: TypedArticle<Archived> = service.typed_archive(id).await?;
    /// assert_eq!(archived.data.status, "archived");
    /// ```
    pub async fn typed_archive(&self, id: Uuid) -> AppResult<TypedArticle<Archived>> {
        let data = self.get(id).await?;

        // Construct the compile-time typed handle.  Returns None when the
        // article is not active, converting a would-be logic error into an
        // explicit Conflict result rather than silently no-oping.
        let active = TypedArticle::<Active>::from_response(data)
            .ok_or_else(|| AppError::Conflict(format!("article {id} is not active")))?;

        // Perform the type-safe state transition (purely in-memory).
        let archived: TypedArticle<Archived> = active.archive();

        // Persist the state change to the database.
        self.delete(id).await?;

        Ok(archived)
    }

    /// Split an article into two parts (SPEC §5.4).
    pub async fn split(&self, id: Uuid) -> AppResult<SplitResponse> {
        let original = self.get(id).await?;
        let content = original.content.as_deref().unwrap_or("");

        // Split at roughly the midpoint, on a paragraph boundary
        let mid = content.len() / 2;
        let split_point = content[mid..].find("\n\n").map(|p| mid + p).unwrap_or(mid);

        let part_a_content = &content[..split_point];
        let part_b_content = &content[split_point..].trim_start();

        // Create two new articles
        let part_a = self
            .create(CreateArticleRequest {
                content: part_a_content.to_string(),
                title: original.title.as_ref().map(|t| format!("{t} (Part 1)")),
                domain_path: Some(original.domain_path.clone()),
                epistemic_type: original.epistemic_type.clone(),
                source_ids: None,
                metadata: Some(original.metadata.clone()),
                facet_function: original.facet_function.clone(),
                facet_scope: original.facet_scope.clone(),
            })
            .await?;

        let part_b = self
            .create(CreateArticleRequest {
                content: part_b_content.to_string(),
                title: original.title.as_ref().map(|t| format!("{t} (Part 2)")),
                domain_path: Some(original.domain_path.clone()),
                epistemic_type: original.epistemic_type.clone(),
                source_ids: None,
                metadata: Some(original.metadata.clone()),
                facet_function: original.facet_function.clone(),
                facet_scope: original.facet_scope.clone(),
            })
            .await?;

        // Create SPLIT_INTO edges: original → each part
        let _ = self
            .graph
            .create_edge(
                id,
                part_a.id,
                EdgeType::SplitInto,
                1.0,
                "algorithmic",
                serde_json::json!({}),
            )
            .await;
        let _ = self
            .graph
            .create_edge(
                id,
                part_b.id,
                EdgeType::SplitInto,
                1.0,
                "algorithmic",
                serde_json::json!({}),
            )
            .await;

        // Inherit provenance edges from original
        if let Ok(edges) = self
            .graph
            .list_edges(
                id,
                TraversalDirection::Inbound,
                Some(&[EdgeType::Originates, EdgeType::CompiledFrom]),
                100,
                false, // active edges only
            )
            .await
        {
            for edge in edges {
                let _ = self
                    .graph
                    .create_edge(
                        edge.source_node_id,
                        part_a.id,
                        edge.edge_type,
                        edge.confidence,
                        "split_inherit",
                        serde_json::json!({}),
                    )
                    .await;
                let _ = self
                    .graph
                    .create_edge(
                        edge.source_node_id,
                        part_b.id,
                        edge.edge_type,
                        edge.confidence,
                        "split_inherit",
                        serde_json::json!({}),
                    )
                    .await;
            }
        }

        // Archive original
        self.delete(id).await?;

        Ok(SplitResponse {
            original_id: id,
            part_a,
            part_b,
        })
    }

    /// Merge two articles into one (SPEC §5.4).
    pub async fn merge(&self, req: MergeRequest) -> AppResult<ArticleResponse> {
        let a = self.get(req.article_id_a).await?;
        let b = self.get(req.article_id_b).await?;

        let merged_content = format!(
            "{}\n\n---\n\n{}",
            a.content.unwrap_or_default(),
            b.content.unwrap_or_default(),
        );

        let merged_title = match (&a.title, &b.title) {
            (Some(ta), Some(tb)) => Some(format!("{ta} + {tb}")),
            (Some(t), None) | (None, Some(t)) => Some(t.clone()),
            _ => None,
        };

        // Create merged article
        let merged = self
            .create(CreateArticleRequest {
                content: merged_content,
                title: merged_title,
                domain_path: Some({
                    let mut dp = a.domain_path.clone();
                    for p in &b.domain_path {
                        if !dp.contains(p) {
                            dp.push(p.clone());
                        }
                    }
                    dp
                }),
                epistemic_type: a.epistemic_type.clone(),
                source_ids: None,
                metadata: Some(a.metadata.clone()),
                facet_function: None,
                facet_scope: None,
            })
            .await?;

        // Create MERGED_FROM edges: merged ← each parent
        let _ = self
            .graph
            .create_edge(
                merged.id,
                req.article_id_a,
                EdgeType::MergedFrom,
                1.0,
                "algorithmic",
                serde_json::json!({}),
            )
            .await;
        let _ = self
            .graph
            .create_edge(
                merged.id,
                req.article_id_b,
                EdgeType::MergedFrom,
                1.0,
                "algorithmic",
                serde_json::json!({}),
            )
            .await;

        // Inherit provenance from both parents
        for parent_id in [req.article_id_a, req.article_id_b] {
            if let Ok(edges) = self
                .graph
                .list_edges(
                    parent_id,
                    TraversalDirection::Inbound,
                    Some(&[EdgeType::Originates, EdgeType::CompiledFrom]),
                    100,
                    false, // active edges only
                )
                .await
            {
                for edge in edges {
                    let _ = self
                        .graph
                        .create_edge(
                            edge.source_node_id,
                            merged.id,
                            edge.edge_type,
                            edge.confidence,
                            "merge_inherit",
                            serde_json::json!({}),
                        )
                        .await;
                }
            }
        }

        // Archive both originals
        self.delete(req.article_id_a).await?;
        self.delete(req.article_id_b).await?;

        Ok(merged)
    }

    /// Queue an async compile job (returns 202 + job ID).
    pub async fn compile(&self, req: CompileRequest) -> AppResult<CompileJobResponse> {
        let job_id = Uuid::new_v4();

        // node_id is NULL for compile tasks — the JSON payload carries
        // source_ids and title_hint.  Binding the JSON object to the UUID
        // node_id column caused a type-mismatch database_error (covalence#30).
        sqlx::query(
            "INSERT INTO covalence.slow_path_queue \
             (id, task_type, node_id, payload, priority, status) \
             VALUES ($1, 'compile', NULL, $2, 2, 'pending')",
        )
        .bind(job_id)
        .bind(serde_json::json!({
            "source_ids": req.source_ids,
            "title_hint": req.title_hint,
            "compilation_focus": req.compilation_focus,
        }))
        .execute(&self.pool)
        .await?;

        Ok(CompileJobResponse {
            job_id,
            status: "pending".into(),
        })
    }

    /// Get provenance chain for an article.
    pub async fn provenance(
        &self,
        id: Uuid,
        max_depth: Option<u32>,
    ) -> AppResult<Vec<ProvenanceLink>> {
        // Verify article exists
        let _ = self.get(id).await?;

        self.graph
            .get_provenance_chain(id, max_depth.unwrap_or(5))
            .await
            .map_err(AppError::Graph)
    }

    /// List articles with optional filters.
    pub async fn list(
        &self,
        limit: i64,
        cursor: Option<Uuid>,
        status: Option<&str>,
    ) -> AppResult<Vec<ArticleResponse>> {
        let limit = limit.min(100);
        let status_filter = status.unwrap_or("active");

        let rows = if let Some(cursor) = cursor {
            sqlx::query(
                "SELECT n.id, n.title, n.content, n.status, n.confidence, \
                 n.epistemic_type, n.domain_path, n.metadata, n.version, n.pinned, n.usage_score, \
                 n.content_hash, n.facet_function, n.facet_scope, n.created_at, n.modified_at, \
                 0::bigint AS contention_count \
                 FROM covalence.nodes n \
                 WHERE n.node_type = 'article' AND n.status = $1 AND n.namespace = $2 AND n.id > $3 \
                 ORDER BY n.created_at DESC LIMIT $4",
            )
            .bind(status_filter)
            .bind(&self.namespace)
            .bind(cursor)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT n.id, n.title, n.content, n.status, n.confidence, \
                 n.epistemic_type, n.domain_path, n.metadata, n.version, n.pinned, n.usage_score, \
                 n.content_hash, n.facet_function, n.facet_scope, n.created_at, n.modified_at, \
                 0::bigint AS contention_count \
                 FROM covalence.nodes n \
                 WHERE n.node_type = 'article' AND n.status = $1 AND n.namespace = $2 \
                 ORDER BY n.created_at DESC LIMIT $3",
            )
            .bind(status_filter)
            .bind(&self.namespace)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(rows.iter().map(article_from_row).collect())
    }
}

fn article_from_row(row: &PgRow) -> ArticleResponse {
    ArticleResponse {
        id: row.get("id"),
        node_type: "article".into(),
        title: row.get("title"),
        content: row.get("content"),
        stale: None,
        status: row.get("status"),
        confidence: row.get::<Option<f64>, _>("confidence").unwrap_or(0.5) as f32,
        epistemic_type: row.get("epistemic_type"),
        domain_path: row
            .get::<Option<Vec<String>>, _>("domain_path")
            .unwrap_or_default(),
        metadata: row.get::<serde_json::Value, _>("metadata"),
        version: row.get::<Option<i32>, _>("version").unwrap_or(1),
        pinned: row.get::<Option<bool>, _>("pinned").unwrap_or(false),
        usage_score: row.get::<Option<f64>, _>("usage_score").unwrap_or(0.0) as f32,
        contention_count: row.get::<Option<i64>, _>("contention_count").unwrap_or(0),
        content_hash: row.get("content_hash"),
        created_at: row.get("created_at"),
        modified_at: row.get("modified_at"),
        facet_function: row.get("facet_function"),
        facet_scope: row.get("facet_scope"),
    }
}

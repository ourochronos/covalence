//! SqlGraphRepository — pure-SQL implementation of GraphRepository (Phase 1).
//!
//! All graph operations run directly against `covalence.edges` and
//! `covalence.nodes`.  Apache AGE is not loaded, not referenced, and not
//! required by any code path in this module.
//!
//! AGE-specific trait methods (`archive_vertex`, `list_age_edge_refs`,
//! `delete_age_edge_by_internal_id`, `create_age_edge_for_sql`) are kept as
//! no-ops / empty returns for rollback safety and trait compatibility.
//! They will be removed when the trait is cleaned up in Phase 2.

use async_trait::async_trait;
use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

use super::repository::*;
use crate::models::*;

/// SQL-only graph repository.  No AGE extension required.
pub struct SqlGraphRepository {
    pool: PgPool,
}

impl SqlGraphRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl GraphRepository for SqlGraphRepository {
    // ── Vertex operations ─────────────────────────────────────────────────────

    /// No-op: vertices are rows in `covalence.nodes`; no separate vertex store
    /// exists in the SQL-only implementation.  Returns 0 (no AGE internal ID).
    async fn create_vertex(
        &self,
        _node_id: Uuid,
        _node_type: NodeType,
        _properties: serde_json::Value,
    ) -> GraphResult<i64> {
        Ok(0)
    }

    /// Delete all edges involving this node from `covalence.edges`.
    /// The caller is responsible for deleting the `covalence.nodes` row itself.
    async fn delete_vertex(&self, node_id: Uuid) -> GraphResult<()> {
        sqlx::query(
            "DELETE FROM covalence.edges \
             WHERE source_node_id = $1 OR target_node_id = $1",
        )
        .bind(node_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ── Edge operations ───────────────────────────────────────────────────────

    /// Create a typed edge — SQL INSERT only.  `age_id` is left NULL.
    /// `valid_from` defaults to the DB `now()` (same as `created_at`); `valid_to` is NULL
    /// (edge is immediately active).
    async fn create_edge(
        &self,
        from_id: Uuid,
        to_id: Uuid,
        edge_type: EdgeType,
        confidence: f32,
        created_by: &str,
        properties: serde_json::Value,
    ) -> GraphResult<Edge> {
        let edge_id = Uuid::new_v4();
        let now = chrono::Utc::now();

        sqlx::query(
            "INSERT INTO covalence.edges \
             (id, source_node_id, target_node_id, edge_type, \
              weight, confidence, metadata, created_at, created_by, valid_from) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $8)",
        )
        .bind(edge_id)
        .bind(from_id)
        .bind(to_id)
        .bind(edge_type.as_label())
        .bind(1.0f32)
        .bind(confidence)
        .bind(&properties)
        .bind(now)
        .bind(created_by)
        .execute(&self.pool)
        .await?;

        Ok(Edge {
            id: edge_id,
            age_id: None,
            source_node_id: from_id,
            target_node_id: to_id,
            edge_type,
            weight: 1.0,
            confidence,
            metadata: properties,
            created_at: now,
            created_by: Some(created_by.to_string()),
            valid_from: now,
            valid_to: None,
        })
    }

    /// Delete an edge from `covalence.edges` by its UUID.
    async fn delete_edge(&self, edge_id: Uuid) -> GraphResult<()> {
        let result = sqlx::query("DELETE FROM covalence.edges WHERE id = $1")
            .bind(edge_id)
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(GraphError::EdgeNotFound(edge_id));
        }
        Ok(())
    }

    /// Supersede an active edge by setting `valid_to = now()`.
    ///
    /// The row is preserved for historical queries.  Returns
    /// [`GraphError::EdgeNotFound`] if no active edge with `edge_id` exists
    /// (either the ID is unknown or the edge was already superseded).
    async fn supersede_edge(&self, edge_id: Uuid) -> GraphResult<()> {
        let result = sqlx::query(
            "UPDATE covalence.edges SET valid_to = now() WHERE id = $1 AND valid_to IS NULL",
        )
        .bind(edge_id)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(GraphError::EdgeNotFound(edge_id));
        }
        Ok(())
    }

    /// List edges from/to a node, optionally filtered by edge type.
    ///
    /// By default only active edges (`valid_to IS NULL`) are returned.
    /// Pass `include_superseded = true` to include superseded/expired edges.
    async fn list_edges(
        &self,
        node_id: Uuid,
        direction: TraversalDirection,
        edge_types: Option<&[EdgeType]>,
        limit: usize,
        include_superseded: bool,
    ) -> GraphResult<Vec<Edge>> {
        let mut sql = String::from(
            "SELECT id, age_id, source_node_id, target_node_id, edge_type, \
             weight, confidence, metadata, created_at, created_by, valid_from, valid_to \
             FROM covalence.edges WHERE ",
        );

        match direction {
            TraversalDirection::Outbound => sql.push_str("source_node_id = $1"),
            TraversalDirection::Inbound => sql.push_str("target_node_id = $1"),
            TraversalDirection::Both => {
                sql.push_str("(source_node_id = $1 OR target_node_id = $1)")
            }
        }

        if let Some(types) = edge_types {
            let labels: Vec<String> = types
                .iter()
                .map(|t| format!("'{}'", t.as_label()))
                .collect();
            sql.push_str(&format!(" AND edge_type IN ({})", labels.join(",")));
        }

        if !include_superseded {
            sql.push_str(" AND valid_to IS NULL");
        }

        sql.push_str(&format!(" ORDER BY created_at DESC LIMIT {limit}"));

        let rows = sqlx::query(&sql)
            .bind(node_id)
            .fetch_all(&self.pool)
            .await?;

        let mut edges = Vec::with_capacity(rows.len());
        for row in rows {
            edges.push(edge_from_row(&row)?);
        }
        Ok(edges)
    }

    // ── Traversal operations ──────────────────────────────────────────────────

    /// BFS neighborhood traversal via SQL recursive CTE on `covalence.edges`.
    async fn find_neighbors(
        &self,
        node_id: Uuid,
        edge_types: Option<&[EdgeType]>,
        direction: TraversalDirection,
        depth: u32,
        limit: usize,
    ) -> GraphResult<Vec<GraphNeighbor>> {
        let depth = depth.min(5); // hard cap per spec

        let direction_clause = match direction {
            TraversalDirection::Outbound => "e.source_node_id = t.node_id",
            TraversalDirection::Inbound => "e.target_node_id = t.node_id",
            TraversalDirection::Both => {
                "(e.source_node_id = t.node_id OR e.target_node_id = t.node_id)"
            }
        };

        let next_node = match direction {
            TraversalDirection::Outbound => "e.target_node_id",
            TraversalDirection::Inbound => "e.source_node_id",
            TraversalDirection::Both => {
                "CASE WHEN e.source_node_id = t.node_id \
                 THEN e.target_node_id ELSE e.source_node_id END"
            }
        };

        let type_filter = if let Some(types) = edge_types {
            let labels: Vec<String> = types
                .iter()
                .map(|t| format!("'{}'", t.as_label()))
                .collect();
            format!("AND e.edge_type IN ({})", labels.join(","))
        } else {
            String::new()
        };

        let sql = format!(
            "WITH RECURSIVE traversal AS (
                SELECT $1::uuid AS node_id, 0 AS depth, ARRAY[$1::uuid] AS path
                UNION ALL
                SELECT {next_node} AS node_id, t.depth + 1, t.path || {next_node}
                FROM traversal t
                JOIN covalence.edges e ON {direction_clause} {type_filter}
                WHERE t.depth < {depth}
                  AND NOT ({next_node} = ANY(t.path))
                  AND e.valid_to IS NULL
            )
            SELECT DISTINCT ON (t.node_id) t.node_id, t.depth,
                   e.id AS edge_id, e.age_id AS edge_age_id,
                   e.source_node_id, e.target_node_id, e.edge_type,
                   e.weight, e.confidence AS edge_confidence,
                   e.metadata AS edge_metadata, e.created_at AS edge_created_at,
                   e.created_by,
                   e.valid_from AS edge_valid_from, e.valid_to AS edge_valid_to
            FROM traversal t
            JOIN covalence.edges e ON (
                (e.source_node_id = t.node_id OR e.target_node_id = t.node_id)
                {type_filter}
                AND e.valid_to IS NULL
            )
            WHERE t.depth > 0
            ORDER BY t.node_id, t.depth
            LIMIT {limit}",
        );

        let rows = sqlx::query(&sql)
            .bind(node_id)
            .fetch_all(&self.pool)
            .await?;

        let mut neighbors = Vec::with_capacity(rows.len());
        for row in &rows {
            let neighbor_id: Uuid = row.try_get("node_id")?;
            let depth: i32 = row.try_get("depth")?;

            let node = self.fetch_node(neighbor_id).await?;
            let edge = edge_from_row(row)?;

            neighbors.push(GraphNeighbor {
                node,
                edge,
                depth: depth as u32,
            });
        }

        Ok(neighbors)
    }

    /// Walk the provenance chain for an article via SQL recursive CTE.
    async fn get_provenance_chain(
        &self,
        article_id: Uuid,
        max_depth: u32,
    ) -> GraphResult<Vec<ProvenanceLink>> {
        let provenance_labels = "'ORIGINATES','COMPILED_FROM','CONFIRMS','SUPERSEDES',\
                                 'DERIVES_FROM','MERGED_FROM','SPLIT_FROM','EXTENDS','ELABORATES'";
        let max_depth = max_depth.min(10);

        let sql = format!(
            "WITH RECURSIVE prov AS (
                SELECT e.source_node_id AS node_id, e.edge_type, e.confidence, 1 AS depth,
                       ARRAY[e.source_node_id] AS path
                FROM covalence.edges e
                WHERE e.target_node_id = $1
                  AND e.edge_type IN ({provenance_labels})
                  AND e.valid_to IS NULL
                UNION ALL
                SELECT e.source_node_id, e.edge_type, e.confidence, p.depth + 1,
                       p.path || e.source_node_id
                FROM prov p
                JOIN covalence.edges e ON e.target_node_id = p.node_id
                WHERE e.edge_type IN ({provenance_labels})
                  AND e.valid_to IS NULL
                  AND p.depth < {max_depth}
                  AND NOT (e.source_node_id = ANY(p.path))
            )
            SELECT DISTINCT ON (node_id) node_id, edge_type, confidence, depth
            FROM prov
            ORDER BY node_id, depth"
        );

        let rows = sqlx::query(&sql)
            .bind(article_id)
            .fetch_all(&self.pool)
            .await?;

        let mut links = Vec::with_capacity(rows.len());
        for row in &rows {
            let node_id: Uuid = row.try_get("node_id")?;
            let edge_type_str: String = row.try_get("edge_type")?;
            let confidence: f64 = row.try_get("confidence")?;
            let depth: i32 = row.try_get("depth")?;

            let edge_type: EdgeType = edge_type_str
                .parse()
                .map_err(|e: String| GraphError::QueryFailed(e))?;

            let node = self.fetch_node(node_id).await?;

            links.push(ProvenanceLink {
                source_node: node,
                edge_type,
                confidence: confidence as f32,
                depth: depth as u32,
            });
        }

        Ok(links)
    }

    /// Find CONTRADICTS/CONTENDS edges involving a node.
    async fn find_contradictions(&self, node_id: Uuid) -> GraphResult<Vec<Edge>> {
        self.list_edges(
            node_id,
            TraversalDirection::Both,
            Some(&[EdgeType::Contradicts, EdgeType::Contends]),
            100,
            false, // active edges only
        )
        .await
    }

    /// Get chain tips: active articles with no incoming SUPERSEDES edge.
    async fn get_chain_tips(&self, limit: usize) -> GraphResult<Vec<Node>> {
        let sql = format!("SELECT * FROM covalence.get_chain_tips() LIMIT {limit}");
        let rows = sqlx::query(&sql).fetch_all(&self.pool).await?;

        let mut nodes = Vec::with_capacity(rows.len());
        for row in &rows {
            let id: Uuid = row.try_get("id")?;
            nodes.push(self.fetch_node(id).await?);
        }
        Ok(nodes)
    }

    // ── Bulk / utility ────────────────────────────────────────────────────────

    /// Count edges in `covalence.edges`.
    async fn count_edges(&self) -> GraphResult<i64> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM covalence.edges")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    /// Count active nodes in `covalence.nodes`.
    async fn count_vertices(&self) -> GraphResult<i64> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM covalence.nodes WHERE status = 'active'")
                .fetch_one(&self.pool)
                .await?;
        Ok(count)
    }

    /// Return (sql_count, sql_count) — always in sync by definition.
    async fn verify_sync(&self) -> GraphResult<(i64, i64)> {
        let count = self.count_edges().await?;
        Ok((count, count))
    }

    // ── AGE-specific stubs (deprecated, no-op in SQL impl) ───────────────────

    /// No-op: no separate AGE graph to clean.  SQL edges are preserved as
    /// history.  Call this when archiving nodes.
    async fn archive_vertex(&self, _node_id: Uuid) -> GraphResult<()> {
        Ok(())
    }

    /// No-op: no AGE edges exist.  Returns an empty vec.
    async fn list_age_edge_refs(&self) -> GraphResult<Vec<(i64, Option<Uuid>)>> {
        Ok(vec![])
    }

    /// No-op: no AGE edges to delete.
    async fn delete_age_edge_by_internal_id(&self, _age_internal_id: i64) -> GraphResult<()> {
        Ok(())
    }

    /// No-op: no AGE graph.  Returns `Ok(None)`.
    async fn create_age_edge_for_sql(
        &self,
        _edge_id: Uuid,
        _from_id: Uuid,
        _to_id: Uuid,
        _edge_type: EdgeType,
        _confidence: f32,
    ) -> GraphResult<Option<i64>> {
        Ok(None)
    }
}

// ── Helper methods ────────────────────────────────────────────────────────────

impl SqlGraphRepository {
    /// Fetch a full Node from the relational table.
    async fn fetch_node(&self, node_id: Uuid) -> GraphResult<Node> {
        let row = sqlx::query(
            "SELECT id, age_id, node_type, title, content, status, \
             confidence, \
             epistemic_type, domain_path, metadata, \
             source_type, reliability, content_hash, fingerprint, size_tokens, \
             pinned, version, usage_score, created_at, modified_at, accessed_at, archived_at \
             FROM covalence.nodes WHERE id = $1",
        )
        .bind(node_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(GraphError::NodeNotFound(node_id))?;

        node_from_row(&row)
    }
}

// ── Row mapping helpers ───────────────────────────────────────────────────────

fn node_from_row(row: &PgRow) -> GraphResult<Node> {
    let node_type_str: String = row.try_get("node_type")?;
    let status_str: String = row.try_get("status")?;

    let node_type = match node_type_str.as_str() {
        "article" => NodeType::Article,
        "source" => NodeType::Source,
        "session" => NodeType::Session,
        "entity" => NodeType::Entity,
        other => {
            return Err(GraphError::QueryFailed(format!(
                "unknown node_type: {other}"
            )));
        }
    };

    let status = match status_str.as_str() {
        "active" => NodeStatus::Active,
        "archived" => NodeStatus::Archived,
        "tombstone" => NodeStatus::Tombstone,
        other => return Err(GraphError::QueryFailed(format!("unknown status: {other}"))),
    };

    let epistemic_str: Option<String> = row.try_get("epistemic_type")?;
    let epistemic_type = epistemic_str.as_deref().map(|s| match s {
        "semantic" => EpistemicType::Semantic,
        "episodic" => EpistemicType::Episodic,
        "procedural" => EpistemicType::Procedural,
        "declarative" => EpistemicType::Declarative,
        _ => EpistemicType::Semantic,
    });

    Ok(Node {
        id: row.try_get("id")?,
        age_id: row.try_get("age_id")?,
        node_type,
        title: row.try_get("title")?,
        content: row.try_get("content")?,
        status,
        confidence: Confidence {
            overall: row.try_get::<Option<f64>, _>("confidence")?.unwrap_or(0.5) as f32,
            source: 0.5,
            method: 0.5,
            consistency: 1.0,
            freshness: 1.0,
            corroboration: 0.0,
            applicability: 1.0f32,
        },
        epistemic_type,
        domain_path: row
            .try_get::<Option<Vec<String>>, _>("domain_path")?
            .unwrap_or_default(),
        metadata: row.try_get::<serde_json::Value, _>("metadata")?,
        source_type: row.try_get("source_type")?,
        reliability: row
            .try_get::<Option<f64>, _>("reliability")?
            .map(|v| v as f32),
        content_hash: row.try_get("content_hash")?,
        fingerprint: row.try_get("fingerprint")?,
        size_tokens: row.try_get("size_tokens")?,
        pinned: row.try_get::<Option<bool>, _>("pinned")?.unwrap_or(false),
        version: row.try_get::<Option<i32>, _>("version")?.unwrap_or(1),
        usage_score: row.try_get::<Option<f64>, _>("usage_score")?.unwrap_or(0.5) as f32,
        created_at: row.try_get("created_at")?,
        modified_at: row.try_get("modified_at")?,
        accessed_at: row.try_get("accessed_at")?,
        archived_at: row.try_get("archived_at")?,
    })
}

fn edge_from_row(row: &PgRow) -> GraphResult<Edge> {
    let edge_type_str: String = row.try_get("edge_type")?;
    let edge_type: EdgeType = edge_type_str
        .parse()
        .map_err(|e: String| GraphError::QueryFailed(e))?;

    let created_at: chrono::DateTime<chrono::Utc> = row
        .try_get("created_at")
        .or_else(|_| row.try_get("edge_created_at"))?;

    // valid_from falls back to created_at if the column isn't present in the
    // result set (e.g. older queries that don't select it explicitly).
    let valid_from: chrono::DateTime<chrono::Utc> = row
        .try_get("valid_from")
        .or_else(|_| row.try_get("edge_valid_from"))
        .unwrap_or(created_at);

    let valid_to: Option<chrono::DateTime<chrono::Utc>> = row
        .try_get("valid_to")
        .or_else(|_| row.try_get("edge_valid_to"))
        .unwrap_or(None);

    Ok(Edge {
        id: row.try_get("id").or_else(|_| row.try_get("edge_id"))?,
        age_id: row
            .try_get("age_id")
            .or_else(|_| row.try_get("edge_age_id"))
            .ok(),
        source_node_id: row.try_get("source_node_id")?,
        target_node_id: row.try_get("target_node_id")?,
        edge_type,
        weight: row.try_get::<Option<f64>, _>("weight")?.unwrap_or(1.0) as f32,
        confidence: row
            .try_get::<Option<f64>, _>("confidence")
            .or_else(|_| row.try_get::<Option<f64>, _>("edge_confidence"))?
            .unwrap_or(1.0) as f32,
        metadata: row
            .try_get::<serde_json::Value, _>("metadata")
            .or_else(|_| row.try_get::<serde_json::Value, _>("edge_metadata"))
            .unwrap_or(serde_json::json!({})),
        created_at,
        created_by: row.try_get("created_by").ok(),
        valid_from,
        valid_to,
    })
}

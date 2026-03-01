//! AgeGraphRepository — Apache AGE implementation of GraphRepository (SPEC §4.4).
//!
//! Translates domain graph operations into AGE Cypher queries via sqlx.
//! agtype is projected to native SQL types at the query boundary.
//! All writes are dual-write: AGE graph + covalence.edges SQL mirror.

use async_trait::async_trait;
use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

use crate::models::*;
use super::repository::*;

/// AGE-backed graph repository.
pub struct AgeGraphRepository {
    pool: PgPool,
    graph_name: String,
}

impl AgeGraphRepository {
    pub fn new(pool: PgPool, graph_name: impl Into<String>) -> Self {
        Self {
            pool,
            graph_name: graph_name.into(),
        }
    }

    /// Build the AGE preamble SQL that must precede every Cypher query.
    /// AGE requires LOAD + SET search_path per session/transaction.
    fn age_preamble(&self) -> String {
        "LOAD 'age'; SET search_path = ag_catalog, \"$user\", public;".to_string()
    }

    /// Execute a Cypher query wrapped in ag_catalog.cypher() and return raw rows.
    /// This is the single point where AGE/agtype is touched — all callers
    /// get native PgRows with projected SQL types.
    async fn cypher_query(
        &self,
        cypher: &str,
        params: Option<&str>,
        columns: &str, // e.g. "(id agtype, name agtype)"
    ) -> GraphResult<Vec<PgRow>> {
        let preamble = self.age_preamble();
        let sql = if let Some(p) = params {
            format!(
                "{preamble} SELECT * FROM cypher('{graph}', $$ {cypher} $$, '{p}') AS {columns};",
                graph = self.graph_name,
            )
        } else {
            format!(
                "{preamble} SELECT * FROM cypher('{graph}', $$ {cypher} $$) AS {columns};",
                graph = self.graph_name,
            )
        };

        sqlx::query(&sql)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| GraphError::Age(format!("cypher query failed: {e}")))
    }

    /// Execute a Cypher statement that returns no rows.
    async fn cypher_exec(
        &self,
        cypher: &str,
        params: Option<&str>,
    ) -> GraphResult<()> {
        let preamble = self.age_preamble();
        let sql = if let Some(p) = params {
            format!(
                "{preamble} SELECT * FROM cypher('{graph}', $$ {cypher} $$, '{p}') AS (result agtype);",
                graph = self.graph_name,
            )
        } else {
            format!(
                "{preamble} SELECT * FROM cypher('{graph}', $$ {cypher} $$) AS (result agtype);",
                graph = self.graph_name,
            )
        };

        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .map_err(|e| GraphError::Age(format!("cypher exec failed: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl GraphRepository for AgeGraphRepository {
    async fn create_vertex(
        &self,
        node_id: Uuid,
        node_type: NodeType,
        properties: serde_json::Value,
    ) -> GraphResult<i64> {
        let label = node_type.age_label();
        let node_id_str = node_id.to_string();

        // Create vertex in AGE with the node UUID as a property
        let cypher = format!(
            "CREATE (n:{label} {{node_id: '{node_id_str}'}}) RETURN id(n)"
        );
        let rows = self.cypher_query(&cypher, None, "(age_id agtype)").await?;

        let age_id: i64 = if let Some(row) = rows.first() {
            // AGE returns agtype — we need to extract the integer.
            // agtype integers can be read as text and parsed.
            let raw: String = row.try_get("age_id")
                .map_err(|e| GraphError::Age(format!("failed to read age_id: {e}")))?;
            raw.trim_matches('"').parse::<i64>()
                .map_err(|e| GraphError::Age(format!("failed to parse age_id: {e}")))?
        } else {
            return Err(GraphError::Age("vertex creation returned no rows".into()));
        };

        // Update the relational node with the AGE vertex ID
        sqlx::query("UPDATE covalence.nodes SET age_id = $1 WHERE id = $2")
            .bind(age_id)
            .bind(node_id)
            .execute(&self.pool)
            .await?;

        Ok(age_id)
    }

    async fn delete_vertex(&self, node_id: Uuid) -> GraphResult<()> {
        let node_id_str = node_id.to_string();

        // Delete all edges connected to this vertex first, then the vertex
        let cypher = format!(
            "MATCH (n {{node_id: '{node_id_str}'}}) DETACH DELETE n"
        );
        self.cypher_exec(&cypher, None).await?;

        // Clean up SQL edges mirror
        sqlx::query(
            "DELETE FROM covalence.edges WHERE source_node_id = $1 OR target_node_id = $1"
        )
            .bind(node_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn create_edge(
        &self,
        from_id: Uuid,
        to_id: Uuid,
        edge_type: EdgeType,
        confidence: f32,
        created_by: &str,
        properties: serde_json::Value,
    ) -> GraphResult<Edge> {
        let label = edge_type.as_label();
        let from_str = from_id.to_string();
        let to_str = to_id.to_string();
        let edge_id = Uuid::new_v4();

        // Create edge in AGE
        let cypher = format!(
            "MATCH (a {{node_id: '{from_str}'}}), (b {{node_id: '{to_str}'}}) \
             CREATE (a)-[e:{label} {{edge_id: '{edge_id}', confidence: {confidence}}}]->(b) \
             RETURN id(e)"
        );
        let rows = self.cypher_query(&cypher, None, "(age_id agtype)").await?;

        let age_id: Option<i64> = if let Some(row) = rows.first() {
            let raw: String = row.try_get("age_id")
                .map_err(|e| GraphError::Age(format!("failed to read edge age_id: {e}")))?;
            raw.trim_matches('"').parse::<i64>().ok()
        } else {
            None
        };

        // Write to SQL edges mirror (same transaction would require a TX — for now, best effort)
        let now = chrono::Utc::now();
        sqlx::query(
            "INSERT INTO covalence.edges (id, age_id, source_node_id, target_node_id, edge_type, \
             weight, confidence, metadata, created_at, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"
        )
            .bind(edge_id)
            .bind(age_id)
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
            age_id,
            source_node_id: from_id,
            target_node_id: to_id,
            edge_type,
            weight: 1.0,
            confidence,
            metadata: properties,
            created_at: now,
            created_by: Some(created_by.to_string()),
        })
    }

    async fn delete_edge(&self, edge_id: Uuid) -> GraphResult<()> {
        // Look up the AGE edge ID from SQL mirror
        let row = sqlx::query(
            "SELECT age_id FROM covalence.edges WHERE id = $1"
        )
            .bind(edge_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(GraphError::EdgeNotFound(edge_id))?;

        let age_id: Option<i64> = row.try_get("age_id").ok();

        // Delete from AGE if we have the internal ID
        if let Some(aid) = age_id {
            let cypher = format!(
                "MATCH ()-[e]->() WHERE id(e) = {aid} DELETE e"
            );
            self.cypher_exec(&cypher, None).await?;
        }

        // Delete from SQL mirror
        sqlx::query("DELETE FROM covalence.edges WHERE id = $1")
            .bind(edge_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn list_edges(
        &self,
        node_id: Uuid,
        direction: TraversalDirection,
        edge_types: Option<&[EdgeType]>,
        limit: usize,
    ) -> GraphResult<Vec<Edge>> {
        // Use SQL mirror for this — no Cypher needed for flat edge lookups
        let mut sql = String::from(
            "SELECT id, age_id, source_node_id, target_node_id, edge_type, \
             weight, confidence, metadata, created_at, created_by \
             FROM covalence.edges WHERE "
        );

        match direction {
            TraversalDirection::Outbound => sql.push_str("source_node_id = $1"),
            TraversalDirection::Inbound => sql.push_str("target_node_id = $1"),
            TraversalDirection::Both => sql.push_str("(source_node_id = $1 OR target_node_id = $1)"),
        }

        if let Some(types) = edge_types {
            let labels: Vec<String> = types.iter().map(|t| format!("'{}'", t.as_label())).collect();
            sql.push_str(&format!(" AND edge_type IN ({})", labels.join(",")));
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

    async fn find_neighbors(
        &self,
        node_id: Uuid,
        edge_types: Option<&[EdgeType]>,
        direction: TraversalDirection,
        depth: u32,
        limit: usize,
    ) -> GraphResult<Vec<GraphNeighbor>> {
        // For v0, use SQL recursive CTE on edges mirror for reliability.
        // AGE Cypher depth>2 has known performance issues (SPEC §11.3).
        // This avoids the AGE deep-traversal risk entirely.
        let depth = depth.min(5); // hard cap per spec

        let direction_clause = match direction {
            TraversalDirection::Outbound => "e.source_node_id = t.node_id",
            TraversalDirection::Inbound => "e.target_node_id = t.node_id",
            TraversalDirection::Both => "(e.source_node_id = t.node_id OR e.target_node_id = t.node_id)",
        };

        let next_node = match direction {
            TraversalDirection::Outbound => "e.target_node_id",
            TraversalDirection::Inbound => "e.source_node_id",
            TraversalDirection::Both => {
                "CASE WHEN e.source_node_id = t.node_id THEN e.target_node_id ELSE e.source_node_id END"
            }
        };

        let type_filter = if let Some(types) = edge_types {
            let labels: Vec<String> = types.iter().map(|t| format!("'{}'", t.as_label())).collect();
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
            )
            SELECT DISTINCT ON (t.node_id) t.node_id, t.depth,
                   e.id AS edge_id, e.age_id AS edge_age_id,
                   e.source_node_id, e.target_node_id, e.edge_type,
                   e.weight, e.confidence AS edge_confidence,
                   e.metadata AS edge_metadata, e.created_at AS edge_created_at,
                   e.created_by
            FROM traversal t
            JOIN covalence.edges e ON (
                (e.source_node_id = t.node_id OR e.target_node_id = t.node_id)
                {type_filter}
            )
            WHERE t.depth > 0
            ORDER BY t.node_id, t.depth
            LIMIT {limit}",
        );

        let rows = sqlx::query(&sql)
            .bind(node_id)
            .fetch_all(&self.pool)
            .await?;

        // For each discovered neighbor, fetch the full node
        let mut neighbors = Vec::with_capacity(rows.len());
        for row in &rows {
            let neighbor_id: Uuid = row.try_get("node_id")?;
            let depth: i32 = row.try_get("depth")?;

            // Fetch full node from relational table
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

    async fn get_provenance_chain(
        &self,
        article_id: Uuid,
        max_depth: u32,
    ) -> GraphResult<Vec<ProvenanceLink>> {
        // Walk provenance edges via SQL recursive CTE
        let provenance_labels = "'ORIGINATES','COMPILED_FROM','CONFIRMS','SUPERSEDES','DERIVES_FROM','MERGED_FROM','SPLIT_FROM','EXTENDS','ELABORATES'";
        let max_depth = max_depth.min(10);

        let sql = format!(
            "WITH RECURSIVE prov AS (
                SELECT e.source_node_id AS node_id, e.edge_type, e.confidence, 1 AS depth,
                       ARRAY[e.source_node_id] AS path
                FROM covalence.edges e
                WHERE e.target_node_id = $1
                  AND e.edge_type IN ({provenance_labels})
                UNION ALL
                SELECT e.source_node_id, e.edge_type, e.confidence, p.depth + 1,
                       p.path || e.source_node_id
                FROM prov p
                JOIN covalence.edges e ON e.target_node_id = p.node_id
                WHERE e.edge_type IN ({provenance_labels})
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
            let confidence: f32 = row.try_get("confidence")?;
            let depth: i32 = row.try_get("depth")?;

            let edge_type: EdgeType = edge_type_str.parse()
                .map_err(|e: String| GraphError::Age(e))?;

            let node = self.fetch_node(node_id).await?;

            links.push(ProvenanceLink {
                source_node: node,
                edge_type,
                confidence,
                depth: depth as u32,
            });
        }

        Ok(links)
    }

    async fn find_contradictions(&self, node_id: Uuid) -> GraphResult<Vec<Edge>> {
        self.list_edges(
            node_id,
            TraversalDirection::Both,
            Some(&[EdgeType::Contradicts, EdgeType::Contends]),
            100,
        ).await
    }

    async fn get_chain_tips(&self, limit: usize) -> GraphResult<Vec<Node>> {
        // Use the SQL function from 001 migration
        let sql = format!(
            "SELECT * FROM covalence.get_chain_tips() LIMIT {limit}"
        );
        let rows = sqlx::query(&sql)
            .fetch_all(&self.pool)
            .await?;

        let mut nodes = Vec::with_capacity(rows.len());
        for row in &rows {
            let id: Uuid = row.try_get("id")?;
            nodes.push(self.fetch_node(id).await?);
        }
        Ok(nodes)
    }

    async fn count_edges(&self) -> GraphResult<i64> {
        let rows = self.cypher_query(
            "MATCH ()-[e]->() RETURN count(e)",
            None,
            "(cnt agtype)",
        ).await?;

        if let Some(row) = rows.first() {
            let raw: String = row.try_get("cnt")
                .map_err(|e| GraphError::Age(format!("count_edges: {e}")))?;
            raw.trim_matches('"').parse::<i64>()
                .map_err(|e| GraphError::Age(format!("count_edges parse: {e}")))
        } else {
            Ok(0)
        }
    }

    async fn count_vertices(&self) -> GraphResult<i64> {
        let rows = self.cypher_query(
            "MATCH (n) RETURN count(n)",
            None,
            "(cnt agtype)",
        ).await?;

        if let Some(row) = rows.first() {
            let raw: String = row.try_get("cnt")
                .map_err(|e| GraphError::Age(format!("count_vertices: {e}")))?;
            raw.trim_matches('"').parse::<i64>()
                .map_err(|e| GraphError::Age(format!("count_vertices parse: {e}")))
        } else {
            Ok(0)
        }
    }

    async fn verify_sync(&self) -> GraphResult<(i64, i64)> {
        let age_count = self.count_edges().await?;

        let sql_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM covalence.edges")
            .fetch_one(&self.pool)
            .await?;

        Ok((age_count, sql_count))
    }
}

// ── Helper methods (not part of the trait) ──────────────────────

impl AgeGraphRepository {
    /// Fetch a full Node from the relational table.
    async fn fetch_node(&self, node_id: Uuid) -> GraphResult<Node> {
        let row = sqlx::query(
            "SELECT id, age_id, node_type, title, content, status, \
             confidence_overall, confidence_source, confidence_method, \
             confidence_consistency, confidence_freshness, confidence_corroboration, \
             confidence_applicability, epistemic_type, domain_path, metadata, \
             source_type, reliability, content_hash, fingerprint, size_tokens, \
             pinned, version, usage_score, created_at, modified_at, accessed_at, archived_at \
             FROM covalence.nodes WHERE id = $1"
        )
            .bind(node_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(GraphError::NodeNotFound(node_id))?;

        node_from_row(&row)
    }
}

// ── Row mapping helpers ─────────────────────────────────────────

fn node_from_row(row: &PgRow) -> GraphResult<Node> {
    let node_type_str: String = row.try_get("node_type")?;
    let status_str: String = row.try_get("status")?;

    let node_type = match node_type_str.as_str() {
        "article" => NodeType::Article,
        "source" => NodeType::Source,
        "session" => NodeType::Session,
        "entity" => NodeType::Entity,
        other => return Err(GraphError::QueryFailed(format!("unknown node_type: {other}"))),
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
            overall: row.try_get::<Option<f64>, _>("confidence_overall")?.unwrap_or(0.5) as f32,
            source: row.try_get::<Option<f64>, _>("confidence_source")?.unwrap_or(0.5) as f32,
            method: row.try_get::<Option<f64>, _>("confidence_method")?.unwrap_or(0.5) as f32,
            consistency: row.try_get::<Option<f64>, _>("confidence_consistency")?.unwrap_or(1.0) as f32,
            freshness: row.try_get::<Option<f64>, _>("confidence_freshness")?.unwrap_or(1.0) as f32,
            corroboration: row.try_get::<Option<f64>, _>("confidence_corroboration")?.unwrap_or(0.0) as f32,
            applicability: row.try_get::<Option<f64>, _>("confidence_applicability")?.unwrap_or(1.0) as f32,
        },
        epistemic_type,
        domain_path: row.try_get::<Option<Vec<String>>, _>("domain_path")?.unwrap_or_default(),
        metadata: row.try_get::<serde_json::Value, _>("metadata")?,
        source_type: row.try_get("source_type")?,
        reliability: row.try_get::<Option<f64>, _>("reliability")?.map(|v| v as f32),
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
    let edge_type: EdgeType = edge_type_str.parse()
        .map_err(|e: String| GraphError::QueryFailed(e))?;

    Ok(Edge {
        id: row.try_get("id").or_else(|_| row.try_get("edge_id"))?,
        age_id: row.try_get("age_id").or_else(|_| row.try_get("edge_age_id")).ok(),
        source_node_id: row.try_get("source_node_id")?,
        target_node_id: row.try_get("target_node_id")?,
        edge_type,
        weight: row.try_get::<Option<f32>, _>("weight")?.unwrap_or(1.0),
        confidence: row.try_get::<Option<f32>, _>("confidence")
            .or_else(|_| row.try_get::<Option<f32>, _>("edge_confidence"))?.unwrap_or(1.0),
        metadata: row.try_get::<serde_json::Value, _>("metadata")
            .or_else(|_| row.try_get::<serde_json::Value, _>("edge_metadata"))
            .unwrap_or(serde_json::json!({})),
        created_at: row.try_get("created_at")
            .or_else(|_| row.try_get("edge_created_at"))?,
        created_by: row.try_get("created_by").ok(),
    })
}

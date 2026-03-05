//! Edge operations — create, delete, list, neighborhood traversal.

use sqlx::{PgPool, Row as _};
use uuid::Uuid;

use crate::errors::*;
use crate::graph::{GraphRepository, SqlGraphRepository};
use crate::models::*;

#[derive(Debug, serde::Deserialize)]
pub struct CreateEdgeRequest {
    pub from_node_id: Uuid,
    pub to_node_id: Uuid,
    pub label: String,
    pub confidence: Option<f32>,
    pub method: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, serde::Deserialize, Default)]
pub struct NeighborhoodParams {
    pub depth: Option<u32>,
    pub direction: Option<String>,
    pub labels: Option<String>, // comma-separated
    pub limit: Option<usize>,
}

pub struct EdgeService {
    #[allow(dead_code)]
    pool: PgPool,
    graph: SqlGraphRepository,
}

impl EdgeService {
    pub fn new(pool: PgPool) -> Self {
        let graph = SqlGraphRepository::new(pool.clone());
        Self { pool, graph }
    }

    /// Create a typed edge between two nodes.
    ///
    /// Enforces inference rules on write:
    /// * **Symmetric-edge enforcement** (covalence#99, covalence#173): when an
    ///   edge whose [`EdgeType::is_symmetric`] returns `true` (e.g. CONTRADICTS,
    ///   CONTRADICTS_CLAIM) is written, the inverse edge is automatically created
    ///   if it does not already exist.  Both directions use
    ///   `ON CONFLICT … DO NOTHING` so the operation is idempotent: writing an
    ///   edge that already exists returns the existing edge rather than an error.
    pub async fn create(&self, req: CreateEdgeRequest) -> AppResult<Edge> {
        let edge_type: EdgeType = req
            .label
            .parse()
            .map_err(|e: String| AppError::BadRequest(e))?;

        let method = req.method.as_deref().unwrap_or("agent_explicit");
        let confidence = req.confidence.unwrap_or(1.0);
        let props = serde_json::json!({"notes": req.notes});

        if edge_type.is_symmetric() {
            // Symmetric types get an idempotent upsert for the primary direction
            // plus an automatic inverse.
            self.ensure_symmetric_edge(
                req.from_node_id,
                req.to_node_id,
                edge_type,
                confidence,
                method,
                &props,
            )
            .await
        } else {
            self.graph
                .create_edge(
                    req.from_node_id,
                    req.to_node_id,
                    edge_type,
                    confidence,
                    method,
                    props,
                )
                .await
                .map_err(AppError::Graph)
        }
    }

    /// Insert a symmetric edge pair and return the primary (from→to) edge.
    ///
    /// Both the primary insert and the inverse insert use
    /// `ON CONFLICT … DO NOTHING` so the helper is idempotent: calling it when
    /// either direction already exists is a no-op for that direction.
    ///
    /// This generalises the former `upsert_contradicts_edge` / raw-inverse-INSERT
    /// pair so that any [`EdgeType::is_symmetric`] type gets the same treatment
    /// without duplication (covalence#173 wave 5 change 2.2).
    async fn ensure_symmetric_edge(
        &self,
        from_id: Uuid,
        to_id: Uuid,
        edge_type: EdgeType,
        confidence: f32,
        created_by: &str,
        props: &serde_json::Value,
    ) -> AppResult<Edge> {
        let label = edge_type.as_label();
        let causal_weight = edge_type.causal_weight();

        // ── Primary direction ─────────────────────────────────────────────────
        // Attempt insert; silently skip if the edge already exists (e.g. the
        // inverse was written first and already created this direction).
        let insert_sql = format!(
            "INSERT INTO covalence.edges \
             (id, source_node_id, target_node_id, edge_type, \
              weight, confidence, causal_weight, metadata, created_by, valid_from) \
             VALUES (gen_random_uuid(), $1, $2, '{label}', 1.0, $3, $4, $5, $6, now()) \
             ON CONFLICT (source_node_id, target_node_id, edge_type) DO NOTHING",
        );
        sqlx::query(&insert_sql)
            .bind(from_id)
            .bind(to_id)
            .bind(confidence)
            .bind(causal_weight)
            .bind(props)
            .bind(created_by)
            .execute(&self.pool)
            .await
            .map_err(AppError::Database)?;

        // Fetch the canonical row (inserted or pre-existing).
        let select_sql = format!(
            "SELECT id, source_node_id, target_node_id, edge_type, \
                    weight, confidence, causal_weight, metadata, created_at, created_by, \
                    valid_from, valid_to \
             FROM covalence.edges \
             WHERE source_node_id = $1 AND target_node_id = $2 \
               AND edge_type = '{label}' \
               AND valid_to IS NULL \
             LIMIT 1",
        );
        let row = sqlx::query(&select_sql)
            .bind(from_id)
            .bind(to_id)
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::Database)?;

        let edge = Edge {
            id: row.try_get("id").map_err(AppError::Database)?,
            source_node_id: row.try_get("source_node_id").map_err(AppError::Database)?,
            target_node_id: row.try_get("target_node_id").map_err(AppError::Database)?,
            edge_type,
            weight: row
                .try_get::<f64, _>("weight")
                .map_err(AppError::Database)? as f32,
            confidence: row
                .try_get::<f64, _>("confidence")
                .map_err(AppError::Database)? as f32,
            causal_weight: row
                .try_get::<f64, _>("causal_weight")
                .map_err(AppError::Database)? as f32,
            metadata: row.try_get("metadata").map_err(AppError::Database)?,
            created_at: row.try_get("created_at").map_err(AppError::Database)?,
            created_by: row.try_get("created_by").map_err(AppError::Database)?,
            valid_from: row.try_get("valid_from").map_err(AppError::Database)?,
            valid_to: row.try_get("valid_to").map_err(AppError::Database)?,
        };

        // ── Inverse direction ─────────────────────────────────────────────────
        // Auto-create the B→A complement so the symmetric invariant is always
        // maintained.  DO NOTHING makes this idempotent if the inverse was
        // already written by an earlier call.
        let inverse_sql = format!(
            "INSERT INTO covalence.edges \
             (id, source_node_id, target_node_id, edge_type, \
              weight, confidence, causal_weight, metadata, created_by, valid_from) \
             VALUES (gen_random_uuid(), $1, $2, '{label}', $3, $4, $5, $6, $7, now()) \
             ON CONFLICT (source_node_id, target_node_id, edge_type) DO NOTHING",
        );
        sqlx::query(&inverse_sql)
            .bind(to_id) // B → becomes source of inverse
            .bind(from_id) // A → becomes target of inverse
            .bind(edge.weight)
            .bind(edge.confidence)
            .bind(edge.causal_weight)
            .bind(serde_json::json!({
                "inferred_by":    "symmetric_edge",
                "source_edge_id": edge.id.to_string(),
            }))
            .bind("kg_inference")
            .execute(&self.pool)
            .await
            .map_err(AppError::Database)?;

        Ok(edge)
    }

    /// Delete an edge.
    pub async fn delete(&self, edge_id: Uuid) -> AppResult<()> {
        self.graph
            .delete_edge(edge_id)
            .await
            .map_err(AppError::Graph)
    }

    /// List edges for a node.
    ///
    /// `include_superseded = true` returns both active and superseded edges.
    /// The default (`false`) returns only currently active edges.
    pub async fn list_for_node(
        &self,
        node_id: Uuid,
        direction: Option<&str>,
        labels: Option<&str>,
        limit: usize,
        include_superseded: bool,
    ) -> AppResult<Vec<Edge>> {
        let dir = match direction {
            Some("outbound") => TraversalDirection::Outbound,
            Some("inbound") => TraversalDirection::Inbound,
            _ => TraversalDirection::Both,
        };

        let edge_types: Option<Vec<EdgeType>> = labels.map(|l| {
            l.split(',')
                .filter_map(|s| s.trim().parse::<EdgeType>().ok())
                .collect()
        });

        self.graph
            .list_edges(
                node_id,
                dir,
                edge_types.as_deref(),
                limit,
                include_superseded,
            )
            .await
            .map_err(AppError::Graph)
    }

    /// Supersede an active edge — sets `valid_to = now()` without deleting the row.
    ///
    /// The edge is preserved for historical queries but excluded from all default
    /// traversals.  Returns an error if the edge does not exist or is already superseded.
    pub async fn supersede(&self, edge_id: Uuid) -> AppResult<()> {
        self.graph
            .supersede_edge(edge_id)
            .await
            .map_err(AppError::Graph)
    }

    /// Neighborhood traversal.
    pub async fn neighborhood(
        &self,
        node_id: Uuid,
        params: NeighborhoodParams,
    ) -> AppResult<Vec<GraphNeighbor>> {
        let depth = params.depth.unwrap_or(2).min(5);
        let limit = params.limit.unwrap_or(50).min(200);
        let dir = match params.direction.as_deref() {
            Some("outbound") => TraversalDirection::Outbound,
            Some("inbound") => TraversalDirection::Inbound,
            _ => TraversalDirection::Both,
        };

        let edge_types: Option<Vec<EdgeType>> = params.labels.as_ref().map(|l| {
            l.split(',')
                .filter_map(|s| s.trim().parse::<EdgeType>().ok())
                .collect()
        });

        self.graph
            .find_neighbors(node_id, edge_types.as_deref(), dir, depth, limit)
            .await
            .map_err(AppError::Graph)
    }
}

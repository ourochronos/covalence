//! Edge operations — create, delete, list, neighborhood traversal.

use sqlx::PgPool;
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
    /// * **CONTRADICTS symmetry** (covalence#99): when A CONTRADICTS B is written,
    ///   the inverse B CONTRADICTS A is automatically created if it does not already
    ///   exist.  Both the primary insert and the inverse insert use
    ///   `ON CONFLICT … DO NOTHING` so that the operation is idempotent: writing
    ///   an edge that already exists returns the existing edge rather than an error.
    pub async fn create(&self, req: CreateEdgeRequest) -> AppResult<Edge> {
        let edge_type: EdgeType = req
            .label
            .parse()
            .map_err(|e: String| AppError::BadRequest(e))?;

        let method = req.method.as_deref().unwrap_or("agent_explicit");
        let confidence = req.confidence.unwrap_or(1.0);
        let props = serde_json::json!({"notes": req.notes});

        // ── Primary edge insert ───────────────────────────────────────────────
        // For CONTRADICTS edges we use an upsert helper so that symmetry
        // enforcement (which may have already created this direction) does not
        // produce a unique-constraint error.  All other edge types go through
        // the standard repository path.
        let edge = if edge_type == EdgeType::Contradicts {
            self.upsert_contradicts_edge(
                req.from_node_id,
                req.to_node_id,
                confidence,
                method,
                &props,
            )
            .await?
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
                .map_err(AppError::Graph)?
        };

        // ── Inference rule: CONTRADICTS symmetry ─────────────────────────────
        // Dung (1995): the "attack" relation must be symmetric.  When A→B
        // CONTRADICTS is written, auto-create B→A CONTRADICTS if absent.
        if edge_type == EdgeType::Contradicts {
            sqlx::query(
                "INSERT INTO covalence.edges \
                 (id, source_node_id, target_node_id, edge_type, \
                  weight, confidence, causal_weight, metadata, created_by, valid_from) \
                 VALUES (gen_random_uuid(), $1, $2, 'CONTRADICTS', $3, $4, $5, $6, $7, now()) \
                 ON CONFLICT (source_node_id, target_node_id, edge_type) DO NOTHING",
            )
            .bind(req.to_node_id) // B → becomes source of inverse
            .bind(req.from_node_id) // A → becomes target of inverse
            .bind(edge.weight)
            .bind(edge.confidence)
            .bind(edge.causal_weight) // CONTRADICTS causal_weight = 0.50
            .bind(serde_json::json!({
                "inferred_by":    "contradicts_symmetry",
                "source_edge_id": edge.id.to_string(),
            }))
            .bind("kg_inference")
            .execute(&self.pool)
            .await
            .map_err(AppError::Database)?;
        }

        Ok(edge)
    }

    /// Insert a CONTRADICTS edge if it does not already exist, or return the
    /// existing active edge.  Uses `ON CONFLICT … DO NOTHING` followed by a
    /// SELECT so the operation is idempotent regardless of which side of a
    /// symmetric pair is written first.
    async fn upsert_contradicts_edge(
        &self,
        from_id: Uuid,
        to_id: Uuid,
        confidence: f32,
        created_by: &str,
        props: &serde_json::Value,
    ) -> AppResult<Edge> {
        // causal_weight for CONTRADICTS is always 0.50.
        let causal_weight = EdgeType::Contradicts.causal_weight();

        // Attempt insert; silently skip if the edge already exists.
        sqlx::query(
            "INSERT INTO covalence.edges \
             (id, source_node_id, target_node_id, edge_type, \
              weight, confidence, causal_weight, metadata, created_by, valid_from) \
             VALUES (gen_random_uuid(), $1, $2, 'CONTRADICTS', 1.0, $3, $4, $5, $6, now()) \
             ON CONFLICT (source_node_id, target_node_id, edge_type) DO NOTHING",
        )
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
        let row = sqlx::query(
            "SELECT id, age_id, source_node_id, target_node_id, edge_type, \
                    weight, confidence, causal_weight, metadata, created_at, created_by, \
                    valid_from, valid_to \
             FROM covalence.edges \
             WHERE source_node_id = $1 AND target_node_id = $2 \
               AND edge_type = 'CONTRADICTS' \
               AND valid_to IS NULL \
             LIMIT 1",
        )
        .bind(from_id)
        .bind(to_id)
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)?;

        use sqlx::Row as _;
        Ok(Edge {
            id: row.try_get("id").map_err(AppError::Database)?,
            age_id: row.try_get("age_id").map_err(AppError::Database)?,
            source_node_id: row.try_get("source_node_id").map_err(AppError::Database)?,
            target_node_id: row.try_get("target_node_id").map_err(AppError::Database)?,
            edge_type: EdgeType::Contradicts,
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
        })
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

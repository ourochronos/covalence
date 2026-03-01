//! Edge operations — create, delete, list, neighborhood traversal.

use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::*;
use crate::graph::{AgeGraphRepository, GraphRepository};
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
    pool: PgPool,
    graph: AgeGraphRepository,
}

impl EdgeService {
    pub fn new(pool: PgPool) -> Self {
        let graph = AgeGraphRepository::new(pool.clone(), "covalence");
        Self { pool, graph }
    }

    /// Create a typed edge between two nodes.
    pub async fn create(&self, req: CreateEdgeRequest) -> AppResult<Edge> {
        let edge_type: EdgeType = req.label.parse()
            .map_err(|e: String| AppError::BadRequest(e))?;

        let method = req.method.as_deref().unwrap_or("agent_explicit");
        let confidence = req.confidence.unwrap_or(1.0);
        let props = serde_json::json!({"notes": req.notes});

        self.graph
            .create_edge(req.from_node_id, req.to_node_id, edge_type, confidence, method, props)
            .await
            .map_err(AppError::Graph)
    }

    /// Delete an edge.
    pub async fn delete(&self, edge_id: Uuid) -> AppResult<()> {
        self.graph.delete_edge(edge_id).await.map_err(AppError::Graph)
    }

    /// List edges for a node.
    pub async fn list_for_node(&self, node_id: Uuid, direction: Option<&str>, labels: Option<&str>, limit: usize) -> AppResult<Vec<Edge>> {
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
            .list_edges(node_id, dir, edge_types.as_deref(), limit)
            .await
            .map_err(AppError::Graph)
    }

    /// Neighborhood traversal.
    pub async fn neighborhood(&self, node_id: Uuid, params: NeighborhoodParams) -> AppResult<Vec<GraphNeighbor>> {
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

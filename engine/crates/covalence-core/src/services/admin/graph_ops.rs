//! Graph sidecar operations: stats, reload, invalidated edge analysis.

use crate::error::Result;

use super::AdminService;

/// Graph statistics snapshot.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphStats {
    /// Number of nodes in the sidecar.
    pub node_count: usize,
    /// Number of edges in the sidecar.
    pub edge_count: usize,
    /// Number of semantic (non-synthetic) edges.
    pub semantic_edge_count: usize,
    /// Number of synthetic (co-occurrence) edges.
    pub synthetic_edge_count: usize,
    /// Graph density (edges / max possible edges).
    pub density: f64,
    /// Number of weakly connected components.
    pub component_count: usize,
}

/// A relationship type with its count of invalidated edges.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InvalidatedEdgeType {
    /// Relationship type (e.g. "RELATED_TO", "co_occurs").
    pub rel_type: String,
    /// Number of invalidated edges with this type.
    pub count: i64,
}

/// A node with a high count of invalidated edges (controversy indicator).
#[derive(Debug, Clone, serde::Serialize)]
pub struct InvalidatedEdgeNode {
    /// Node UUID.
    pub node_id: uuid::Uuid,
    /// Canonical node name.
    pub canonical_name: String,
    /// Node type.
    pub node_type: String,
    /// Number of invalidated edges touching this node.
    pub invalidated_edge_count: i64,
}

/// Statistics about invalidated edges in the graph.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InvalidatedEdgeStats {
    /// Total number of invalidated edges.
    pub total_invalidated: i64,
    /// Total number of valid (non-invalidated) edges.
    pub total_valid: i64,
    /// Top relationship types by invalidated edge count.
    pub top_types: Vec<InvalidatedEdgeType>,
    /// Nodes with the highest number of invalidated edges.
    pub top_nodes: Vec<InvalidatedEdgeNode>,
}

impl AdminService {
    /// Get graph statistics from the sidecar.
    pub async fn graph_stats(&self) -> GraphStats {
        match self.graph.stats().await {
            Ok(engine_stats) => GraphStats {
                node_count: engine_stats.node_count,
                edge_count: engine_stats.edge_count,
                semantic_edge_count: engine_stats.semantic_edge_count,
                synthetic_edge_count: engine_stats.synthetic_edge_count,
                density: engine_stats.density,
                component_count: engine_stats.component_count,
            },
            Err(e) => {
                tracing::warn!(error = %e, "failed to get graph stats");
                GraphStats {
                    node_count: 0,
                    edge_count: 0,
                    semantic_edge_count: 0,
                    synthetic_edge_count: 0,
                    density: 0.0,
                    component_count: 0,
                }
            }
        }
    }

    /// Reload the graph sidecar from PG.
    pub async fn reload_graph(&self) -> Result<GraphStats> {
        self.graph.reload(self.repo.pool()).await?;
        Ok(self.graph_stats().await)
    }

    /// Retrieve statistics about invalidated edges.
    ///
    /// Returns the total count, top relationship types, and nodes
    /// with the highest invalidated edge counts (controversy
    /// indicators). These edges are normally invisible to the graph
    /// sidecar (`full_reload` filters `WHERE invalid_at IS NULL`).
    pub async fn invalidated_edge_stats(
        &self,
        type_limit: usize,
        node_limit: usize,
    ) -> Result<InvalidatedEdgeStats> {
        // Total invalidated + valid via SP.
        let stats_row: (i64, i64) = sqlx::query_as("SELECT * FROM sp_invalidated_edge_stats()")
            .fetch_one(self.repo.pool())
            .await?;
        let total_invalidated = stats_row.0;
        let total_valid = stats_row.1;

        // Top invalidated edge types via SP.
        let top_types: Vec<(String, i64)> =
            sqlx::query_as("SELECT * FROM sp_top_invalidated_rel_types($1)")
                .bind(type_limit as i32)
                .fetch_all(self.repo.pool())
                .await?;

        // Nodes with the highest invalidated-edge count.
        //
        // We UNION source and target sides so a node touching many
        // invalidated edges on either direction is surfaced.
        let top_nodes: Vec<(uuid::Uuid, String, String, i64)> = sqlx::query_as(
            "SELECT n.id, n.canonical_name, n.node_type, cnt \
             FROM ( \
                 SELECT node_id, SUM(c) AS cnt FROM ( \
                     SELECT source_node_id AS node_id, COUNT(*) AS c \
                     FROM edges WHERE invalid_at IS NOT NULL \
                     GROUP BY 1 \
                     UNION ALL \
                     SELECT target_node_id AS node_id, COUNT(*) AS c \
                     FROM edges WHERE invalid_at IS NOT NULL \
                     GROUP BY 1 \
                 ) sub \
                 GROUP BY node_id \
             ) agg \
             JOIN nodes n ON n.id = agg.node_id \
             ORDER BY cnt DESC \
             LIMIT $1",
        )
        .bind(node_limit as i64)
        .fetch_all(self.repo.pool())
        .await?;

        Ok(InvalidatedEdgeStats {
            total_invalidated,
            total_valid,
            top_types: top_types
                .into_iter()
                .map(|(rel_type, count)| InvalidatedEdgeType { rel_type, count })
                .collect(),
            top_nodes: top_nodes
                .into_iter()
                .map(
                    |(node_id, canonical_name, node_type, count)| InvalidatedEdgeNode {
                        node_id,
                        canonical_name,
                        node_type,
                        invalidated_edge_count: count,
                    },
                )
                .collect(),
        })
    }
}

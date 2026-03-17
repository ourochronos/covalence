//! NodeLandmarkRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::models::node::Node;
use crate::storage::traits::NodeLandmarkRepo;
use crate::types::clearance::ClearanceLevel;
use crate::types::ids::NodeId;
use crate::types::opinion::Opinion;

use super::PgRepo;

impl NodeLandmarkRepo for PgRepo {
    async fn list_landmarks(&self, limit: i64) -> Result<Vec<Node>> {
        let rows = sqlx::query(
            "SELECT id, canonical_name, node_type, description,
                    properties, confidence_breakdown,
                    clearance_level, first_seen, last_seen,
                    mention_count
             FROM nodes
             WHERE clearance_level >= 0
             ORDER BY mention_count DESC
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(landmark_node_from_row).collect())
    }
}

/// Convert a PG row to a Node (same shape as node.rs helper).
fn landmark_node_from_row(row: &sqlx::postgres::PgRow) -> Node {
    let clearance_i32: i32 = row.get("clearance_level");
    let confidence_json: Option<serde_json::Value> = row.get("confidence_breakdown");

    Node {
        id: row.get::<NodeId, _>("id"),
        canonical_name: row.get("canonical_name"),
        node_type: row.get("node_type"),
        entity_class: None,
        domain_entropy: None,
        primary_domain: None,
        description: row.get("description"),
        properties: row.get("properties"),
        confidence_breakdown: confidence_json.as_ref().and_then(Opinion::from_json),
        clearance_level: ClearanceLevel::from_i32_or_default(clearance_i32),
        first_seen: row.get("first_seen"),
        last_seen: row.get("last_seen"),
        mention_count: row.get("mention_count"),
    }
}

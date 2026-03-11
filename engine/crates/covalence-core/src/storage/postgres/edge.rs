//! EdgeRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::models::edge::Edge;
use crate::storage::traits::EdgeRepo;
use crate::types::causal::CausalLevel;
use crate::types::clearance::ClearanceLevel;
use crate::types::ids::{EdgeId, NodeId};
use crate::types::opinion::Opinion;

use super::PgRepo;

impl EdgeRepo for PgRepo {
    async fn create(&self, edge: &Edge) -> Result<()> {
        let causal_str = edge.causal_level.as_ref().map(CausalLevel::as_str);
        let confidence_json = edge.confidence_breakdown.as_ref().map(|o| o.to_json());

        sqlx::query(
            "INSERT INTO edges (
                id, source_node_id, target_node_id, rel_type,
                causal_level, properties, weight, confidence,
                confidence_breakdown, clearance_level,
                is_synthetic, valid_from, valid_until,
                invalid_at, invalidated_by,
                recorded_at, created_at
            ) VALUES (
                $1, $2, $3, $4,
                $5, $6, $7, $8,
                $9, $10,
                $11, $12, $13,
                $14, $15,
                $16, $17
            )",
        )
        .bind(edge.id)
        .bind(edge.source_node_id)
        .bind(edge.target_node_id)
        .bind(&edge.rel_type)
        .bind(causal_str)
        .bind(&edge.properties)
        .bind(edge.weight)
        .bind(edge.confidence)
        .bind(&confidence_json)
        .bind(edge.clearance_level.as_i32())
        .bind(edge.is_synthetic)
        .bind(edge.valid_from)
        .bind(edge.valid_until)
        .bind(edge.invalid_at)
        .bind(edge.invalidated_by)
        .bind(edge.recorded_at)
        .bind(edge.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get(&self, id: EdgeId) -> Result<Option<Edge>> {
        let row = sqlx::query(
            "SELECT id, source_node_id, target_node_id,
                    rel_type, causal_level, properties,
                    weight, confidence,
                    confidence_breakdown, clearance_level,
                    is_synthetic, valid_from, valid_until,
                    invalid_at, invalidated_by,
                    recorded_at, created_at
             FROM edges WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| edge_from_row(&r)))
    }

    async fn list_from_node(&self, node_id: NodeId) -> Result<Vec<Edge>> {
        let rows = sqlx::query(
            "SELECT id, source_node_id, target_node_id,
                    rel_type, causal_level, properties,
                    weight, confidence,
                    confidence_breakdown, clearance_level,
                    is_synthetic, valid_from, valid_until,
                    invalid_at, invalidated_by,
                    recorded_at, created_at
             FROM edges
             WHERE source_node_id = $1",
        )
        .bind(node_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(edge_from_row).collect())
    }

    async fn list_to_node(&self, node_id: NodeId) -> Result<Vec<Edge>> {
        let rows = sqlx::query(
            "SELECT id, source_node_id, target_node_id,
                    rel_type, causal_level, properties,
                    weight, confidence,
                    confidence_breakdown, clearance_level,
                    is_synthetic, valid_from, valid_until,
                    invalid_at, invalidated_by,
                    recorded_at, created_at
             FROM edges
             WHERE target_node_id = $1",
        )
        .bind(node_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(edge_from_row).collect())
    }

    async fn list_between(&self, source_id: NodeId, target_id: NodeId) -> Result<Vec<Edge>> {
        let rows = sqlx::query(
            "SELECT id, source_node_id, target_node_id,
                    rel_type, causal_level, properties,
                    weight, confidence,
                    confidence_breakdown, clearance_level,
                    is_synthetic, valid_from, valid_until,
                    invalid_at, invalidated_by,
                    recorded_at, created_at
             FROM edges
             WHERE source_node_id = $1
               AND target_node_id = $2",
        )
        .bind(source_id)
        .bind(target_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(edge_from_row).collect())
    }

    async fn update(&self, edge: &Edge) -> Result<()> {
        let causal_str = edge.causal_level.as_ref().map(CausalLevel::as_str);
        let confidence_json = edge.confidence_breakdown.as_ref().map(|o| o.to_json());

        sqlx::query(
            "UPDATE edges SET
                source_node_id = $2, target_node_id = $3,
                rel_type = $4, causal_level = $5,
                properties = $6, weight = $7,
                confidence = $8,
                confidence_breakdown = $9,
                clearance_level = $10,
                is_synthetic = $11,
                valid_from = $12, valid_until = $13,
                invalid_at = $14, invalidated_by = $15,
                recorded_at = $16, created_at = $17
             WHERE id = $1",
        )
        .bind(edge.id)
        .bind(edge.source_node_id)
        .bind(edge.target_node_id)
        .bind(&edge.rel_type)
        .bind(causal_str)
        .bind(&edge.properties)
        .bind(edge.weight)
        .bind(edge.confidence)
        .bind(&confidence_json)
        .bind(edge.clearance_level.as_i32())
        .bind(edge.is_synthetic)
        .bind(edge.valid_from)
        .bind(edge.valid_until)
        .bind(edge.invalid_at)
        .bind(edge.invalidated_by)
        .bind(edge.recorded_at)
        .bind(edge.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn invalidate(&self, id: EdgeId, invalidated_by: EdgeId) -> Result<()> {
        sqlx::query(
            "UPDATE edges
             SET invalid_at = NOW(), invalidated_by = $2
             WHERE id = $1",
        )
        .bind(id)
        .bind(invalidated_by)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_active(&self) -> Result<Vec<Edge>> {
        let rows = sqlx::query(
            "SELECT id, source_node_id, target_node_id,
                    rel_type, causal_level, properties,
                    weight, confidence,
                    confidence_breakdown, clearance_level,
                    is_synthetic, valid_from, valid_until,
                    invalid_at, invalidated_by,
                    recorded_at, created_at
             FROM edges
             WHERE invalid_at IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(edge_from_row).collect())
    }

    async fn delete(&self, id: EdgeId) -> Result<bool> {
        let result = sqlx::query("DELETE FROM edges WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn delete_by_node(&self, node_id: NodeId) -> Result<u64> {
        let result = sqlx::query(
            "DELETE FROM edges
             WHERE source_node_id = $1
                OR target_node_id = $1",
        )
        .bind(node_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

fn edge_from_row(row: &sqlx::postgres::PgRow) -> Edge {
    let clearance_i32: i32 = row.get("clearance_level");
    let causal_str: Option<String> = row.get("causal_level");
    let confidence_json: Option<serde_json::Value> = row.get("confidence_breakdown");

    Edge {
        id: row.get("id"),
        source_node_id: row.get("source_node_id"),
        target_node_id: row.get("target_node_id"),
        rel_type: row.get("rel_type"),
        causal_level: causal_str.as_deref().and_then(CausalLevel::from_str_opt),
        properties: row.get("properties"),
        weight: row.get("weight"),
        confidence: row.get("confidence"),
        confidence_breakdown: confidence_json.as_ref().and_then(Opinion::from_json),
        clearance_level: ClearanceLevel::from_i32(clearance_i32).unwrap_or_default(),
        is_synthetic: row.get("is_synthetic"),
        valid_from: row.get("valid_from"),
        valid_until: row.get("valid_until"),
        invalid_at: row.get("invalid_at"),
        invalidated_by: row.get("invalidated_by"),
        recorded_at: row.get("recorded_at"),
        created_at: row.get("created_at"),
    }
}

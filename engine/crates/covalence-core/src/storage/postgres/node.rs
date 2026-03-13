//! NodeRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::models::node::Node;
use crate::storage::traits::NodeRepo;
use crate::types::clearance::ClearanceLevel;
use crate::types::ids::NodeId;
use crate::types::opinion::Opinion;

use super::PgRepo;

impl NodeRepo for PgRepo {
    async fn create(&self, node: &Node) -> Result<()> {
        let confidence_json = node.confidence_breakdown.as_ref().map(|o| o.to_json());

        sqlx::query(
            "INSERT INTO nodes (
                id, canonical_name, node_type, description,
                properties, confidence_breakdown,
                clearance_level, first_seen, last_seen,
                mention_count
            ) VALUES (
                $1, $2, $3, $4,
                $5, $6,
                $7, $8, $9,
                $10
            )",
        )
        .bind(node.id)
        .bind(&node.canonical_name)
        .bind(&node.node_type)
        .bind(&node.description)
        .bind(&node.properties)
        .bind(&confidence_json)
        .bind(node.clearance_level.as_i32())
        .bind(node.first_seen)
        .bind(node.last_seen)
        .bind(node.mention_count)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get(&self, id: NodeId) -> Result<Option<Node>> {
        let row = sqlx::query(
            "SELECT id, canonical_name, node_type, description,
                    properties, confidence_breakdown,
                    clearance_level, first_seen, last_seen,
                    mention_count
             FROM nodes WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| node_from_row(&r)))
    }

    async fn find_by_name(&self, name: &str) -> Result<Option<Node>> {
        let row = sqlx::query(
            "SELECT id, canonical_name, node_type, description,
                    properties, confidence_breakdown,
                    clearance_level, first_seen, last_seen,
                    mention_count
             FROM nodes
             WHERE LOWER(canonical_name) = LOWER($1)",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| node_from_row(&r)))
    }

    async fn update(&self, node: &Node) -> Result<()> {
        let confidence_json = node.confidence_breakdown.as_ref().map(|o| o.to_json());

        sqlx::query(
            "UPDATE nodes SET
                canonical_name = $2, node_type = $3,
                description = $4, properties = $5,
                confidence_breakdown = $6, clearance_level = $7,
                first_seen = $8, last_seen = $9,
                mention_count = $10
             WHERE id = $1",
        )
        .bind(node.id)
        .bind(&node.canonical_name)
        .bind(&node.node_type)
        .bind(&node.description)
        .bind(&node.properties)
        .bind(&confidence_json)
        .bind(node.clearance_level.as_i32())
        .bind(node.first_seen)
        .bind(node.last_seen)
        .bind(node.mention_count)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete(&self, id: NodeId) -> Result<bool> {
        let result = sqlx::query("DELETE FROM nodes WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn update_embedding(&self, id: NodeId, embedding: &[f64]) -> Result<()> {
        // Format as pgvector literal and cast to halfvec.
        let pgvec = format!(
            "[{}]",
            embedding
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        sqlx::query(
            "UPDATE nodes \
             SET embedding = $1::halfvec \
             WHERE id = $2",
        )
        .bind(&pgvec)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_ungrounded(&self) -> Result<Vec<Node>> {
        let rows = sqlx::query(
            "SELECT n.id, n.canonical_name, n.node_type,
                    n.description, n.properties,
                    n.confidence_breakdown, n.clearance_level,
                    n.first_seen, n.last_seen, n.mention_count
             FROM nodes n
             WHERE NOT EXISTS (
                 SELECT 1 FROM extractions e
                 WHERE e.entity_type = 'node'
                   AND e.entity_id = n.id
                   AND e.is_superseded = false
                   AND (e.chunk_id IS NOT NULL
                        OR e.statement_id IS NOT NULL)
             )",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(node_from_row).collect())
    }

    async fn list_by_type(&self, node_type: &str, limit: i64, offset: i64) -> Result<Vec<Node>> {
        let rows = sqlx::query(
            "SELECT id, canonical_name, node_type, description,
                    properties, confidence_breakdown,
                    clearance_level, first_seen, last_seen,
                    mention_count
             FROM nodes
             WHERE node_type = $1
             ORDER BY canonical_name ASC
             LIMIT $2 OFFSET $3",
        )
        .bind(node_type)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(node_from_row).collect())
    }

    async fn get_many(&self, ids: &[NodeId]) -> Result<Vec<Node>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let uuids: Vec<uuid::Uuid> = ids.iter().map(|id| id.into_uuid()).collect();
        let rows = sqlx::query(
            "SELECT id, canonical_name, node_type, description,
                    properties, confidence_breakdown,
                    clearance_level, first_seen, last_seen,
                    mention_count
             FROM nodes WHERE id = ANY($1)",
        )
        .bind(&uuids)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(node_from_row).collect())
    }

    async fn batch_update_opinions(
        &self,
        updates: &[(NodeId, Option<serde_json::Value>)],
    ) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let ids: Vec<uuid::Uuid> = updates.iter().map(|(id, _)| id.into_uuid()).collect();
        let opinions: Vec<Option<serde_json::Value>> =
            updates.iter().map(|(_, o)| o.clone()).collect();

        sqlx::query(
            "UPDATE nodes SET confidence_breakdown = v.opinion
             FROM unnest($1::uuid[], $2::jsonb[])
                  AS v(id, opinion)
             WHERE nodes.id = v.id",
        )
        .bind(&ids)
        .bind(&opinions)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn node_from_row(row: &sqlx::postgres::PgRow) -> Node {
    let clearance_i32: i32 = row.get("clearance_level");
    let confidence_json: Option<serde_json::Value> = row.get("confidence_breakdown");

    Node {
        id: row.get("id"),
        canonical_name: row.get("canonical_name"),
        node_type: row.get("node_type"),
        description: row.get("description"),
        properties: row.get("properties"),
        confidence_breakdown: confidence_json.as_ref().and_then(Opinion::from_json),
        clearance_level: ClearanceLevel::from_i32_or_default(clearance_i32),
        first_seen: row.get("first_seen"),
        last_seen: row.get("last_seen"),
        mention_count: row.get("mention_count"),
    }
}

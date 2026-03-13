//! PostgreSQL implementation of [`UnresolvedEntityRepo`].

use sqlx::Row;

use crate::error::Result;
use crate::models::unresolved_entity::UnresolvedEntity;
use crate::storage::traits::UnresolvedEntityRepo;
use crate::types::ids::{NodeId, SourceId};

use super::PgRepo;

impl UnresolvedEntityRepo for PgRepo {
    async fn create(&self, entity: &UnresolvedEntity) -> Result<()> {
        let embedding_f32: Option<Vec<f32>> = entity
            .embedding
            .as_ref()
            .map(|e| e.iter().map(|&v| v as f32).collect());

        sqlx::query(
            "INSERT INTO unresolved_entities (
                id, source_id, statement_id, chunk_id,
                extracted_name, entity_type, description,
                embedding, confidence
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8::halfvec, $9)",
        )
        .bind(entity.id)
        .bind(entity.source_id)
        .bind(entity.statement_id)
        .bind(entity.chunk_id)
        .bind(&entity.extracted_name)
        .bind(&entity.entity_type)
        .bind(&entity.description)
        .bind(&embedding_f32)
        .bind(entity.confidence)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get(&self, id: uuid::Uuid) -> Result<Option<UnresolvedEntity>> {
        let row = sqlx::query(
            "SELECT id, source_id, statement_id, chunk_id,
                    extracted_name, entity_type, description,
                    embedding::float4[]::float8[] AS embedding,
                    confidence, resolved_node_id, resolved_at,
                    created_at
             FROM unresolved_entities
             WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| unresolved_entity_from_row(&r)))
    }

    async fn list_pending(&self) -> Result<Vec<UnresolvedEntity>> {
        let rows = sqlx::query(
            "SELECT id, source_id, statement_id, chunk_id,
                    extracted_name, entity_type, description,
                    embedding::float4[]::float8[] AS embedding,
                    confidence, resolved_node_id, resolved_at,
                    created_at
             FROM unresolved_entities
             WHERE resolved_node_id IS NULL
             ORDER BY created_at",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(unresolved_entity_from_row).collect())
    }

    async fn list_by_source(&self, source_id: SourceId) -> Result<Vec<UnresolvedEntity>> {
        let rows = sqlx::query(
            "SELECT id, source_id, statement_id, chunk_id,
                    extracted_name, entity_type, description,
                    embedding::float4[]::float8[] AS embedding,
                    confidence, resolved_node_id, resolved_at,
                    created_at
             FROM unresolved_entities
             WHERE source_id = $1
             ORDER BY created_at",
        )
        .bind(source_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(unresolved_entity_from_row).collect())
    }

    async fn mark_resolved(&self, id: uuid::Uuid, node_id: NodeId) -> Result<()> {
        sqlx::query(
            "UPDATE unresolved_entities
             SET resolved_node_id = $2, resolved_at = now()
             WHERE id = $1",
        )
        .bind(id)
        .bind(node_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_by_source(&self, source_id: SourceId) -> Result<u64> {
        let result = sqlx::query("DELETE FROM unresolved_entities WHERE source_id = $1")
            .bind(source_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn count_pending(&self) -> Result<i64> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS cnt FROM unresolved_entities
             WHERE resolved_node_id IS NULL",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get("cnt"))
    }
}

/// Map a database row to an [`UnresolvedEntity`].
fn unresolved_entity_from_row(row: &sqlx::postgres::PgRow) -> UnresolvedEntity {
    let embedding: Option<Vec<f64>> = row.get("embedding");
    UnresolvedEntity {
        id: row.get("id"),
        source_id: row.get("source_id"),
        statement_id: row.get("statement_id"),
        chunk_id: row.get("chunk_id"),
        extracted_name: row.get("extracted_name"),
        entity_type: row.get("entity_type"),
        description: row.get("description"),
        embedding,
        confidence: row.get("confidence"),
        resolved_node_id: row.get("resolved_node_id"),
        resolved_at: row.get("resolved_at"),
        created_at: row.get("created_at"),
    }
}

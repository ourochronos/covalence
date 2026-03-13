//! ExtractionRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::models::extraction::Extraction;
use crate::storage::traits::ExtractionRepo;
use crate::types::ids::{ChunkId, EdgeId, ExtractionId, NodeId, SourceId};

use super::PgRepo;

impl ExtractionRepo for PgRepo {
    async fn create(&self, extraction: &Extraction) -> Result<()> {
        sqlx::query(
            "INSERT INTO extractions (
                id, chunk_id, statement_id, entity_type, entity_id,
                extraction_method, confidence,
                is_superseded, extracted_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(extraction.id)
        .bind(extraction.chunk_id)
        .bind(extraction.statement_id)
        .bind(&extraction.entity_type)
        .bind(extraction.entity_id)
        .bind(&extraction.extraction_method)
        .bind(extraction.confidence)
        .bind(extraction.is_superseded)
        .bind(extraction.extracted_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get(&self, id: ExtractionId) -> Result<Option<Extraction>> {
        let row = sqlx::query(
            "SELECT id, chunk_id, statement_id, entity_type, entity_id,
                    extraction_method, confidence,
                    is_superseded, extracted_at
             FROM extractions WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| extraction_from_row(&r)))
    }

    async fn list_by_chunk(&self, chunk_id: ChunkId) -> Result<Vec<Extraction>> {
        let rows = sqlx::query(
            "SELECT id, chunk_id, statement_id, entity_type, entity_id,
                    extraction_method, confidence,
                    is_superseded, extracted_at
             FROM extractions
             WHERE chunk_id = $1
             ORDER BY extracted_at ASC",
        )
        .bind(chunk_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(extraction_from_row).collect())
    }

    async fn list_active_for_entity(
        &self,
        entity_type: &str,
        entity_id: uuid::Uuid,
    ) -> Result<Vec<Extraction>> {
        let rows = sqlx::query(
            "SELECT id, chunk_id, statement_id, entity_type, entity_id,
                    extraction_method, confidence,
                    is_superseded, extracted_at
             FROM extractions
             WHERE entity_type = $1
               AND entity_id = $2
               AND NOT is_superseded
             ORDER BY extracted_at ASC",
        )
        .bind(entity_type)
        .bind(entity_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(extraction_from_row).collect())
    }

    async fn mark_superseded(&self, id: ExtractionId) -> Result<()> {
        sqlx::query(
            "UPDATE extractions
             SET is_superseded = true
             WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn mark_superseded_by_source(&self, source_id: SourceId) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE extractions
             SET is_superseded = true
             WHERE (
                 chunk_id IN (SELECT id FROM chunks WHERE source_id = $1)
                 OR statement_id IN (SELECT id FROM statements WHERE source_id = $1)
             )
             AND NOT is_superseded",
        )
        .bind(source_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn delete_by_source(&self, source_id: SourceId) -> Result<u64> {
        let result = sqlx::query(
            "DELETE FROM extractions
             WHERE chunk_id IN (SELECT id FROM chunks WHERE source_id = $1)
                OR statement_id IN (SELECT id FROM statements WHERE source_id = $1)",
        )
        .bind(source_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn list_node_ids_by_source(&self, source_id: SourceId) -> Result<Vec<NodeId>> {
        let rows = sqlx::query(
            "SELECT DISTINCT entity_id
             FROM extractions
             WHERE entity_type = 'node'
               AND (
                   chunk_id IN (SELECT id FROM chunks WHERE source_id = $1)
                   OR statement_id IN (SELECT id FROM statements WHERE source_id = $1)
               )",
        )
        .bind(source_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|r| {
                let uuid: uuid::Uuid = r.get("entity_id");
                NodeId::from_uuid(uuid)
            })
            .collect())
    }

    async fn count_active_by_entity(
        &self,
        entity_type: &str,
        entity_id: uuid::Uuid,
    ) -> Result<i64> {
        let row = sqlx::query(
            "SELECT COUNT(*) as cnt
             FROM extractions
             WHERE entity_type = $1
               AND entity_id = $2
               AND NOT is_superseded",
        )
        .bind(entity_type)
        .bind(entity_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get("cnt"))
    }

    async fn list_active_for_entities(
        &self,
        entity_type: &str,
        entity_ids: &[uuid::Uuid],
    ) -> Result<Vec<Extraction>> {
        if entity_ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT id, chunk_id, statement_id, entity_type, entity_id,
                    extraction_method, confidence,
                    is_superseded, extracted_at
             FROM extractions
             WHERE entity_type = $1
               AND entity_id = ANY($2)
               AND NOT is_superseded
             ORDER BY entity_id, extracted_at ASC",
        )
        .bind(entity_type)
        .bind(entity_ids)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(extraction_from_row).collect())
    }

    async fn list_edge_ids_by_source(&self, source_id: SourceId) -> Result<Vec<EdgeId>> {
        let rows = sqlx::query(
            "SELECT DISTINCT entity_id
             FROM extractions
             WHERE entity_type = 'edge'
               AND (
                   chunk_id IN (SELECT id FROM chunks WHERE source_id = $1)
                   OR statement_id IN (
                       SELECT id FROM statements WHERE source_id = $1
                   )
               )",
        )
        .bind(source_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|r| {
                let uuid: uuid::Uuid = r.get("entity_id");
                EdgeId::from_uuid(uuid)
            })
            .collect())
    }
}

fn extraction_from_row(row: &sqlx::postgres::PgRow) -> Extraction {
    Extraction {
        id: row.get("id"),
        chunk_id: row.get("chunk_id"),
        statement_id: row.get("statement_id"),
        entity_type: row.get("entity_type"),
        entity_id: row.get("entity_id"),
        extraction_method: row.get("extraction_method"),
        confidence: row.get("confidence"),
        is_superseded: row.get("is_superseded"),
        extracted_at: row.get("extracted_at"),
    }
}

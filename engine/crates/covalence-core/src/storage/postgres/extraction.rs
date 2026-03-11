//! ExtractionRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::models::extraction::Extraction;
use crate::storage::traits::ExtractionRepo;
use crate::types::ids::{ChunkId, ExtractionId, SourceId};

use super::PgRepo;

impl ExtractionRepo for PgRepo {
    async fn create(&self, extraction: &Extraction) -> Result<()> {
        sqlx::query(
            "INSERT INTO extractions (
                id, chunk_id, entity_type, entity_id,
                extraction_method, confidence,
                is_superseded, extracted_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(extraction.id)
        .bind(extraction.chunk_id)
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
            "SELECT id, chunk_id, entity_type, entity_id,
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
            "SELECT id, chunk_id, entity_type, entity_id,
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
            "SELECT id, chunk_id, entity_type, entity_id,
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
             WHERE chunk_id IN (
                 SELECT id FROM chunks WHERE source_id = $1
             )
             AND NOT is_superseded",
        )
        .bind(source_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

fn extraction_from_row(row: &sqlx::postgres::PgRow) -> Extraction {
    Extraction {
        id: row.get("id"),
        chunk_id: row.get("chunk_id"),
        entity_type: row.get("entity_type"),
        entity_id: row.get("entity_id"),
        extraction_method: row.get("extraction_method"),
        confidence: row.get("confidence"),
        is_superseded: row.get("is_superseded"),
        extracted_at: row.get("extracted_at"),
    }
}

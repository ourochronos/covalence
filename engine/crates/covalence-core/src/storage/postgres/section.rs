//! SectionRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::models::section::Section;
use crate::storage::traits::SectionRepo;
use crate::types::clearance::ClearanceLevel;
use crate::types::ids::{SectionId, SourceId, StatementId};

use super::PgRepo;

impl SectionRepo for PgRepo {
    async fn create(&self, section: &Section) -> Result<()> {
        let stmt_ids: Vec<uuid::Uuid> = section
            .statement_ids
            .iter()
            .map(|id| id.into_uuid())
            .collect();

        sqlx::query(
            "INSERT INTO sections (
                id, source_id, title, summary, content_hash,
                statement_ids, cluster_label, ordinal,
                clearance_level, created_at, updated_at
            ) VALUES (
                $1, $2, $3, $4, $5,
                $6, $7, $8,
                $9, $10, $11
            )",
        )
        .bind(section.id)
        .bind(section.source_id)
        .bind(&section.title)
        .bind(&section.summary)
        .bind(&section.content_hash)
        .bind(&stmt_ids)
        .bind(&section.cluster_label)
        .bind(section.ordinal)
        .bind(section.clearance_level.as_i32())
        .bind(section.created_at)
        .bind(section.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get(&self, id: SectionId) -> Result<Option<Section>> {
        let row = sqlx::query(
            "SELECT id, source_id, title, summary, content_hash,
                    statement_ids, cluster_label, ordinal,
                    clearance_level, created_at, updated_at
             FROM sections WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| section_from_row(&r)))
    }

    async fn list_by_source(&self, source_id: SourceId) -> Result<Vec<Section>> {
        let rows = sqlx::query(
            "SELECT id, source_id, title, summary, content_hash,
                    statement_ids, cluster_label, ordinal,
                    clearance_level, created_at, updated_at
             FROM sections
             WHERE source_id = $1
             ORDER BY ordinal ASC",
        )
        .bind(source_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(section_from_row).collect())
    }

    async fn delete_by_source(&self, source_id: SourceId) -> Result<u64> {
        let result = sqlx::query("DELETE FROM sections WHERE source_id = $1")
            .bind(source_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn update_embedding(&self, id: SectionId, embedding: &[f64]) -> Result<()> {
        let pgvec = format!(
            "[{}]",
            embedding
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        sqlx::query(
            "UPDATE sections SET embedding = $2::halfvec \
             WHERE id = $1",
        )
        .bind(id)
        .bind(&pgvec)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_summary(
        &self,
        id: SectionId,
        summary: &str,
        content_hash: &[u8],
    ) -> Result<()> {
        sqlx::query(
            "UPDATE sections SET summary = $2, content_hash = $3, \
             updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(summary)
        .bind(content_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn count_by_source(&self, source_id: SourceId) -> Result<i64> {
        let row = sqlx::query(
            "SELECT COUNT(*) as cnt FROM sections \
             WHERE source_id = $1",
        )
        .bind(source_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get("cnt"))
    }
}

fn section_from_row(row: &sqlx::postgres::PgRow) -> Section {
    let clearance_i32: i32 = row.get("clearance_level");
    let stmt_uuids: Vec<uuid::Uuid> = row.get("statement_ids");

    Section {
        id: row.get("id"),
        source_id: row.get("source_id"),
        title: row.get("title"),
        summary: row.get("summary"),
        content_hash: row.get("content_hash"),
        embedding: None, // Loaded separately; halfvec is not directly mapped.
        statement_ids: stmt_uuids.into_iter().map(StatementId::from_uuid).collect(),
        cluster_label: row.get("cluster_label"),
        ordinal: row.get("ordinal"),
        clearance_level: ClearanceLevel::from_i32_or_default(clearance_i32),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

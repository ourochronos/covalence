//! StatementRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::models::statement::Statement;
use crate::storage::traits::StatementRepo;
use crate::types::clearance::ClearanceLevel;
use crate::types::ids::{SectionId, SourceId, StatementId};

use super::PgRepo;

impl StatementRepo for PgRepo {
    async fn create(&self, statement: &Statement) -> Result<()> {
        sqlx::query(
            "INSERT INTO statements (
                id, source_id, content, content_hash,
                byte_start, byte_end, heading_path,
                paragraph_index, ordinal, confidence,
                section_id, clearance_level, is_evicted,
                extraction_method, created_at
            ) VALUES (
                $1, $2, $3, $4,
                $5, $6, $7,
                $8, $9, $10,
                $11, $12, $13,
                $14, $15
            )",
        )
        .bind(statement.id)
        .bind(statement.source_id)
        .bind(&statement.content)
        .bind(&statement.content_hash)
        .bind(statement.byte_start)
        .bind(statement.byte_end)
        .bind(&statement.heading_path)
        .bind(statement.paragraph_index)
        .bind(statement.ordinal)
        .bind(statement.confidence)
        .bind(statement.section_id)
        .bind(statement.clearance_level.as_i32())
        .bind(statement.is_evicted)
        .bind(&statement.extraction_method)
        .bind(statement.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn batch_create(&self, statements: &[Statement]) -> Result<()> {
        if statements.is_empty() {
            return Ok(());
        }

        let ids: Vec<uuid::Uuid> = statements.iter().map(|s| s.id.into_uuid()).collect();
        let source_ids: Vec<uuid::Uuid> =
            statements.iter().map(|s| s.source_id.into_uuid()).collect();
        let contents: Vec<&str> = statements.iter().map(|s| s.content.as_str()).collect();
        let content_hashes: Vec<&[u8]> = statements
            .iter()
            .map(|s| s.content_hash.as_slice())
            .collect();
        let byte_starts: Vec<i32> = statements.iter().map(|s| s.byte_start).collect();
        let byte_ends: Vec<i32> = statements.iter().map(|s| s.byte_end).collect();
        let heading_paths: Vec<Option<&str>> = statements
            .iter()
            .map(|s| s.heading_path.as_deref())
            .collect();
        let paragraph_indices: Vec<Option<i32>> =
            statements.iter().map(|s| s.paragraph_index).collect();
        let ordinals: Vec<i32> = statements.iter().map(|s| s.ordinal).collect();
        let confidences: Vec<f64> = statements.iter().map(|s| s.confidence).collect();
        let section_ids: Vec<Option<uuid::Uuid>> = statements
            .iter()
            .map(|s| s.section_id.map(|id| id.into_uuid()))
            .collect();
        let clearance_levels: Vec<i32> = statements
            .iter()
            .map(|s| s.clearance_level.as_i32())
            .collect();
        let is_evicted: Vec<bool> = statements.iter().map(|s| s.is_evicted).collect();
        let extraction_methods: Vec<&str> = statements
            .iter()
            .map(|s| s.extraction_method.as_str())
            .collect();
        let created_ats: Vec<chrono::DateTime<chrono::Utc>> =
            statements.iter().map(|s| s.created_at).collect();

        sqlx::query(
            "INSERT INTO statements (
                id, source_id, content, content_hash,
                byte_start, byte_end, heading_path,
                paragraph_index, ordinal, confidence,
                section_id, clearance_level, is_evicted,
                extraction_method, created_at
            ) SELECT * FROM UNNEST(
                $1::uuid[], $2::uuid[], $3::text[], $4::bytea[],
                $5::int[], $6::int[], $7::text[],
                $8::int[], $9::int[], $10::float8[],
                $11::uuid[], $12::int[], $13::bool[],
                $14::text[], $15::timestamptz[]
            )",
        )
        .bind(&ids)
        .bind(&source_ids)
        .bind(&contents)
        .bind(&content_hashes)
        .bind(&byte_starts)
        .bind(&byte_ends)
        .bind(&heading_paths)
        .bind(&paragraph_indices)
        .bind(&ordinals)
        .bind(&confidences)
        .bind(&section_ids)
        .bind(&clearance_levels)
        .bind(&is_evicted)
        .bind(&extraction_methods)
        .bind(&created_ats)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get(&self, id: StatementId) -> Result<Option<Statement>> {
        let row = sqlx::query(
            "SELECT id, source_id, content, content_hash,
                    byte_start, byte_end, heading_path,
                    paragraph_index, ordinal, confidence,
                    section_id, clearance_level, is_evicted,
                    extraction_method, created_at
             FROM statements WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| statement_from_row(&r)))
    }

    async fn list_by_source(&self, source_id: SourceId) -> Result<Vec<Statement>> {
        let rows = sqlx::query(
            "SELECT id, source_id, content, content_hash,
                    byte_start, byte_end, heading_path,
                    paragraph_index, ordinal, confidence,
                    section_id, clearance_level, is_evicted,
                    extraction_method, created_at
             FROM statements
             WHERE source_id = $1
             ORDER BY ordinal ASC",
        )
        .bind(source_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(statement_from_row).collect())
    }

    async fn list_by_section(&self, section_id: SectionId) -> Result<Vec<Statement>> {
        let rows = sqlx::query(
            "SELECT id, source_id, content, content_hash,
                    byte_start, byte_end, heading_path,
                    paragraph_index, ordinal, confidence,
                    section_id, clearance_level, is_evicted,
                    extraction_method, created_at
             FROM statements
             WHERE section_id = $1
             ORDER BY ordinal ASC",
        )
        .bind(section_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(statement_from_row).collect())
    }

    async fn get_by_content_hash(
        &self,
        source_id: SourceId,
        hash: &[u8],
    ) -> Result<Option<Statement>> {
        let row = sqlx::query(
            "SELECT id, source_id, content, content_hash,
                    byte_start, byte_end, heading_path,
                    paragraph_index, ordinal, confidence,
                    section_id, clearance_level, is_evicted,
                    extraction_method, created_at
             FROM statements
             WHERE source_id = $1 AND content_hash = $2",
        )
        .bind(source_id)
        .bind(hash)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| statement_from_row(&r)))
    }

    async fn delete_by_source(&self, source_id: SourceId) -> Result<u64> {
        let result = sqlx::query("DELETE FROM statements WHERE source_id = $1")
            .bind(source_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn update_embedding(&self, id: StatementId, embedding: &[f64]) -> Result<()> {
        let pgvec = format!(
            "[{}]",
            embedding
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        sqlx::query(
            "UPDATE statements SET embedding = $2::halfvec \
             WHERE id = $1",
        )
        .bind(id)
        .bind(&pgvec)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn assign_section(&self, id: StatementId, section_id: SectionId) -> Result<()> {
        sqlx::query("UPDATE statements SET section_id = $2 WHERE id = $1")
            .bind(id)
            .bind(section_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn mark_evicted(&self, id: StatementId) -> Result<()> {
        sqlx::query("UPDATE statements SET is_evicted = true WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn count_by_source(&self, source_id: SourceId) -> Result<i64> {
        let row = sqlx::query(
            "SELECT COUNT(*) as cnt FROM statements \
             WHERE source_id = $1",
        )
        .bind(source_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get("cnt"))
    }
}

fn statement_from_row(row: &sqlx::postgres::PgRow) -> Statement {
    let clearance_i32: i32 = row.get("clearance_level");
    Statement {
        id: row.get("id"),
        source_id: row.get("source_id"),
        content: row.get("content"),
        content_hash: row.get("content_hash"),
        embedding: None, // Loaded separately; halfvec is not directly mapped.
        byte_start: row.get("byte_start"),
        byte_end: row.get("byte_end"),
        heading_path: row.get("heading_path"),
        paragraph_index: row.get("paragraph_index"),
        ordinal: row.get("ordinal"),
        confidence: row.get("confidence"),
        section_id: row.get("section_id"),
        clearance_level: ClearanceLevel::from_i32_or_default(clearance_i32),
        is_evicted: row.get("is_evicted"),
        extraction_method: row.get("extraction_method"),
        created_at: row.get("created_at"),
    }
}

//! SourceRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::models::source::Source;
use crate::storage::traits::SourceRepo;
use crate::types::clearance::ClearanceLevel;
use crate::types::ids::SourceId;

use super::PgRepo;

impl SourceRepo for PgRepo {
    async fn create(&self, source: &Source) -> Result<()> {
        sqlx::query(
            "INSERT INTO sources (
                id, source_type, uri, title, author,
                project, domain,
                created_date,
                ingested_at, content_hash, metadata, raw_content,
                trust_alpha, trust_beta, reliability_score,
                clearance_level, update_class, supersedes_id,
                content_version, normalized_content, normalized_hash,
                summary
            ) VALUES (
                $1, $2, $3, $4, $5,
                $6, $7,
                $8,
                $9, $10, $11, $12,
                $13, $14, $15,
                $16, $17, $18,
                $19, $20, $21,
                $22
            )",
        )
        .bind(source.id)
        .bind(&source.source_type)
        .bind(&source.uri)
        .bind(&source.title)
        .bind(&source.author)
        .bind(&source.project)
        .bind(&source.domain)
        .bind(source.created_date)
        .bind(source.ingested_at)
        .bind(&source.content_hash)
        .bind(&source.metadata)
        .bind(&source.raw_content)
        .bind(source.trust_alpha)
        .bind(source.trust_beta)
        .bind(source.reliability_score)
        .bind(source.clearance_level.as_i32())
        .bind(&source.update_class)
        .bind(source.supersedes_id)
        .bind(source.content_version)
        .bind(&source.normalized_content)
        .bind(&source.normalized_hash)
        .bind(&source.summary)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get(&self, id: SourceId) -> Result<Option<Source>> {
        let row = sqlx::query(
            "SELECT id, source_type, uri, title, author,
                    project, domain,
                    created_date, ingested_at, content_hash,
                    metadata, raw_content, trust_alpha, trust_beta,
                    reliability_score, clearance_level, update_class,
                    supersedes_id, content_version,
                    normalized_content, normalized_hash, summary
             FROM sources WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| source_from_row(&r)))
    }

    async fn get_by_hash(&self, hash: &[u8]) -> Result<Option<Source>> {
        let row = sqlx::query(
            "SELECT id, source_type, uri, title, author,
                    project, domain,
                    created_date, ingested_at, content_hash,
                    metadata, raw_content, trust_alpha, trust_beta,
                    reliability_score, clearance_level, update_class,
                    supersedes_id, content_version,
                    normalized_content, normalized_hash, summary
             FROM sources WHERE content_hash = $1",
        )
        .bind(hash)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| source_from_row(&r)))
    }

    async fn get_by_normalized_hash(&self, hash: &[u8]) -> Result<Option<Source>> {
        let row = sqlx::query(
            "SELECT id, source_type, uri, title, author,
                    project, domain,
                    created_date, ingested_at, content_hash,
                    metadata, raw_content, trust_alpha, trust_beta,
                    reliability_score, clearance_level, update_class,
                    supersedes_id, content_version,
                    normalized_content, normalized_hash, summary
             FROM sources WHERE normalized_hash = $1",
        )
        .bind(hash)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| source_from_row(&r)))
    }

    async fn update(&self, source: &Source) -> Result<()> {
        sqlx::query(
            "UPDATE sources SET
                source_type = $2, uri = $3, title = $4,
                author = $5, project = $6, domain = $7,
                created_date = $8, ingested_at = $9,
                content_hash = $10, metadata = $11, raw_content = $12,
                trust_alpha = $13, trust_beta = $14,
                reliability_score = $15, clearance_level = $16,
                update_class = $17, supersedes_id = $18,
                content_version = $19,
                normalized_content = $20, normalized_hash = $21,
                summary = $22
             WHERE id = $1",
        )
        .bind(source.id)
        .bind(&source.source_type)
        .bind(&source.uri)
        .bind(&source.title)
        .bind(&source.author)
        .bind(&source.project)
        .bind(&source.domain)
        .bind(source.created_date)
        .bind(source.ingested_at)
        .bind(&source.content_hash)
        .bind(&source.metadata)
        .bind(&source.raw_content)
        .bind(source.trust_alpha)
        .bind(source.trust_beta)
        .bind(source.reliability_score)
        .bind(source.clearance_level.as_i32())
        .bind(&source.update_class)
        .bind(source.supersedes_id)
        .bind(source.content_version)
        .bind(&source.normalized_content)
        .bind(&source.normalized_hash)
        .bind(&source.summary)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete(&self, id: SourceId) -> Result<bool> {
        let result = sqlx::query("DELETE FROM sources WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn list(&self, limit: i64, offset: i64) -> Result<Vec<Source>> {
        let rows = sqlx::query(
            "SELECT id, source_type, uri, title, author,
                    project, domain,
                    created_date, ingested_at, content_hash,
                    metadata, raw_content, trust_alpha, trust_beta,
                    reliability_score, clearance_level, update_class,
                    supersedes_id, content_version,
                    normalized_content, normalized_hash, summary
             FROM sources
             ORDER BY ingested_at DESC
             LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(source_from_row).collect())
    }

    async fn count(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) as count FROM sources")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get("count"))
    }

    async fn update_embedding(&self, id: SourceId, embedding: &[f64]) -> Result<()> {
        let pgvec = format!(
            "[{}]",
            embedding
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        sqlx::query(
            "UPDATE sources SET embedding = $2::halfvec \
             WHERE id = $1",
        )
        .bind(id)
        .bind(&pgvec)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn clear_embedding(&self, id: SourceId) -> Result<()> {
        sqlx::query("UPDATE sources SET embedding = NULL WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn update_summary(&self, id: SourceId, summary: &str) -> Result<()> {
        sqlx::query("UPDATE sources SET summary = $2 WHERE id = $1")
            .bind(id)
            .bind(summary)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

fn source_from_row(row: &sqlx::postgres::PgRow) -> Source {
    let clearance_i32: i32 = row.get("clearance_level");
    Source {
        id: row.get("id"),
        source_type: row.get("source_type"),
        uri: row.get("uri"),
        title: row.get("title"),
        author: row.get("author"),
        project: row.get("project"),
        domain: row.get("domain"),
        created_date: row.get("created_date"),
        ingested_at: row.get("ingested_at"),
        content_hash: row.get("content_hash"),
        metadata: row.get("metadata"),
        raw_content: row.get("raw_content"),
        trust_alpha: row.get("trust_alpha"),
        trust_beta: row.get("trust_beta"),
        reliability_score: row.get("reliability_score"),
        clearance_level: ClearanceLevel::from_i32_or_default(clearance_i32),
        update_class: row.get("update_class"),
        supersedes_id: row.get("supersedes_id"),
        content_version: row.get("content_version"),
        embedding: None, // Loaded separately; halfvec is not directly mapped.
        normalized_content: row.get("normalized_content"),
        normalized_hash: row.get("normalized_hash"),
        summary: row.get("summary"),
    }
}

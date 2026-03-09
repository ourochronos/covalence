//! ArticleRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::models::article::Article;
use crate::storage::traits::ArticleRepo;
use crate::types::clearance::ClearanceLevel;
use crate::types::ids::{ArticleId, NodeId};
use crate::types::opinion::Opinion;

use super::PgRepo;

impl ArticleRepo for PgRepo {
    async fn create(&self, article: &Article) -> Result<()> {
        let confidence_json = article.confidence_breakdown.as_ref().map(|o| o.to_json());
        let source_node_uuids: Vec<uuid::Uuid> =
            article.source_node_ids.iter().map(|id| id.0).collect();

        sqlx::query(
            "INSERT INTO articles (
                id, title, body, confidence,
                confidence_breakdown, domain_path, version,
                content_hash, source_node_ids, clearance_level,
                created_at, updated_at
            ) VALUES (
                $1, $2, $3, $4,
                $5, $6, $7,
                $8, $9, $10,
                $11, $12
            )",
        )
        .bind(article.id)
        .bind(&article.title)
        .bind(&article.body)
        .bind(article.confidence)
        .bind(&confidence_json)
        .bind(&article.domain_path)
        .bind(article.version)
        .bind(&article.content_hash)
        .bind(&source_node_uuids)
        .bind(article.clearance_level.as_i32())
        .bind(article.created_at)
        .bind(article.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get(&self, id: ArticleId) -> Result<Option<Article>> {
        let row = sqlx::query(
            "SELECT id, title, body, confidence,
                    confidence_breakdown, domain_path, version,
                    content_hash, source_node_ids, clearance_level,
                    created_at, updated_at
             FROM articles WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| article_from_row(&r)))
    }

    async fn update(&self, article: &Article) -> Result<()> {
        let confidence_json = article.confidence_breakdown.as_ref().map(|o| o.to_json());
        let source_node_uuids: Vec<uuid::Uuid> =
            article.source_node_ids.iter().map(|id| id.0).collect();

        sqlx::query(
            "UPDATE articles SET
                title = $2, body = $3, confidence = $4,
                confidence_breakdown = $5, domain_path = $6,
                version = $7, content_hash = $8,
                source_node_ids = $9, clearance_level = $10,
                created_at = $11, updated_at = $12
             WHERE id = $1",
        )
        .bind(article.id)
        .bind(&article.title)
        .bind(&article.body)
        .bind(article.confidence)
        .bind(&confidence_json)
        .bind(&article.domain_path)
        .bind(article.version)
        .bind(&article.content_hash)
        .bind(&source_node_uuids)
        .bind(article.clearance_level.as_i32())
        .bind(article.created_at)
        .bind(article.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete(&self, id: ArticleId) -> Result<bool> {
        let result = sqlx::query("DELETE FROM articles WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_by_domain(
        &self,
        domain_prefix: &[String],
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Article>> {
        let rows = sqlx::query(
            "SELECT id, title, body, confidence,
                    confidence_breakdown, domain_path, version,
                    content_hash, source_node_ids, clearance_level,
                    created_at, updated_at
             FROM articles
             WHERE domain_path @> $1
             ORDER BY updated_at DESC
             LIMIT $2 OFFSET $3",
        )
        .bind(domain_prefix)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(article_from_row).collect())
    }
}

fn article_from_row(row: &sqlx::postgres::PgRow) -> Article {
    let clearance_i32: i32 = row.get("clearance_level");
    let confidence_json: Option<serde_json::Value> = row.get("confidence_breakdown");
    let source_uuids: Vec<uuid::Uuid> = row.get("source_node_ids");

    Article {
        id: row.get("id"),
        title: row.get("title"),
        body: row.get("body"),
        confidence: row.get("confidence"),
        confidence_breakdown: confidence_json.as_ref().and_then(Opinion::from_json),
        domain_path: row.get("domain_path"),
        version: row.get("version"),
        content_hash: row.get("content_hash"),
        source_node_ids: source_uuids.into_iter().map(NodeId::from_uuid).collect(),
        clearance_level: ClearanceLevel::from_i32(clearance_i32).unwrap_or_default(),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

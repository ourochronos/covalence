//! SearchTraceRepo and SearchFeedbackRepo implementations for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::models::trace::{SearchFeedback, SearchTrace};
use crate::storage::traits::{SearchFeedbackRepo, SearchTraceRepo};

use super::PgRepo;

impl SearchTraceRepo for PgRepo {
    async fn create(&self, trace: &SearchTrace) -> Result<()> {
        sqlx::query(
            "INSERT INTO search_traces (
                id, query_text, strategy, dimension_counts,
                result_count, execution_ms, created_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(trace.id)
        .bind(&trace.query_text)
        .bind(&trace.strategy)
        .bind(&trace.dimension_counts)
        .bind(trace.result_count)
        .bind(trace.execution_ms)
        .bind(trace.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get(&self, id: uuid::Uuid) -> Result<Option<SearchTrace>> {
        let row = sqlx::query(
            "SELECT id, query_text, strategy, dimension_counts,
                    result_count, execution_ms, created_at
             FROM search_traces WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| trace_from_row(&r)))
    }

    async fn list_recent(&self, limit: i64) -> Result<Vec<SearchTrace>> {
        let rows = sqlx::query(
            "SELECT id, query_text, strategy, dimension_counts,
                    result_count, execution_ms, created_at
             FROM search_traces
             ORDER BY created_at DESC
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(trace_from_row).collect())
    }
}

impl SearchFeedbackRepo for PgRepo {
    async fn create(&self, feedback: &SearchFeedback) -> Result<()> {
        sqlx::query(
            "INSERT INTO search_feedback (
                id, query_text, result_id, relevance,
                comment, created_at
            ) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(feedback.id)
        .bind(&feedback.query_text)
        .bind(feedback.result_id)
        .bind(feedback.relevance)
        .bind(&feedback.comment)
        .bind(feedback.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_recent(&self, limit: i64) -> Result<Vec<SearchFeedback>> {
        let rows = sqlx::query(
            "SELECT id, query_text, result_id, relevance,
                    comment, created_at
             FROM search_feedback
             ORDER BY created_at DESC
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(feedback_from_row).collect())
    }
}

/// Convert a PG row to a SearchTrace.
fn trace_from_row(row: &sqlx::postgres::PgRow) -> SearchTrace {
    SearchTrace {
        id: row.get("id"),
        query_text: row.get("query_text"),
        strategy: row.get("strategy"),
        dimension_counts: row.get("dimension_counts"),
        result_count: row.get("result_count"),
        execution_ms: row.get("execution_ms"),
        created_at: row.get("created_at"),
    }
}

/// Convert a PG row to a SearchFeedback.
fn feedback_from_row(row: &sqlx::postgres::PgRow) -> SearchFeedback {
    SearchFeedback {
        id: row.get("id"),
        query_text: row.get("query_text"),
        result_id: row.get("result_id"),
        relevance: row.get("relevance"),
        comment: row.get("comment"),
        created_at: row.get("created_at"),
    }
}

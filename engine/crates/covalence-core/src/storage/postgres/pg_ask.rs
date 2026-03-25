//! AskRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::storage::traits::AskRepo;

use super::PgRepo;

impl AskRepo for PgRepo {
    async fn get_outgoing_edges(
        &self,
        node_id: uuid::Uuid,
        rel_types: &[String],
        limit: i64,
    ) -> Result<Vec<(String, String)>> {
        let rows = sqlx::query("SELECT * FROM sp_get_outgoing_edges($1, $2, $3)")
            .bind(node_id)
            .bind(rel_types)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let name: String = r.get("canonical_name");
                let rel: String = r.get("rel_type");
                (name, rel)
            })
            .collect())
    }

    async fn get_incoming_edges(
        &self,
        node_id: uuid::Uuid,
        rel_types: &[String],
        limit: i64,
    ) -> Result<Vec<(String, String)>> {
        let rows = sqlx::query("SELECT * FROM sp_get_incoming_edges($1, $2, $3)")
            .bind(node_id)
            .bind(rel_types)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let name: String = r.get("canonical_name");
                let rel: String = r.get("rel_type");
                (name, rel)
            })
            .collect())
    }
}

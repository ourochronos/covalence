//! NodeAliasRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::models::node_alias::NodeAlias;
use crate::storage::traits::NodeAliasRepo;
use crate::types::ids::{AliasId, NodeId};

use super::PgRepo;

impl NodeAliasRepo for PgRepo {
    async fn create(&self, alias: &NodeAlias) -> Result<()> {
        sqlx::query(
            "INSERT INTO node_aliases (
                id, node_id, alias, source_chunk_id
            ) VALUES ($1, $2, $3, $4)",
        )
        .bind(alias.id)
        .bind(alias.node_id)
        .bind(&alias.alias)
        .bind(alias.source_chunk_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get(&self, id: AliasId) -> Result<Option<NodeAlias>> {
        let row = sqlx::query(
            "SELECT id, node_id, alias, source_chunk_id
             FROM node_aliases WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| alias_from_row(&r)))
    }

    async fn list_by_node(&self, node_id: NodeId) -> Result<Vec<NodeAlias>> {
        let rows = sqlx::query(
            "SELECT id, node_id, alias, source_chunk_id
             FROM node_aliases
             WHERE node_id = $1
             ORDER BY alias ASC",
        )
        .bind(node_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(alias_from_row).collect())
    }

    async fn find_by_alias(&self, alias: &str) -> Result<Vec<NodeAlias>> {
        let rows = sqlx::query(
            "SELECT id, node_id, alias, source_chunk_id
             FROM node_aliases
             WHERE alias ILIKE $1
             ORDER BY alias ASC",
        )
        .bind(format!("%{alias}%"))
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(alias_from_row).collect())
    }

    async fn delete(&self, id: AliasId) -> Result<bool> {
        let result = sqlx::query("DELETE FROM node_aliases WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

fn alias_from_row(row: &sqlx::postgres::PgRow) -> NodeAlias {
    NodeAlias {
        id: row.get("id"),
        node_id: row.get("node_id"),
        alias: row.get("alias"),
        source_chunk_id: row.get("source_chunk_id"),
    }
}

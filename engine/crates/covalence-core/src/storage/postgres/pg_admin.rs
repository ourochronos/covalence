//! AdminRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::storage::traits::AdminRepo;
use crate::types::ids::NodeId;

use super::PgRepo;

#[allow(clippy::type_complexity)]
impl AdminRepo for PgRepo {
    async fn ping(&self) -> Result<bool> {
        Ok(sqlx::query("SELECT 1").execute(&self.pool).await.is_ok())
    }

    async fn data_health_report(&self) -> Result<(i64, i64, i64, i64, i64, i64, i64, i64)> {
        let row: (i64, i64, i64, i64, i64, i64, i64, i64) =
            sqlx::query_as("SELECT * FROM sp_data_health_report()")
                .fetch_one(&self.pool)
                .await?;
        Ok(row)
    }

    async fn metrics_counts(&self) -> Result<(i64, i64, i64, i64)> {
        let chunk_row = sqlx::query("SELECT COUNT(*) as count FROM chunks")
            .fetch_one(&self.pool)
            .await?;
        let chunk_count: i64 = chunk_row.get("count");

        let article_row = sqlx::query("SELECT COUNT(*) as count FROM articles")
            .fetch_one(&self.pool)
            .await?;
        let article_count: i64 = article_row.get("count");

        let trace_row = sqlx::query("SELECT COUNT(*) as count FROM search_traces")
            .fetch_one(&self.pool)
            .await?;
        let search_trace_count: i64 = trace_row.get("count");

        let summary_row = sqlx::query(
            "SELECT COUNT(*) as count FROM chunks \
             WHERE level LIKE 'summary_%'",
        )
        .fetch_one(&self.pool)
        .await?;
        let summary_chunk_count: i64 = summary_row.get("count");

        Ok((
            chunk_count,
            article_count,
            search_trace_count,
            summary_chunk_count,
        ))
    }

    async fn list_all_nodes(&self) -> Result<Vec<(uuid::Uuid, String, String)>> {
        let rows: Vec<(uuid::Uuid, String, String)> =
            sqlx::query_as("SELECT id, canonical_name, node_type FROM nodes")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    async fn count_edges_for_node(&self, node_id: uuid::Uuid) -> Result<i64> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM edges \
             WHERE source_node_id = $1 \
                OR target_node_id = $1",
        )
        .bind(node_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(count)
    }

    async fn nullify_invalidated_by_for_node(&self, node_id: uuid::Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE edges SET invalidated_by = NULL \
             WHERE invalidated_by IN ( \
                 SELECT id FROM edges \
                 WHERE source_node_id = $1 \
                    OR target_node_id = $1 \
             )",
        )
        .bind(node_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn clear_unresolved_for_node(&self, node_id: uuid::Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE unresolved_entities \
             SET resolved_node_id = NULL \
             WHERE resolved_node_id = $1",
        )
        .bind(node_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_nodes_without_embeddings(
        &self,
        limit: i32,
    ) -> Result<Vec<(uuid::Uuid, String, Option<String>)>> {
        let rows: Vec<(uuid::Uuid, String, Option<String>)> =
            sqlx::query_as("SELECT * FROM sp_list_nodes_without_embeddings($1)")
                .bind(limit)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    async fn list_all_node_ids(&self) -> Result<Vec<uuid::Uuid>> {
        let ids: Vec<uuid::Uuid> = sqlx::query_scalar("SELECT id FROM nodes")
            .fetch_all(&self.pool)
            .await?;
        Ok(ids)
    }

    async fn list_all_nonsynthetic_edge_ids(&self) -> Result<Vec<uuid::Uuid>> {
        let ids: Vec<uuid::Uuid> =
            sqlx::query_scalar("SELECT id FROM edges WHERE NOT is_synthetic")
                .fetch_all(&self.pool)
                .await?;
        Ok(ids)
    }

    async fn list_unsummarized_code_nodes(
        &self,
        code_types: &[&str],
    ) -> Result<
        Vec<(
            uuid::Uuid,
            String,
            String,
            Option<String>,
            Option<serde_json::Value>,
        )>,
    > {
        let rows: Vec<(
            uuid::Uuid,
            String,
            String,
            Option<String>,
            Option<serde_json::Value>,
        )> = sqlx::query_as(
            "SELECT id, canonical_name, node_type, \
                    description, properties \
                 FROM nodes \
                 WHERE node_type = ANY($1) \
                   AND (properties IS NULL \
                        OR properties->>'semantic_summary' \
                           IS NULL)",
        )
        .bind(code_types)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn update_node_properties(
        &self,
        id: uuid::Uuid,
        properties: &serde_json::Value,
    ) -> Result<()> {
        sqlx::query("UPDATE nodes SET properties = $2 WHERE id = $1")
            .bind(id)
            .bind(properties)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn invalidated_edge_stats(&self) -> Result<(i64, i64)> {
        let row: (i64, i64) = sqlx::query_as("SELECT * FROM sp_invalidated_edge_stats()")
            .fetch_one(&self.pool)
            .await?;
        Ok(row)
    }

    async fn top_invalidated_rel_types(&self, limit: i32) -> Result<Vec<(String, i64)>> {
        let rows: Vec<(String, i64)> =
            sqlx::query_as("SELECT * FROM sp_top_invalidated_rel_types($1)")
                .bind(limit)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    async fn top_invalidated_edge_nodes(
        &self,
        limit: i64,
    ) -> Result<Vec<(uuid::Uuid, String, String, i64)>> {
        let rows: Vec<(uuid::Uuid, String, String, i64)> = sqlx::query_as(
            "SELECT n.id, n.canonical_name, n.node_type, cnt \
             FROM ( \
                 SELECT node_id, SUM(c) AS cnt FROM ( \
                     SELECT source_node_id AS node_id, COUNT(*) AS c \
                     FROM edges WHERE invalid_at IS NOT NULL \
                     GROUP BY 1 \
                     UNION ALL \
                     SELECT target_node_id AS node_id, COUNT(*) AS c \
                     FROM edges WHERE invalid_at IS NOT NULL \
                     GROUP BY 1 \
                 ) sub \
                 GROUP BY node_id \
             ) agg \
             JOIN nodes n ON n.id = agg.node_id \
             ORDER BY cnt DESC \
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn find_cooccurrence_pairs(
        &self,
        min_cooccurrences: i32,
        max_degree: i32,
    ) -> Result<Vec<(uuid::Uuid, uuid::Uuid, i64)>> {
        let rows: Vec<(uuid::Uuid, uuid::Uuid, i64)> =
            sqlx::query_as("SELECT * FROM sp_find_cooccurrence_pairs($1, $2)")
                .bind(min_cooccurrences)
                .bind(max_degree)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    async fn list_code_nodes_with_embeddings(
        &self,
        code_types: &[String],
    ) -> Result<Vec<(uuid::Uuid, String, String)>> {
        let rows: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(
            "SELECT id, canonical_name, node_type FROM nodes \
                 WHERE node_type = ANY($1) \
                   AND embedding IS NOT NULL",
        )
        .bind(code_types)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn find_nearest_non_code_nodes(
        &self,
        code_id: uuid::Uuid,
        code_types: &[String],
        limit: i64,
    ) -> Result<Vec<(uuid::Uuid, String, f64)>> {
        let rows: Vec<(uuid::Uuid, String, f64)> = sqlx::query_as(
            "SELECT n.id, n.canonical_name, \
                    (n.embedding <=> (SELECT embedding \
                     FROM nodes WHERE id = $1)) AS dist \
             FROM nodes n \
             WHERE n.node_type != ALL($3) \
               AND n.embedding IS NOT NULL \
               AND n.id != $1 \
             ORDER BY dist ASC \
             LIMIT $2",
        )
        .bind(code_id)
        .bind(limit)
        .bind(code_types)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn check_edge_exists(
        &self,
        source_id: uuid::Uuid,
        target_id: uuid::Uuid,
        rel_type: &str,
    ) -> Result<bool> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM edges \
             WHERE source_node_id = $1 \
               AND target_node_id = $2 \
               AND rel_type = $3)",
        )
        .bind(source_id)
        .bind(target_id)
        .bind(rel_type)
        .fetch_one(&self.pool)
        .await?;
        Ok(exists)
    }

    async fn get_node_provenance_sources(
        &self,
        node_ids: &[uuid::Uuid],
    ) -> Result<Vec<(uuid::Uuid, Option<String>, Option<String>)>> {
        let rows = sqlx::query_as::<_, (uuid::Uuid, Option<String>, Option<String>)>(
            "SELECT * FROM sp_get_node_provenance_sources($1)",
        )
        .bind(node_ids)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn tombstone_node(&self, node_id: NodeId) -> Result<()> {
        sqlx::query("SELECT sp_tombstone_node($1)")
            .bind(node_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_invalidated_edges_for_node(
        &self,
        node_id: uuid::Uuid,
        limit: i32,
    ) -> Result<Vec<(uuid::Uuid, uuid::Uuid)>> {
        let rows: Vec<(uuid::Uuid, uuid::Uuid)> = sqlx::query_as(
            "SELECT source_node_id, target_node_id \
             FROM sp_get_invalidated_edges_for_node($1, $2)",
        )
        .bind(node_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

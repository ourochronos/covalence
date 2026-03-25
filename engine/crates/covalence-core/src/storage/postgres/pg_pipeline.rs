//! PipelineRepo implementation for PostgreSQL.
//!
//! Handles entity resolution, ingestion, and source processing queries
//! that were previously raw SQL in the pipeline/source service layer.

use crate::error::Result;
use crate::models::node::Node;
use crate::storage::traits::PipelineRepo;
use crate::types::ids::{AliasId, ChunkId, NodeId, SourceId, StatementId};

use super::PgRepo;

impl PipelineRepo for PgRepo {
    async fn advisory_xact_lock(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        key: i64,
    ) -> Result<()> {
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(key)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    async fn get_node_by_name_exact_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        name: &str,
    ) -> Result<Option<NodeId>> {
        let existing: Option<NodeId> = sqlx::query_scalar("SELECT sp_get_node_by_name_exact($1)")
            .bind(name)
            .fetch_one(&mut **tx)
            .await?;
        Ok(existing)
    }

    async fn bump_node_mention_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        node_id: NodeId,
    ) -> Result<()> {
        sqlx::query("SELECT sp_bump_node_mention($1)")
            .bind(node_id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    async fn get_node_properties_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        node_id: NodeId,
    ) -> Result<Option<serde_json::Value>> {
        let (props_val,): (Option<serde_json::Value>,) =
            sqlx::query_as("SELECT sp_get_node_properties($1)")
                .bind(node_id)
                .fetch_one(&mut **tx)
                .await?;
        Ok(props_val)
    }

    async fn update_node_ast_hash_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        node_id: NodeId,
        props: &serde_json::Value,
        description: &Option<String>,
    ) -> Result<()> {
        sqlx::query("SELECT sp_update_node_ast_hash($1, $2, $3)")
            .bind(node_id)
            .bind(props)
            .bind(description)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    async fn update_node_ast_hash_only_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        node_id: NodeId,
        hash: &str,
    ) -> Result<()> {
        sqlx::query("SELECT sp_update_node_ast_hash_only($1, $2)")
            .bind(node_id)
            .bind(hash)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    async fn create_extraction_from_chunk_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        ext_id: uuid::Uuid,
        chunk_id: ChunkId,
        entity_id: uuid::Uuid,
        method: &str,
        confidence: f64,
    ) -> Result<()> {
        sqlx::query("SELECT sp_create_extraction_from_chunk($1, $2, $3, $4, $5)")
            .bind(ext_id)
            .bind(chunk_id)
            .bind(entity_id)
            .bind(method)
            .bind(confidence)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    async fn create_extraction_from_statement_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        ext_id: uuid::Uuid,
        stmt_id: StatementId,
        entity_id: uuid::Uuid,
        method: &str,
        confidence: f64,
    ) -> Result<()> {
        sqlx::query("SELECT sp_create_extraction_from_statement($1, $2, $3, $4, $5)")
            .bind(ext_id)
            .bind(stmt_id)
            .bind(entity_id)
            .bind(method)
            .bind(confidence)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    async fn get_alias_by_text_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        alias_text: &str,
    ) -> Result<Option<uuid::Uuid>> {
        let existing: Option<uuid::Uuid> = sqlx::query_scalar("SELECT sp_get_alias_by_text($1)")
            .bind(alias_text)
            .fetch_one(&mut **tx)
            .await?;
        Ok(existing)
    }

    async fn create_alias_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        alias_id: AliasId,
        node_id: NodeId,
        alias_text: &str,
        chunk_id: ChunkId,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO node_aliases \
             (id, node_id, alias, source_chunk_id) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(alias_id)
        .bind(node_id)
        .bind(alias_text)
        .bind(chunk_id)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    async fn create_node_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        node: &Node,
    ) -> Result<()> {
        let confidence_json = node.confidence_breakdown.as_ref().map(|o| o.to_json());
        sqlx::query(
            "INSERT INTO nodes (
                id, canonical_name, node_type, entity_class,
                description,
                properties, confidence_breakdown,
                clearance_level, first_seen, last_seen,
                mention_count
            ) VALUES (
                $1, $2, $3, $4,
                $5,
                $6, $7,
                $8, $9, $10,
                $11
            )",
        )
        .bind(node.id)
        .bind(&node.canonical_name)
        .bind(&node.node_type)
        .bind(&node.entity_class)
        .bind(&node.description)
        .bind(&node.properties)
        .bind(&confidence_json)
        .bind(node.clearance_level.as_i32())
        .bind(node.first_seen)
        .bind(node.last_seen)
        .bind(node.mention_count)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    async fn get_chunk_content_for_entity(
        &self,
        node_id: NodeId,
        def_pattern: &str,
    ) -> Result<Option<String>> {
        let content: Option<String> =
            sqlx::query_scalar("SELECT sp_get_chunk_content_for_entity($1, $2)")
                .bind(node_id)
                .bind(def_pattern)
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten();
        Ok(content)
    }

    async fn get_chunk_by_source_pattern(
        &self,
        source_id: SourceId,
        def_pattern: &str,
    ) -> Result<Option<String>> {
        let content: Option<String> =
            sqlx::query_scalar("SELECT sp_get_chunk_by_source_pattern($1, $2)")
                .bind(source_id)
                .bind(def_pattern)
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten();
        Ok(content)
    }

    async fn update_node_semantic_summary(&self, node_id: NodeId, summary: &str) -> Result<()> {
        sqlx::query("SELECT sp_update_node_semantic_summary($1, $2)")
            .bind(node_id)
            .bind(summary)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn update_node_processing(
        &self,
        node_id: NodeId,
        metadata: &serde_json::Value,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE nodes SET processing = jsonb_set(\
               COALESCE(processing, '{}'), '{summary}', $2::jsonb\
             ) WHERE id = $1",
        )
        .bind(node_id)
        .bind(metadata)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_chunk_processing(
        &self,
        chunk_id: uuid::Uuid,
        stage: &str,
        metadata: &serde_json::Value,
    ) -> Result<()> {
        sqlx::query("SELECT sp_update_chunk_processing($1, $2, $3)")
            .bind(chunk_id)
            .bind(stage)
            .bind(metadata)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_entity_summaries_by_source(
        &self,
        source_id: SourceId,
    ) -> Result<Vec<(String, String, String)>> {
        let rows: Vec<(String, String, String)> =
            sqlx::query_as("SELECT * FROM sp_get_entity_summaries_by_source($1)")
                .bind(source_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    async fn update_source_processing(
        &self,
        source_id: SourceId,
        stage: &str,
        metadata: &serde_json::Value,
    ) -> Result<()> {
        sqlx::query("SELECT sp_update_source_processing($1, $2, $3)")
            .bind(source_id)
            .bind(stage)
            .bind(metadata)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn update_source_status(&self, source_id: SourceId, status: &str) -> Result<()> {
        sqlx::query("SELECT sp_update_source_status($1, $2)")
            .bind(source_id)
            .bind(status)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn update_source_status_conditional(
        &self,
        source_id: SourceId,
        new_status: &str,
        unless_status: &str,
    ) -> Result<()> {
        sqlx::query("SELECT sp_update_source_status_conditional($1, $2, $3)")
            .bind(source_id)
            .bind(new_status)
            .bind(unless_status)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn node_has_embedding(&self, node_id: uuid::Uuid) -> Result<bool> {
        let has: bool = sqlx::query_scalar("SELECT embedding IS NOT NULL FROM nodes WHERE id = $1")
            .bind(node_id)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(true);
        Ok(has)
    }

    async fn list_node_ids_with_embeddings(
        &self,
        node_ids: &[uuid::Uuid],
    ) -> Result<Vec<uuid::Uuid>> {
        let ids: Vec<uuid::Uuid> = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT id FROM nodes \
                 WHERE id = ANY($1) \
                 AND embedding IS NOT NULL",
        )
        .bind(node_ids)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        Ok(ids)
    }

    async fn list_chunk_ids_by_source(&self, source_id: SourceId) -> Result<Vec<(uuid::Uuid,)>> {
        let rows: Vec<(uuid::Uuid,)> = sqlx::query_as("SELECT id FROM chunks WHERE source_id = $1")
            .bind(source_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    async fn update_source_supersession_metadata(
        &self,
        source_id: SourceId,
        metadata: &serde_json::Value,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE sources SET metadata = jsonb_set(\
               COALESCE(metadata, '{}'), '{_supersession}', $2::jsonb\
             ) WHERE id = $1",
        )
        .bind(source_id)
        .bind(metadata)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn mark_source_superseded(
        &self,
        old_id: SourceId,
        new_id: SourceId,
        update_class: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE sources \
             SET update_class = $2, superseded_by = $3, superseded_at = NOW() \
             WHERE id = $1",
        )
        .bind(old_id)
        .bind(update_class)
        .bind(new_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn find_source_by_uri(
        &self,
        uri: &str,
    ) -> Result<Option<(SourceId, Option<String>, i32)>> {
        use sqlx::Row;
        let row = sqlx::query(
            "SELECT id, raw_content, content_version \
             FROM sources \
             WHERE uri = $1 \
             ORDER BY content_version DESC \
             LIMIT 1",
        )
        .bind(uri)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| {
            let id: SourceId = r.get("id");
            let raw_content: Option<String> = r.get("raw_content");
            let content_version: i32 = r.get("content_version");
            (id, raw_content, content_version)
        }))
    }

    async fn delete_source_cascade(
        &self,
        source_id: SourceId,
    ) -> Result<(i64, i64, i64, i64, i64, i64, i64)> {
        let cascade: (i64, i64, i64, i64, i64, i64, i64) =
            sqlx::query_as("SELECT * FROM sp_delete_source_cascade($1)")
                .bind(source_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(cascade)
    }

    async fn find_extraction_chunk_content(
        &self,
        node_id: NodeId,
        def_pattern: &str,
    ) -> Result<Option<String>> {
        let content: Option<String> = sqlx::query_scalar(
            "SELECT c.content FROM extractions ex \
             JOIN chunks c ON c.id = ex.chunk_id \
             WHERE ex.entity_id = $1 AND ex.entity_type = 'node' \
               AND ex.chunk_id IS NOT NULL \
               AND c.content LIKE '%' || $2 || '%' \
             ORDER BY ex.confidence DESC \
             LIMIT 1",
        )
        .bind(node_id)
        .bind(def_pattern)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();
        Ok(content)
    }

    async fn find_source_chunk_content(
        &self,
        source_id: SourceId,
        def_pattern: &str,
    ) -> Result<Option<String>> {
        let content: Option<String> = sqlx::query_scalar(
            "SELECT c.content FROM chunks c \
             WHERE c.source_id = $1 \
               AND c.content LIKE '%' || $2 || '%' \
             ORDER BY LENGTH(c.content) ASC \
             LIMIT 1",
        )
        .bind(source_id)
        .bind(def_pattern)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();
        Ok(content)
    }

    async fn update_node_summary_inline(
        &self,
        node_id: NodeId,
        summary_json: &serde_json::Value,
        description: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE nodes SET \
               properties = jsonb_set(\
                 COALESCE(properties, '{}'), \
                 '{semantic_summary}', \
                 $2::jsonb\
               ), \
               description = $3, \
               embedding = NULL \
             WHERE id = $1",
        )
        .bind(node_id)
        .bind(summary_json)
        .bind(description)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_code_entity_summaries(
        &self,
        source_id: SourceId,
        code_entity_class: &str,
    ) -> Result<Vec<(String, String, String)>> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT canonical_name, node_type, \
                    COALESCE(properties->>'semantic_summary', \
                             description, canonical_name) \
             FROM nodes n \
             JOIN extractions ex ON ex.entity_id = n.id \
               AND ex.entity_type = 'node' \
             JOIN chunks c ON c.id = ex.chunk_id \
             WHERE c.source_id = $1 \
               AND n.entity_class = $2 \
             GROUP BY n.id, canonical_name, node_type, \
                      properties, description \
             ORDER BY n.node_type, n.canonical_name",
        )
        .bind(source_id)
        .bind(code_entity_class)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        Ok(rows)
    }
}

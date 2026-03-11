//! ChunkRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::models::chunk::Chunk;
use crate::storage::traits::ChunkRepo;
use crate::types::clearance::ClearanceLevel;
use crate::types::ids::{ChunkId, SourceId};

use super::PgRepo;

impl ChunkRepo for PgRepo {
    async fn create(&self, chunk: &Chunk) -> Result<()> {
        sqlx::query(
            "INSERT INTO chunks (
                id, source_id, parent_chunk_id, level,
                ordinal, content, content_hash,
                contextual_prefix, token_count,
                structural_hierarchy,
                clearance_level, parent_alignment,
                extraction_method, landscape_metrics,
                metadata, byte_start, byte_end,
                content_offset, created_at
            ) VALUES (
                $1, $2, $3, $4,
                $5, $6, $7,
                $8, $9,
                $10::ltree,
                $11, $12,
                $13, $14,
                $15, $16, $17,
                $18, $19
            )",
        )
        .bind(chunk.id)
        .bind(chunk.source_id)
        .bind(chunk.parent_chunk_id)
        .bind(&chunk.level)
        .bind(chunk.ordinal)
        .bind(&chunk.content)
        .bind(&chunk.content_hash)
        .bind(&chunk.contextual_prefix)
        .bind(chunk.token_count)
        .bind(&chunk.structural_hierarchy)
        .bind(chunk.clearance_level.as_i32())
        .bind(chunk.parent_alignment)
        .bind(&chunk.extraction_method)
        .bind(&chunk.landscape_metrics)
        .bind(&chunk.metadata)
        .bind(chunk.byte_start)
        .bind(chunk.byte_end)
        .bind(chunk.content_offset)
        .bind(chunk.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn batch_create(&self, chunks: &[Chunk]) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }

        let ids: Vec<uuid::Uuid> = chunks.iter().map(|c| c.id.into_uuid()).collect();
        let source_ids: Vec<uuid::Uuid> = chunks.iter().map(|c| c.source_id.into_uuid()).collect();
        let parent_ids: Vec<Option<uuid::Uuid>> = chunks
            .iter()
            .map(|c| c.parent_chunk_id.map(|p| p.into_uuid()))
            .collect();
        let levels: Vec<&str> = chunks.iter().map(|c| c.level.as_str()).collect();
        let ordinals: Vec<i32> = chunks.iter().map(|c| c.ordinal).collect();
        let contents: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
        let content_hashes: Vec<&[u8]> = chunks.iter().map(|c| c.content_hash.as_slice()).collect();
        let prefixes: Vec<Option<&str>> = chunks
            .iter()
            .map(|c| c.contextual_prefix.as_deref())
            .collect();
        let token_counts: Vec<i32> = chunks.iter().map(|c| c.token_count).collect();
        let hierarchies: Vec<&str> = chunks
            .iter()
            .map(|c| c.structural_hierarchy.as_str())
            .collect();
        let clearances: Vec<i32> = chunks.iter().map(|c| c.clearance_level.as_i32()).collect();
        let parent_alignments: Vec<Option<f64>> =
            chunks.iter().map(|c| c.parent_alignment).collect();
        let extraction_methods: Vec<Option<&str>> = chunks
            .iter()
            .map(|c| c.extraction_method.as_deref())
            .collect();
        let landscape_metrics_vec: Vec<Option<&serde_json::Value>> = chunks
            .iter()
            .map(|c| c.landscape_metrics.as_ref())
            .collect();
        let metadatas: Vec<&serde_json::Value> = chunks.iter().map(|c| &c.metadata).collect();
        let byte_starts: Vec<Option<i32>> = chunks.iter().map(|c| c.byte_start).collect();
        let byte_ends: Vec<Option<i32>> = chunks.iter().map(|c| c.byte_end).collect();
        let content_offsets: Vec<Option<i32>> = chunks.iter().map(|c| c.content_offset).collect();
        let created_ats: Vec<chrono::DateTime<chrono::Utc>> =
            chunks.iter().map(|c| c.created_at).collect();

        sqlx::query(
            "INSERT INTO chunks (
                id, source_id, parent_chunk_id, level,
                ordinal, content, content_hash,
                contextual_prefix, token_count,
                structural_hierarchy,
                clearance_level, parent_alignment,
                extraction_method, landscape_metrics,
                metadata, byte_start, byte_end,
                content_offset, created_at
            )
            SELECT * FROM UNNEST(
                $1::uuid[], $2::uuid[], $3::uuid[],
                $4::text[], $5::int4[],
                $6::text[], $7::bytea[], $8::text[],
                $9::int4[], $10::ltree[],
                $11::int4[], $12::float8[],
                $13::text[], $14::jsonb[],
                $15::jsonb[], $16::int4[], $17::int4[],
                $18::int4[], $19::timestamptz[]
            )",
        )
        .bind(&ids)
        .bind(&source_ids)
        .bind(&parent_ids)
        .bind(&levels)
        .bind(&ordinals)
        .bind(&contents)
        .bind(&content_hashes)
        .bind(&prefixes)
        .bind(&token_counts)
        .bind(&hierarchies)
        .bind(&clearances)
        .bind(&parent_alignments)
        .bind(&extraction_methods)
        .bind(&landscape_metrics_vec)
        .bind(&metadatas)
        .bind(&byte_starts)
        .bind(&byte_ends)
        .bind(&content_offsets)
        .bind(&created_ats)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get(&self, id: ChunkId) -> Result<Option<Chunk>> {
        let row = sqlx::query(
            "SELECT id, source_id, parent_chunk_id, level,
                    ordinal, content, content_hash,
                    contextual_prefix, token_count,
                    structural_hierarchy::text,
                    clearance_level, parent_alignment,
                    extraction_method, landscape_metrics,
                    metadata, byte_start, byte_end,
                    content_offset, created_at
             FROM chunks WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| chunk_from_row(&r)))
    }

    async fn list_by_source(&self, source_id: SourceId) -> Result<Vec<Chunk>> {
        let rows = sqlx::query(
            "SELECT id, source_id, parent_chunk_id, level,
                    ordinal, content, content_hash,
                    contextual_prefix, token_count,
                    structural_hierarchy::text,
                    clearance_level, parent_alignment,
                    extraction_method, landscape_metrics,
                    metadata, byte_start, byte_end,
                    content_offset, created_at
             FROM chunks
             WHERE source_id = $1
             ORDER BY ordinal ASC",
        )
        .bind(source_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(chunk_from_row).collect())
    }

    async fn list_children(&self, parent_id: ChunkId) -> Result<Vec<Chunk>> {
        let rows = sqlx::query(
            "SELECT id, source_id, parent_chunk_id, level,
                    ordinal, content, content_hash,
                    contextual_prefix, token_count,
                    structural_hierarchy::text,
                    clearance_level, parent_alignment,
                    extraction_method, landscape_metrics,
                    metadata, byte_start, byte_end,
                    content_offset, created_at
             FROM chunks
             WHERE parent_chunk_id = $1
             ORDER BY ordinal ASC",
        )
        .bind(parent_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(chunk_from_row).collect())
    }

    async fn delete(&self, id: ChunkId) -> Result<bool> {
        let result = sqlx::query("DELETE FROM chunks WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn delete_by_source(&self, source_id: SourceId) -> Result<u64> {
        let result = sqlx::query("DELETE FROM chunks WHERE source_id = $1")
            .bind(source_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn update_parent_alignment(&self, id: ChunkId, alignment: f64) -> Result<()> {
        sqlx::query(
            "UPDATE chunks \
             SET parent_alignment = $2 \
             WHERE id = $1",
        )
        .bind(id)
        .bind(alignment)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_landscape(
        &self,
        id: ChunkId,
        parent_alignment: Option<f64>,
        extraction_method: &str,
        landscape_metrics: Option<serde_json::Value>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE chunks
             SET parent_alignment = $2,
                 extraction_method = $3,
                 landscape_metrics = $4
             WHERE id = $1",
        )
        .bind(id)
        .bind(parent_alignment)
        .bind(extraction_method)
        .bind(&landscape_metrics)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_landscape_metrics(
        &self,
        id: ChunkId,
        metrics: serde_json::Value,
    ) -> Result<()> {
        // Merge new keys into the existing JSONB object using
        // PostgreSQL's `||` concatenation operator. If the column
        // is NULL, COALESCE seeds it with an empty object.
        sqlx::query(
            "UPDATE chunks \
             SET landscape_metrics = \
                 COALESCE(landscape_metrics, '{}'::jsonb) || $2 \
             WHERE id = $1",
        )
        .bind(id)
        .bind(&metrics)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_embedding(&self, id: ChunkId, embedding: &[f64]) -> Result<()> {
        // Format as pgvector literal and cast to halfvec.
        let pgvec = format!(
            "[{}]",
            embedding
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        sqlx::query(
            "UPDATE chunks \
             SET embedding = $1::halfvec \
             WHERE id = $2",
        )
        .bind(&pgvec)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn chunk_from_row(row: &sqlx::postgres::PgRow) -> Chunk {
    let clearance_i32: i32 = row.get("clearance_level");
    Chunk {
        id: row.get("id"),
        source_id: row.get("source_id"),
        parent_chunk_id: row.get("parent_chunk_id"),
        level: row.get("level"),
        ordinal: row.get("ordinal"),
        content: row.get("content"),
        content_hash: row.get("content_hash"),
        contextual_prefix: row.get("contextual_prefix"),
        token_count: row.get("token_count"),
        structural_hierarchy: row.get("structural_hierarchy"),
        clearance_level: ClearanceLevel::from_i32(clearance_i32).unwrap_or_default(),
        parent_alignment: row.get("parent_alignment"),
        extraction_method: row.get("extraction_method"),
        landscape_metrics: row.get("landscape_metrics"),
        metadata: row.get("metadata"),
        byte_start: row.get("byte_start"),
        byte_end: row.get("byte_end"),
        content_offset: row.get("content_offset"),
        created_at: row.get("created_at"),
    }
}

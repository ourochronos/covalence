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
                clearance_level,
                metadata, byte_start, byte_end,
                content_offset, created_at
            ) VALUES (
                $1, $2, $3, $4,
                $5, $6, $7,
                $8, $9,
                $10::ltree,
                $11,
                $12, $13, $14,
                $15, $16
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
                clearance_level,
                metadata, byte_start, byte_end,
                content_offset, created_at
            )
            SELECT * FROM UNNEST(
                $1::uuid[], $2::uuid[], $3::uuid[],
                $4::text[], $5::int4[],
                $6::text[], $7::bytea[], $8::text[],
                $9::int4[], $10::ltree[],
                $11::int4[],
                $12::jsonb[], $13::int4[], $14::int4[],
                $15::int4[], $16::timestamptz[]
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
                    clearance_level,
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
                    clearance_level,
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
                    clearance_level,
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
        clearance_level: ClearanceLevel::from_i32_or_default(clearance_i32),
        metadata: row.get("metadata"),
        byte_start: row.get("byte_start"),
        byte_end: row.get("byte_end"),
        content_offset: row.get("content_offset"),
        created_at: row.get("created_at"),
    }
}

//! AgentMemoryRepo implementation for PostgreSQL.

use sqlx::Row;

use crate::error::Result;
use crate::models::agent_memory::AgentMemory;
use crate::storage::traits::AgentMemoryRepo;

use super::PgRepo;

impl AgentMemoryRepo for PgRepo {
    async fn create(&self, memory: &AgentMemory) -> Result<()> {
        sqlx::query(
            "INSERT INTO agent_memories \
             (id, source_id, agent_id, topic, task_id, confidence, \
              access_count, last_accessed, expires_at, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(memory.id)
        .bind(memory.source_id)
        .bind(&memory.agent_id)
        .bind(&memory.topic)
        .bind(&memory.task_id)
        .bind(memory.confidence)
        .bind(memory.access_count)
        .bind(memory.last_accessed)
        .bind(memory.expires_at)
        .bind(memory.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get(&self, id: uuid::Uuid) -> Result<Option<AgentMemory>> {
        let row = sqlx::query(
            "SELECT id, source_id, agent_id, topic, task_id, \
                    confidence, access_count, last_accessed, \
                    expires_at, created_at \
             FROM agent_memories WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| memory_from_row(&r)))
    }

    async fn get_by_source(&self, source_id: uuid::Uuid) -> Result<Option<AgentMemory>> {
        let row = sqlx::query(
            "SELECT id, source_id, agent_id, topic, task_id, \
                    confidence, access_count, last_accessed, \
                    expires_at, created_at \
             FROM agent_memories WHERE source_id = $1",
        )
        .bind(source_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| memory_from_row(&r)))
    }

    async fn list_by_agent(&self, agent_id: &str, limit: i64) -> Result<Vec<AgentMemory>> {
        let rows = sqlx::query(
            "SELECT id, source_id, agent_id, topic, task_id, \
                    confidence, access_count, last_accessed, \
                    expires_at, created_at \
             FROM agent_memories \
             WHERE agent_id = $1 \
             ORDER BY created_at DESC \
             LIMIT $2",
        )
        .bind(agent_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(memory_from_row).collect())
    }

    async fn list_by_topic(
        &self,
        topic: &str,
        agent_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<AgentMemory>> {
        let rows = match agent_id {
            Some(aid) => {
                sqlx::query(
                    "SELECT id, source_id, agent_id, topic, task_id, \
                            confidence, access_count, last_accessed, \
                            expires_at, created_at \
                     FROM agent_memories \
                     WHERE topic = $1 AND agent_id = $2 \
                     ORDER BY created_at DESC \
                     LIMIT $3",
                )
                .bind(topic)
                .bind(aid)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "SELECT id, source_id, agent_id, topic, task_id, \
                            confidence, access_count, last_accessed, \
                            expires_at, created_at \
                     FROM agent_memories \
                     WHERE topic = $1 \
                     ORDER BY created_at DESC \
                     LIMIT $2",
                )
                .bind(topic)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
            }
        };

        Ok(rows.iter().map(memory_from_row).collect())
    }

    async fn increment_access(&self, source_id: uuid::Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE agent_memories \
             SET access_count = access_count + 1, \
                 last_accessed = NOW() \
             WHERE source_id = $1",
        )
        .bind(source_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn find_expired(&self, retention_days: i64) -> Result<Vec<AgentMemory>> {
        let rows = sqlx::query(
            "SELECT id, source_id, agent_id, topic, task_id, \
                    confidence, access_count, last_accessed, \
                    expires_at, created_at \
             FROM agent_memories \
             WHERE expires_at IS NOT NULL \
               AND expires_at < NOW() \
             UNION ALL \
             SELECT id, source_id, agent_id, topic, task_id, \
                    confidence, access_count, last_accessed, \
                    expires_at, created_at \
             FROM agent_memories \
             WHERE expires_at IS NULL \
               AND created_at < NOW() - make_interval(days => $1) \
             ORDER BY created_at ASC",
        )
        .bind(retention_days as i32)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(memory_from_row).collect())
    }

    async fn delete(&self, id: uuid::Uuid) -> Result<bool> {
        let result = sqlx::query("DELETE FROM agent_memories WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn count_by_agent(&self, agent_id: &str) -> Result<i64> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS cnt FROM agent_memories \
             WHERE agent_id = $1",
        )
        .bind(agent_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get::<i64, _>("cnt"))
    }

    async fn count_all(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS cnt FROM agent_memories")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get::<i64, _>("cnt"))
    }
}

fn memory_from_row(row: &sqlx::postgres::PgRow) -> AgentMemory {
    AgentMemory {
        id: row.get("id"),
        source_id: row.get("source_id"),
        agent_id: row.get("agent_id"),
        topic: row.get("topic"),
        task_id: row.get("task_id"),
        confidence: row.get("confidence"),
        access_count: row.get("access_count"),
        last_accessed: row.get("last_accessed"),
        expires_at: row.get("expires_at"),
        created_at: row.get("created_at"),
    }
}

//! Persist computed graph embeddings into `covalence.graph_embeddings`.

use anyhow::Result;
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

/// Store computed embeddings into the `covalence.graph_embeddings` table.
///
/// Uses a pgvector literal `[0.1,0.2,...]` for the COPY cast. Returns the
/// number of rows upserted.
pub async fn store_embeddings(
    pool: &PgPool,
    embeddings: HashMap<Uuid, Vec<f32>>,
    method: &str,
) -> Result<usize> {
    let mut count = 0;
    for (node_id, embedding) in &embeddings {
        let vec_str = format!(
            "[{}]",
            embedding
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        sqlx::query(
            "INSERT INTO covalence.graph_embeddings (node_id, method, embedding)
             VALUES ($1, $2, $3::vector)
             ON CONFLICT (node_id, method)
             DO UPDATE SET embedding = EXCLUDED.embedding, computed_at = NOW()",
        )
        .bind(node_id)
        .bind(method)
        .bind(&vec_str)
        .execute(pool)
        .await?;
        count += 1;
    }
    Ok(count)
}

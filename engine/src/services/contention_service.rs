//! Contention system — detecting and resolving contradictions (SPEC §6.4).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct Contention {
    pub id: Uuid,
    pub node_id: Option<Uuid>,
    pub source_node_id: Option<Uuid>,
    pub description: Option<String>,
    pub status: String,
    pub resolution: Option<String>,
    pub severity: Option<String>,
    pub detected_at: Option<DateTime<Utc>>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct ResolveRequest {
    pub resolution: String,
    pub rationale: String,
}

fn contention_from_row(row: &sqlx::postgres::PgRow) -> Result<Contention, sqlx::Error> {
    Ok(Contention {
        id: row.try_get("id")?,
        node_id: row.try_get("node_id")?,
        source_node_id: row.try_get("source_node_id")?,
        description: row.try_get("description")?,
        status: row
            .try_get::<Option<String>, _>("status")?
            .unwrap_or_default(),
        resolution: row.try_get("resolution")?,
        severity: row.try_get("severity")?,
        detected_at: row.try_get("detected_at")?,
        resolved_at: row.try_get("resolved_at")?,
    })
}

pub struct ContentionService {
    pool: PgPool,
    namespace: String,
}

impl ContentionService {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            namespace: "default".into(),
        }
    }

    /// Set the namespace for this service instance.
    pub fn with_namespace(mut self, ns: String) -> Self {
        self.namespace = ns;
        self
    }

    pub async fn list(
        &self,
        node_id: Option<Uuid>,
        status: Option<String>,
    ) -> Result<Vec<Contention>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, node_id, source_node_id, description, status,
                    resolution, severity, detected_at, resolved_at
             FROM covalence.contentions
             WHERE namespace = $1
               AND ($2::uuid IS NULL OR node_id = $2)
               AND ($3::text IS NULL OR status = $3)
             ORDER BY detected_at DESC",
        )
        .bind(&self.namespace)
        .bind(node_id)
        .bind(status.as_deref())
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(contention_from_row).collect()
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Contention>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, node_id, source_node_id, description, status,
                    resolution, severity, detected_at, resolved_at
             FROM covalence.contentions WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(r) => Ok(Some(contention_from_row(&r)?)),
            None => Ok(None),
        }
    }

    #[allow(dead_code)]
    pub async fn detect(
        &self,
        node_id: Uuid,
        source_node_id: Uuid,
        description: &str,
    ) -> Result<Contention, sqlx::Error> {
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO covalence.contentions \
             (id, node_id, source_node_id, status, description, namespace, detected_at)
             VALUES ($1, $2, $3, 'detected', $4, $5, now())
             RETURNING id, node_id, source_node_id, description, status,
                       resolution, severity, detected_at, resolved_at",
        )
        .bind(id)
        .bind(node_id)
        .bind(source_node_id)
        .bind(description)
        .bind(&self.namespace)
        .fetch_one(&self.pool)
        .await?;
        contention_from_row(&row)
    }

    pub async fn resolve(&self, id: Uuid, req: ResolveRequest) -> Result<Contention, sqlx::Error> {
        let valid = ["supersede_a", "supersede_b", "accept_both", "dismiss"];
        if !valid.contains(&req.resolution.as_str()) {
            return Err(sqlx::Error::Protocol(format!(
                "invalid resolution: {}",
                req.resolution
            )));
        }

        // Store rationale in resolution field as "type: rationale"
        let resolution_text = format!("{}: {}", req.resolution, req.rationale);

        let row = sqlx::query(
            "UPDATE covalence.contentions
             SET status = 'resolved', resolution = $2, resolved_at = now()
             WHERE id = $1
             RETURNING id, node_id, source_node_id, description, status,
                       resolution, severity, detected_at, resolved_at",
        )
        .bind(id)
        .bind(&resolution_text)
        .fetch_one(&self.pool)
        .await?;
        contention_from_row(&row)
    }
}

//! Contention system — detecting and resolving contradictions (SPEC §6.4).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::models::ContentionType;

#[derive(Debug, Serialize)]
pub struct Contention {
    pub id: Uuid,
    pub node_id: Option<Uuid>,
    pub source_node_id: Option<Uuid>,
    pub description: Option<String>,
    pub status: String,
    pub resolution: Option<String>,
    pub severity: Option<String>,
    pub contention_type: ContentionType,
    pub detected_at: Option<DateTime<Utc>>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct ResolveRequest {
    pub resolution: String,
    pub rationale: String,
}

fn contention_from_row(row: &sqlx::postgres::PgRow) -> Result<Contention, sqlx::Error> {
    let contention_type_str: Option<String> = row.try_get("contention_type").ok();
    let contention_type = contention_type_str
        .as_deref()
        .and_then(|s| s.parse::<ContentionType>().ok())
        .unwrap_or_default();

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
        contention_type,
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
                    resolution, severity, contention_type, detected_at, resolved_at
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
                    resolution, severity, contention_type, detected_at, resolved_at
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

    /// Create a new contention row.  `contention_type` defaults to `Rebuttal`.
    #[allow(dead_code)]
    pub async fn detect(
        &self,
        node_id: Uuid,
        source_node_id: Uuid,
        description: &str,
    ) -> Result<Contention, sqlx::Error> {
        self.detect_typed(
            node_id,
            source_node_id,
            description,
            ContentionType::Rebuttal,
        )
        .await
    }

    /// Create a new contention row with an explicit [`ContentionType`].
    ///
    /// Uses `ON CONFLICT (node_id, source_node_id) DO NOTHING` (covalence#98)
    /// so that duplicate contention pairs are silently de-duplicated at the DB
    /// level.  When a conflict is detected the existing row is fetched and
    /// returned instead of the newly inserted one.
    #[allow(dead_code)]
    pub async fn detect_typed(
        &self,
        node_id: Uuid,
        source_node_id: Uuid,
        description: &str,
        contention_type: ContentionType,
    ) -> Result<Contention, sqlx::Error> {
        let id = Uuid::new_v4();

        // ON CONFLICT DO NOTHING returns no row when the (node_id, source_node_id)
        // pair already exists.  We use fetch_optional and fall back to SELECT.
        let maybe_row = sqlx::query(
            "INSERT INTO covalence.contentions \
             (id, node_id, source_node_id, status, description, contention_type, namespace, detected_at) \
             VALUES ($1, $2, $3, 'detected', $4, $5, $6, now()) \
             ON CONFLICT (node_id, source_node_id) DO NOTHING \
             RETURNING id, node_id, source_node_id, description, status, \
                       resolution, severity, contention_type, detected_at, resolved_at",
        )
        .bind(id)
        .bind(node_id)
        .bind(source_node_id)
        .bind(description)
        .bind(contention_type.as_str())
        .bind(&self.namespace)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = maybe_row {
            // Fresh insert succeeded — return it directly.
            return contention_from_row(&row);
        }

        // Conflict — fetch the existing row for this (node_id, source_node_id) pair.
        let row = sqlx::query(
            "SELECT id, node_id, source_node_id, description, status, \
                    resolution, severity, contention_type, detected_at, resolved_at \
             FROM covalence.contentions \
             WHERE node_id = $1 AND source_node_id = $2 \
             LIMIT 1",
        )
        .bind(node_id)
        .bind(source_node_id)
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
                       resolution, severity, contention_type, detected_at, resolved_at",
        )
        .bind(id)
        .bind(&resolution_text)
        .fetch_one(&self.pool)
        .await?;
        contention_from_row(&row)
    }
}

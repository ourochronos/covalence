//! Edge service — CRUD wrapper for graph edges.

use std::sync::Arc;

use crate::error::{Error, Result};
use crate::models::audit::{AuditAction, AuditLog};
use crate::models::edge::Edge;
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{AuditLogRepo, EdgeRepo};
use crate::types::ids::{AuditLogId, EdgeId, NodeId};

/// Service for graph edge operations.
pub struct EdgeService {
    repo: Arc<PgRepo>,
}

impl EdgeService {
    /// Create a new edge service.
    pub fn new(repo: Arc<PgRepo>) -> Self {
        Self { repo }
    }

    /// Get an edge by ID.
    pub async fn get(&self, id: EdgeId) -> Result<Option<Edge>> {
        EdgeRepo::get(&*self.repo, id).await
    }

    /// List all edges originating from a node.
    pub async fn list_from_node(&self, node_id: NodeId) -> Result<Vec<Edge>> {
        EdgeRepo::list_from_node(&*self.repo, node_id).await
    }

    /// List all edges pointing to a node.
    pub async fn list_to_node(&self, node_id: NodeId) -> Result<Vec<Edge>> {
        EdgeRepo::list_to_node(&*self.repo, node_id).await
    }

    /// Apply a correction to an edge's fields.
    ///
    /// Updates any supplied fields (rel_type, confidence) and
    /// records an audit log entry.
    pub async fn correct(
        &self,
        id: EdgeId,
        rel_type: Option<String>,
        confidence: Option<f64>,
    ) -> Result<AuditLogId> {
        let mut edge = EdgeRepo::get(&*self.repo, id)
            .await?
            .ok_or(Error::NotFound {
                entity_type: "edge",
                id: id.to_string(),
            })?;

        let mut changes = serde_json::Map::new();

        if let Some(ref rt) = rel_type {
            changes.insert(
                "rel_type".into(),
                serde_json::json!({
                    "old": edge.rel_type,
                    "new": rt,
                }),
            );
            edge.rel_type = rt.clone();
        }
        if let Some(conf) = confidence {
            if !conf.is_finite() || !(0.0..=1.0).contains(&conf) {
                return Err(crate::error::Error::InvalidInput(format!(
                    "confidence must be finite and in [0.0, 1.0], got {conf}"
                )));
            }
            changes.insert(
                "confidence".into(),
                serde_json::json!({
                    "old": edge.confidence,
                    "new": conf,
                }),
            );
            edge.confidence = conf;
        }

        EdgeRepo::update(&*self.repo, &edge).await?;

        let audit = AuditLog::new(
            AuditAction::EdgeCorrect,
            "api:correct".to_string(),
            serde_json::Value::Object(changes),
        )
        .with_target("edge", id.into_uuid());
        let audit_id = audit.id;
        AuditLogRepo::create(&*self.repo, &audit).await?;

        Ok(audit_id)
    }

    /// Delete an edge with a reason, logging to the audit table.
    pub async fn delete_with_reason(&self, id: EdgeId, reason: String) -> Result<AuditLogId> {
        let deleted = EdgeRepo::delete(&*self.repo, id).await?;

        if !deleted {
            return Err(Error::NotFound {
                entity_type: "edge",
                id: id.to_string(),
            });
        }

        let audit = AuditLog::new(
            AuditAction::EdgeDelete,
            "api:delete".to_string(),
            serde_json::json!({
                "edge_id": id.into_uuid(),
                "reason": reason,
            }),
        )
        .with_target("edge", id.into_uuid());
        let audit_id = audit.id;
        AuditLogRepo::create(&*self.repo, &audit).await?;

        Ok(audit_id)
    }
}

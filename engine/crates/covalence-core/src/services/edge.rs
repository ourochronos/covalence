//! Edge service — CRUD wrapper for graph edges.

use std::sync::Arc;

use crate::error::Result;
use crate::models::edge::Edge;
use crate::storage::postgres::PgRepo;
use crate::storage::traits::EdgeRepo;
use crate::types::ids::{EdgeId, NodeId};

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
}

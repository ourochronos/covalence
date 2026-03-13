//! Node service — graph node operations with provenance.

use std::sync::Arc;

use crate::error::{Error, Result};
use crate::graph::SharedGraph;
use crate::graph::sidecar::{EdgeMeta, NodeMeta};
use crate::graph::traversal::bfs_neighborhood;
use crate::models::audit::{AuditAction, AuditLog};
use crate::models::chunk::Chunk;
use crate::models::edge::Edge;
use crate::models::extraction::Extraction;
use crate::models::node::Node;
use crate::models::node_alias::NodeAlias;
use crate::models::source::Source;
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{
    AuditLogRepo, ChunkRepo, EdgeRepo, ExtractionRepo, NodeAliasRepo, NodeLandmarkRepo, NodeRepo,
    SourceRepo,
};
use crate::types::ids::{AliasId, AuditLogId, EdgeId, NodeId};

/// A provenance chain linking a node back through extractions, chunks,
/// and sources.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProvenanceChain {
    /// The node this provenance is for.
    pub node_id: NodeId,
    /// Extraction records that produced this node.
    pub extractions: Vec<Extraction>,
    /// Chunks the extractions came from.
    pub chunks: Vec<Chunk>,
    /// Sources the chunks came from.
    pub sources: Vec<Source>,
}

/// Specification for a node split target.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SplitSpec {
    /// Name for the new node.
    pub name: String,
    /// Type for the new node.
    pub node_type: String,
    /// Optional description.
    pub description: Option<String>,
    /// Edge IDs to reassign to this new node.
    pub edge_ids: Vec<EdgeId>,
}

/// Epistemic confidence explanation for a node.
///
/// Contains the Subjective Logic opinion breakdown and provenance
/// statistics used to derive the node's confidence score.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NodeExplanation {
    /// Degree of positive evidence.
    pub belief: f64,
    /// Degree of negative evidence.
    pub disbelief: f64,
    /// Degree of ignorance.
    pub uncertainty: f64,
    /// Prior probability absent evidence.
    pub base_rate: f64,
    /// Projected probability: `belief + base_rate * uncertainty`.
    pub projected_probability: f64,
    /// Number of distinct sources contributing to this node.
    pub source_count: usize,
    /// Number of extraction records for this node.
    pub extraction_count: usize,
}

/// Service for graph node operations.
pub struct NodeService {
    repo: Arc<PgRepo>,
    graph: SharedGraph,
}

impl NodeService {
    /// Create a new node service.
    pub fn new(repo: Arc<PgRepo>, graph: SharedGraph) -> Self {
        Self { repo, graph }
    }

    /// Get a node by ID.
    pub async fn get(&self, id: NodeId) -> Result<Option<Node>> {
        NodeRepo::get(&*self.repo, id).await
    }

    /// List nodes by type with pagination.
    pub async fn list_by_type(
        &self,
        node_type: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Node>> {
        NodeRepo::list_by_type(&*self.repo, node_type, limit, offset).await
    }

    /// Get the neighborhood of a node via BFS on the graph sidecar.
    pub async fn neighborhood(&self, id: NodeId, hops: usize) -> Result<Vec<Node>> {
        let neighbor_ids = {
            let graph = self.graph.read().await;
            bfs_neighborhood(&graph, id.into_uuid(), hops, None)
        };

        let mut nodes = Vec::with_capacity(neighbor_ids.len());
        for (uuid, _distance) in neighbor_ids {
            if let Some(node) = NodeRepo::get(&*self.repo, NodeId::from_uuid(uuid)).await? {
                nodes.push(node);
            }
        }
        Ok(nodes)
    }

    /// Trace the full provenance chain for a node.
    pub async fn provenance(&self, id: NodeId) -> Result<ProvenanceChain> {
        let extractions =
            ExtractionRepo::list_active_for_entity(&*self.repo, "node", id.into_uuid()).await?;

        let mut chunks = Vec::new();
        let mut source_ids = std::collections::HashSet::new();
        for ext in &extractions {
            if let Some(chunk_id) = ext.chunk_id {
                if let Some(chunk) = ChunkRepo::get(&*self.repo, chunk_id).await? {
                    source_ids.insert(chunk.source_id);
                    chunks.push(chunk);
                }
            }
        }

        let mut sources = Vec::new();
        for sid in source_ids {
            if let Some(source) = SourceRepo::get(&*self.repo, sid).await? {
                sources.push(source);
            }
        }

        Ok(ProvenanceChain {
            node_id: id,
            extractions,
            chunks,
            sources,
        })
    }

    /// Build a confidence explanation for a node.
    ///
    /// Returns the Subjective Logic opinion breakdown, projected
    /// probability, count of contributing sources, and count of
    /// extraction records. Returns `None` if the node does not exist.
    pub async fn explain(&self, id: NodeId) -> Result<Option<NodeExplanation>> {
        let node = match NodeRepo::get(&*self.repo, id).await? {
            Some(n) => n,
            None => return Ok(None),
        };

        let opinion = node.confidence_breakdown.unwrap_or_default();

        let extractions =
            ExtractionRepo::list_active_for_entity(&*self.repo, "node", id.into_uuid()).await?;

        let mut source_ids = std::collections::HashSet::new();
        for ext in &extractions {
            if let Some(chunk_id) = ext.chunk_id {
                if let Some(chunk) = ChunkRepo::get(&*self.repo, chunk_id).await? {
                    source_ids.insert(chunk.source_id);
                }
            }
        }

        Ok(Some(NodeExplanation {
            belief: opinion.belief,
            disbelief: opinion.disbelief,
            uncertainty: opinion.uncertainty,
            base_rate: opinion.base_rate,
            projected_probability: opinion.projected_probability(),
            source_count: source_ids.len(),
            extraction_count: extractions.len(),
        }))
    }

    /// Resolve a name to a node (case-insensitive).
    pub async fn resolve(&self, name: &str) -> Result<Option<Node>> {
        NodeRepo::find_by_name(&*self.repo, name).await
    }

    /// Merge multiple nodes into a target node.
    ///
    /// Retargets all edges from source nodes to the target, copies
    /// aliases, accumulates mention counts, and soft-deletes sources
    /// by setting clearance_level to -1.
    pub async fn merge(&self, source_ids: Vec<NodeId>, target_id: NodeId) -> Result<AuditLogId> {
        // Verify target exists
        let mut target = NodeRepo::get(&*self.repo, target_id)
            .await?
            .ok_or(Error::NotFound {
                entity_type: "node",
                id: target_id.to_string(),
            })?;

        let mut merged_names = Vec::new();

        for source_id in &source_ids {
            let source_node =
                NodeRepo::get(&*self.repo, *source_id)
                    .await?
                    .ok_or(Error::NotFound {
                        entity_type: "node",
                        id: source_id.to_string(),
                    })?;

            merged_names.push(source_node.canonical_name.clone());

            // Retarget outgoing edges
            let outgoing = EdgeRepo::list_from_node(&*self.repo, *source_id).await?;
            for mut edge in outgoing {
                edge.source_node_id = target_id;
                EdgeRepo::update(&*self.repo, &edge).await?;
            }

            // Retarget incoming edges
            let incoming = EdgeRepo::list_to_node(&*self.repo, *source_id).await?;
            for mut edge in incoming {
                edge.target_node_id = target_id;
                EdgeRepo::update(&*self.repo, &edge).await?;
            }

            // Copy aliases to target
            let aliases = NodeAliasRepo::list_by_node(&*self.repo, *source_id).await?;
            for alias in &aliases {
                let new_alias = NodeAlias {
                    id: AliasId::new(),
                    node_id: target_id,
                    alias: alias.alias.clone(),
                    source_chunk_id: alias.source_chunk_id,
                };
                NodeAliasRepo::create(&*self.repo, &new_alias).await?;
            }

            // Accumulate mention count
            target.mention_count += source_node.mention_count;

            // Merge properties and description
            target.merge_properties(&source_node.properties);
            target.merge_description(source_node.description.as_deref());

            // Soft-delete source node (clearance -1 marks it as merged)
            let mut tombstone = source_node;
            tombstone.clearance_level = crate::types::clearance::ClearanceLevel::LocalStrict;
            // Use raw SQL to set clearance_level = -1 since
            // ClearanceLevel enum doesn't have a tombstone variant
            sqlx::query("UPDATE nodes SET clearance_level = -1 WHERE id = $1")
                .bind(*source_id)
                .execute(self.repo.pool())
                .await?;
        }

        // Save updated target
        NodeRepo::update(&*self.repo, &target).await?;

        // Update graph sidecar
        {
            let mut g = self.graph.write().await;
            for source_id in &source_ids {
                if let Err(e) = g.remove_node(source_id.into_uuid()) {
                    tracing::warn!(
                        node_id = %source_id,
                        error = %e,
                        "failed to remove merged source node from sidecar"
                    );
                }
            }
            // Rebuild target node in sidecar
            if let Err(e) = g.remove_node(target_id.into_uuid()) {
                tracing::warn!(
                    node_id = %target_id,
                    error = %e,
                    "failed to remove target node from sidecar before rebuild"
                );
            }
            if let Err(e) = g.add_node(NodeMeta {
                id: target_id.into_uuid(),
                node_type: target.node_type.clone(),
                canonical_name: target.canonical_name.clone(),
                clearance_level: target.clearance_level.as_i32(),
            }) {
                tracing::warn!(
                    node_id = %target_id,
                    error = %e,
                    "failed to add rebuilt target node to sidecar"
                );
            }
            // Re-add edges for target from PG
            let out_edges = EdgeRepo::list_from_node(&*self.repo, target_id).await?;
            let in_edges = EdgeRepo::list_to_node(&*self.repo, target_id).await?;
            for edge in out_edges.iter().chain(in_edges.iter()) {
                if let Err(e) = g.add_edge(
                    edge.source_node_id.into_uuid(),
                    edge.target_node_id.into_uuid(),
                    edge_to_meta(edge),
                ) {
                    tracing::warn!(
                        edge_id = %edge.id,
                        error = %e,
                        "failed to add edge to sidecar during merge rebuild"
                    );
                }
            }
        }

        // Create audit log
        let source_uuids: Vec<_> = source_ids.iter().map(|id| id.into_uuid()).collect();
        let audit = AuditLog::new(
            AuditAction::MergeNodes,
            "system:merge".to_string(),
            serde_json::json!({
                "source_ids": source_uuids,
                "target_id": target_id.into_uuid(),
                "merged_names": merged_names,
            }),
        )
        .with_target("node", target_id.into_uuid());

        let audit_id = audit.id;
        AuditLogRepo::create(&*self.repo, &audit).await?;

        Ok(audit_id)
    }

    /// Apply a correction to a node's fields.
    ///
    /// Updates any supplied fields (canonical_name, node_type,
    /// description, confidence) and records an audit log entry.
    pub async fn correct(
        &self,
        id: NodeId,
        canonical_name: Option<String>,
        node_type: Option<String>,
        description: Option<String>,
        confidence: Option<f64>,
    ) -> Result<AuditLogId> {
        let mut node = NodeRepo::get(&*self.repo, id)
            .await?
            .ok_or(Error::NotFound {
                entity_type: "node",
                id: id.to_string(),
            })?;

        let mut changes = serde_json::Map::new();

        if let Some(ref name) = canonical_name {
            changes.insert(
                "canonical_name".into(),
                serde_json::json!({
                    "old": node.canonical_name,
                    "new": name,
                }),
            );
            node.canonical_name = name.clone();
        }
        if let Some(ref nt) = node_type {
            changes.insert(
                "node_type".into(),
                serde_json::json!({
                    "old": node.node_type,
                    "new": nt,
                }),
            );
            node.node_type = nt.clone();
        }
        if let Some(ref desc) = description {
            changes.insert(
                "description".into(),
                serde_json::json!({
                    "old": node.description,
                    "new": desc,
                }),
            );
            node.description = Some(desc.clone());
        }
        if let Some(conf) = confidence {
            if !conf.is_finite() || !(0.0..=1.0).contains(&conf) {
                return Err(crate::error::Error::InvalidInput(format!(
                    "confidence must be finite and in [0.0, 1.0], got {conf}"
                )));
            }
            changes.insert("confidence".into(), serde_json::json!({ "new": conf }));
            // Store as a simple belief opinion.
            node.confidence_breakdown =
                crate::types::opinion::Opinion::new(conf, 0.0, 1.0 - conf, 0.5);
        }

        NodeRepo::update(&*self.repo, &node).await?;

        // Update sidecar.
        {
            let mut g = self.graph.write().await;
            if let Err(e) = g.remove_node(id.into_uuid()) {
                tracing::warn!(
                    node_id = %id,
                    error = %e,
                    "failed to remove node from sidecar before correction rebuild"
                );
            }
            if let Err(e) = g.add_node(NodeMeta {
                id: id.into_uuid(),
                node_type: node.node_type.clone(),
                canonical_name: node.canonical_name.clone(),
                clearance_level: node.clearance_level.as_i32(),
            }) {
                tracing::warn!(
                    node_id = %id,
                    error = %e,
                    "failed to add corrected node to sidecar"
                );
            }
        }

        let audit = AuditLog::new(
            AuditAction::NodeCorrect,
            "api:correct".to_string(),
            serde_json::Value::Object(changes),
        )
        .with_target("node", id.into_uuid());
        let audit_id = audit.id;
        AuditLogRepo::create(&*self.repo, &audit).await?;

        Ok(audit_id)
    }

    /// Add a free-text annotation to a node's properties.
    ///
    /// Appends to the `annotations` array in the node's JSONB
    /// properties field.
    pub async fn annotate(&self, id: NodeId, text: String) -> Result<AuditLogId> {
        let mut node = NodeRepo::get(&*self.repo, id)
            .await?
            .ok_or(Error::NotFound {
                entity_type: "node",
                id: id.to_string(),
            })?;

        // Ensure properties is an object and append annotation.
        if let serde_json::Value::Object(ref mut map) = node.properties {
            let annotations = map
                .entry("annotations")
                .or_insert_with(|| serde_json::Value::Array(vec![]));
            if let serde_json::Value::Array(arr) = annotations {
                arr.push(serde_json::json!({
                    "text": text,
                    "created_at": chrono::Utc::now().to_rfc3339(),
                }));
            }
        }

        NodeRepo::update(&*self.repo, &node).await?;

        let audit = AuditLog::new(
            AuditAction::NodeAnnotate,
            "api:annotate".to_string(),
            serde_json::json!({ "text": text }),
        )
        .with_target("node", id.into_uuid());
        let audit_id = audit.id;
        AuditLogRepo::create(&*self.repo, &audit).await?;

        Ok(audit_id)
    }

    /// List landmark nodes ordered by mention count (proxy for
    /// betweenness centrality).
    pub async fn list_landmarks(&self, limit: usize) -> Result<Vec<Node>> {
        NodeLandmarkRepo::list_landmarks(&*self.repo, limit as i64).await
    }

    /// Split a node into multiple new nodes per the given specs.
    ///
    /// Each spec creates a new node and reassigns the listed edges.
    /// The original node is soft-deleted (clearance_level = -1).
    pub async fn split(&self, id: NodeId, specs: Vec<SplitSpec>) -> Result<Vec<NodeId>> {
        // Verify original exists
        let _original = NodeRepo::get(&*self.repo, id)
            .await?
            .ok_or(Error::NotFound {
                entity_type: "node",
                id: id.to_string(),
            })?;

        let mut new_ids = Vec::with_capacity(specs.len());

        for spec in &specs {
            // Create new node
            let mut new_node = Node::new(spec.name.clone(), spec.node_type.clone());
            new_node.description = spec.description.clone();
            NodeRepo::create(&*self.repo, &new_node).await?;

            let new_id = new_node.id;
            new_ids.push(new_id);

            // Retarget specified edges
            for edge_id in &spec.edge_ids {
                if let Some(mut edge) = EdgeRepo::get(&*self.repo, *edge_id).await? {
                    if edge.source_node_id == id {
                        edge.source_node_id = new_id;
                    }
                    if edge.target_node_id == id {
                        edge.target_node_id = new_id;
                    }
                    EdgeRepo::update(&*self.repo, &edge).await?;
                }
            }

            // Add new node to sidecar
            {
                let mut g = self.graph.write().await;
                if let Err(e) = g.add_node(NodeMeta {
                    id: new_id.into_uuid(),
                    node_type: new_node.node_type.clone(),
                    canonical_name: new_node.canonical_name.clone(),
                    clearance_level: new_node.clearance_level.as_i32(),
                }) {
                    tracing::warn!(
                        node_id = %new_id,
                        error = %e,
                        "failed to add split node to sidecar"
                    );
                }
            }
        }

        // Soft-delete original node
        sqlx::query("UPDATE nodes SET clearance_level = -1 WHERE id = $1")
            .bind(id)
            .execute(self.repo.pool())
            .await?;

        // Remove original from sidecar and re-add edges for new nodes
        {
            let mut g = self.graph.write().await;
            if let Err(e) = g.remove_node(id.into_uuid()) {
                tracing::warn!(
                    node_id = %id,
                    error = %e,
                    "failed to remove original node from sidecar during split"
                );
            }

            for new_id in &new_ids {
                let out_edges = EdgeRepo::list_from_node(&*self.repo, *new_id).await?;
                let in_edges = EdgeRepo::list_to_node(&*self.repo, *new_id).await?;
                for edge in out_edges.iter().chain(in_edges.iter()) {
                    if let Err(e) = g.add_edge(
                        edge.source_node_id.into_uuid(),
                        edge.target_node_id.into_uuid(),
                        edge_to_meta(edge),
                    ) {
                        tracing::warn!(
                            edge_id = %edge.id,
                            error = %e,
                            "failed to add edge to sidecar during split rebuild"
                        );
                    }
                }
            }
        }

        // Create audit log
        let new_uuids: Vec<_> = new_ids.iter().map(|nid| nid.into_uuid()).collect();
        let spec_names: Vec<_> = specs.iter().map(|s| s.name.clone()).collect();
        let audit = AuditLog::new(
            AuditAction::SplitNode,
            "system:split".to_string(),
            serde_json::json!({
                "original_id": id.into_uuid(),
                "new_ids": new_uuids,
                "new_names": spec_names,
            }),
        )
        .with_target("node", id.into_uuid());

        AuditLogRepo::create(&*self.repo, &audit).await?;

        Ok(new_ids)
    }
}

/// Convert an Edge model to sidecar EdgeMeta.
fn edge_to_meta(edge: &Edge) -> EdgeMeta {
    EdgeMeta {
        id: edge.id.into_uuid(),
        rel_type: edge.rel_type.clone(),
        weight: edge.weight,
        confidence: edge.confidence,
        causal_level: edge.causal_level,
        clearance_level: edge.clearance_level.as_i32(),
        is_synthetic: edge.is_synthetic,
    }
}

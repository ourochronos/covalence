//! Knowledge gap analysis and ontology clustering.

use std::collections::HashMap;

use crate::consolidation::ontology::{
    self, ClusterLevel, ClusterResult, build_entity_clusters, build_rel_type_clusters,
    build_type_clusters,
};
use crate::error::{Error, Result};

use super::AdminService;

/// A knowledge gap — an entity frequently referenced but never explained.
#[derive(Debug, Clone, serde::Serialize)]
pub struct KnowledgeGap {
    /// Node UUID.
    pub node_id: uuid::Uuid,
    /// Canonical entity name.
    pub canonical_name: String,
    /// Entity type (e.g. "concept", "entity").
    pub node_type: String,
    /// Number of incoming edges (references to this entity).
    pub in_degree: usize,
    /// Number of outgoing edges (explanations from this entity).
    pub out_degree: usize,
    /// Gap score: in_degree - out_degree (higher = bigger gap).
    pub gap_score: f64,
    /// Source URIs that reference this entity.
    pub referenced_by: Vec<String>,
}

impl AdminService {
    /// Identify knowledge gaps — entities with high in-degree but low
    /// out-degree. These are concepts the system references frequently
    /// but has no source material explaining.
    ///
    /// Gap score = `in_degree - out_degree`. Entities with zero
    /// out-degree and high in-degree represent the biggest blind spots.
    pub async fn knowledge_gaps(
        &self,
        min_in_degree: usize,
        min_label_length: usize,
        exclude_types: &[String],
        limit: usize,
    ) -> Result<Vec<KnowledgeGap>> {
        // Phase 1: compute in/out degree via the graph engine trait.
        let exclude_refs: Vec<&str> = exclude_types.iter().map(|s| s.as_str()).collect();
        let engine_gaps = self
            .graph
            .knowledge_gaps(min_in_degree, min_label_length, &exclude_refs, limit)
            .await?;
        let candidates: Vec<(uuid::Uuid, String, String, usize, usize)> = engine_gaps
            .into_iter()
            .map(|g| (g.id, g.name, g.node_type, g.in_degree, g.out_degree))
            .collect();

        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        // Phase 2: batch-fetch source URIs for the gap nodes via SP.
        let node_ids: Vec<uuid::Uuid> = candidates.iter().map(|c| c.0).collect();
        let rows = sqlx::query_as::<_, (uuid::Uuid, Option<String>, Option<String>)>(
            "SELECT * FROM sp_get_node_provenance_sources($1)",
        )
        .bind(&node_ids)
        .fetch_all(self.repo.pool())
        .await?;

        let mut refs_map: HashMap<uuid::Uuid, Vec<String>> = HashMap::new();
        for (node_id, uri, title) in rows {
            let label = uri.or(title).unwrap_or_else(|| node_id.to_string());
            refs_map.entry(node_id).or_default().push(label);
        }

        // Build final results.
        let gaps = candidates
            .into_iter()
            .map(|(id, name, ntype, in_deg, out_deg)| {
                let gap_score = in_deg as f64 - out_deg as f64;
                let referenced_by = refs_map.remove(&id).unwrap_or_default();
                KnowledgeGap {
                    node_id: id,
                    canonical_name: name,
                    node_type: ntype,
                    in_degree: in_deg,
                    out_degree: out_deg,
                    gap_score,
                    referenced_by,
                }
            })
            .collect();

        Ok(gaps)
    }

    /// Run ontology clustering at the specified level(s) using
    /// HDBSCAN.
    ///
    /// When `dry_run` is true, returns discovered clusters without
    /// writing them back to the database. When false, stores cluster
    /// definitions and updates canonical labels on nodes/edges, then
    /// reloads the graph sidecar.
    ///
    /// `min_cluster_size` controls the minimum number of labels
    /// required to form a cluster (default: 2). Labels that don't
    /// belong to any cluster are returned as noise.
    pub async fn cluster_ontology(
        &self,
        level: Option<ClusterLevel>,
        min_cluster_size: usize,
        dry_run: bool,
    ) -> Result<ClusterResult> {
        let embedder = self.embedder.as_ref().ok_or_else(|| {
            Error::Config("no embedder configured for ontology clustering".into())
        })?;

        let pool = self.repo.pool();
        let mut combined = ClusterResult {
            clusters: Vec::new(),
            noise_labels: Vec::new(),
        };

        let do_entity = level.is_none() || level == Some(ClusterLevel::Entity);
        let do_type = level.is_none() || level == Some(ClusterLevel::EntityType);
        let do_rel = level.is_none() || level == Some(ClusterLevel::RelationType);

        if do_entity {
            let r = build_entity_clusters(pool, embedder.as_ref(), min_cluster_size).await?;
            tracing::info!(
                clusters = r.clusters.len(),
                noise = r.noise_labels.len(),
                "entity name clusters discovered"
            );
            combined.clusters.extend(r.clusters);
            combined.noise_labels.extend(r.noise_labels);
        }
        if do_type {
            let r = build_type_clusters(pool, embedder.as_ref(), min_cluster_size).await?;
            tracing::info!(
                clusters = r.clusters.len(),
                noise = r.noise_labels.len(),
                "entity type clusters discovered"
            );
            combined.clusters.extend(r.clusters);
            combined.noise_labels.extend(r.noise_labels);
        }
        if do_rel {
            let r = build_rel_type_clusters(pool, embedder.as_ref(), min_cluster_size).await?;
            tracing::info!(
                clusters = r.clusters.len(),
                noise = r.noise_labels.len(),
                "relationship type clusters discovered"
            );
            combined.clusters.extend(r.clusters);
            combined.noise_labels.extend(r.noise_labels);
        }

        if !dry_run && !combined.clusters.is_empty() {
            ontology::apply_clusters(pool, &combined.clusters, min_cluster_size).await?;
            tracing::info!(
                total = combined.clusters.len(),
                "ontology clusters applied, reloading graph"
            );
            self.graph.reload(self.repo.pool()).await?;
        }

        Ok(combined)
    }
}

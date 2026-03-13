//! Tier 5 HDBSCAN batch entity resolution.
//!
//! Processes the unresolved_entities pool: embeds entity names,
//! clusters with HDBSCAN, and resolves each cluster to a new
//! canonical node. Noise entities are created as individual nodes.
//! Called via `/admin/tier5/resolve` or as part of deep consolidation.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::ingestion::embedder::{Embedder, truncate_and_validate};
use crate::models::node::Node;
use crate::models::unresolved_entity::UnresolvedEntity;
use crate::storage::postgres::PgRepo;
use crate::storage::traits::{NodeRepo, UnresolvedEntityRepo};

use super::ontology::LabelWithCount;

/// Configuration for Tier 5 HDBSCAN resolution.
#[derive(Debug, Clone)]
pub struct Tier5Config {
    /// Minimum cluster size for HDBSCAN (default: 2).
    pub min_cluster_size: usize,
    /// Target embedding dimension for node storage.
    pub node_embed_dim: usize,
}

impl Default for Tier5Config {
    fn default() -> Self {
        Self {
            min_cluster_size: 2,
            node_embed_dim: 256,
        }
    }
}

/// Report from a Tier 5 batch resolution run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tier5Report {
    /// Total entities processed from the pool.
    pub entities_processed: usize,
    /// Number of clusters formed by HDBSCAN.
    pub clusters_formed: usize,
    /// Number of entities resolved via clustering.
    pub clustered_resolved: usize,
    /// Number of noise entities promoted to individual nodes.
    pub noise_promoted: usize,
    /// Number of entities that already had no embedding (skipped).
    pub skipped_no_embedding: usize,
}

/// Run Tier 5 HDBSCAN batch resolution on all pending unresolved entities.
///
/// Algorithm:
/// 1. Fetch all pending entities from `unresolved_entities`
/// 2. Embed entity names that lack embeddings
/// 3. Cluster with HDBSCAN (density-based, no threshold needed)
/// 4. For each cluster: create a node with the canonical name
///    (highest confidence entity) and resolve all members to it
/// 5. For noise entities: create individual nodes and resolve
pub async fn resolve_tier5(
    repo: &Arc<PgRepo>,
    embedder: &dyn Embedder,
    config: &Tier5Config,
) -> Result<Tier5Report> {
    let pending = UnresolvedEntityRepo::list_pending(repo.as_ref()).await?;

    if pending.is_empty() {
        return Ok(Tier5Report {
            entities_processed: 0,
            clusters_formed: 0,
            clustered_resolved: 0,
            noise_promoted: 0,
            skipped_no_embedding: 0,
        });
    }

    let total = pending.len();
    tracing::info!(pending_count = total, "starting Tier 5 batch resolution");

    // Embed entity names that lack embeddings.
    let names_to_embed: Vec<String> = pending
        .iter()
        .filter(|e| e.embedding.is_none())
        .map(|e| e.extracted_name.clone())
        .collect();

    let mut embeddings_map: std::collections::HashMap<String, Vec<f64>> =
        std::collections::HashMap::new();

    if !names_to_embed.is_empty() {
        let unique_names: Vec<String> = names_to_embed
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .cloned()
            .collect();

        let embedded = embedder.embed(&unique_names).await.map_err(|e| {
            Error::EntityResolution(format!("failed to embed unresolved entity names: {e}"))
        })?;

        for (name, emb) in unique_names.into_iter().zip(embedded) {
            embeddings_map.insert(name, emb);
        }
    }

    // Build labels + embeddings vectors aligned by index.
    let mut labels: Vec<LabelWithCount> = Vec::with_capacity(total);
    let mut embeddings: Vec<Vec<f64>> = Vec::with_capacity(total);
    let mut entity_indices: Vec<usize> = Vec::with_capacity(total);
    let mut skipped = 0usize;

    for (idx, entity) in pending.iter().enumerate() {
        let emb = if let Some(ref e) = entity.embedding {
            e.clone()
        } else if let Some(e) = embeddings_map.get(&entity.extracted_name) {
            e.clone()
        } else {
            skipped += 1;
            continue;
        };

        labels.push(LabelWithCount {
            label: entity.extracted_name.clone(),
            count: 1, // unresolved entities don't have mention counts
        });
        embeddings.push(emb);
        entity_indices.push(idx);
    }

    if labels.is_empty() {
        return Ok(Tier5Report {
            entities_processed: total,
            clusters_formed: 0,
            clustered_resolved: 0,
            noise_promoted: 0,
            skipped_no_embedding: skipped,
        });
    }

    // Run HDBSCAN clustering.
    let cluster_result = super::ontology::cluster_labels(
        &labels,
        &embeddings,
        config.min_cluster_size,
        super::ontology::ClusterLevel::Entity,
    )?;

    let mut clustered_resolved = 0usize;
    let mut noise_promoted = 0usize;

    // Build a reverse map: label → entity indices in the pending list.
    let mut label_to_pending: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, label) in labels.iter().enumerate() {
        label_to_pending
            .entry(label.label.clone())
            .or_default()
            .push(entity_indices[i]);
    }

    // Process clusters: create canonical node, resolve all members.
    for cluster in &cluster_result.clusters {
        // Pick the entity with highest confidence as canonical.
        let mut best_confidence = f64::NEG_INFINITY;
        let mut canonical_entity: Option<&UnresolvedEntity> = None;

        for member_label in &cluster.member_labels {
            if let Some(indices) = label_to_pending.get(member_label) {
                for &idx in indices {
                    if pending[idx].confidence > best_confidence {
                        best_confidence = pending[idx].confidence;
                        canonical_entity = Some(&pending[idx]);
                    }
                }
            }
        }

        let canonical = match canonical_entity {
            Some(e) => e,
            None => continue,
        };

        // Check if a node with this canonical name already exists.
        let node_id = if let Some(existing) =
            NodeRepo::find_by_name(repo.as_ref(), &canonical.extracted_name).await?
        {
            existing.id
        } else {
            // Create new node for this cluster.
            let mut node = Node::new(
                canonical.extracted_name.clone(),
                canonical.entity_type.clone(),
            );
            node.description = canonical.description.clone();
            NodeRepo::create(repo.as_ref(), &node).await?;

            // Embed the node.
            if let Some(emb) = embeddings_map.get(&canonical.extracted_name) {
                if let Ok(truncated) = truncate_and_validate(emb, config.node_embed_dim, "nodes") {
                    NodeRepo::update_embedding(repo.as_ref(), node.id, &truncated).await?;
                }
            }

            node.id
        };

        // Mark all entities in this cluster as resolved.
        for member_label in &cluster.member_labels {
            if let Some(indices) = label_to_pending.get(member_label) {
                for &idx in indices {
                    UnresolvedEntityRepo::mark_resolved(repo.as_ref(), pending[idx].id, node_id)
                        .await?;
                    clustered_resolved += 1;
                }
            }
        }
    }

    // Process noise entities: create individual nodes.
    for noise_label in &cluster_result.noise_labels {
        if let Some(indices) = label_to_pending.get(noise_label) {
            for &idx in indices {
                let entity = &pending[idx];

                // Check if a node already exists.
                let node_id = if let Some(existing) =
                    NodeRepo::find_by_name(repo.as_ref(), &entity.extracted_name).await?
                {
                    existing.id
                } else {
                    let mut node =
                        Node::new(entity.extracted_name.clone(), entity.entity_type.clone());
                    node.description = entity.description.clone();
                    NodeRepo::create(repo.as_ref(), &node).await?;

                    if let Some(emb) = embeddings_map.get(&entity.extracted_name) {
                        if let Ok(truncated) =
                            truncate_and_validate(emb, config.node_embed_dim, "nodes")
                        {
                            NodeRepo::update_embedding(repo.as_ref(), node.id, &truncated).await?;
                        }
                    }

                    node.id
                };

                UnresolvedEntityRepo::mark_resolved(repo.as_ref(), entity.id, node_id).await?;
                noise_promoted += 1;
            }
        }
    }

    let report = Tier5Report {
        entities_processed: total,
        clusters_formed: cluster_result.clusters.len(),
        clustered_resolved,
        noise_promoted,
        skipped_no_embedding: skipped,
    };

    tracing::info!(
        entities = total,
        clusters = report.clusters_formed,
        clustered = clustered_resolved,
        noise = noise_promoted,
        skipped = skipped,
        "Tier 5 batch resolution complete"
    );

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier5_config_defaults() {
        let config = Tier5Config::default();
        assert_eq!(config.min_cluster_size, 2);
        assert_eq!(config.node_embed_dim, 256);
    }

    #[test]
    fn tier5_report_serializes() {
        let report = Tier5Report {
            entities_processed: 10,
            clusters_formed: 3,
            clustered_resolved: 7,
            noise_promoted: 3,
            skipped_no_embedding: 0,
        };
        let json = serde_json::to_string(&report).unwrap();
        let restored: Tier5Report = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.entities_processed, 10);
        assert_eq!(restored.clusters_formed, 3);
    }
}

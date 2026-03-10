//! Emergent ontology via embed-cluster-resolve.
//!
//! Instead of predefined ontologies, lets structure emerge from data:
//! extract freely, embed labels, cluster by cosine similarity, and
//! pick canonical names. The same mechanism applies at three levels:
//! entity names, entity types, and relationship types.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::ingestion::Embedder;
use crate::ingestion::landscape::cosine_similarity;

/// The level at which ontology clustering operates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClusterLevel {
    /// Cluster entity names (e.g., "NYC" and "New York City").
    Entity,
    /// Cluster entity type labels (e.g., "person" and "individual").
    EntityType,
    /// Cluster relationship type labels (e.g., "works_at" and
    /// "employed_by").
    RelationType,
}

/// A cluster of semantically equivalent labels discovered by
/// embedding similarity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OntologyCluster {
    /// Unique cluster identifier.
    pub id: Uuid,
    /// The level this cluster operates at.
    pub level: ClusterLevel,
    /// The canonical (most frequent) label in the cluster.
    pub canonical_label: String,
    /// All member labels in the cluster (includes canonical).
    pub member_labels: Vec<String>,
    /// Total mention count across all member labels.
    pub member_count: usize,
}

/// A label with its associated mention count, used as input to
/// clustering.
#[derive(Debug, Clone)]
pub struct LabelWithCount {
    /// The label text.
    pub label: String,
    /// How many times this label appears in the graph.
    pub count: usize,
}

/// Cluster labels by cosine similarity of their embeddings.
///
/// Uses greedy agglomerative clustering: labels are processed in
/// descending order of mention count. Each label is compared
/// against existing cluster centroids. If the maximum similarity
/// exceeds `threshold`, the label joins that cluster; otherwise
/// it seeds a new cluster.
///
/// The canonical label for each cluster is the one with the
/// highest mention count (i.e., the first label added).
pub fn cluster_labels(
    labels: &[LabelWithCount],
    embeddings: &[Vec<f64>],
    threshold: f64,
    level: ClusterLevel,
) -> Result<Vec<OntologyCluster>> {
    if labels.len() != embeddings.len() {
        return Err(Error::InvalidInput(format!(
            "labels count ({}) != embeddings count ({})",
            labels.len(),
            embeddings.len()
        )));
    }

    if labels.is_empty() {
        return Ok(Vec::new());
    }

    // Sort indices by descending mention count so the most
    // frequent label becomes each cluster's canonical name.
    let mut indices: Vec<usize> = (0..labels.len()).collect();
    indices.sort_by(|&a, &b| labels[b].count.cmp(&labels[a].count));

    // Each cluster tracks: canonical index, member indices,
    // centroid embedding (average of members).
    struct BuildCluster {
        canonical_idx: usize,
        member_indices: Vec<usize>,
        centroid: Vec<f64>,
    }

    let mut clusters: Vec<BuildCluster> = Vec::new();

    for &idx in &indices {
        let emb = &embeddings[idx];

        // Find the most similar existing cluster centroid.
        let mut best_sim = f64::NEG_INFINITY;
        let mut best_cluster: Option<usize> = None;

        for (ci, cluster) in clusters.iter().enumerate() {
            let sim = cosine_similarity(emb, &cluster.centroid);
            if sim > best_sim {
                best_sim = sim;
                best_cluster = Some(ci);
            }
        }

        if best_sim >= threshold {
            // Merge into the best cluster and update centroid.
            if let Some(ci) = best_cluster {
                let cluster = &mut clusters[ci];
                let n = cluster.member_indices.len() as f64;
                // Incremental centroid update:
                // new_centroid = (old_centroid * n + new_emb) / (n+1)
                for (j, val) in emb.iter().enumerate() {
                    if j < cluster.centroid.len() {
                        cluster.centroid[j] = (cluster.centroid[j] * n + val) / (n + 1.0);
                    }
                }
                cluster.member_indices.push(idx);
            }
        } else {
            // Seed a new cluster.
            clusters.push(BuildCluster {
                canonical_idx: idx,
                member_indices: vec![idx],
                centroid: emb.clone(),
            });
        }
    }

    // Convert to OntologyCluster output.
    let result = clusters
        .into_iter()
        .map(|c| {
            let member_labels: Vec<String> = c
                .member_indices
                .iter()
                .map(|&i| labels[i].label.clone())
                .collect();
            let member_count: usize = c.member_indices.iter().map(|&i| labels[i].count).sum();
            OntologyCluster {
                id: Uuid::new_v4(),
                level,
                canonical_label: labels[c.canonical_idx].label.clone(),
                member_labels,
                member_count,
            }
        })
        .collect();

    Ok(result)
}

/// Build ontology clusters for entity canonical names.
///
/// Queries all node `canonical_name` values with their mention
/// counts from the database, embeds them, and clusters by cosine
/// similarity.
pub async fn build_entity_clusters(
    pool: &PgPool,
    embedder: &dyn Embedder,
    threshold: f64,
) -> Result<Vec<OntologyCluster>> {
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT canonical_name, mention_count \
         FROM nodes \
         WHERE canonical_name IS NOT NULL \
         GROUP BY canonical_name, mention_count \
         ORDER BY mention_count DESC",
    )
    .fetch_all(pool)
    .await
    .map_err(Error::Database)?;

    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let labels: Vec<LabelWithCount> = rows
        .iter()
        .map(|(name, count)| LabelWithCount {
            label: name.clone(),
            count: *count as usize,
        })
        .collect();

    let texts: Vec<String> = labels.iter().map(|l| l.label.clone()).collect();
    let embeddings = embedder.embed(&texts).await?;

    cluster_labels(&labels, &embeddings, threshold, ClusterLevel::Entity)
}

/// Build ontology clusters for entity type labels.
///
/// Queries all distinct `node_type` values with their occurrence
/// counts, embeds them, and clusters by cosine similarity.
pub async fn build_type_clusters(
    pool: &PgPool,
    embedder: &dyn Embedder,
    threshold: f64,
) -> Result<Vec<OntologyCluster>> {
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT node_type, COUNT(*) as cnt \
         FROM nodes \
         WHERE node_type IS NOT NULL \
         GROUP BY node_type \
         ORDER BY cnt DESC",
    )
    .fetch_all(pool)
    .await
    .map_err(Error::Database)?;

    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let labels: Vec<LabelWithCount> = rows
        .iter()
        .map(|(name, count)| LabelWithCount {
            label: name.clone(),
            count: *count as usize,
        })
        .collect();

    let texts: Vec<String> = labels.iter().map(|l| l.label.clone()).collect();
    let embeddings = embedder.embed(&texts).await?;

    cluster_labels(&labels, &embeddings, threshold, ClusterLevel::EntityType)
}

/// Build ontology clusters for relationship type labels.
///
/// Queries all distinct `rel_type` values from edges with their
/// occurrence counts, embeds them, and clusters by cosine
/// similarity.
pub async fn build_rel_type_clusters(
    pool: &PgPool,
    embedder: &dyn Embedder,
    threshold: f64,
) -> Result<Vec<OntologyCluster>> {
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT rel_type, COUNT(*) as cnt \
         FROM edges \
         WHERE rel_type IS NOT NULL \
         GROUP BY rel_type \
         ORDER BY cnt DESC",
    )
    .fetch_all(pool)
    .await
    .map_err(Error::Database)?;

    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let labels: Vec<LabelWithCount> = rows
        .iter()
        .map(|(name, count)| LabelWithCount {
            label: name.clone(),
            count: *count as usize,
        })
        .collect();

    let texts: Vec<String> = labels.iter().map(|l| l.label.clone()).collect();
    let embeddings = embedder.embed(&texts).await?;

    cluster_labels(&labels, &embeddings, threshold, ClusterLevel::RelationType)
}

/// Apply discovered clusters: store definitions in
/// `ontology_clusters` and write canonical labels back to
/// nodes/edges.
///
/// Existing clusters at the same level are cleared first to
/// ensure idempotent application.
pub async fn apply_clusters(
    pool: &PgPool,
    clusters: &[OntologyCluster],
    threshold: f64,
) -> Result<()> {
    if clusters.is_empty() {
        return Ok(());
    }

    // Group by level for targeted clearing.
    let levels: Vec<&str> = clusters
        .iter()
        .map(|c| match c.level {
            ClusterLevel::Entity => "entity",
            ClusterLevel::EntityType => "entity_type",
            ClusterLevel::RelationType => "rel_type",
        })
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    // Clear existing clusters at these levels.
    for level in &levels {
        // Unlink nodes from clusters being cleared.
        if *level == "entity" {
            sqlx::query(
                "UPDATE nodes SET cluster_id = NULL \
                 WHERE cluster_id IN (\
                     SELECT id FROM ontology_clusters WHERE level = $1\
                 )",
            )
            .bind(level)
            .execute(pool)
            .await
            .map_err(Error::Database)?;
        }

        sqlx::query("DELETE FROM ontology_clusters WHERE level = $1")
            .bind(level)
            .execute(pool)
            .await
            .map_err(Error::Database)?;
    }

    // Insert cluster definitions and apply canonical labels.
    for cluster in clusters {
        let level_str = match cluster.level {
            ClusterLevel::Entity => "entity",
            ClusterLevel::EntityType => "entity_type",
            ClusterLevel::RelationType => "rel_type",
        };

        let member_labels_json = serde_json::to_value(&cluster.member_labels)
            .unwrap_or(serde_json::Value::Array(vec![]));

        sqlx::query(
            "INSERT INTO ontology_clusters \
                 (id, level, canonical_label, member_labels, member_count, threshold) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(cluster.id)
        .bind(level_str)
        .bind(&cluster.canonical_label)
        .bind(&member_labels_json)
        .bind(cluster.member_count as i32)
        .bind(threshold)
        .execute(pool)
        .await
        .map_err(Error::Database)?;

        // Write back canonical labels to the source tables.
        match cluster.level {
            ClusterLevel::Entity => {
                // Link nodes whose canonical_name matches any
                // member label to this cluster.
                for label in &cluster.member_labels {
                    sqlx::query(
                        "UPDATE nodes SET cluster_id = $1 \
                         WHERE canonical_name = $2",
                    )
                    .bind(cluster.id)
                    .bind(label)
                    .execute(pool)
                    .await
                    .map_err(Error::Database)?;
                }
            }
            ClusterLevel::EntityType => {
                // Set canonical_type for all nodes whose node_type
                // is a member of this cluster.
                for label in &cluster.member_labels {
                    sqlx::query(
                        "UPDATE nodes SET canonical_type = $1 \
                         WHERE node_type = $2",
                    )
                    .bind(&cluster.canonical_label)
                    .bind(label)
                    .execute(pool)
                    .await
                    .map_err(Error::Database)?;
                }
            }
            ClusterLevel::RelationType => {
                // Set canonical_rel_type for all edges whose
                // rel_type is a member of this cluster.
                for label in &cluster.member_labels {
                    sqlx::query(
                        "UPDATE edges SET canonical_rel_type = $1 \
                         WHERE rel_type = $2",
                    )
                    .bind(&cluster.canonical_label)
                    .bind(label)
                    .execute(pool)
                    .await
                    .map_err(Error::Database)?;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingestion::landscape::cosine_similarity;

    /// Helper: build a unit vector with `dim_index` set to 1.0.
    fn unit_vec(dim: usize, dim_index: usize) -> Vec<f64> {
        let mut v = vec![0.0; dim];
        if dim_index < dim {
            v[dim_index] = 1.0;
        }
        v
    }

    /// Helper: build a vector from a slice.
    fn vec_of(vals: &[f64]) -> Vec<f64> {
        vals.to_vec()
    }

    #[test]
    fn empty_input_returns_empty() {
        let result = cluster_labels(&[], &[], 0.8, ClusterLevel::Entity);
        assert!(result.is_ok());
        assert!(result.as_ref().ok().is_some_and(|v| v.is_empty()));
    }

    #[test]
    fn mismatched_lengths_returns_error() {
        let labels = vec![LabelWithCount {
            label: "a".into(),
            count: 1,
        }];
        let embeddings: Vec<Vec<f64>> = vec![];
        let result = cluster_labels(&labels, &embeddings, 0.8, ClusterLevel::Entity);
        assert!(result.is_err());
    }

    #[test]
    fn identical_labels_cluster_together() {
        let labels = vec![
            LabelWithCount {
                label: "New York City".into(),
                count: 10,
            },
            LabelWithCount {
                label: "NYC".into(),
                count: 5,
            },
        ];
        let emb = vec![1.0, 0.5, 0.3];
        let embeddings = vec![emb.clone(), emb];

        let clusters = cluster_labels(&labels, &embeddings, 0.9, ClusterLevel::Entity)
            .expect("clustering should succeed");

        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].canonical_label, "New York City");
        assert_eq!(clusters[0].member_labels.len(), 2);
        assert_eq!(clusters[0].member_count, 15);
        assert_eq!(clusters[0].level, ClusterLevel::Entity);
    }

    #[test]
    fn different_labels_stay_separate() {
        let labels = vec![
            LabelWithCount {
                label: "person".into(),
                count: 10,
            },
            LabelWithCount {
                label: "location".into(),
                count: 8,
            },
            LabelWithCount {
                label: "event".into(),
                count: 3,
            },
        ];
        let embeddings = vec![unit_vec(3, 0), unit_vec(3, 1), unit_vec(3, 2)];

        let clusters = cluster_labels(&labels, &embeddings, 0.5, ClusterLevel::EntityType)
            .expect("clustering should succeed");

        assert_eq!(clusters.len(), 3);
    }

    #[test]
    fn canonical_label_is_highest_count() {
        let labels = vec![
            LabelWithCount {
                label: "Alpha".into(),
                count: 2,
            },
            LabelWithCount {
                label: "Beta".into(),
                count: 10,
            },
        ];
        let embeddings = vec![vec_of(&[1.0, 0.0]), vec_of(&[0.99, 0.01])];

        let clusters = cluster_labels(&labels, &embeddings, 0.9, ClusterLevel::Entity)
            .expect("clustering should succeed");

        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].canonical_label, "Beta");
    }

    #[test]
    fn threshold_boundary_behavior() {
        let a = vec_of(&[1.0, 0.0, 0.0]);
        let b = vec_of(&[0.99, 0.1, 0.0]);
        let sim = cosine_similarity(&a, &b);

        let labels = vec![
            LabelWithCount {
                label: "A".into(),
                count: 5,
            },
            LabelWithCount {
                label: "B".into(),
                count: 3,
            },
        ];

        // With threshold above the similarity, they stay separate.
        let high = cluster_labels(
            &labels,
            &[a.clone(), b.clone()],
            sim + 0.01,
            ClusterLevel::Entity,
        )
        .expect("clustering should succeed");
        assert_eq!(high.len(), 2);

        // With threshold at or below the similarity, they merge.
        let low = cluster_labels(&labels, &[a, b], sim - 0.01, ClusterLevel::Entity)
            .expect("clustering should succeed");
        assert_eq!(low.len(), 1);
    }

    #[test]
    fn single_label_forms_single_cluster() {
        let labels = vec![LabelWithCount {
            label: "singleton".into(),
            count: 42,
        }];
        let embeddings = vec![vec_of(&[1.0, 0.5])];

        let clusters = cluster_labels(&labels, &embeddings, 0.9, ClusterLevel::RelationType)
            .expect("clustering should succeed");

        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].canonical_label, "singleton");
        assert_eq!(clusters[0].member_count, 42);
        assert_eq!(clusters[0].level, ClusterLevel::RelationType);
    }

    #[test]
    fn cluster_level_preserved() {
        let labels = vec![LabelWithCount {
            label: "x".into(),
            count: 1,
        }];
        let embeddings = vec![vec_of(&[1.0])];

        for level in [
            ClusterLevel::Entity,
            ClusterLevel::EntityType,
            ClusterLevel::RelationType,
        ] {
            let clusters = cluster_labels(&labels, &embeddings, 0.9, level)
                .expect("clustering should succeed");
            assert_eq!(clusters[0].level, level);
        }
    }

    #[test]
    fn mixed_clustering_some_merge_some_separate() {
        let labels = vec![
            LabelWithCount {
                label: "works_at".into(),
                count: 20,
            },
            LabelWithCount {
                label: "employed_by".into(),
                count: 15,
            },
            LabelWithCount {
                label: "located_in".into(),
                count: 10,
            },
            LabelWithCount {
                label: "situated_in".into(),
                count: 8,
            },
            LabelWithCount {
                label: "contradicts".into(),
                count: 3,
            },
        ];
        let embeddings = vec![
            vec_of(&[1.0, 0.0, 0.0]),   // works_at
            vec_of(&[0.98, 0.05, 0.0]), // employed_by ~ works_at
            vec_of(&[0.0, 1.0, 0.0]),   // located_in
            vec_of(&[0.02, 0.99, 0.0]), // situated_in ~ located_in
            vec_of(&[0.0, 0.0, 1.0]),   // contradicts (alone)
        ];

        let clusters = cluster_labels(&labels, &embeddings, 0.9, ClusterLevel::RelationType)
            .expect("clustering should succeed");

        assert_eq!(clusters.len(), 3);

        let work_cluster = clusters
            .iter()
            .find(|c| c.canonical_label == "works_at")
            .expect("should have works_at cluster");
        assert!(
            work_cluster
                .member_labels
                .contains(&"employed_by".to_string())
        );
        assert_eq!(work_cluster.member_count, 35);

        let loc_cluster = clusters
            .iter()
            .find(|c| c.canonical_label == "located_in")
            .expect("should have located_in cluster");
        assert!(
            loc_cluster
                .member_labels
                .contains(&"situated_in".to_string())
        );

        let contra_cluster = clusters
            .iter()
            .find(|c| c.canonical_label == "contradicts")
            .expect("should have contradicts cluster");
        assert_eq!(contra_cluster.member_labels.len(), 1);
    }

    #[test]
    fn cosine_similarity_reuse_from_landscape() {
        let a = vec_of(&[1.0, 0.0, 0.0]);
        let b = vec_of(&[1.0, 0.0, 0.0]);
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-10);

        let c = vec_of(&[0.0, 1.0, 0.0]);
        assert!(cosine_similarity(&a, &c).abs() < 1e-10);

        let z = vec_of(&[0.0, 0.0, 0.0]);
        assert_eq!(cosine_similarity(&a, &z), 0.0);

        let short = vec_of(&[1.0]);
        assert_eq!(cosine_similarity(&a, &short), 0.0);
    }
}

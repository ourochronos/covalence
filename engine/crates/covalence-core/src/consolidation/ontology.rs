//! Emergent ontology via embed-cluster-resolve.
//!
//! Instead of predefined ontologies, lets structure emerge from data:
//! extract freely, embed labels, cluster with HDBSCAN, and pick
//! canonical names. The same mechanism applies at three levels:
//! entity names, entity types, and relationship types.
//!
//! Uses HDBSCAN (Hierarchical Density-Based Spatial Clustering of
//! Applications with Noise) which finds natural density-based
//! clusters without requiring a similarity threshold. Genuinely
//! unique labels are correctly identified as noise (unclustered).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::ingestion::Embedder;

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
/// density-based clustering.
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

/// Result of HDBSCAN-based clustering, including noise labels
/// that did not belong to any cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterResult {
    /// Discovered clusters.
    pub clusters: Vec<OntologyCluster>,
    /// Labels that HDBSCAN identified as noise (unclustered).
    /// These are genuinely unique labels that don't belong to
    /// any density-based group.
    pub noise_labels: Vec<String>,
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

/// L2-normalize a vector to unit length.
///
/// Euclidean distance between unit vectors is monotonically related
/// to cosine distance: `||a - b|| = sqrt(2 * (1 - cos(a, b)))`.
/// This gives HDBSCAN better numerical properties than raw cosine
/// distance matrices.
fn l2_normalize(v: &[f64]) -> Vec<f64> {
    let norm: f64 = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm > 0.0 {
        v.iter().map(|x| x / norm).collect()
    } else {
        v.to_vec()
    }
}

/// Cluster labels using HDBSCAN on cosine distances of their
/// embeddings.
///
/// Uses density-based clustering to find natural groupings without
/// requiring a similarity threshold. Labels that don't belong to
/// any cluster are returned as noise.
///
/// `min_cluster_size` is the minimum number of labels required to
/// form a cluster (default: 2).
pub fn cluster_labels(
    labels: &[LabelWithCount],
    embeddings: &[Vec<f64>],
    min_cluster_size: usize,
    level: ClusterLevel,
) -> Result<ClusterResult> {
    if labels.len() != embeddings.len() {
        return Err(Error::InvalidInput(format!(
            "labels count ({}) != embeddings count ({})",
            labels.len(),
            embeddings.len()
        )));
    }

    if labels.is_empty() {
        return Ok(ClusterResult {
            clusters: Vec::new(),
            noise_labels: Vec::new(),
        });
    }

    // With fewer points than min_cluster_size, HDBSCAN will
    // classify everything as noise. Return all as noise.
    if labels.len() < min_cluster_size {
        return Ok(ClusterResult {
            clusters: Vec::new(),
            noise_labels: labels.iter().map(|l| l.label.clone()).collect(),
        });
    }

    // L2-normalize embeddings so Euclidean distance is
    // monotonically related to cosine distance. This gives
    // HDBSCAN much better numerical properties than raw
    // cosine distance matrices.
    let normalized: Vec<Vec<f64>> = embeddings.iter().map(|v| l2_normalize(v)).collect();

    // Run HDBSCAN with Euclidean distance on unit vectors.
    // min_samples(1) ensures core distance = distance to nearest
    // neighbor, which works well for small label sets.
    let hyper_params = hdbscan::HdbscanHyperParams::builder()
        .min_cluster_size(min_cluster_size)
        .min_samples(1)
        .dist_metric(hdbscan::DistanceMetric::Euclidean)
        .build();
    let clusterer = hdbscan::Hdbscan::new(&normalized, hyper_params);
    let assignments = clusterer
        .cluster()
        .map_err(|e| Error::InvalidInput(format!("HDBSCAN error: {e:?}")))?;

    // Group indices by cluster assignment.
    let mut cluster_map: HashMap<i32, Vec<usize>> = HashMap::new();
    let mut noise_indices: Vec<usize> = Vec::new();

    for (idx, &cluster_id) in assignments.iter().enumerate() {
        if cluster_id < 0 {
            noise_indices.push(idx);
        } else {
            cluster_map.entry(cluster_id).or_default().push(idx);
        }
    }

    // Build OntologyCluster for each HDBSCAN cluster.
    let mut clusters: Vec<OntologyCluster> = cluster_map
        .into_values()
        .map(|mut indices| {
            // Sort by descending mention count so the most frequent
            // label becomes canonical.
            indices.sort_by(|&a, &b| labels[b].count.cmp(&labels[a].count));

            let canonical_idx = indices[0];
            let member_labels: Vec<String> =
                indices.iter().map(|&i| labels[i].label.clone()).collect();
            let member_count: usize = indices.iter().map(|&i| labels[i].count).sum();

            OntologyCluster {
                id: Uuid::new_v4(),
                level,
                canonical_label: labels[canonical_idx].label.clone(),
                member_labels,
                member_count,
            }
        })
        .collect();

    // Sort clusters by member_count descending for deterministic output.
    clusters.sort_by(|a, b| b.member_count.cmp(&a.member_count));

    let noise_labels: Vec<String> = noise_indices
        .iter()
        .map(|&i| labels[i].label.clone())
        .collect();

    Ok(ClusterResult {
        clusters,
        noise_labels,
    })
}

/// Build ontology clusters for entity canonical names.
///
/// Queries all node `canonical_name` values with their mention
/// counts from the database, embeds them, and clusters with
/// HDBSCAN.
pub async fn build_entity_clusters(
    pool: &PgPool,
    embedder: &dyn Embedder,
    min_cluster_size: usize,
) -> Result<ClusterResult> {
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT canonical_name, mention_count::int8 \
         FROM nodes \
         WHERE canonical_name IS NOT NULL \
         GROUP BY canonical_name, mention_count \
         ORDER BY mention_count DESC",
    )
    .fetch_all(pool)
    .await
    .map_err(Error::Database)?;

    if rows.is_empty() {
        return Ok(ClusterResult {
            clusters: Vec::new(),
            noise_labels: Vec::new(),
        });
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

    cluster_labels(&labels, &embeddings, min_cluster_size, ClusterLevel::Entity)
}

/// Build ontology clusters for entity type labels.
///
/// Queries all distinct `node_type` values with their occurrence
/// counts, embeds them, and clusters with HDBSCAN.
pub async fn build_type_clusters(
    pool: &PgPool,
    embedder: &dyn Embedder,
    min_cluster_size: usize,
) -> Result<ClusterResult> {
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
        return Ok(ClusterResult {
            clusters: Vec::new(),
            noise_labels: Vec::new(),
        });
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

    cluster_labels(
        &labels,
        &embeddings,
        min_cluster_size,
        ClusterLevel::EntityType,
    )
}

/// Build ontology clusters for relationship type labels.
///
/// Queries all distinct `rel_type` values from edges with their
/// occurrence counts, embeds them, and clusters with HDBSCAN.
pub async fn build_rel_type_clusters(
    pool: &PgPool,
    embedder: &dyn Embedder,
    min_cluster_size: usize,
) -> Result<ClusterResult> {
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
        return Ok(ClusterResult {
            clusters: Vec::new(),
            noise_labels: Vec::new(),
        });
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

    cluster_labels(
        &labels,
        &embeddings,
        min_cluster_size,
        ClusterLevel::RelationType,
    )
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
    min_cluster_size: usize,
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

        let member_labels_json = serde_json::to_value(&cluster.member_labels).map_err(|e| {
            Error::Consolidation(format!("failed to serialize member labels: {e}"))
        })?;

        sqlx::query(
            "INSERT INTO ontology_clusters \
                 (id, level, canonical_label, member_labels, \
                  member_count, min_cluster_size) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(cluster.id)
        .bind(level_str)
        .bind(&cluster.canonical_label)
        .bind(&member_labels_json)
        .bind(cluster.member_count as i32)
        .bind(min_cluster_size as i32)
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
        let result = cluster_labels(&[], &[], 2, ClusterLevel::Entity);
        assert!(result.is_ok());
        let cr = result.unwrap();
        assert!(cr.clusters.is_empty());
        assert!(cr.noise_labels.is_empty());
    }

    #[test]
    fn mismatched_lengths_returns_error() {
        let labels = vec![LabelWithCount {
            label: "a".into(),
            count: 1,
        }];
        let embeddings: Vec<Vec<f64>> = vec![];
        let result = cluster_labels(&labels, &embeddings, 2, ClusterLevel::Entity);
        assert!(result.is_err());
    }

    #[test]
    fn two_pairs_cluster_with_noise() {
        // Two pairs of similar labels + one outlier. HDBSCAN
        // should find two clusters and one noise point.
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
            vec_of(&[0.95, 0.05, 0.0]), // employed_by
            vec_of(&[0.0, 1.0, 0.0]),   // located_in
            vec_of(&[0.05, 0.95, 0.0]), // situated_in
            vec_of(&[0.0, 0.0, 1.0]),   // contradicts
        ];

        let result = cluster_labels(&labels, &embeddings, 2, ClusterLevel::RelationType)
            .expect("clustering should succeed");

        // Should find 2 clusters.
        assert_eq!(
            result.clusters.len(),
            2,
            "expected 2 clusters, got {:?}",
            result
        );

        // "contradicts" should be noise.
        assert!(
            result.noise_labels.contains(&"contradicts".to_string()),
            "contradicts should be noise"
        );

        // works_at and employed_by should be in the same cluster.
        let work_cluster = result
            .clusters
            .iter()
            .find(|c| c.member_labels.contains(&"works_at".to_string()))
            .expect("should have works_at cluster");
        assert!(
            work_cluster
                .member_labels
                .contains(&"employed_by".to_string())
        );
        assert_eq!(work_cluster.canonical_label, "works_at");

        // located_in and situated_in should be in the same cluster.
        let loc_cluster = result
            .clusters
            .iter()
            .find(|c| c.member_labels.contains(&"located_in".to_string()))
            .expect("should have located_in cluster");
        assert!(
            loc_cluster
                .member_labels
                .contains(&"situated_in".to_string())
        );
    }

    #[test]
    fn orthogonal_labels_become_noise() {
        // Orthogonal embeddings should all be noise since each
        // label is in its own density region.
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

        let result = cluster_labels(&labels, &embeddings, 2, ClusterLevel::EntityType)
            .expect("clustering should succeed");

        assert!(result.clusters.is_empty());
        assert_eq!(result.noise_labels.len(), 3);
    }

    #[test]
    fn canonical_label_is_highest_count() {
        // When similar labels cluster, the one with the highest
        // mention count should become canonical.
        let labels = vec![
            LabelWithCount {
                label: "works_at".into(),
                count: 2,
            },
            LabelWithCount {
                label: "employed_by".into(),
                count: 10,
            },
            LabelWithCount {
                label: "located_in".into(),
                count: 5,
            },
            LabelWithCount {
                label: "situated_in".into(),
                count: 4,
            },
            LabelWithCount {
                label: "contradicts".into(),
                count: 1,
            },
        ];
        let embeddings = vec![
            vec_of(&[1.0, 0.0, 0.0]),   // works_at
            vec_of(&[0.95, 0.05, 0.0]), // employed_by
            vec_of(&[0.0, 1.0, 0.0]),   // located_in
            vec_of(&[0.05, 0.95, 0.0]), // situated_in
            vec_of(&[0.0, 0.0, 1.0]),   // contradicts
        ];

        let result = cluster_labels(&labels, &embeddings, 2, ClusterLevel::RelationType)
            .expect("clustering should succeed");

        // The works_at cluster should have employed_by as canonical
        // (higher count).
        let cluster = result
            .clusters
            .iter()
            .find(|c| c.member_labels.contains(&"employed_by".to_string()))
            .expect("should have cluster containing employed_by");
        assert_eq!(cluster.canonical_label, "employed_by");
    }

    #[test]
    fn single_label_is_noise() {
        let labels = vec![LabelWithCount {
            label: "singleton".into(),
            count: 42,
        }];
        let embeddings = vec![vec_of(&[1.0, 0.5])];

        let result = cluster_labels(&labels, &embeddings, 2, ClusterLevel::RelationType)
            .expect("clustering should succeed");

        assert!(result.clusters.is_empty());
        assert_eq!(result.noise_labels, vec!["singleton"]);
    }

    #[test]
    fn cluster_level_preserved() {
        // 5 labels: 2 pairs + noise to test level preservation.
        let labels = vec![
            LabelWithCount {
                label: "a".into(),
                count: 1,
            },
            LabelWithCount {
                label: "b".into(),
                count: 1,
            },
            LabelWithCount {
                label: "c".into(),
                count: 1,
            },
            LabelWithCount {
                label: "d".into(),
                count: 1,
            },
            LabelWithCount {
                label: "e".into(),
                count: 1,
            },
        ];
        let embeddings = vec![
            vec_of(&[1.0, 0.0, 0.0]),
            vec_of(&[0.95, 0.05, 0.0]),
            vec_of(&[0.0, 1.0, 0.0]),
            vec_of(&[0.05, 0.95, 0.0]),
            vec_of(&[0.0, 0.0, 1.0]),
        ];

        for level in [
            ClusterLevel::Entity,
            ClusterLevel::EntityType,
            ClusterLevel::RelationType,
        ] {
            let result =
                cluster_labels(&labels, &embeddings, 2, level).expect("clustering should succeed");
            if !result.clusters.is_empty() {
                assert_eq!(result.clusters[0].level, level);
            }
        }
    }

    #[test]
    fn realistic_ten_labels_three_clusters() {
        // 10 labels: 3 groups of 3 similar + 1 unique outlier.
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
                label: "hired_at".into(),
                count: 10,
            },
            LabelWithCount {
                label: "located_in".into(),
                count: 12,
            },
            LabelWithCount {
                label: "situated_in".into(),
                count: 9,
            },
            LabelWithCount {
                label: "based_in".into(),
                count: 7,
            },
            LabelWithCount {
                label: "created_by".into(),
                count: 8,
            },
            LabelWithCount {
                label: "authored_by".into(),
                count: 6,
            },
            LabelWithCount {
                label: "contradicts".into(),
                count: 3,
            },
            LabelWithCount {
                label: "associated_with".into(),
                count: 5,
            },
        ];
        let embeddings = vec![
            vec_of(&[1.0, 0.0, 0.0, 0.0]),   // works_at
            vec_of(&[0.95, 0.05, 0.0, 0.0]), // employed_by
            vec_of(&[0.93, 0.07, 0.0, 0.0]), // hired_at
            vec_of(&[0.0, 1.0, 0.0, 0.0]),   // located_in
            vec_of(&[0.0, 0.95, 0.05, 0.0]), // situated_in
            vec_of(&[0.0, 0.93, 0.07, 0.0]), // based_in
            vec_of(&[0.0, 0.0, 1.0, 0.0]),   // created_by
            vec_of(&[0.0, 0.0, 0.95, 0.05]), // authored_by
            vec_of(&[0.0, 0.0, 0.0, 1.0]),   // contradicts
            vec_of(&[0.5, 0.5, 0.0, 0.0]),   // associated_with
        ];

        let result = cluster_labels(&labels, &embeddings, 2, ClusterLevel::RelationType)
            .expect("clustering should succeed");

        // Should find at least 2 clusters.
        assert!(
            result.clusters.len() >= 2,
            "expected >= 2 clusters, got {:?}",
            result
        );

        // works_at cluster should have the 3 employment labels.
        let work_cluster = result
            .clusters
            .iter()
            .find(|c| c.member_labels.contains(&"works_at".to_string()));
        if let Some(wc) = work_cluster {
            assert_eq!(wc.canonical_label, "works_at");
            assert!(wc.member_labels.len() >= 2);
        }

        // contradicts should be noise (genuinely unique).
        assert!(
            result.noise_labels.contains(&"contradicts".to_string()),
            "contradicts should be noise"
        );
    }

    #[test]
    fn noise_labels_returned_for_outliers() {
        // Two dense groups + scattered outliers. HDBSCAN needs
        // density contrast between multiple groups to work well.
        let labels = vec![
            LabelWithCount {
                label: "cat".into(),
                count: 10,
            },
            LabelWithCount {
                label: "kitten".into(),
                count: 8,
            },
            LabelWithCount {
                label: "feline".into(),
                count: 5,
            },
            LabelWithCount {
                label: "dog".into(),
                count: 9,
            },
            LabelWithCount {
                label: "puppy".into(),
                count: 7,
            },
            LabelWithCount {
                label: "canine".into(),
                count: 4,
            },
            LabelWithCount {
                label: "rocket".into(),
                count: 2,
            },
        ];
        let embeddings = vec![
            vec_of(&[1.0, 0.0, 0.0]),   // cat
            vec_of(&[0.95, 0.05, 0.0]), // kitten
            vec_of(&[0.93, 0.07, 0.0]), // feline
            vec_of(&[0.0, 1.0, 0.0]),   // dog
            vec_of(&[0.05, 0.95, 0.0]), // puppy
            vec_of(&[0.07, 0.93, 0.0]), // canine
            vec_of(&[0.0, 0.0, 1.0]),   // rocket (outlier)
        ];

        let result = cluster_labels(&labels, &embeddings, 2, ClusterLevel::Entity)
            .expect("clustering should succeed");

        // Should find 2 clusters (cat-family and dog-family).
        assert!(
            result.clusters.len() >= 2,
            "expected >= 2 clusters, got {:?}",
            result
        );

        // Outlier should be noise.
        assert!(
            result.noise_labels.contains(&"rocket".to_string()),
            "rocket should be noise, got noise: {:?}",
            result.noise_labels
        );

        // Cat-family should cluster together.
        let cat_cluster = result
            .clusters
            .iter()
            .find(|c| c.member_labels.contains(&"cat".to_string()));
        assert!(cat_cluster.is_some(), "should have a cat cluster");
        assert_eq!(cat_cluster.unwrap().canonical_label, "cat");
    }

    #[test]
    fn l2_normalize_produces_unit_vector() {
        let v = vec![3.0, 4.0];
        let n = l2_normalize(&v);
        let norm: f64 = n.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!((norm - 1.0).abs() < 1e-10);
        assert!((n[0] - 0.6).abs() < 1e-10);
        assert!((n[1] - 0.8).abs() < 1e-10);
    }

    #[test]
    fn l2_normalize_zero_vector_unchanged() {
        let v = vec![0.0, 0.0, 0.0];
        let n = l2_normalize(&v);
        assert_eq!(n, v);
    }

    #[test]
    fn fewer_than_min_cluster_size_all_noise() {
        let labels = vec![LabelWithCount {
            label: "solo".into(),
            count: 1,
        }];
        let embeddings = vec![vec_of(&[1.0])];

        let result = cluster_labels(
            &labels,
            &embeddings,
            3, // min_cluster_size=3 but only 1 label
            ClusterLevel::Entity,
        )
        .expect("clustering should succeed");

        assert!(result.clusters.is_empty());
        assert_eq!(result.noise_labels, vec!["solo"]);
    }
}

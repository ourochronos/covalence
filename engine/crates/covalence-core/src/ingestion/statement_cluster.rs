//! Hierarchical agglomerative clustering (HAC) for statement
//! embeddings.
//!
//! Groups semantically related statements into clusters that will
//! become sections. Uses complete-linkage HAC with cosine similarity
//! so that all statements within a cluster are mutually similar.
//!
//! O(n^2) space and O(n^3) time — acceptable for the expected
//! 20–200 statements per source.

/// A cluster assignment: maps each statement index to a cluster ID.
pub type ClusterAssignments = Vec<usize>;

/// Configuration for HAC clustering.
#[derive(Debug, Clone)]
pub struct ClusterConfig {
    /// Minimum cosine similarity between cluster members for merging.
    /// Default: 0.75.
    pub similarity_threshold: f64,
    /// Minimum number of statements to form a cluster. Singletons
    /// are merged into the nearest cluster. Default: 2.
    pub min_cluster_size: usize,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.75,
            min_cluster_size: 2,
        }
    }
}

/// Run complete-linkage HAC on statement embeddings.
///
/// Returns a vector of cluster IDs (0-indexed), one per input
/// embedding. Embeddings are expected to be L2-normalized (unit
/// vectors) so that dot product equals cosine similarity.
///
/// Statements without embeddings are assigned to a separate
/// singleton cluster each.
pub fn cluster_statements(
    embeddings: &[Option<&[f32]>],
    config: &ClusterConfig,
) -> ClusterAssignments {
    let n = embeddings.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![0];
    }

    // Build the initial similarity matrix (upper triangle).
    // sim[i][j] stores cosine similarity between statement i and j.
    let mut sim = vec![vec![f64::NEG_INFINITY; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let s = match (embeddings[i], embeddings[j]) {
                (Some(a), Some(b)) => cosine_similarity_f32(a, b),
                _ => f64::NEG_INFINITY, // Can't compare without embeddings.
            };
            sim[i][j] = s;
            sim[j][i] = s;
        }
    }

    // Each statement starts in its own cluster.
    let mut cluster_of: Vec<usize> = (0..n).collect();
    let mut next_cluster = n;

    // Members of each cluster (cluster_id → set of statement indices).
    let mut members: std::collections::HashMap<usize, Vec<usize>> =
        (0..n).map(|i| (i, vec![i])).collect();

    // Iteratively merge the closest pair of clusters.
    loop {
        // Find the pair of distinct clusters with the highest
        // *minimum* similarity (complete linkage).
        let mut best_sim = f64::NEG_INFINITY;
        let mut best_pair: Option<(usize, usize)> = None;

        let active_clusters: Vec<usize> = members.keys().copied().collect();
        for (idx_a, &ca) in active_clusters.iter().enumerate() {
            for &cb in active_clusters.iter().skip(idx_a + 1) {
                let min_sim = complete_linkage_sim(&members[&ca], &members[&cb], &sim);
                if min_sim > best_sim {
                    best_sim = min_sim;
                    best_pair = Some((ca, cb));
                }
            }
        }

        let (ca, cb) = match best_pair {
            Some(pair) if best_sim >= config.similarity_threshold => pair,
            _ => break, // No merge above threshold.
        };

        // Merge cb into ca under a new cluster id.
        let merged_id = next_cluster;
        next_cluster += 1;

        let mut merged_members = members.remove(&ca).unwrap();
        merged_members.extend(members.remove(&cb).unwrap());

        for &idx in &merged_members {
            cluster_of[idx] = merged_id;
        }
        members.insert(merged_id, merged_members);
    }

    // Remap cluster IDs to contiguous 0-based range.
    let mut id_map = std::collections::HashMap::new();
    let mut next_id = 0usize;
    let mut result = vec![0usize; n];
    for i in 0..n {
        let cid = cluster_of[i];
        let mapped = *id_map.entry(cid).or_insert_with(|| {
            let id = next_id;
            next_id += 1;
            id
        });
        result[i] = mapped;
    }

    // Handle min_cluster_size: merge singletons into nearest cluster.
    if config.min_cluster_size > 1 && next_id > 1 {
        merge_small_clusters(&mut result, embeddings, config.min_cluster_size);
    }

    result
}

/// Complete-linkage: minimum similarity across all pairs between
/// two clusters. A merge only happens if *every* pair exceeds the
/// threshold.
fn complete_linkage_sim(members_a: &[usize], members_b: &[usize], sim: &[Vec<f64>]) -> f64 {
    let mut min_sim = f64::INFINITY;
    for &a in members_a {
        for &b in members_b {
            let s = sim[a][b];
            if s < min_sim {
                min_sim = s;
            }
        }
    }
    min_sim
}

/// Merge clusters smaller than `min_size` into their nearest
/// neighbor cluster (by average cosine similarity).
fn merge_small_clusters(assignments: &mut [usize], embeddings: &[Option<&[f32]>], min_size: usize) {
    let n = assignments.len();

    // Count members per cluster.
    let mut counts: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for &c in assignments.iter() {
        *counts.entry(c).or_default() += 1;
    }

    // For each small cluster, find the nearest large cluster.
    let small: Vec<usize> = counts
        .iter()
        .filter(|&(_, &count)| count < min_size)
        .map(|(&cid, _)| cid)
        .collect();

    if small.is_empty() {
        return;
    }

    let large: Vec<usize> = counts
        .iter()
        .filter(|&(_, &count)| count >= min_size)
        .map(|(&cid, _)| cid)
        .collect();

    if large.is_empty() {
        // All clusters are small — just leave them as-is.
        return;
    }

    for small_cid in &small {
        let small_indices: Vec<usize> = (0..n).filter(|&i| assignments[i] == *small_cid).collect();

        // Find the large cluster with highest average similarity.
        let mut best_target = large[0];
        let mut best_avg = f64::NEG_INFINITY;

        for &large_cid in &large {
            let large_indices: Vec<usize> =
                (0..n).filter(|&i| assignments[i] == large_cid).collect();

            let mut total = 0.0f64;
            let mut count = 0usize;
            for &si in &small_indices {
                for &li in &large_indices {
                    if let (Some(a), Some(b)) = (embeddings[si], embeddings[li]) {
                        total += cosine_similarity_f32(a, b);
                        count += 1;
                    }
                }
            }
            let avg = if count > 0 {
                total / count as f64
            } else {
                f64::NEG_INFINITY
            };
            if avg > best_avg {
                best_avg = avg;
                best_target = large_cid;
            }
        }

        // Reassign.
        for &si in &small_indices {
            assignments[si] = best_target;
        }
    }

    // Re-compact IDs.
    let mut id_map = std::collections::HashMap::new();
    let mut next_id = 0usize;
    for a in assignments.iter_mut() {
        let mapped = *id_map.entry(*a).or_insert_with(|| {
            let id = next_id;
            next_id += 1;
            id
        });
        *a = mapped;
    }
}

/// Cosine similarity between two f32 vectors.
///
/// Used by HAC clustering and novelty gating
/// (`select_novel_statements`).
pub(crate) fn cosine_similarity_f32(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let xf = *x as f64;
        let yf = *y as f64;
        dot += xf * yf;
        norm_a += xf * xf;
        norm_b += yf * yf;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < 1e-12 {
        return 0.0;
    }
    dot / denom
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_embedding(vals: &[f32]) -> Vec<f32> {
        vals.to_vec()
    }

    #[test]
    fn cluster_empty() {
        let result = cluster_statements(&[], &ClusterConfig::default());
        assert!(result.is_empty());
    }

    #[test]
    fn cluster_single() {
        let emb = make_embedding(&[1.0, 0.0, 0.0]);
        let embeddings: Vec<Option<&[f32]>> = vec![Some(&emb)];
        let result = cluster_statements(&embeddings, &ClusterConfig::default());
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn cluster_identical_vectors() {
        let emb = make_embedding(&[1.0, 0.0, 0.0]);
        let embeddings: Vec<Option<&[f32]>> = vec![Some(&emb), Some(&emb), Some(&emb)];
        let result = cluster_statements(&embeddings, &ClusterConfig::default());
        // All identical → same cluster.
        assert_eq!(result[0], result[1]);
        assert_eq!(result[1], result[2]);
    }

    #[test]
    fn cluster_orthogonal_vectors() {
        let a = make_embedding(&[1.0, 0.0, 0.0]);
        let b = make_embedding(&[0.0, 1.0, 0.0]);
        let c = make_embedding(&[0.0, 0.0, 1.0]);
        let embeddings: Vec<Option<&[f32]>> = vec![Some(&a), Some(&b), Some(&c)];
        let config = ClusterConfig {
            similarity_threshold: 0.75,
            min_cluster_size: 1, // Don't merge singletons for this test.
        };
        let result = cluster_statements(&embeddings, &config);
        // All orthogonal (cosine = 0) → separate clusters.
        assert_ne!(result[0], result[1]);
        assert_ne!(result[1], result[2]);
        assert_ne!(result[0], result[2]);
    }

    #[test]
    fn cluster_two_groups() {
        // Group 1: similar vectors.
        let a1 = make_embedding(&[1.0, 0.1, 0.0]);
        let a2 = make_embedding(&[1.0, 0.2, 0.0]);
        // Group 2: similar but distant from group 1.
        let b1 = make_embedding(&[0.0, 0.1, 1.0]);
        let b2 = make_embedding(&[0.0, 0.2, 1.0]);

        let embeddings: Vec<Option<&[f32]>> = vec![Some(&a1), Some(&a2), Some(&b1), Some(&b2)];
        let config = ClusterConfig {
            similarity_threshold: 0.9,
            min_cluster_size: 1,
        };
        let result = cluster_statements(&embeddings, &config);
        assert_eq!(result[0], result[1]); // Group 1 together.
        assert_eq!(result[2], result[3]); // Group 2 together.
        assert_ne!(result[0], result[2]); // Groups separate.
    }

    #[test]
    fn cluster_with_missing_embeddings() {
        let a = make_embedding(&[1.0, 0.0, 0.0]);
        let embeddings: Vec<Option<&[f32]>> = vec![Some(&a), None, Some(&a)];
        let config = ClusterConfig {
            similarity_threshold: 0.75,
            min_cluster_size: 1,
        };
        let result = cluster_statements(&embeddings, &config);
        // a[0] and a[2] should cluster together.
        assert_eq!(result[0], result[2]);
        // None embedding is a separate cluster.
        assert_ne!(result[0], result[1]);
    }

    #[test]
    fn cosine_similarity_identical() {
        let v = [1.0f32, 2.0, 3.0];
        let sim = cosine_similarity_f32(&v, &v);
        assert!((sim - 1.0).abs() < 1e-10);
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let a = [1.0f32, 0.0, 0.0];
        let b = [0.0f32, 1.0, 0.0];
        let sim = cosine_similarity_f32(&a, &b);
        assert!(sim.abs() < 1e-10);
    }

    #[test]
    fn min_cluster_size_merges_singletons() {
        let a = make_embedding(&[1.0, 0.0, 0.0]);
        let b = make_embedding(&[0.9, 0.1, 0.0]); // Close to a.
        let c = make_embedding(&[0.0, 0.0, 1.0]); // Distant singleton.
        let embeddings: Vec<Option<&[f32]>> = vec![Some(&a), Some(&b), Some(&c)];
        let config = ClusterConfig {
            similarity_threshold: 0.8,
            min_cluster_size: 2,
        };
        let result = cluster_statements(&embeddings, &config);
        // a and b cluster together. c is a singleton → merged into
        // the nearest large cluster.
        assert_eq!(result[0], result[1]);
        // c should be merged into the a/b cluster (the only large
        // cluster).
        assert_eq!(result[0], result[2]);
    }
}

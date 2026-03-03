//! Spectral embeddings from the graph Laplacian.
//!
//! Algorithm:
//! 1. Build the adjacency matrix A from the graph (undirected, weight = 1)
//! 2. Compute degree matrix D (diagonal of node degrees)
//! 3. Compute Laplacian L = D − A  (or normalized variant)
//! 4. Find the k smallest eigenvectors of L
//! 5. Each node's embedding = its row in the eigenvector matrix
//!
//! Ported from valence-v2 with `GraphView`/`TripleStore`/`NodeId` replaced
//! by `&CovalenceGraph`/`Uuid`.

use std::collections::HashMap;

use anyhow::{Context, Result};
use faer::prelude::*;
use petgraph::visit::EdgeRef;
use uuid::Uuid;

use crate::graph::CovalenceGraph;

/// Configuration for spectral embedding computation.
#[derive(Debug, Clone)]
pub struct SpectralConfig {
    /// Number of embedding dimensions (default: 64)
    pub dimensions: usize,
    /// Whether to use the normalized Laplacian (default: true)
    pub normalize: bool,
}

impl Default for SpectralConfig {
    fn default() -> Self {
        Self {
            dimensions: 64,
            normalize: true,
        }
    }
}

impl SpectralConfig {
    #[allow(dead_code)]
    pub fn new(dimensions: usize) -> Self {
        Self {
            dimensions,
            normalize: true,
        }
    }
}

/// Compute spectral embeddings from a `CovalenceGraph`.
///
/// Returns a map of `Uuid → embedding vector`. Returns an empty map for
/// graphs with 0 or 1 nodes (insufficient for eigenvector decomposition).
pub fn compute_spectral(
    graph: &CovalenceGraph,
    config: SpectralConfig,
) -> Result<HashMap<Uuid, Vec<f32>>> {
    let node_count = graph.node_count();

    if node_count == 0 {
        return Ok(HashMap::new());
    }

    // Maximum useful dimensions is node_count − 1 (trivial eigenvalue is discarded)
    let dimensions = config.dimensions.min(node_count.saturating_sub(1));
    if dimensions == 0 {
        return Ok(HashMap::new());
    }

    let (adjacency, degree, node_order) = build_adjacency_matrix(graph);

    let laplacian = if config.normalize {
        compute_normalized_laplacian(&adjacency, &degree)
    } else {
        compute_laplacian(&adjacency, &degree)
    };

    let embeddings_matrix =
        compute_eigenvectors(&laplacian, dimensions).context("failed to compute eigenvectors")?;

    let mut embeddings = HashMap::new();
    for (i, &node_id) in node_order.iter().enumerate() {
        let embedding: Vec<f32> = (0..dimensions)
            .map(|j| embeddings_matrix[(i, j)] as f32)
            .collect();
        embeddings.insert(node_id, embedding);
    }

    Ok(embeddings)
}

// ─── Matrix construction ──────────────────────────────────────────────────────

/// Build the adjacency matrix and degree vector from a `CovalenceGraph`.
///
/// All edges are treated as **undirected** and have weight 1.0 (CovalenceGraph
/// stores edge types as strings, not numeric weights).
///
/// Returns `(adjacency, degree_vector, ordered_node_ids)`.
fn build_adjacency_matrix(graph: &CovalenceGraph) -> (Mat<f64>, Vec<f64>, Vec<Uuid>) {
    let n = graph.node_count();

    let mut node_order: Vec<Uuid> = graph.index.keys().cloned().collect();
    node_order.sort();

    let node_to_idx: HashMap<Uuid, usize> = node_order
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, i))
        .collect();

    let mut adjacency = Mat::zeros(n, n);
    let mut degree = vec![0.0f64; n];

    for edge_ref in graph.graph.edge_references() {
        // node_weight returns &Uuid (the node payload stored in DiGraph<Uuid, String>)
        let src_uuid = graph.graph.node_weight(edge_ref.source()).copied();
        let tgt_uuid = graph.graph.node_weight(edge_ref.target()).copied();

        if let (Some(src_id), Some(tgt_id)) = (src_uuid, tgt_uuid)
            && let (Some(&i), Some(&j)) = (node_to_idx.get(&src_id), node_to_idx.get(&tgt_id))
        {
            let weight = 1.0f64; // uniform weights (edge_type is a String label)

            // Treat as undirected
            adjacency[(i, j)] = weight;
            adjacency[(j, i)] = weight;

            degree[i] += weight;
            degree[j] += weight;
        }
    }

    (adjacency, degree, node_order)
}

/// Unnormalized Laplacian: L = D − A
fn compute_laplacian(adjacency: &Mat<f64>, degree: &[f64]) -> Mat<f64> {
    let n = adjacency.nrows();
    let mut laplacian = adjacency.clone();

    // Negate adjacency
    for i in 0..n {
        for j in 0..n {
            laplacian[(i, j)] = -laplacian[(i, j)];
        }
    }

    // Add degree diagonal
    for i in 0..n {
        laplacian[(i, i)] += degree[i];
    }

    laplacian
}

/// Normalized Laplacian: L_norm = D^(-½) · L · D^(-½)
fn compute_normalized_laplacian(adjacency: &Mat<f64>, degree: &[f64]) -> Mat<f64> {
    let n = adjacency.nrows();

    let d_inv_sqrt: Vec<f64> = degree
        .iter()
        .map(|&d| if d > 0.0 { 1.0 / d.sqrt() } else { 0.0 })
        .collect();

    let mut laplacian = Mat::zeros(n, n);

    for i in 0..n {
        for j in 0..n {
            if i == j {
                laplacian[(i, i)] = if degree[i] > 0.0 { 1.0 } else { 0.0 };
            } else {
                laplacian[(i, j)] = -adjacency[(i, j)] * d_inv_sqrt[i] * d_inv_sqrt[j];
            }
        }
    }

    laplacian
}

// ─── Eigendecomposition ───────────────────────────────────────────────────────

/// Compute the k smallest (non-trivial) eigenvectors of the Laplacian.
fn compute_eigenvectors(laplacian: &Mat<f64>, k: usize) -> Result<Mat<f64>> {
    let n = laplacian.nrows();

    // Real symmetric eigendecomposition (Laplacian is PSD)
    let eigendecomp = laplacian.selfadjoint_eigendecomposition(faer::Side::Lower);

    let eigenvalues = eigendecomp.s().column_vector();
    let eigenvectors = eigendecomp.u();

    // Sort indices by eigenvalue ascending
    let mut eigen_pairs: Vec<(usize, f64)> = eigenvalues
        .iter()
        .enumerate()
        .map(|(i, &val)| (i, val))
        .collect();
    eigen_pairs.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    // Skip the trivial constant eigenvector (eigenvalue ≈ 0)
    let start_idx = if !eigen_pairs.is_empty() && eigen_pairs[0].1.abs() < 1e-10 {
        1
    } else {
        0
    };

    let selected: Vec<usize> = eigen_pairs
        .iter()
        .skip(start_idx)
        .take(k)
        .map(|(i, _)| *i)
        .collect();

    let actual_k = selected.len();
    let mut result = Mat::zeros(n, actual_k);
    for (col, &eigen_idx) in selected.iter().enumerate() {
        for row in 0..n {
            result[(row, col)] = eigenvectors[(row, eigen_idx)];
        }
    }

    Ok(result)
}

//! Node2Vec: Random walk-based graph embeddings.
//!
//! Approach:
//! 1. Generate biased random walks from each node
//! 2. Treat walks as "sentences", nodes as "words"
//! 3. Train skip-gram model to predict context nodes
//! 4. Node embeddings = skip-gram input vectors
//!
//! Ported from valence-v2 with `GraphView`/`TripleStore`/`NodeId` replaced
//! by `&CovalenceGraph`/`Uuid`.

use std::collections::HashMap;

use anyhow::Result;
use rand::Rng;
use rand::seq::SliceRandom;
use uuid::Uuid;

use crate::graph::CovalenceGraph;

/// Configuration for Node2Vec embeddings.
#[derive(Debug, Clone)]
pub struct Node2VecConfig {
    /// Number of dimensions for the embedding (default: 64)
    pub dimensions: usize,
    /// Length of each random walk (default: 80)
    pub walk_length: usize,
    /// Number of walks to start from each node (default: 10)
    pub walks_per_node: usize,
    /// Return parameter p: controls likelihood of returning to the previous node.
    /// Higher p → less likely to return (more exploration). Default: 1.0
    pub p: f64,
    /// In-out parameter q: controls breadth vs depth of search.
    /// q > 1: BFS-like (local neighborhood); q < 1: DFS-like. Default: 1.0
    pub q: f64,
    /// Context window size for skip-gram training (default: 5)
    pub window: usize,
    /// Number of training epochs (default: 5)
    pub epochs: usize,
    /// Learning rate for gradient descent (default: 0.025)
    pub learning_rate: f64,
}

impl Default for Node2VecConfig {
    fn default() -> Self {
        Self {
            dimensions: 64,
            walk_length: 80,
            walks_per_node: 10,
            p: 1.0,
            q: 1.0,
            window: 5,
            epochs: 5,
            learning_rate: 0.025,
        }
    }
}

impl Node2VecConfig {
    #[allow(dead_code)]
    pub fn new(dimensions: usize) -> Self {
        Self {
            dimensions,
            ..Default::default()
        }
    }
}

/// Compute Node2Vec embeddings from a `CovalenceGraph`.
///
/// Returns a map of `Uuid → embedding vector`. Isolated nodes (no outgoing
/// edges) produce no walks and are excluded from the result.
pub fn compute_node2vec(
    graph: &CovalenceGraph,
    config: Node2VecConfig,
) -> Result<HashMap<Uuid, Vec<f32>>> {
    let node_count = graph.node_count();

    if node_count == 0 {
        return Ok(HashMap::new());
    }

    let walks = generate_walks(graph, &config)?;

    if walks.is_empty() {
        return Ok(HashMap::new());
    }

    train_skipgram(&walks, graph, &config)
}

// ─── Walk generation ──────────────────────────────────────────────────────────

fn generate_walks(graph: &CovalenceGraph, config: &Node2VecConfig) -> Result<Vec<Vec<Uuid>>> {
    let mut walks = Vec::new();
    let mut rng = rand::thread_rng();

    // Sorted for determinism in tests
    let mut nodes: Vec<Uuid> = graph.index.keys().cloned().collect();
    nodes.sort();

    for &start_node in &nodes {
        for _ in 0..config.walks_per_node {
            let walk = generate_walk(graph, start_node, config, &mut rng)?;
            if walk.len() > 1 {
                walks.push(walk);
            }
        }
    }

    Ok(walks)
}

fn generate_walk<R: Rng>(
    graph: &CovalenceGraph,
    start_node: Uuid,
    config: &Node2VecConfig,
    rng: &mut R,
) -> Result<Vec<Uuid>> {
    let mut walk = vec![start_node];

    for _ in 1..config.walk_length {
        let current = *walk.last().unwrap();
        let neighbors = graph.neighbors(&current);

        if neighbors.is_empty() {
            break;
        }

        let next = if walk.len() == 1 {
            // First step: uniform random selection
            *neighbors.choose(rng).unwrap()
        } else {
            let prev = walk[walk.len() - 2];
            select_next_node(&neighbors, prev, config, rng)
        };

        walk.push(next);
    }

    Ok(walk)
}

fn select_next_node<R: Rng>(
    neighbors: &[Uuid],
    prev_node: Uuid,
    config: &Node2VecConfig,
    rng: &mut R,
) -> Uuid {
    let probabilities: Vec<f64> = neighbors
        .iter()
        .map(|&neighbor| {
            if neighbor == prev_node {
                1.0 / config.p // return to previous
            } else {
                1.0 / config.q // explore further
            }
        })
        .collect();

    let sum: f64 = probabilities.iter().sum();
    if sum == 0.0 {
        return *neighbors.choose(rng).unwrap();
    }

    let threshold: f64 = rng.r#gen();
    let mut cumulative = 0.0;
    for (i, prob) in probabilities.iter().enumerate() {
        cumulative += prob / sum;
        if threshold <= cumulative {
            return neighbors[i];
        }
    }

    *neighbors.last().unwrap()
}

// ─── Skip-gram training ───────────────────────────────────────────────────────

fn train_skipgram(
    walks: &[Vec<Uuid>],
    graph: &CovalenceGraph,
    config: &Node2VecConfig,
) -> Result<HashMap<Uuid, Vec<f32>>> {
    // Build vocabulary from all nodes in graph (sorted for determinism)
    let mut vocab: Vec<Uuid> = graph.index.keys().cloned().collect();
    vocab.sort();
    let vocab_size = vocab.len();

    let node_to_idx: HashMap<Uuid, usize> =
        vocab.iter().enumerate().map(|(i, &id)| (id, i)).collect();

    let mut embeddings = initialize_embeddings(vocab_size, config.dimensions);
    let mut context_embeddings = initialize_embeddings(vocab_size, config.dimensions);

    for epoch in 0..config.epochs {
        let mut epoch_loss = 0.0f64;
        let mut sample_count = 0usize;

        for walk in walks {
            for (i, &center_node) in walk.iter().enumerate() {
                let center_idx = node_to_idx[&center_node];

                let window_start = i.saturating_sub(config.window);
                let window_end = (i + config.window + 1).min(walk.len());

                for (j, &context_node) in
                    walk.iter().enumerate().take(window_end).skip(window_start)
                {
                    if i == j {
                        continue;
                    }
                    let context_idx = node_to_idx[&context_node];

                    // Positive sample
                    epoch_loss += train_pair(
                        &mut embeddings,
                        &mut context_embeddings,
                        center_idx,
                        context_idx,
                        true,
                        config.learning_rate,
                    );
                    sample_count += 1;

                    // Negative sampling (5 negatives per positive)
                    for _ in 0..5 {
                        let neg_idx = rand::random::<usize>() % vocab_size;
                        if neg_idx != context_idx {
                            epoch_loss += train_pair(
                                &mut embeddings,
                                &mut context_embeddings,
                                center_idx,
                                neg_idx,
                                false,
                                config.learning_rate,
                            );
                            sample_count += 1;
                        }
                    }
                }
            }
        }

        let avg_loss = if sample_count > 0 {
            epoch_loss / sample_count as f64
        } else {
            0.0
        };

        if epoch % 2 == 0 {
            tracing::debug!(
                epoch = epoch + 1,
                total = config.epochs,
                avg_loss,
                "node2vec training epoch"
            );
        }
    }

    // Collect only nodes that appeared in at least one walk
    let nodes_in_walks: std::collections::HashSet<Uuid> = walks.iter().flatten().copied().collect();

    let mut result = HashMap::new();
    for (node, &idx) in &node_to_idx {
        if nodes_in_walks.contains(node) {
            let embedding = embeddings[idx].iter().map(|&v| v as f32).collect();
            result.insert(*node, embedding);
        }
    }

    Ok(result)
}

fn initialize_embeddings(vocab_size: usize, dimensions: usize) -> Vec<Vec<f64>> {
    let mut rng = rand::thread_rng();
    (0..vocab_size)
        .map(|_| (0..dimensions).map(|_| rng.r#gen::<f64>() - 0.5).collect())
        .collect()
}

fn train_pair(
    embeddings: &mut [Vec<f64>],
    context_embeddings: &mut [Vec<f64>],
    center_idx: usize,
    context_idx: usize,
    is_positive: bool,
    learning_rate: f64,
) -> f64 {
    let dimensions = embeddings[0].len();

    let mut dot = 0.0f64;
    for d in 0..dimensions {
        dot += embeddings[center_idx][d] * context_embeddings[context_idx][d];
    }

    let sigmoid = 1.0 / (1.0 + (-dot).exp());
    let label = if is_positive { 1.0 } else { 0.0 };
    let error = label - sigmoid;

    for d in 0..dimensions {
        let gradient = error * learning_rate;
        let center_val = embeddings[center_idx][d];
        let context_val = context_embeddings[context_idx][d];
        embeddings[center_idx][d] += gradient * context_val;
        context_embeddings[context_idx][d] += gradient * center_val;
    }

    let loss = if is_positive {
        -(sigmoid.ln())
    } else {
        -((1.0 - sigmoid).ln())
    };

    if loss.is_finite() { loss } else { 0.0 }
}

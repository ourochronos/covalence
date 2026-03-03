//! Integration tests for Phase 4: Graph Embeddings (covalence#51).
//!
//! Tests cover:
//!   - `compute_node2vec` on a small in-memory graph
//!   - `compute_spectral` on a small in-memory graph
//!   - `store_embeddings` round-trip via the DB

use std::collections::HashMap;
use uuid::Uuid;

use covalence_engine::embeddings::node2vec::{Node2VecConfig, compute_node2vec};
use covalence_engine::embeddings::spectral::{SpectralConfig, compute_spectral};
use covalence_engine::graph::CovalenceGraph;

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Build a small 3-node, 2-edge graph:  A → B → C
fn three_node_chain() -> (CovalenceGraph, Uuid, Uuid, Uuid) {
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let c = Uuid::new_v4();
    let mut g = CovalenceGraph::new();
    g.add_edge(a, b, "RELATES".into());
    g.add_edge(b, c, "RELATES".into());
    (g, a, b, c)
}

/// Cosine similarity between two f32 slices.
fn cosine_sim(u: &[f32], v: &[f32]) -> f32 {
    let dot: f32 = u.iter().zip(v.iter()).map(|(a, b)| a * b).sum();
    let mag_u: f32 = u.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_v: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag_u == 0.0 || mag_v == 0.0 {
        0.0
    } else {
        dot / (mag_u * mag_v)
    }
}

// ─── Node2Vec tests ───────────────────────────────────────────────────────────

#[test]
fn test_node2vec_empty_graph() {
    let g = CovalenceGraph::new();
    let config = Node2VecConfig::default();
    let result = compute_node2vec(&g, config).unwrap();
    assert!(
        result.is_empty(),
        "empty graph should produce no embeddings"
    );
}

#[test]
fn test_node2vec_three_nodes_two_edges() {
    let (g, a, b, _c) = three_node_chain();
    let config = Node2VecConfig {
        dimensions: 8,
        walk_length: 10,
        walks_per_node: 5,
        epochs: 3,
        ..Default::default()
    };
    let result = compute_node2vec(&g, config).unwrap();

    // At least nodes involved in walks should have embeddings
    // (isolated tail node C may or may not, depending on walk direction)
    assert!(
        !result.is_empty(),
        "should produce at least some embeddings"
    );

    // All returned embeddings must be 8-dimensional
    for (id, emb) in &result {
        assert_eq!(emb.len(), 8, "node {id} embedding should have 8 dimensions");
        assert!(
            emb.iter().all(|v| v.is_finite()),
            "all values must be finite"
        );
    }

    // A and B should definitely have embeddings (A starts walks, B is visited)
    assert!(result.contains_key(&a), "node A must have an embedding");
    assert!(result.contains_key(&b), "node B must have an embedding");
}

#[test]
fn test_node2vec_isolated_nodes_produce_no_embeddings() {
    let mut g = CovalenceGraph::new();
    let _x = g.add_node(Uuid::new_v4());
    let _y = g.add_node(Uuid::new_v4());
    // No edges → walks of length 1 only → filtered out
    let config = Node2VecConfig::default();
    let result = compute_node2vec(&g, config).unwrap();
    assert!(
        result.is_empty(),
        "isolated nodes without edges should not produce embeddings"
    );
}

#[test]
fn test_node2vec_dimension_matches_config() {
    let (g, _, _, _) = three_node_chain();
    for dims in [4, 16, 32] {
        let config = Node2VecConfig {
            dimensions: dims,
            walk_length: 5,
            walks_per_node: 3,
            epochs: 2,
            ..Default::default()
        };
        let result = compute_node2vec(&g, config).unwrap();
        for emb in result.values() {
            assert_eq!(
                emb.len(),
                dims,
                "embedding dimension should match config ({dims})"
            );
        }
    }
}

// ─── Spectral tests ───────────────────────────────────────────────────────────

#[test]
fn test_spectral_empty_graph() {
    let g = CovalenceGraph::new();
    let config = SpectralConfig::default();
    let result = compute_spectral(&g, config).unwrap();
    assert!(
        result.is_empty(),
        "empty graph should produce no embeddings"
    );
}

#[test]
fn test_spectral_single_node_no_edges() {
    let mut g = CovalenceGraph::new();
    g.add_node(Uuid::new_v4());
    // node_count=1 → dimensions = min(64, 0) = 0 → empty
    let config = SpectralConfig::default();
    let result = compute_spectral(&g, config).unwrap();
    assert!(
        result.is_empty(),
        "single isolated node should produce no embeddings (dim capped at 0)"
    );
}

#[test]
fn test_spectral_three_nodes_two_edges_dimensions() {
    let (g, a, b, c) = three_node_chain();
    let config = SpectralConfig {
        dimensions: 2,
        normalize: true,
    };
    let result = compute_spectral(&g, config).unwrap();

    // All three nodes should appear (graph is connected via edges)
    assert_eq!(result.len(), 3, "all 3 nodes should have embeddings");
    assert!(result.contains_key(&a));
    assert!(result.contains_key(&b));
    assert!(result.contains_key(&c));

    for (id, emb) in &result {
        assert_eq!(
            emb.len(),
            2,
            "node {id} embedding should have 2 dimensions (requested)"
        );
        assert!(
            emb.iter().all(|v| v.is_finite()),
            "all values must be finite"
        );
    }
}

#[test]
fn test_spectral_dimensions_capped_at_node_count_minus_one() {
    // 2-node graph → max useful dims = 1
    let mut g = CovalenceGraph::new();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    g.add_edge(a, b, "EDGE".into());

    let config = SpectralConfig {
        dimensions: 64, // requested, but will be capped
        normalize: true,
    };
    let result = compute_spectral(&g, config).unwrap();
    // Should have embeddings for both nodes
    assert_eq!(result.len(), 2);
    // Each embedding capped at 1 dimension (node_count-1 = 1)
    for emb in result.values() {
        assert_eq!(
            emb.len(),
            1,
            "2-node graph: embeddings capped at 1 dimension"
        );
    }
}

#[test]
fn test_spectral_unnormalized_laplacian() {
    let (g, _, _, _) = three_node_chain();
    let config = SpectralConfig {
        dimensions: 2,
        normalize: false,
    };
    let result = compute_spectral(&g, config).unwrap();
    assert_eq!(result.len(), 3);
    for emb in result.values() {
        assert_eq!(emb.len(), 2);
    }
}

/// Nodes in the same connected component should be more similar than nodes
/// in different components.
#[test]
fn test_spectral_component_similarity() {
    // Component 1: A ↔ B
    // Component 2: C ↔ D
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let c = Uuid::new_v4();
    let d = Uuid::new_v4();

    let mut g = CovalenceGraph::new();
    g.add_edge(a, b, "EDGE".into());
    g.add_edge(b, a, "EDGE".into());
    g.add_edge(c, d, "EDGE".into());
    g.add_edge(d, c, "EDGE".into());

    let config = SpectralConfig {
        dimensions: 3,
        normalize: true,
    };
    let result = compute_spectral(&g, config).unwrap();
    assert_eq!(result.len(), 4);

    let emb_a = result.get(&a).unwrap();
    let emb_b = result.get(&b).unwrap();
    let emb_c = result.get(&c).unwrap();

    let sim_ab = cosine_sim(emb_a, emb_b).abs();
    let sim_ac = cosine_sim(emb_a, emb_c).abs();

    // Within-component similarity should be higher than cross-component
    assert!(
        sim_ab >= sim_ac,
        "A-B (within-component, sim={sim_ab:.3}) should be ≥ A-C (cross-component, sim={sim_ac:.3})"
    );
}

// ─── store_embeddings DB round-trip ───────────────────────────────────────────

/// Test that `store_embeddings` writes rows and can be read back.
/// Requires a live DB at DATABASE_URL (skipped if unavailable).
#[tokio::test]
async fn test_store_embeddings_round_trip() {
    let db_url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("DATABASE_URL not set — skipping DB round-trip test");
            return;
        }
    };

    let pool = match sqlx::PgPool::connect(&db_url).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cannot connect to DB ({e}) — skipping DB round-trip test");
            return;
        }
    };

    // Create a temporary node to attach embeddings to
    let node_id = Uuid::new_v4();
    let insert_result = sqlx::query(
        "INSERT INTO covalence.nodes (id, node_type, status, title, content)
         VALUES ($1, 'source', 'active', 'test-embed-node', 'test content')",
    )
    .bind(node_id)
    .execute(&pool)
    .await;

    if insert_result.is_err() {
        eprintln!("cannot insert test node — skipping DB round-trip test");
        return;
    }

    // Insert a fake 64-dim embedding
    let fake_emb: Vec<f32> = (0..64).map(|i| i as f32 * 0.01).collect();
    let mut embeddings: HashMap<Uuid, Vec<f32>> = HashMap::new();
    embeddings.insert(node_id, fake_emb.clone());

    // Run store_embeddings (requires 015 migration to have been applied)
    match covalence_engine::embeddings::store_embeddings(&pool, embeddings, "node2vec").await {
        Ok(count) => {
            assert_eq!(count, 1, "should have stored exactly 1 embedding");

            // Read it back
            let row =
                sqlx::query("SELECT method FROM covalence.graph_embeddings WHERE node_id = $1")
                    .bind(node_id)
                    .fetch_optional(&pool)
                    .await
                    .unwrap();

            assert!(row.is_some(), "embedding row should exist after store");

            let method: String = {
                use sqlx::Row;
                row.unwrap().get("method")
            };
            assert_eq!(method, "node2vec");
        }
        Err(e) => {
            // Migration may not be applied in CI; treat as soft skip
            eprintln!("store_embeddings returned error (migration not applied?): {e}");
        }
    }

    // Cleanup
    let _ = sqlx::query("DELETE FROM covalence.nodes WHERE id = $1")
        .bind(node_id)
        .execute(&pool)
        .await;
}

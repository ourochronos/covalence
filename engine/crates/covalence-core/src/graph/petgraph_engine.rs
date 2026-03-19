//! `PetgraphEngine` — in-memory petgraph sidecar implementation of [`GraphEngine`].
//!
//! Wraps the existing [`SharedGraph`] (petgraph sidecar) and delegates each
//! trait method to the corresponding algorithm or sidecar operation. All
//! methods acquire the `RwLock` read guard and run synchronously — the graph
//! is CPU-bound and fast for typical knowledge-graph sizes.

use std::collections::HashMap;

use petgraph::visit::EdgeRef;
use uuid::Uuid;

use crate::consolidation::contention::detect_contentions;
use crate::error::Result;
use crate::graph::algorithms;
use crate::graph::community::{detect_communities_with_min_size, label_communities};
use crate::graph::engine::{
    BfsNode, BfsOptions, Contention, GapCandidate, GraphEngine, GraphStats, Neighbor, ReloadResult,
};
use crate::graph::sidecar::SharedGraph;
use crate::graph::sync::full_reload;
use crate::graph::topology::{TopologyMap, build_topology};
use crate::graph::traversal;

/// In-memory petgraph sidecar implementation of [`GraphEngine`].
///
/// Each method acquires the `RwLock` read guard synchronously. The petgraph
/// sidecar is fast enough for our graph size that `spawn_blocking` is
/// unnecessary.
pub struct PetgraphEngine {
    graph: SharedGraph,
}

impl PetgraphEngine {
    /// Create a new `PetgraphEngine` wrapping the given shared graph.
    pub fn new(graph: SharedGraph) -> Self {
        Self { graph }
    }
}

#[async_trait::async_trait]
impl GraphEngine for PetgraphEngine {
    // ----- Stats -----

    /// Graph summary statistics.
    async fn stats(&self) -> Result<GraphStats> {
        let g = self.graph.read().await;
        let n = g.node_count();
        let e = g.edge_count();
        let density = if n > 1 {
            e as f64 / (n as f64 * (n as f64 - 1.0))
        } else {
            0.0
        };

        // Count synthetic vs semantic edges
        let synthetic_edge_count = g
            .graph()
            .edge_weights()
            .filter(|ew| ew.is_synthetic)
            .count();
        let semantic_edge_count = e - synthetic_edge_count;

        // Count weakly connected components via BFS
        let component_count = count_weak_components(&g);

        Ok(GraphStats {
            node_count: n,
            edge_count: e,
            semantic_edge_count,
            synthetic_edge_count,
            density,
            component_count,
        })
    }

    /// Number of nodes.
    async fn node_count(&self) -> Result<usize> {
        let g = self.graph.read().await;
        Ok(g.node_count())
    }

    /// Number of active edges.
    async fn edge_count(&self) -> Result<usize> {
        let g = self.graph.read().await;
        Ok(g.edge_count())
    }

    // ----- Node access -----

    /// Get a node's metadata by UUID.
    async fn get_node(&self, id: Uuid) -> Result<Option<crate::graph::sidecar::NodeMeta>> {
        let g = self.graph.read().await;
        Ok(g.get_node(id).cloned())
    }

    /// Get outgoing neighbors of a node.
    async fn neighbors_out(&self, id: Uuid) -> Result<Vec<Neighbor>> {
        let g = self.graph.read().await;
        let Some(idx) = g.node_index(id) else {
            return Ok(Vec::new());
        };

        let neighbors = g
            .graph()
            .edges(idx)
            .map(|edge| {
                let edge_meta = &g.graph()[edge.id()];
                let target_meta = &g.graph()[edge.target()];
                Neighbor {
                    id: target_meta.id,
                    rel_type: edge_meta.rel_type.clone(),
                    is_synthetic: edge_meta.is_synthetic,
                    confidence: edge_meta.confidence,
                    weight: edge_meta.weight,
                    name: target_meta.canonical_name.clone(),
                    node_type: target_meta.node_type.clone(),
                }
            })
            .collect();

        Ok(neighbors)
    }

    /// Get incoming neighbors of a node.
    async fn neighbors_in(&self, id: Uuid) -> Result<Vec<Neighbor>> {
        let g = self.graph.read().await;
        let Some(idx) = g.node_index(id) else {
            return Ok(Vec::new());
        };

        let neighbors = g
            .graph()
            .edges_directed(idx, petgraph::Direction::Incoming)
            .map(|edge| {
                let edge_meta = &g.graph()[edge.id()];
                let source_meta = &g.graph()[edge.source()];
                Neighbor {
                    id: source_meta.id,
                    rel_type: edge_meta.rel_type.clone(),
                    is_synthetic: edge_meta.is_synthetic,
                    confidence: edge_meta.confidence,
                    weight: edge_meta.weight,
                    name: source_meta.canonical_name.clone(),
                    node_type: source_meta.node_type.clone(),
                }
            })
            .collect();

        Ok(neighbors)
    }

    /// In-degree of a node.
    async fn degree_in(&self, id: Uuid) -> Result<usize> {
        let g = self.graph.read().await;
        let Some(idx) = g.node_index(id) else {
            return Ok(0);
        };
        Ok(g.graph()
            .edges_directed(idx, petgraph::Direction::Incoming)
            .count())
    }

    /// Out-degree of a node.
    async fn degree_out(&self, id: Uuid) -> Result<usize> {
        let g = self.graph.read().await;
        let Some(idx) = g.node_index(id) else {
            return Ok(0);
        };
        Ok(g.graph().edges(idx).count())
    }

    // ----- Traversal -----

    /// BFS neighborhood discovery from a start node.
    async fn bfs_neighborhood(&self, start: Uuid, options: BfsOptions) -> Result<Vec<BfsNode>> {
        let g = self.graph.read().await;

        let deny_refs: Vec<&str> = options.deny_rel_types.iter().map(|s| s.as_str()).collect();
        let edge_deny = if deny_refs.is_empty() {
            None
        } else {
            Some(deny_refs.as_slice())
        };

        let raw = traversal::bfs_neighborhood_full(
            &g,
            start,
            options.max_hops,
            None, // no allow-list filter
            options.skip_synthetic,
            edge_deny,
        );

        let nodes = raw
            .into_iter()
            .map(|(node_id, hops)| {
                let (name, node_type) = g
                    .get_node(node_id)
                    .map(|m| (m.canonical_name.clone(), m.node_type.clone()))
                    .unwrap_or_default();
                BfsNode {
                    id: node_id,
                    hops,
                    name,
                    node_type,
                }
            })
            .collect();

        Ok(nodes)
    }

    /// Shortest path between two nodes. Returns the path as node UUIDs,
    /// or None if no path exists.
    async fn shortest_path(&self, from: Uuid, to: Uuid) -> Result<Option<Vec<Uuid>>> {
        let g = self.graph.read().await;
        Ok(traversal::shortest_path(&g, from, to))
    }

    // ----- Algorithms -----

    /// PageRank scores for all nodes.
    async fn pagerank(&self, damping: f64, iterations: usize) -> Result<HashMap<Uuid, f64>> {
        let g = self.graph.read().await;
        Ok(algorithms::pagerank(g.graph(), damping, iterations))
    }

    /// TrustRank: biased PageRank from trusted seed nodes.
    async fn trust_rank(
        &self,
        seeds: &[(Uuid, f64)],
        damping: f64,
        iterations: usize,
    ) -> Result<HashMap<Uuid, f64>> {
        let g = self.graph.read().await;
        Ok(algorithms::trust_rank(
            g.graph(),
            seeds,
            damping,
            iterations,
        ))
    }

    /// Structural importance (betweenness centrality approximation).
    async fn structural_importance(&self) -> Result<HashMap<Uuid, f64>> {
        let g = self.graph.read().await;
        Ok(algorithms::structural_importance(g.graph()))
    }

    /// Spreading activation from seed nodes with decay.
    ///
    /// Converts `max_hops` to a threshold via `decay^max_hops` so the
    /// underlying algorithm stops after energy has decayed past the
    /// equivalent hop distance.
    async fn spreading_activation(
        &self,
        seeds: &[(Uuid, f64)],
        decay: f64,
        max_hops: usize,
    ) -> Result<HashMap<Uuid, f64>> {
        let g = self.graph.read().await;
        // Convert max_hops to an energy threshold: after max_hops
        // multiplications by decay, the remaining energy is decay^max_hops.
        let threshold = decay.powi(max_hops as i32);
        Ok(algorithms::spreading_activation(
            g.graph(),
            seeds,
            decay,
            threshold,
        ))
    }

    /// Community detection (k-core based).
    async fn communities(
        &self,
        min_size: usize,
    ) -> Result<Vec<crate::graph::community::Community>> {
        let g = self.graph.read().await;
        let mut comms = detect_communities_with_min_size(g.graph(), min_size);
        label_communities(g.graph(), &mut comms);
        Ok(comms)
    }

    /// Build full topology map (communities + PageRank + bridges + landmarks).
    async fn topology(&self) -> Result<TopologyMap> {
        let g = self.graph.read().await;
        Ok(build_topology(g.graph()))
    }

    /// Detect contentious (contradictory) relationships.
    ///
    /// Maps the edge-centric `consolidation::contention::Contention` records
    /// into the trait's source-grouped `engine::Contention` format.
    async fn contentions(&self) -> Result<Vec<Contention>> {
        let g = self.graph.read().await;
        let raw = detect_contentions(g.graph());

        // Group contentions by (source_id, rel_type) so that multiple
        // contradictory targets from the same source appear together.
        let mut grouped: HashMap<(Uuid, String), (String, Vec<(Uuid, String)>)> = HashMap::new();
        for c in raw {
            let source_name = g
                .get_node(c.node_a)
                .map(|m| m.canonical_name.clone())
                .unwrap_or_default();
            let target_name = g
                .get_node(c.node_b)
                .map(|m| m.canonical_name.clone())
                .unwrap_or_default();

            let entry = grouped
                .entry((c.node_a, c.rel_type.clone()))
                .or_insert_with(|| (source_name, Vec::new()));
            entry.1.push((c.node_b, target_name));
        }

        let contentions = grouped
            .into_iter()
            .map(
                |((source_id, rel_type), (source_name, targets))| Contention {
                    source_id,
                    source_name,
                    rel_type,
                    targets,
                },
            )
            .collect();

        Ok(contentions)
    }

    /// Knowledge gap detection by degree imbalance.
    async fn knowledge_gaps(
        &self,
        min_in_degree: usize,
        min_label_length: usize,
        exclude_types: &[&str],
        limit: usize,
    ) -> Result<Vec<GapCandidate>> {
        let g = self.graph.read().await;
        let graph = g.graph();

        let mut candidates: Vec<GapCandidate> = Vec::new();

        for idx in graph.node_indices() {
            let meta = &graph[idx];

            if meta.canonical_name.len() < min_label_length {
                continue;
            }

            if exclude_types.iter().any(|&t| t == meta.node_type) {
                continue;
            }

            let in_degree = graph
                .edges_directed(idx, petgraph::Direction::Incoming)
                .count();
            let out_degree = graph.edges(idx).count();

            if in_degree >= min_in_degree && in_degree > out_degree {
                candidates.push(GapCandidate {
                    id: meta.id,
                    name: meta.canonical_name.clone(),
                    node_type: meta.node_type.clone(),
                    in_degree,
                    out_degree,
                });
            }
        }

        // Sort by gap score (in_degree - out_degree) descending
        candidates.sort_by(|a, b| {
            let score_a = a.in_degree as f64 - a.out_degree as f64;
            let score_b = b.in_degree as f64 - b.out_degree as f64;
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(limit);

        Ok(candidates)
    }

    // ----- Mutations -----

    /// Full reload from PostgreSQL. Replaces all in-memory state.
    async fn reload(&self, pool: &sqlx::PgPool) -> Result<ReloadResult> {
        full_reload(pool, self.graph.clone()).await?;
        let g = self.graph.read().await;
        Ok(ReloadResult {
            node_count: g.node_count(),
            edge_count: g.edge_count(),
        })
    }
}

/// Count weakly connected components via BFS.
///
/// Iterates all nodes, performing BFS through both outgoing and incoming
/// edges for undirected connectivity.
fn count_weak_components(sidecar: &crate::graph::sidecar::GraphSidecar) -> usize {
    use std::collections::HashSet;

    let graph = sidecar.graph();
    let mut visited: HashSet<petgraph::stable_graph::NodeIndex> =
        HashSet::with_capacity(graph.node_count());
    let mut components = 0usize;

    for start in graph.node_indices() {
        if !visited.insert(start) {
            continue;
        }
        components += 1;
        let mut stack = vec![start];
        while let Some(v) = stack.pop() {
            for edge in graph.edges(v) {
                if visited.insert(edge.target()) {
                    stack.push(edge.target());
                }
            }
            for edge in graph.edges_directed(v, petgraph::Direction::Incoming) {
                if visited.insert(edge.source()) {
                    stack.push(edge.source());
                }
            }
        }
    }

    components
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::RwLock;
    use uuid::Uuid;

    use crate::graph::engine::{BfsOptions, GraphEngine};
    use crate::graph::sidecar::{EdgeMeta, GraphSidecar, NodeMeta};

    use super::PetgraphEngine;

    /// Create a `PetgraphEngine` wrapping an empty graph.
    fn empty_engine() -> PetgraphEngine {
        let graph = Arc::new(RwLock::new(GraphSidecar::new()));
        PetgraphEngine::new(graph)
    }

    fn make_node(name: &str) -> NodeMeta {
        NodeMeta {
            id: Uuid::new_v4(),
            node_type: "entity".into(),
            entity_class: None,
            canonical_name: name.into(),
            clearance_level: 0,
        }
    }

    fn make_edge(rel: &str) -> EdgeMeta {
        EdgeMeta {
            id: Uuid::new_v4(),
            rel_type: rel.into(),
            weight: 1.0,
            confidence: 0.9,
            causal_level: None,
            clearance_level: 0,
            is_synthetic: false,
            has_valid_from: false,
        }
    }

    /// Build an engine with a small triangle graph: A <-> B <-> C <-> A.
    fn triangle_engine() -> (PetgraphEngine, Uuid, Uuid, Uuid) {
        let mut g = GraphSidecar::new();
        let a = make_node("Alpha");
        let b = make_node("Beta");
        let c = make_node("Gamma");
        let a_id = a.id;
        let b_id = b.id;
        let c_id = c.id;
        g.add_node(a).unwrap();
        g.add_node(b).unwrap();
        g.add_node(c).unwrap();
        g.add_edge(a_id, b_id, make_edge("related")).unwrap();
        g.add_edge(b_id, a_id, make_edge("related")).unwrap();
        g.add_edge(b_id, c_id, make_edge("causes")).unwrap();
        g.add_edge(c_id, b_id, make_edge("causes")).unwrap();
        g.add_edge(a_id, c_id, make_edge("related")).unwrap();
        g.add_edge(c_id, a_id, make_edge("related")).unwrap();

        let graph = Arc::new(RwLock::new(g));
        (PetgraphEngine::new(graph), a_id, b_id, c_id)
    }

    #[tokio::test]
    async fn empty_graph_stats() {
        let engine = empty_engine();
        let stats = engine.stats().await.unwrap();
        assert_eq!(stats.node_count, 0);
        assert_eq!(stats.edge_count, 0);
        assert_eq!(stats.semantic_edge_count, 0);
        assert_eq!(stats.synthetic_edge_count, 0);
        assert!((stats.density - 0.0).abs() < f64::EPSILON);
        assert_eq!(stats.component_count, 0);
    }

    #[tokio::test]
    async fn empty_graph_node_edge_count() {
        let engine = empty_engine();
        assert_eq!(engine.node_count().await.unwrap(), 0);
        assert_eq!(engine.edge_count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn triangle_stats() {
        let (engine, _, _, _) = triangle_engine();
        let stats = engine.stats().await.unwrap();
        assert_eq!(stats.node_count, 3);
        assert_eq!(stats.edge_count, 6);
        assert_eq!(stats.semantic_edge_count, 6);
        assert_eq!(stats.synthetic_edge_count, 0);
        assert!(stats.density > 0.0);
        assert_eq!(stats.component_count, 1);
    }

    #[tokio::test]
    async fn get_node_found() {
        let (engine, a_id, _, _) = triangle_engine();
        let node = engine.get_node(a_id).await.unwrap();
        assert!(node.is_some());
        assert_eq!(node.unwrap().canonical_name, "Alpha");
    }

    #[tokio::test]
    async fn get_node_not_found() {
        let engine = empty_engine();
        let node = engine.get_node(Uuid::new_v4()).await.unwrap();
        assert!(node.is_none());
    }

    #[tokio::test]
    async fn neighbors_out_returns_targets() {
        let (engine, a_id, b_id, c_id) = triangle_engine();
        let out = engine.neighbors_out(a_id).await.unwrap();
        assert_eq!(out.len(), 2);
        let ids: Vec<Uuid> = out.iter().map(|n| n.id).collect();
        assert!(ids.contains(&b_id));
        assert!(ids.contains(&c_id));
    }

    #[tokio::test]
    async fn neighbors_in_returns_sources() {
        let (engine, a_id, b_id, c_id) = triangle_engine();
        let inc = engine.neighbors_in(a_id).await.unwrap();
        assert_eq!(inc.len(), 2);
        let ids: Vec<Uuid> = inc.iter().map(|n| n.id).collect();
        assert!(ids.contains(&b_id));
        assert!(ids.contains(&c_id));
    }

    #[tokio::test]
    async fn degree_in_out() {
        let (engine, a_id, _, _) = triangle_engine();
        assert_eq!(engine.degree_in(a_id).await.unwrap(), 2);
        assert_eq!(engine.degree_out(a_id).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn degree_unknown_node() {
        let engine = empty_engine();
        assert_eq!(engine.degree_in(Uuid::new_v4()).await.unwrap(), 0);
        assert_eq!(engine.degree_out(Uuid::new_v4()).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn bfs_neighborhood_basic() {
        let (engine, a_id, _, _) = triangle_engine();
        let opts = BfsOptions {
            max_hops: 1,
            skip_synthetic: false,
            deny_rel_types: Vec::new(),
        };
        let nodes = engine.bfs_neighborhood(a_id, opts).await.unwrap();
        // 1-hop from A should find B and C
        assert_eq!(nodes.len(), 2);
        assert!(nodes.iter().all(|n| n.hops == 1));
    }

    #[tokio::test]
    async fn shortest_path_exists() {
        let (engine, a_id, _, c_id) = triangle_engine();
        let path = engine.shortest_path(a_id, c_id).await.unwrap();
        assert!(path.is_some());
        let path = path.unwrap();
        assert_eq!(path[0], a_id);
        assert_eq!(*path.last().unwrap(), c_id);
    }

    #[tokio::test]
    async fn shortest_path_disconnected() {
        let mut g = GraphSidecar::new();
        let a = make_node("X");
        let b = make_node("Y");
        let a_id = a.id;
        let b_id = b.id;
        g.add_node(a).unwrap();
        g.add_node(b).unwrap();
        let engine = PetgraphEngine::new(Arc::new(RwLock::new(g)));
        let path = engine.shortest_path(a_id, b_id).await.unwrap();
        assert!(path.is_none());
    }

    #[tokio::test]
    async fn pagerank_triangle() {
        let (engine, a_id, b_id, c_id) = triangle_engine();
        let scores = engine.pagerank(0.85, 50).await.unwrap();
        assert!(scores.contains_key(&a_id));
        assert!(scores.contains_key(&b_id));
        assert!(scores.contains_key(&c_id));
        // Symmetric graph: all scores should be roughly equal
        let sa = scores[&a_id];
        let sb = scores[&b_id];
        let sc = scores[&c_id];
        assert!((sa - sb).abs() < 0.01);
        assert!((sb - sc).abs() < 0.01);
    }

    #[tokio::test]
    async fn trust_rank_basic() {
        let (engine, a_id, b_id, c_id) = triangle_engine();
        let scores = engine.trust_rank(&[(a_id, 1.0)], 0.85, 50).await.unwrap();
        assert!(scores.contains_key(&a_id));
        assert!(scores.contains_key(&b_id));
        assert!(scores.contains_key(&c_id));
    }

    #[tokio::test]
    async fn structural_importance_nonempty() {
        let (engine, a_id, _, _) = triangle_engine();
        let scores = engine.structural_importance().await.unwrap();
        assert!(scores.contains_key(&a_id));
    }

    #[tokio::test]
    async fn spreading_activation_basic() {
        let (engine, a_id, _, _) = triangle_engine();
        let scores = engine
            .spreading_activation(&[(a_id, 1.0)], 0.7, 2)
            .await
            .unwrap();
        // Seed node and its neighbors should be activated
        assert!(!scores.is_empty());
    }

    #[tokio::test]
    async fn communities_triangle() {
        let (engine, _, _, _) = triangle_engine();
        let comms = engine.communities(2).await.unwrap();
        // All 3 nodes form one community
        assert!(!comms.is_empty());
        let total: usize = comms.iter().map(|c| c.node_ids.len()).sum();
        assert_eq!(total, 3);
    }

    #[tokio::test]
    async fn topology_nonempty() {
        let (engine, _, _, _) = triangle_engine();
        let topo = engine.topology().await.unwrap();
        assert_eq!(topo.total_nodes, 3);
        assert_eq!(topo.total_edges, 6);
        assert!(!topo.domains.is_empty());
    }

    #[tokio::test]
    async fn contentions_empty_graph() {
        let engine = empty_engine();
        let c = engine.contentions().await.unwrap();
        assert!(c.is_empty());
    }

    #[tokio::test]
    async fn contentions_with_contradicts_edge() {
        let mut g = GraphSidecar::new();
        let a = make_node("ClaimA");
        let b = make_node("ClaimB");
        let a_id = a.id;
        let b_id = b.id;
        g.add_node(a).unwrap();
        g.add_node(b).unwrap();
        g.add_edge(
            a_id,
            b_id,
            EdgeMeta {
                id: Uuid::new_v4(),
                rel_type: "contradicts".into(),
                weight: 1.0,
                confidence: 0.85,
                causal_level: None,
                clearance_level: 0,
                is_synthetic: false,
                has_valid_from: false,
            },
        )
        .unwrap();

        let engine = PetgraphEngine::new(Arc::new(RwLock::new(g)));
        let contentions = engine.contentions().await.unwrap();
        assert_eq!(contentions.len(), 1);
        assert_eq!(contentions[0].source_id, a_id);
        assert_eq!(contentions[0].rel_type, "contradicts");
        assert_eq!(contentions[0].targets.len(), 1);
        assert_eq!(contentions[0].targets[0].0, b_id);
    }

    #[tokio::test]
    async fn knowledge_gaps_empty_graph() {
        let engine = empty_engine();
        let gaps = engine.knowledge_gaps(1, 1, &[], 10).await.unwrap();
        assert!(gaps.is_empty());
    }

    #[tokio::test]
    async fn knowledge_gaps_detects_imbalance() {
        let mut g = GraphSidecar::new();
        // Node "Hub" has 3 incoming edges but 0 outgoing => gap
        let hub = make_node("HubNode");
        let hub_id = hub.id;
        g.add_node(hub).unwrap();

        for i in 0..3 {
            let leaf = make_node(&format!("Leaf{i}"));
            let leaf_id = leaf.id;
            g.add_node(leaf).unwrap();
            g.add_edge(leaf_id, hub_id, make_edge("references"))
                .unwrap();
        }

        let engine = PetgraphEngine::new(Arc::new(RwLock::new(g)));
        let gaps = engine.knowledge_gaps(2, 1, &[], 10).await.unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].id, hub_id);
        assert_eq!(gaps[0].in_degree, 3);
        assert_eq!(gaps[0].out_degree, 0);
    }

    #[tokio::test]
    async fn knowledge_gaps_respects_exclude_types() {
        let mut g = GraphSidecar::new();
        let hub = NodeMeta {
            id: Uuid::new_v4(),
            node_type: "person".into(),
            entity_class: None,
            canonical_name: "SomePerson".into(),
            clearance_level: 0,
        };
        let hub_id = hub.id;
        g.add_node(hub).unwrap();

        for i in 0..3 {
            let leaf = make_node(&format!("Ref{i}"));
            let leaf_id = leaf.id;
            g.add_node(leaf).unwrap();
            g.add_edge(leaf_id, hub_id, make_edge("references"))
                .unwrap();
        }

        let engine = PetgraphEngine::new(Arc::new(RwLock::new(g)));
        // Excluding "person" type should filter out the hub
        let gaps = engine.knowledge_gaps(2, 1, &["person"], 10).await.unwrap();
        assert!(gaps.is_empty());
    }

    #[tokio::test]
    async fn two_components_counted() {
        let mut g = GraphSidecar::new();
        let a = make_node("A");
        let b = make_node("B");
        let a_id = a.id;
        let b_id = b.id;
        g.add_node(a).unwrap();
        g.add_node(b).unwrap();
        g.add_edge(a_id, b_id, make_edge("related")).unwrap();

        // Disconnected node
        let c = make_node("C");
        g.add_node(c).unwrap();

        let engine = PetgraphEngine::new(Arc::new(RwLock::new(g)));
        let stats = engine.stats().await.unwrap();
        assert_eq!(stats.component_count, 2);
    }
}

//! Graph traversal algorithms — BFS, DFS, shortest path.
//!
//! All traversals support optional edge-type filtering and hop-decay
//! scoring: `score(node, hops) = base_score * decay^hops`.

use std::collections::{HashMap, HashSet, VecDeque};

use petgraph::Direction;
use petgraph::stable_graph::NodeIndex;
use petgraph::visit::EdgeRef;
use uuid::Uuid;

use super::sidecar::GraphSidecar;

/// Default hop-decay factor: `score = base * 0.7^hops`.
const HOP_DECAY: f64 = 0.7;

/// BFS neighborhood traversal with optional edge-type filtering.
///
/// Returns a list of `(node_uuid, hop_distance)` pairs for all nodes
/// reachable from `start` within `max_hops`. The start node is not included.
///
/// When `skip_synthetic` is true, synthetic (co-occurrence) edges are
/// excluded from traversal, restricting BFS to semantic relationships.
pub fn bfs_neighborhood(
    graph: &GraphSidecar,
    start: Uuid,
    max_hops: usize,
    edge_filter: Option<&[String]>,
) -> Vec<(Uuid, usize)> {
    bfs_neighborhood_filtered(graph, start, max_hops, edge_filter, false)
}

/// BFS neighborhood traversal that can skip synthetic edges.
///
/// Same as [`bfs_neighborhood`] but with an explicit `skip_synthetic`
/// flag. When true, edges marked `is_synthetic` are not traversed.
pub fn bfs_neighborhood_filtered(
    graph: &GraphSidecar,
    start: Uuid,
    max_hops: usize,
    edge_filter: Option<&[String]>,
    skip_synthetic: bool,
) -> Vec<(Uuid, usize)> {
    let Some(start_idx) = graph.node_index(start) else {
        return Vec::new();
    };

    let mut visited: HashSet<NodeIndex> = HashSet::new();
    visited.insert(start_idx);

    let mut queue: VecDeque<(NodeIndex, usize)> = VecDeque::new();
    queue.push_back((start_idx, 0));

    let mut results = Vec::new();

    while let Some((current, depth)) = queue.pop_front() {
        if depth >= max_hops {
            continue;
        }

        // Traverse both outgoing and incoming edges so that
        // neighborhood discovery is direction-agnostic. In a
        // knowledge graph, "A causes B" means B is a neighbor of
        // A and A is a neighbor of B.
        for direction in [Direction::Outgoing, Direction::Incoming] {
            for edge in graph.graph.edges_directed(current, direction) {
                let edge_meta = &graph.graph[edge.id()];

                // Skip synthetic edges when requested.
                if skip_synthetic && edge_meta.is_synthetic {
                    continue;
                }

                // Apply edge-type filter if provided.
                if let Some(filter) = edge_filter {
                    if !filter.iter().any(|f| f == &edge_meta.rel_type) {
                        continue;
                    }
                }

                // For outgoing edges the neighbor is the target;
                // for incoming edges the neighbor is the source.
                let neighbor = match direction {
                    Direction::Outgoing => edge.target(),
                    Direction::Incoming => edge.source(),
                };
                if visited.insert(neighbor) {
                    let next_depth = depth + 1;
                    results.push((graph.graph[neighbor].id, next_depth));
                    queue.push_back((neighbor, next_depth));
                }
            }
        }
    }

    results
}

/// DFS neighborhood traversal up to `max_hops` depth.
///
/// Returns `(node_uuid, hop_distance)` for all reachable nodes. The start
/// node is not included.
pub fn dfs_neighborhood(graph: &GraphSidecar, start: Uuid, max_hops: usize) -> Vec<(Uuid, usize)> {
    let Some(start_idx) = graph.node_index(start) else {
        return Vec::new();
    };

    let mut visited: HashSet<NodeIndex> = HashSet::new();
    visited.insert(start_idx);

    let mut stack: Vec<(NodeIndex, usize)> = vec![(start_idx, 0)];
    let mut results = Vec::new();

    while let Some((current, depth)) = stack.pop() {
        if depth >= max_hops {
            continue;
        }

        // Traverse both directions for undirected neighborhood.
        for direction in [Direction::Outgoing, Direction::Incoming] {
            for edge in graph.graph.edges_directed(current, direction) {
                let neighbor = match direction {
                    Direction::Outgoing => edge.target(),
                    Direction::Incoming => edge.source(),
                };
                if visited.insert(neighbor) {
                    let next_depth = depth + 1;
                    results.push((graph.graph[neighbor].id, next_depth));
                    stack.push((neighbor, next_depth));
                }
            }
        }
    }

    results
}

/// Find the shortest path between two nodes using BFS.
///
/// Returns the sequence of node UUIDs from `from` to `to` (inclusive),
/// or `None` if no path exists.
pub fn shortest_path(graph: &GraphSidecar, from: Uuid, to: Uuid) -> Option<Vec<Uuid>> {
    let from_idx = graph.node_index(from)?;
    let to_idx = graph.node_index(to)?;

    if from_idx == to_idx {
        return Some(vec![from]);
    }

    let mut visited: HashSet<NodeIndex> = HashSet::new();
    visited.insert(from_idx);

    // parent map for path reconstruction
    let mut parent: HashMap<NodeIndex, NodeIndex> = HashMap::new();
    let mut queue: VecDeque<NodeIndex> = VecDeque::new();
    queue.push_back(from_idx);

    while let Some(current) = queue.pop_front() {
        // Traverse both directions to find paths regardless of
        // edge directionality.
        for direction in [Direction::Outgoing, Direction::Incoming] {
            for edge in graph.graph.edges_directed(current, direction) {
                let neighbor = match direction {
                    Direction::Outgoing => edge.target(),
                    Direction::Incoming => edge.source(),
                };
                if visited.insert(neighbor) {
                    parent.insert(neighbor, current);
                    if neighbor == to_idx {
                        // Reconstruct path
                        let mut path = vec![to];
                        let mut cursor = to_idx;
                        while let Some(&prev) = parent.get(&cursor) {
                            path.push(graph.graph[prev].id);
                            cursor = prev;
                        }
                        path.reverse();
                        return Some(path);
                    }
                    queue.push_back(neighbor);
                }
            }
        }
    }

    None
}

/// Compute hop-decay score: `base * 0.7^hops`.
pub fn hop_decay_score(base: f64, hops: usize) -> f64 {
    base * HOP_DECAY.powi(hops as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::sidecar::{EdgeMeta, NodeMeta};

    fn add_node(g: &mut GraphSidecar, name: &str) -> Uuid {
        let id = Uuid::new_v4();
        g.add_node(NodeMeta {
            id,
            node_type: "entity".into(),
            canonical_name: name.into(),
            clearance_level: 0,
        })
        .unwrap();
        id
    }

    fn add_edge(g: &mut GraphSidecar, src: Uuid, tgt: Uuid, rel: &str) {
        g.add_edge(
            src,
            tgt,
            EdgeMeta {
                id: Uuid::new_v4(),
                rel_type: rel.into(),
                weight: 1.0,
                confidence: 0.9,
                causal_level: None,
                clearance_level: 0,
                is_synthetic: false,
            },
        )
        .unwrap();
    }

    /// Build: A -> B -> C -> D
    fn linear_graph() -> (GraphSidecar, Uuid, Uuid, Uuid, Uuid) {
        let mut g = GraphSidecar::new();
        let a = add_node(&mut g, "A");
        let b = add_node(&mut g, "B");
        let c = add_node(&mut g, "C");
        let d = add_node(&mut g, "D");
        add_edge(&mut g, a, b, "related");
        add_edge(&mut g, b, c, "causes");
        add_edge(&mut g, c, d, "related");
        (g, a, b, c, d)
    }

    #[test]
    fn bfs_finds_neighbors() {
        let (g, a, b, c, _d) = linear_graph();
        let result = bfs_neighborhood(&g, a, 2, None);
        let ids: Vec<Uuid> = result.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&b));
        assert!(ids.contains(&c));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn bfs_respects_edge_filter() {
        let (g, a, b, _c, _d) = linear_graph();
        let filter = vec!["related".to_string()];
        let result = bfs_neighborhood(&g, a, 3, Some(&filter));
        // A->B is "related", B->C is "causes" (filtered out)
        let ids: Vec<Uuid> = result.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&b));
        assert!(!ids.contains(&_c));
    }

    #[test]
    fn dfs_finds_all_reachable() {
        let (g, a, b, c, d) = linear_graph();
        let result = dfs_neighborhood(&g, a, 10);
        let ids: Vec<Uuid> = result.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&b));
        assert!(ids.contains(&c));
        assert!(ids.contains(&d));
    }

    #[test]
    fn shortest_path_linear() {
        let (g, a, _b, _c, d) = linear_graph();
        let path = shortest_path(&g, a, d).unwrap();
        assert_eq!(path.len(), 4);
        assert_eq!(path[0], a);
        assert_eq!(path[3], d);
    }

    #[test]
    fn shortest_path_reverse_direction() {
        let (g, a, _b, _c, d) = linear_graph();
        // D→A traverses incoming edges (reverse of A→B→C→D).
        // Now reachable since we traverse both directions.
        let path = shortest_path(&g, d, a).unwrap();
        assert_eq!(path.len(), 4);
        assert_eq!(path[0], d);
        assert_eq!(path[3], a);
    }

    #[test]
    fn shortest_path_no_route() {
        // Two disconnected nodes have no path.
        let mut g = GraphSidecar::new();
        let x = add_node(&mut g, "X");
        let y = add_node(&mut g, "Y");
        let path = shortest_path(&g, x, y);
        assert!(path.is_none());
    }

    #[test]
    fn shortest_path_same_node() {
        let (g, a, _, _, _) = linear_graph();
        let path = shortest_path(&g, a, a).unwrap();
        assert_eq!(path, vec![a]);
    }

    #[test]
    fn hop_decay_scoring() {
        let score = hop_decay_score(1.0, 0);
        assert!((score - 1.0).abs() < f64::EPSILON);

        let score = hop_decay_score(1.0, 1);
        assert!((score - 0.7).abs() < f64::EPSILON);

        let score = hop_decay_score(1.0, 2);
        assert!((score - 0.49).abs() < 1e-10);
    }

    #[test]
    fn bfs_filtered_skips_synthetic() {
        let mut g = GraphSidecar::new();
        let a = add_node(&mut g, "A");
        let b = add_node(&mut g, "B");
        let c = add_node(&mut g, "C");

        // A -> B is semantic (should be traversed)
        add_edge(&mut g, a, b, "related");
        // B -> C is synthetic (should be skipped)
        g.add_edge(
            b,
            c,
            EdgeMeta {
                id: Uuid::new_v4(),
                rel_type: "co_occurs".into(),
                weight: 1.0,
                confidence: 0.5,
                causal_level: None,
                clearance_level: 0,
                is_synthetic: true,
            },
        )
        .unwrap();

        // Without skip_synthetic: finds B and C
        let all = bfs_neighborhood(&g, a, 3, None);
        assert_eq!(all.len(), 2);

        // With skip_synthetic: finds only B
        let semantic = bfs_neighborhood_filtered(&g, a, 3, None, true);
        assert_eq!(semantic.len(), 1);
        assert_eq!(semantic[0].0, b);
    }

    #[test]
    fn bfs_traverses_incoming_edges() {
        // Build: A -> B. Starting from B should find A via the
        // incoming edge, not just outgoing.
        let mut g = GraphSidecar::new();
        let a = add_node(&mut g, "A");
        let b = add_node(&mut g, "B");
        add_edge(&mut g, a, b, "related");

        let result = bfs_neighborhood(&g, b, 1, None);
        let ids: Vec<Uuid> = result.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&a), "B should find A via incoming edge");
    }

    #[test]
    fn dfs_traverses_incoming_edges() {
        let mut g = GraphSidecar::new();
        let a = add_node(&mut g, "A");
        let b = add_node(&mut g, "B");
        add_edge(&mut g, a, b, "related");

        let result = dfs_neighborhood(&g, b, 1);
        let ids: Vec<Uuid> = result.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&a), "B should find A via incoming edge");
    }
}

//! Graph traversal algorithms — BFS, DFS, shortest path.
//!
//! All traversals support optional edge-type filtering and hop-decay
//! scoring: `score(node, hops) = base_score * decay^hops`.

use std::collections::{HashMap, HashSet, VecDeque};

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
pub fn bfs_neighborhood(
    graph: &GraphSidecar,
    start: Uuid,
    max_hops: usize,
    edge_filter: Option<&[String]>,
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

        for edge in graph.graph.edges(current) {
            // Apply edge-type filter if provided
            if let Some(filter) = edge_filter {
                if !filter.iter().any(|f| f == &graph.graph[edge.id()].rel_type) {
                    continue;
                }
            }

            let neighbor = edge.target();
            if visited.insert(neighbor) {
                let next_depth = depth + 1;
                results.push((graph.graph[neighbor].id, next_depth));
                queue.push_back((neighbor, next_depth));
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

        for edge in graph.graph.edges(current) {
            let neighbor = edge.target();
            if visited.insert(neighbor) {
                let next_depth = depth + 1;
                results.push((graph.graph[neighbor].id, next_depth));
                stack.push((neighbor, next_depth));
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
        for edge in graph.graph.edges(current) {
            let neighbor = edge.target();
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
    fn shortest_path_no_route() {
        let (g, _a, _b, _c, d) = linear_graph();
        // D has no outgoing edges, and we ask from D -> A (reversed)
        let path = shortest_path(&g, d, _a);
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
}

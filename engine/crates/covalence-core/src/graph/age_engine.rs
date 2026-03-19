//! `AgeEngine` -- Apache AGE backend implementing [`GraphEngine`].
//!
//! Executes Cypher queries against PostgreSQL with the AGE extension.
//! Graph data lives in an AGE graph (default name `covalence_graph`)
//! within the same PostgreSQL instance that stores Covalence data.
//!
//! ## Design
//!
//! - **Single vertex label `Node`** with properties: `uuid`, `name`,
//!   `node_type`, `clearance_level`, `confidence`.
//! - **Single edge label `E`** with properties: `uuid`, `rel_type`,
//!   `weight`, `confidence`, `is_synthetic`.
//! - AGE queries use `ag_catalog.cypher()`. Each connection must
//!   `LOAD 'age'` and set `search_path` before executing Cypher.
//! - Algorithms not available in Cypher (PageRank, TrustRank,
//!   structural importance, communities) fetch the full adjacency
//!   list and compute in Rust, reusing the existing algorithm
//!   implementations.

use std::collections::HashMap;

use petgraph::visit::EdgeRef;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::consolidation::contention::detect_contentions;
use crate::error::{Error, Result};
use crate::graph::algorithms;
use crate::graph::community::{detect_communities_with_min_size, label_communities};
use crate::graph::engine::{
    BfsNode, BfsOptions, Contention, GapCandidate, GraphEngine, GraphStats, Neighbor, ReloadResult,
};
use crate::graph::sidecar::{EdgeMeta, GraphSidecar, NodeMeta};
use crate::graph::topology::{TopologyMap, build_topology};

/// Batch size for bulk Cypher CREATE operations during reload.
const RELOAD_BATCH_SIZE: usize = 100;

/// Apache AGE graph engine backed by PostgreSQL.
///
/// Queries run through `ag_catalog.cypher()` against a named AGE
/// graph. For algorithms that AGE does not natively support, the
/// engine fetches the adjacency list into a temporary petgraph
/// `StableDiGraph` and runs the existing Rust implementations.
pub struct AgeEngine {
    /// Connection pool to the PostgreSQL instance with AGE installed.
    pool: PgPool,
    /// Name of the AGE graph (e.g. `covalence_graph`).
    graph_name: String,
}

impl AgeEngine {
    /// Create a new `AgeEngine`.
    ///
    /// The `graph_name` identifies the AGE graph schema. The graph
    /// must already exist (call [`reload`] to create and populate it).
    pub fn new(pool: PgPool, graph_name: impl Into<String>) -> Self {
        Self {
            pool,
            graph_name: graph_name.into(),
        }
    }

    /// Acquire a connection with AGE loaded and search_path set.
    ///
    /// Each connection from the pool needs `LOAD 'age'` and a
    /// search_path that includes `ag_catalog` so that `agtype` and
    /// `cypher()` are available.
    async fn age_conn(&self) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query("LOAD 'age'").execute(&mut *conn).await?;
        sqlx::query("SET search_path = ag_catalog, \"$user\", public")
            .execute(&mut *conn)
            .await?;
        Ok(conn)
    }

    /// Execute a Cypher query that returns a single `agtype` column
    /// named `result`, and return the raw rows.
    async fn cypher_query(
        &self,
        cypher: &str,
        conn: &mut sqlx::pool::PoolConnection<sqlx::Postgres>,
    ) -> Result<Vec<PgRow>> {
        self.cypher_multi(cypher, &["result"], conn).await
    }

    /// Execute a Cypher query with named columns, casting all
    /// `agtype` results to `text` so sqlx can decode them as
    /// `String`.
    async fn cypher_multi(
        &self,
        cypher: &str,
        columns: &[&str],
        conn: &mut sqlx::pool::PoolConnection<sqlx::Postgres>,
    ) -> Result<Vec<PgRow>> {
        let col_defs = columns
            .iter()
            .map(|c| format!("{c} ag_catalog.agtype"))
            .collect::<Vec<_>>()
            .join(", ");
        let col_casts = columns
            .iter()
            .map(|c| format!("{c}::text"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {col_casts} FROM \
             (SELECT * FROM ag_catalog.cypher('{}', $$ {} $$) \
              AS ({col_defs})) sub",
            self.graph_name, cypher
        );
        let rows = sqlx::query(&sql).fetch_all(&mut **conn).await?;
        Ok(rows)
    }

    /// Execute a Cypher COUNT query and return the count as `usize`.
    async fn cypher_count(&self, cypher: &str) -> Result<usize> {
        let mut conn = self.age_conn().await?;
        let rows = self.cypher_query(cypher, &mut conn).await?;
        if let Some(row) = rows.first() {
            let raw: String = row.try_get("result")?;
            Ok(parse_agtype_int(&raw) as usize)
        } else {
            Ok(0)
        }
    }

    /// Build a temporary in-memory petgraph from the AGE graph.
    ///
    /// This is used by algorithms that need the full adjacency list
    /// (PageRank, TrustRank, communities, etc.). The graph is
    /// ephemeral and discarded after the algorithm completes.
    async fn build_temp_graph(&self) -> Result<GraphSidecar> {
        let mut conn = self.age_conn().await?;
        let mut sidecar = GraphSidecar::new();

        // Fetch all nodes
        let node_rows = self
            .cypher_multi(
                "MATCH (n:Node) \
                 RETURN n.uuid, n.name, n.node_type, \
                        n.clearance_level",
                &["uuid", "name", "ntype", "cl"],
                &mut conn,
            )
            .await?;

        for row in &node_rows {
            let uuid_str: String = row.try_get("uuid")?;
            let name: String = row.try_get("name")?;
            let ntype: String = row.try_get("ntype")?;
            let cl: String = row.try_get("cl")?;

            let uuid = parse_agtype_uuid(&uuid_str)?;
            let _ = sidecar.add_node(NodeMeta {
                id: uuid,
                node_type: parse_agtype_string(&ntype),
                entity_class: None,
                canonical_name: parse_agtype_string(&name),
                clearance_level: parse_agtype_int(&cl) as i32,
            });
        }

        // Fetch all edges
        let edge_rows = self
            .cypher_multi(
                "MATCH (a:Node)-[e:E]->(b:Node) \
                 RETURN e.uuid, a.uuid, b.uuid, e.rel_type, \
                        e.weight, e.confidence, e.is_synthetic",
                &["euuid", "auuid", "buuid", "rel", "w", "conf", "synth"],
                &mut conn,
            )
            .await?;

        for row in &edge_rows {
            let euuid_str: String = row.try_get("euuid")?;
            let auuid_str: String = row.try_get("auuid")?;
            let buuid_str: String = row.try_get("buuid")?;
            let rel: String = row.try_get("rel")?;
            let w: String = row.try_get("w")?;
            let conf: String = row.try_get("conf")?;
            let synth: String = row.try_get("synth")?;

            let edge_uuid = parse_agtype_uuid(&euuid_str)?;
            let source_uuid = parse_agtype_uuid(&auuid_str)?;
            let target_uuid = parse_agtype_uuid(&buuid_str)?;

            let _ = sidecar.add_edge(
                source_uuid,
                target_uuid,
                EdgeMeta {
                    id: edge_uuid,
                    rel_type: parse_agtype_string(&rel),
                    weight: parse_agtype_float(&w),
                    confidence: parse_agtype_float(&conf),
                    causal_level: None,
                    clearance_level: 0,
                    is_synthetic: parse_agtype_bool(&synth),
                    has_valid_from: false,
                },
            );
        }

        Ok(sidecar)
    }
}

#[async_trait::async_trait]
impl GraphEngine for AgeEngine {
    // ----- Stats -----

    /// Graph summary statistics.
    async fn stats(&self) -> Result<GraphStats> {
        let node_count = self.node_count().await?;
        let edge_count = self.edge_count().await?;

        let synthetic_edge_count = self
            .cypher_count(
                "MATCH ()-[e:E]->() WHERE e.is_synthetic = true \
                 RETURN count(e)",
            )
            .await?;

        let semantic_edge_count = edge_count.saturating_sub(synthetic_edge_count);

        let density = if node_count > 1 {
            edge_count as f64 / (node_count as f64 * (node_count as f64 - 1.0))
        } else {
            0.0
        };

        // Component count requires graph traversal; build temp graph
        // for small graphs, or use a heuristic for large ones.
        let component_count = if node_count == 0 {
            0
        } else {
            let g = self.build_temp_graph().await?;
            count_weak_components(&g)
        };

        Ok(GraphStats {
            node_count,
            edge_count,
            semantic_edge_count,
            synthetic_edge_count,
            density,
            component_count,
        })
    }

    /// Number of nodes.
    async fn node_count(&self) -> Result<usize> {
        self.cypher_count("MATCH (n:Node) RETURN count(n)").await
    }

    /// Number of active edges.
    async fn edge_count(&self) -> Result<usize> {
        self.cypher_count("MATCH ()-[e:E]->() RETURN count(e)")
            .await
    }

    // ----- Node access -----

    /// Get a node's metadata by UUID.
    async fn get_node(&self, id: Uuid) -> Result<Option<NodeMeta>> {
        let mut conn = self.age_conn().await?;
        let cypher = format!(
            "MATCH (n:Node) WHERE n.uuid = '{}' \
             RETURN n.uuid, n.name, n.node_type, n.clearance_level",
            id
        );
        let rows = self
            .cypher_multi(&cypher, &["uuid", "name", "ntype", "cl"], &mut conn)
            .await?;

        if let Some(row) = rows.first() {
            let uuid_str: String = row.try_get("uuid")?;
            let name: String = row.try_get("name")?;
            let ntype: String = row.try_get("ntype")?;
            let cl: String = row.try_get("cl")?;

            Ok(Some(NodeMeta {
                id: parse_agtype_uuid(&uuid_str)?,
                node_type: parse_agtype_string(&ntype),
                entity_class: None,
                canonical_name: parse_agtype_string(&name),
                clearance_level: parse_agtype_int(&cl) as i32,
            }))
        } else {
            Ok(None)
        }
    }

    /// Get outgoing neighbors of a node.
    async fn neighbors_out(&self, id: Uuid) -> Result<Vec<Neighbor>> {
        let mut conn = self.age_conn().await?;
        let cypher = format!(
            "MATCH (n:Node)-[e:E]->(m:Node) WHERE n.uuid = '{}' \
             RETURN m.uuid, m.name, m.node_type, \
                    e.rel_type, e.is_synthetic, e.confidence, e.weight",
            id
        );
        let rows = self
            .cypher_multi(
                &cypher,
                &["uuid", "name", "ntype", "rel", "synth", "conf", "w"],
                &mut conn,
            )
            .await?;

        let mut neighbors = Vec::with_capacity(rows.len());
        for row in &rows {
            let uuid_str: String = row.try_get("uuid")?;
            let name: String = row.try_get("name")?;
            let ntype: String = row.try_get("ntype")?;
            let rel: String = row.try_get("rel")?;
            let synth: String = row.try_get("synth")?;
            let conf: String = row.try_get("conf")?;
            let w: String = row.try_get("w")?;

            neighbors.push(Neighbor {
                id: parse_agtype_uuid(&uuid_str)?,
                rel_type: parse_agtype_string(&rel),
                is_synthetic: parse_agtype_bool(&synth),
                confidence: parse_agtype_float(&conf),
                weight: parse_agtype_float(&w),
                name: parse_agtype_string(&name),
                node_type: parse_agtype_string(&ntype),
            });
        }

        Ok(neighbors)
    }

    /// Get incoming neighbors of a node.
    async fn neighbors_in(&self, id: Uuid) -> Result<Vec<Neighbor>> {
        let mut conn = self.age_conn().await?;
        let cypher = format!(
            "MATCH (m:Node)-[e:E]->(n:Node) WHERE n.uuid = '{}' \
             RETURN m.uuid, m.name, m.node_type, \
                    e.rel_type, e.is_synthetic, e.confidence, e.weight",
            id
        );
        let rows = self
            .cypher_multi(
                &cypher,
                &["uuid", "name", "ntype", "rel", "synth", "conf", "w"],
                &mut conn,
            )
            .await?;

        let mut neighbors = Vec::with_capacity(rows.len());
        for row in &rows {
            let uuid_str: String = row.try_get("uuid")?;
            let name: String = row.try_get("name")?;
            let ntype: String = row.try_get("ntype")?;
            let rel: String = row.try_get("rel")?;
            let synth: String = row.try_get("synth")?;
            let conf: String = row.try_get("conf")?;
            let w: String = row.try_get("w")?;

            neighbors.push(Neighbor {
                id: parse_agtype_uuid(&uuid_str)?,
                rel_type: parse_agtype_string(&rel),
                is_synthetic: parse_agtype_bool(&synth),
                confidence: parse_agtype_float(&conf),
                weight: parse_agtype_float(&w),
                name: parse_agtype_string(&name),
                node_type: parse_agtype_string(&ntype),
            });
        }

        Ok(neighbors)
    }

    /// In-degree of a node.
    async fn degree_in(&self, id: Uuid) -> Result<usize> {
        let cypher = format!(
            "MATCH ()-[e:E]->(n:Node) WHERE n.uuid = '{}' \
             RETURN count(e)",
            id
        );
        self.cypher_count(&cypher).await
    }

    /// Out-degree of a node.
    async fn degree_out(&self, id: Uuid) -> Result<usize> {
        let cypher = format!(
            "MATCH (n:Node)-[e:E]->() WHERE n.uuid = '{}' \
             RETURN count(e)",
            id
        );
        self.cypher_count(&cypher).await
    }

    // ----- Traversal -----

    /// BFS neighborhood discovery from a start node.
    ///
    /// Fetches the graph into a temporary petgraph sidecar and
    /// delegates to the existing BFS implementation, ensuring
    /// identical behavior to `PetgraphEngine`.
    async fn bfs_neighborhood(&self, start: Uuid, options: BfsOptions) -> Result<Vec<BfsNode>> {
        let g = self.build_temp_graph().await?;

        let deny_refs: Vec<&str> = options.deny_rel_types.iter().map(|s| s.as_str()).collect();
        let edge_deny = if deny_refs.is_empty() {
            None
        } else {
            Some(deny_refs.as_slice())
        };

        let raw = crate::graph::traversal::bfs_neighborhood_full(
            &g,
            start,
            options.max_hops,
            None,
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

    /// Shortest path between two nodes.
    ///
    /// Delegates to the existing BFS-based shortest path
    /// implementation via a temporary petgraph sidecar.
    async fn shortest_path(&self, from: Uuid, to: Uuid) -> Result<Option<Vec<Uuid>>> {
        let g = self.build_temp_graph().await?;
        Ok(crate::graph::traversal::shortest_path(&g, from, to))
    }

    // ----- Algorithms -----

    /// PageRank scores for all nodes.
    ///
    /// Fetches the full graph into a temporary petgraph sidecar and
    /// runs the existing PageRank implementation.
    async fn pagerank(&self, damping: f64, iterations: usize) -> Result<HashMap<Uuid, f64>> {
        let g = self.build_temp_graph().await?;
        Ok(algorithms::pagerank(g.graph(), damping, iterations))
    }

    /// TrustRank: biased PageRank from trusted seed nodes.
    async fn trust_rank(
        &self,
        seeds: &[(Uuid, f64)],
        damping: f64,
        iterations: usize,
    ) -> Result<HashMap<Uuid, f64>> {
        let g = self.build_temp_graph().await?;
        Ok(algorithms::trust_rank(
            g.graph(),
            seeds,
            damping,
            iterations,
        ))
    }

    /// Structural importance (betweenness centrality approximation).
    async fn structural_importance(&self) -> Result<HashMap<Uuid, f64>> {
        let g = self.build_temp_graph().await?;
        Ok(algorithms::structural_importance(g.graph()))
    }

    /// Spreading activation from seed nodes with decay.
    async fn spreading_activation(
        &self,
        seeds: &[(Uuid, f64)],
        decay: f64,
        max_hops: usize,
    ) -> Result<HashMap<Uuid, f64>> {
        let g = self.build_temp_graph().await?;
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
        let g = self.build_temp_graph().await?;
        let mut comms = detect_communities_with_min_size(g.graph(), min_size);
        label_communities(g.graph(), &mut comms);
        Ok(comms)
    }

    /// Build full topology map.
    async fn topology(&self) -> Result<TopologyMap> {
        let g = self.build_temp_graph().await?;
        Ok(build_topology(g.graph()))
    }

    /// Detect contentious (contradictory) relationships.
    async fn contentions(&self) -> Result<Vec<Contention>> {
        let g = self.build_temp_graph().await?;
        let raw = detect_contentions(g.graph());

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
    ///
    /// Runs the same algorithm as `PetgraphEngine` but against
    /// a temporary graph built from AGE data.
    async fn knowledge_gaps(
        &self,
        min_in_degree: usize,
        min_label_length: usize,
        exclude_types: &[&str],
        limit: usize,
    ) -> Result<Vec<GapCandidate>> {
        let g = self.build_temp_graph().await?;
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

    /// Full reload from PostgreSQL into the AGE graph.
    ///
    /// Drops and recreates the AGE graph, then bulk-inserts all
    /// nodes and active edges from the `nodes` and `edges` tables
    /// using batched Cypher CREATE statements.
    async fn reload(&self, pool: &sqlx::PgPool) -> Result<ReloadResult> {
        let mut conn = self.age_conn().await?;

        // Drop existing graph (ignore error if it doesn't exist)
        let drop_sql = format!("SELECT ag_catalog.drop_graph('{}', true)", self.graph_name);
        let _ = sqlx::query(&drop_sql).execute(&mut *conn).await;

        // Create new graph
        let create_sql = format!("SELECT ag_catalog.create_graph('{}')", self.graph_name);
        sqlx::query(&create_sql).execute(&mut *conn).await?;

        // Re-initialize AGE on the connection after graph creation
        sqlx::query("LOAD 'age'").execute(&mut *conn).await?;
        sqlx::query("SET search_path = ag_catalog, \"$user\", public")
            .execute(&mut *conn)
            .await?;

        // Fetch all nodes from PostgreSQL
        let node_rows = sqlx::query(
            "SELECT id, \
             COALESCE(canonical_type, node_type) AS node_type, \
             canonical_name, clearance_level FROM nodes",
        )
        .fetch_all(pool)
        .await?;

        // Batch-insert nodes into AGE
        let mut node_count = 0usize;
        for chunk in node_rows.chunks(RELOAD_BATCH_SIZE) {
            let creates: Vec<String> = chunk
                .iter()
                .map(|row| {
                    let id: Uuid = row.get("id");
                    let node_type: String = row.get("node_type");
                    let name: String = row.get("canonical_name");
                    let cl: i32 = row.get("clearance_level");
                    format!(
                        "(:Node {{uuid: '{}', name: '{}', \
                         node_type: '{}', clearance_level: {}}})",
                        id,
                        escape_cypher(&name),
                        escape_cypher(&node_type),
                        cl
                    )
                })
                .collect();

            let cypher = format!("CREATE {}", creates.join(", "));
            let _ = self.cypher_multi(&cypher, &["v"], &mut conn).await?;
            node_count += chunk.len();
        }

        // Fetch all active edges from PostgreSQL
        let edge_rows = sqlx::query(
            "SELECT id, source_node_id, target_node_id, \
             COALESCE(canonical_rel_type, rel_type) AS rel_type, \
             weight, confidence, is_synthetic \
             FROM edges WHERE invalid_at IS NULL",
        )
        .fetch_all(pool)
        .await?;

        // Insert edges one at a time (each needs MATCH for endpoints)
        let mut edge_count = 0usize;
        let mut edge_errors = 0usize;
        for row in &edge_rows {
            let edge_id: Uuid = row.get("id");
            let source_id: Uuid = row.get("source_node_id");
            let target_id: Uuid = row.get("target_node_id");
            let rel_type: String = row.get("rel_type");
            let weight: f64 = row.get("weight");
            let confidence: f64 = row.get("confidence");
            let is_synthetic: bool = row.get("is_synthetic");

            let cypher = format!(
                "MATCH (a:Node {{uuid: '{}'}}), \
                 (b:Node {{uuid: '{}'}}) \
                 CREATE (a)-[:E {{uuid: '{}', \
                 rel_type: '{}', weight: {}, \
                 confidence: {}, is_synthetic: {}}}]->(b)",
                source_id,
                target_id,
                edge_id,
                escape_cypher(&rel_type),
                weight,
                confidence,
                is_synthetic
            );
            match self.cypher_multi(&cypher, &["e"], &mut conn).await {
                Ok(_) => edge_count += 1,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        edge_id = %edge_id,
                        "failed to insert edge into AGE graph"
                    );
                    edge_errors += 1;
                }
            }
        }

        if edge_errors > 0 {
            tracing::warn!(
                edge_errors,
                "AGE reload completed with edge insertion errors"
            );
        }

        tracing::info!(
            node_count,
            edge_count,
            graph = %self.graph_name,
            "AGE graph reload complete"
        );

        Ok(ReloadResult {
            node_count,
            edge_count,
        })
    }
}

// ---- agtype parsing helpers ----

/// Parse an agtype integer value (e.g. `"42"` or `42`).
fn parse_agtype_int(raw: &str) -> i64 {
    let s = raw.trim().trim_matches('"');
    s.parse::<i64>().unwrap_or(0)
}

/// Parse an agtype float value (e.g. `"1.5"` or `1.5`).
fn parse_agtype_float(raw: &str) -> f64 {
    let s = raw.trim().trim_matches('"');
    s.parse::<f64>().unwrap_or(0.0)
}

/// Parse an agtype boolean value (e.g. `"true"` or `true`).
fn parse_agtype_bool(raw: &str) -> bool {
    let s = raw.trim().trim_matches('"').to_lowercase();
    s == "true"
}

/// Parse an agtype string value, stripping surrounding quotes.
fn parse_agtype_string(raw: &str) -> String {
    raw.trim().trim_matches('"').to_string()
}

/// Parse an agtype value as a UUID.
fn parse_agtype_uuid(raw: &str) -> Result<Uuid> {
    let s = raw.trim().trim_matches('"');
    s.parse::<Uuid>()
        .map_err(|e| Error::Graph(format!("invalid UUID in agtype: {s} ({e})")))
}

/// Escape single quotes in a string for use in Cypher literals.
fn escape_cypher(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Count weakly connected components via BFS over a `GraphSidecar`.
///
/// Iterates all nodes, performing BFS through both outgoing and
/// incoming edges for undirected connectivity.
fn count_weak_components(sidecar: &GraphSidecar) -> usize {
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
            for edge in graph.edges_directed(v, petgraph::Direction::Outgoing) {
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
    use super::*;

    #[test]
    fn parse_agtype_int_plain() {
        assert_eq!(parse_agtype_int("42"), 42);
    }

    #[test]
    fn parse_agtype_int_quoted() {
        assert_eq!(parse_agtype_int("\"42\""), 42);
    }

    #[test]
    fn parse_agtype_int_invalid() {
        assert_eq!(parse_agtype_int("not_a_number"), 0);
    }

    #[test]
    fn parse_agtype_float_plain() {
        assert!((parse_agtype_float("1.5") - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_agtype_float_quoted() {
        assert!((parse_agtype_float("\"0.85\"") - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_agtype_float_invalid() {
        assert!((parse_agtype_float("abc") - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_agtype_bool_true() {
        assert!(parse_agtype_bool("true"));
        assert!(parse_agtype_bool("\"true\""));
        assert!(parse_agtype_bool("True"));
    }

    #[test]
    fn parse_agtype_bool_false() {
        assert!(!parse_agtype_bool("false"));
        assert!(!parse_agtype_bool("\"false\""));
        assert!(!parse_agtype_bool("anything_else"));
    }

    #[test]
    fn parse_agtype_string_strips_quotes() {
        assert_eq!(parse_agtype_string("\"hello world\""), "hello world");
    }

    #[test]
    fn parse_agtype_string_no_quotes() {
        assert_eq!(parse_agtype_string("plain"), "plain");
    }

    #[test]
    fn parse_agtype_uuid_valid() {
        let id = Uuid::new_v4();
        let raw = format!("\"{}\"", id);
        let parsed = parse_agtype_uuid(&raw).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn parse_agtype_uuid_unquoted() {
        let id = Uuid::new_v4();
        let raw = id.to_string();
        let parsed = parse_agtype_uuid(&raw).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn parse_agtype_uuid_invalid() {
        assert!(parse_agtype_uuid("not-a-uuid").is_err());
    }

    #[test]
    fn escape_cypher_single_quotes() {
        assert_eq!(escape_cypher("it's"), "it\\'s");
    }

    #[test]
    fn escape_cypher_backslashes() {
        assert_eq!(escape_cypher("a\\b"), "a\\\\b");
    }

    #[test]
    fn escape_cypher_combined() {
        assert_eq!(escape_cypher("it's a\\b"), "it\\'s a\\\\b");
    }

    #[test]
    fn escape_cypher_clean_string() {
        assert_eq!(escape_cypher("hello world"), "hello world");
    }

    #[tokio::test]
    async fn engine_construction() {
        // Verify that AgeEngine can be constructed without panicking.
        // This does not test connectivity (that requires a live AGE
        // database and belongs in integration tests).
        let pool = PgPool::connect_lazy("postgres://test:test@localhost:5435/test").unwrap();
        let engine = AgeEngine::new(pool, "test_graph");
        assert_eq!(engine.graph_name, "test_graph");
    }

    #[test]
    fn count_weak_components_empty() {
        let g = GraphSidecar::new();
        assert_eq!(count_weak_components(&g), 0);
    }

    #[test]
    fn count_weak_components_single_node() {
        let mut g = GraphSidecar::new();
        let _ = g.add_node(NodeMeta {
            id: Uuid::new_v4(),
            node_type: "entity".into(),
            entity_class: None,
            canonical_name: "Solo".into(),
            clearance_level: 0,
        });
        assert_eq!(count_weak_components(&g), 1);
    }

    #[test]
    fn count_weak_components_connected_pair() {
        let mut g = GraphSidecar::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        g.add_node(NodeMeta {
            id: a,
            node_type: "entity".into(),
            entity_class: None,
            canonical_name: "A".into(),
            clearance_level: 0,
        })
        .unwrap();
        g.add_node(NodeMeta {
            id: b,
            node_type: "entity".into(),
            entity_class: None,
            canonical_name: "B".into(),
            clearance_level: 0,
        })
        .unwrap();
        g.add_edge(
            a,
            b,
            EdgeMeta {
                id: Uuid::new_v4(),
                rel_type: "related".into(),
                weight: 1.0,
                confidence: 0.9,
                causal_level: None,
                clearance_level: 0,
                is_synthetic: false,
                has_valid_from: false,
            },
        )
        .unwrap();
        assert_eq!(count_weak_components(&g), 1);
    }

    #[test]
    fn count_weak_components_two_disconnected() {
        let mut g = GraphSidecar::new();
        let _ = g.add_node(NodeMeta {
            id: Uuid::new_v4(),
            node_type: "entity".into(),
            entity_class: None,
            canonical_name: "A".into(),
            clearance_level: 0,
        });
        let _ = g.add_node(NodeMeta {
            id: Uuid::new_v4(),
            node_type: "entity".into(),
            entity_class: None,
            canonical_name: "B".into(),
            clearance_level: 0,
        });
        assert_eq!(count_weak_components(&g), 2);
    }
}

//! Knowledge Audit Service — epistemic assessment for a topic query.
//!
//! Implements `GET /admin/knowledge/audit?q=<topic>` (covalence#55).
//!
//! Aggregate pipeline:
//! 1. 4-D search for the topic (articles + sources, standard mode)
//! 2. Gather contentions for each top article
//! 3. Compute provenance summary from linked source nodes
//! 4. Compute confidence distribution across result set
//! 5. Extract graph context from the shared in-memory graph

use std::collections::HashMap;

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppResult;
use crate::graph::{
    SharedGraph,
    algorithms::{betweenness_centrality, connected_components, pagerank},
};
use crate::services::contention_service::ContentionService;
use crate::services::search_service::{SearchRequest, SearchResult, SearchService};

// ─── Response types ───────────────────────────────────────────────────────────

/// A single article in the consensus ranking.
#[derive(Debug, Serialize)]
pub struct ConsensusEntry {
    pub node_id: Uuid,
    pub title: Option<String>,
    pub score: f64,
    pub confidence: f64,
    /// Topological score from PageRank (if available).
    pub topological_score: Option<f64>,
    pub node_type: String,
    pub domain_path: Option<Vec<String>>,
    pub content_preview: String,
}

/// Contention record with both sides identified.
#[derive(Debug, Serialize)]
pub struct ContentionEntry {
    pub id: Uuid,
    /// The article node that is in contention.
    pub article_id: Option<Uuid>,
    /// The source node that triggered the contention.
    pub source_id: Option<Uuid>,
    pub description: Option<String>,
    pub status: String,
    pub severity: Option<String>,
    pub detected_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Aggregate summary of provenance sources supporting the consensus articles.
#[derive(Debug, Serialize)]
pub struct ProvenanceSummary {
    pub total_sources: usize,
    pub avg_reliability: f64,
    pub source_types: HashMap<String, usize>,
}

/// Distribution statistics of confidence scores across search results.
#[derive(Debug, Serialize)]
pub struct ConfidenceDistribution {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub median: f64,
}

/// Node reference with a graph centrality score.
#[derive(Debug, Serialize)]
pub struct CentralNode {
    pub node_id: Uuid,
    pub centrality_score: f64,
}

/// Graph topology metrics derived from the shared in-memory graph.
#[derive(Debug, Serialize)]
pub struct GraphContext {
    pub connected_components: usize,
    pub avg_pagerank: f64,
    pub central_nodes: Vec<CentralNode>,
}

/// Full audit response returned by `GET /admin/knowledge/audit`.
#[derive(Debug, Serialize)]
pub struct AuditResponse {
    pub topic: String,
    pub consensus: Vec<ConsensusEntry>,
    pub contentions: Vec<ContentionEntry>,
    pub provenance_summary: ProvenanceSummary,
    pub confidence_distribution: ConfidenceDistribution,
    pub graph_context: GraphContext,
}

// ─── Service ──────────────────────────────────────────────────────────────────

pub struct AuditService {
    pool: PgPool,
    graph: SharedGraph,
}

impl AuditService {
    pub fn new(pool: PgPool, graph: SharedGraph) -> Self {
        Self { pool, graph }
    }

    pub async fn audit(&self, topic: &str) -> AppResult<AuditResponse> {
        // ── Step 1: Search for topic ──────────────────────────────────────────
        let search_req = SearchRequest {
            query: topic.to_string(),
            embedding: None,
            intent: None,
            session_id: None,
            node_types: None, // articles + sources
            limit: 20,
            weights: None,
            mode: None, // standard flat search
            recency_bias: None,
            domain_path: None,
            strategy: None,
            max_hops: None,
            after: None,
            before: None,
            min_score: None,
            spreading_activation: None,
            facet_function: None,
            facet_scope: None,
        };

        let search_svc = SearchService::new(self.pool.clone()).with_graph(self.graph.clone());
        search_svc.init().await;

        let (mut results, _meta) = search_svc
            .search(search_req)
            .await
            .map_err(crate::errors::AppError::Internal)?;

        // Sort by composite score descending (already sorted, but be explicit).
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // ── Step 2: Build consensus entries ──────────────────────────────────
        let consensus: Vec<ConsensusEntry> = results
            .iter()
            .map(|r| ConsensusEntry {
                node_id: r.node_id,
                title: r.title.clone(),
                score: r.score,
                confidence: r.confidence,
                topological_score: r.topological_score,
                node_type: r.node_type.clone(),
                domain_path: r.domain_path.clone(),
                content_preview: r.content_preview.clone(),
            })
            .collect();

        // ── Step 3: Gather contentions for top article IDs ────────────────────
        let top_article_ids: Vec<Uuid> = results
            .iter()
            .filter(|r| r.node_type == "article")
            .take(10)
            .map(|r| r.node_id)
            .collect();

        let contention_svc = ContentionService::new(self.pool.clone());
        let mut contentions: Vec<ContentionEntry> = Vec::new();
        let mut seen_contention_ids: std::collections::HashSet<Uuid> =
            std::collections::HashSet::new();

        for article_id in &top_article_ids {
            let article_contentions = contention_svc
                .list(Some(*article_id), None)
                .await
                .unwrap_or_default();

            for c in article_contentions {
                if seen_contention_ids.insert(c.id) {
                    contentions.push(ContentionEntry {
                        id: c.id,
                        article_id: c.node_id,
                        source_id: c.source_node_id,
                        description: c.description,
                        status: c.status,
                        severity: c.severity,
                        detected_at: c.detected_at,
                    });
                }
            }
        }

        // ── Step 4: Provenance summary ────────────────────────────────────────
        let provenance_summary = self.provenance_summary(&top_article_ids).await?;

        // ── Step 5: Confidence distribution ──────────────────────────────────
        let confidence_distribution = compute_confidence_distribution(&results);

        // ── Step 6: Graph context ─────────────────────────────────────────────
        let graph_context = self.graph_context().await;

        Ok(AuditResponse {
            topic: topic.to_string(),
            consensus,
            contentions,
            provenance_summary,
            confidence_distribution,
            graph_context,
        })
    }

    // ── Provenance helper ─────────────────────────────────────────────────────

    /// Query linked source nodes for the given article IDs, then aggregate
    /// reliability and source-type counts.
    async fn provenance_summary(&self, article_ids: &[Uuid]) -> AppResult<ProvenanceSummary> {
        if article_ids.is_empty() {
            return Ok(ProvenanceSummary {
                total_sources: 0,
                avg_reliability: 0.0,
                source_types: HashMap::new(),
            });
        }

        let rows = sqlx::query_as::<_, (Uuid, Option<f64>, Option<String>)>(
            "SELECT DISTINCT s.id,
                    s.reliability,
                    s.source_type
             FROM   covalence.edges e
             JOIN   covalence.nodes s ON s.id = e.source_node_id
             WHERE  e.target_node_id = ANY($1)
               AND  e.edge_type IN ('ORIGINATES', 'COMPILED_FROM', 'CONFIRMS')
               AND  s.node_type = 'source'
               AND  s.status   = 'active'",
        )
        .bind(article_ids)
        .fetch_all(&self.pool)
        .await?;

        let total_sources = rows.len();
        let mut reliability_sum = 0.0f64;
        let mut source_types: HashMap<String, usize> = HashMap::new();

        for (_id, reliability, source_type) in &rows {
            reliability_sum += reliability.unwrap_or(0.5);
            let stype = source_type.clone().unwrap_or_else(|| "unknown".to_string());
            *source_types.entry(stype).or_insert(0) += 1;
        }

        let avg_reliability = if total_sources > 0 {
            reliability_sum / total_sources as f64
        } else {
            0.0
        };

        Ok(ProvenanceSummary {
            total_sources,
            avg_reliability,
            source_types,
        })
    }

    // ── Graph context helper ──────────────────────────────────────────────────

    /// Read the shared in-memory graph and compute topology metrics.
    /// Runs all graph algorithms with the read-lock held, then releases it.
    async fn graph_context(&self) -> GraphContext {
        let graph = self.graph.read().await;

        // PageRank (damping=0.85, iterations=20 — matches admin_graph_pagerank)
        let pr_scores = pagerank(&graph, 0.85, 20);

        let avg_pagerank = if pr_scores.is_empty() {
            0.0
        } else {
            pr_scores.values().sum::<f64>() / pr_scores.len() as f64
        };

        // Betweenness centrality — top 3 nodes
        let centrality = betweenness_centrality(&graph);
        let mut centrality_vec: Vec<(Uuid, f64)> = centrality.into_iter().collect();
        centrality_vec.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let central_nodes: Vec<CentralNode> = centrality_vec
            .into_iter()
            .take(3)
            .map(|(id, score)| CentralNode {
                node_id: id,
                centrality_score: score,
            })
            .collect();

        // Connected components (Kosaraju SCC)
        let components = connected_components(&graph);
        let component_count = components.len();

        drop(graph); // release read-lock explicitly

        GraphContext {
            connected_components: component_count,
            avg_pagerank,
            central_nodes,
        }
    }
}

// ─── Confidence distribution ──────────────────────────────────────────────────

fn compute_confidence_distribution(results: &[SearchResult]) -> ConfidenceDistribution {
    if results.is_empty() {
        return ConfidenceDistribution {
            min: 0.0,
            max: 0.0,
            mean: 0.0,
            median: 0.0,
        };
    }

    let mut scores: Vec<f64> = results.iter().map(|r| r.confidence).collect();
    scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let min = *scores.first().unwrap_or(&0.0);
    let max = *scores.last().unwrap_or(&0.0);
    let mean = scores.iter().sum::<f64>() / scores.len() as f64;
    let median = {
        let n = scores.len();
        if n % 2 == 0 {
            (scores[n / 2 - 1] + scores[n / 2]) / 2.0
        } else {
            scores[n / 2]
        }
    };

    ConfidenceDistribution {
        min,
        max,
        mean,
        median,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_result(confidence: f64) -> SearchResult {
        SearchResult {
            node_id: Uuid::new_v4(),
            score: confidence,
            vector_score: None,
            lexical_score: None,
            graph_score: None,
            structural_score: None,
            confidence,
            trust_score: None,
            node_type: "article".to_string(),
            title: None,
            content_preview: String::new(),
            domain_path: None,
            expanded_from: None,
            graph_hops: None,
            created_at: None,
            topological_score: None,
            facet_function: None,
            facet_scope: None,
        }
    }

    #[test]
    fn confidence_dist_empty() {
        let dist = compute_confidence_distribution(&[]);
        assert_eq!(dist.min, 0.0);
        assert_eq!(dist.max, 0.0);
        assert_eq!(dist.mean, 0.0);
        assert_eq!(dist.median, 0.0);
    }

    #[test]
    fn confidence_dist_single() {
        let results = vec![make_result(0.7)];
        let dist = compute_confidence_distribution(&results);
        assert!((dist.min - 0.7).abs() < 1e-9);
        assert!((dist.max - 0.7).abs() < 1e-9);
        assert!((dist.mean - 0.7).abs() < 1e-9);
        assert!((dist.median - 0.7).abs() < 1e-9);
    }

    #[test]
    fn confidence_dist_even_count() {
        let results = vec![
            make_result(0.2),
            make_result(0.4),
            make_result(0.6),
            make_result(0.8),
        ];
        let dist = compute_confidence_distribution(&results);
        assert!((dist.min - 0.2).abs() < 1e-9);
        assert!((dist.max - 0.8).abs() < 1e-9);
        assert!((dist.mean - 0.5).abs() < 1e-9);
        // median of [0.2, 0.4, 0.6, 0.8] → (0.4 + 0.6) / 2 = 0.5
        assert!((dist.median - 0.5).abs() < 1e-9);
    }

    #[test]
    fn confidence_dist_odd_count() {
        let results = vec![make_result(0.3), make_result(0.5), make_result(0.9)];
        let dist = compute_confidence_distribution(&results);
        assert!((dist.min - 0.3).abs() < 1e-9);
        assert!((dist.max - 0.9).abs() < 1e-9);
        // mean = (0.3 + 0.5 + 0.9) / 3 = 1.7 / 3 ≈ 0.5667
        assert!((dist.mean - (1.7 / 3.0)).abs() < 1e-9);
        // median of [0.3, 0.5, 0.9] → 0.5
        assert!((dist.median - 0.5).abs() < 1e-9);
    }
}

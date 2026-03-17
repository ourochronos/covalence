//! Coverage analysis, erosion detection, and whitespace roadmap.

use crate::error::Result;

use super::AnalysisService;

/// A single coverage item (orphaned code or unimplemented spec).
#[derive(Debug, Clone, serde::Serialize)]
pub struct CoverageItem {
    /// Node UUID.
    pub node_id: uuid::Uuid,
    /// Node name.
    pub name: String,
    /// Node type.
    pub node_type: String,
    /// File path (for code nodes).
    pub file_path: Option<String>,
    /// Why this item is flagged.
    pub reason: String,
}

/// Result of coverage analysis.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CoverageResult {
    /// Code nodes with no path to any Component.
    pub orphan_code: Vec<CoverageItem>,
    /// Spec/concept nodes with no IMPLEMENTS_INTENT edges.
    pub unimplemented_specs: Vec<CoverageItem>,
    /// Fraction of spec topics that have implementation coverage.
    pub coverage_score: f64,
}

/// A component with its erosion metric.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ErosionItem {
    /// Component node UUID.
    pub component_id: uuid::Uuid,
    /// Component name.
    pub component_name: String,
    /// Component description (design intent).
    pub spec_intent: String,
    /// Drift score: 1 - mean(cosine similarity) of child code nodes.
    pub drift_score: f64,
    /// Code nodes that diverge most from the component's intent.
    pub divergent_nodes: Vec<DivergentNode>,
}

/// A code node that diverges from its parent component's intent.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DivergentNode {
    /// Code node UUID.
    pub node_id: uuid::Uuid,
    /// Code node name.
    pub name: String,
    /// Semantic summary of the code node.
    pub summary: Option<String>,
    /// Cosine distance from the component's embedding.
    pub distance: f64,
}

/// Result of erosion detection.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ErosionResult {
    /// Components with drift above the threshold.
    pub eroded_components: Vec<ErosionItem>,
    /// Total components analyzed.
    pub total_components: u64,
}

/// A node representative in a whitespace gap.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WhitespaceNode {
    /// Node name.
    pub name: String,
    /// Node type.
    pub node_type: String,
}

/// A research cluster with no bridge edges to any Component.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WhitespaceGap {
    /// Source UUID.
    pub source_id: uuid::Uuid,
    /// Source title.
    pub title: String,
    /// Source URI.
    pub uri: Option<String>,
    /// Number of entities extracted from this source.
    pub node_count: u64,
    /// Representative entity names from the source.
    pub representative_nodes: Vec<WhitespaceNode>,
    /// Components connected via any bridge edge.
    pub connected_components: Vec<String>,
    /// Spec topics connected via IMPLEMENTS_INTENT.
    pub connected_spec_topics: Vec<String>,
    /// Human-readable gap assessment.
    pub assessment: String,
}

/// Result of whitespace roadmap analysis.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WhitespaceResult {
    /// Research clusters with no bridge edges.
    pub gaps: Vec<WhitespaceGap>,
    /// Total research sources analyzed.
    pub total_research_sources: u64,
    /// Sources with zero bridge edges.
    pub unbridged_sources: u64,
    /// Fraction of research sources that are unbridged.
    pub whitespace_score: f64,
}

impl AnalysisService {
    // ------------------------------------------------------------------
    // Capability 6: Coverage Analysis
    // ------------------------------------------------------------------

    /// Detect orphaned code (no Component parent) and unimplemented specs
    /// (spec concepts with no IMPLEMENTS_INTENT edges).
    pub async fn coverage_analysis(&self) -> Result<CoverageResult> {
        // Orphan code: code-class nodes from code-domain sources with no
        // PART_OF_COMPONENT edge. Uses entity_class and domain fields
        // instead of hardcoded type lists and source_type checks.
        let orphan_rows: Vec<(uuid::Uuid, String, String, String)> = sqlx::query_as(
            "SELECT n.id, n.canonical_name, n.node_type, \
                    COALESCE(n.properties->>'file_path', '') \
             FROM nodes n \
             WHERE n.entity_class = 'code' \
               AND EXISTS ( \
                 SELECT 1 FROM extractions ex \
                 JOIN chunks c ON ex.chunk_id = c.id \
                 JOIN sources s ON c.source_id = s.id \
                 WHERE ex.entity_id = n.id \
                   AND s.domain = 'code' \
               ) \
               AND NOT EXISTS ( \
                 SELECT 1 FROM edges e \
                 WHERE e.source_node_id = n.id \
                   AND e.rel_type = 'PART_OF_COMPONENT' \
               ) \
             ORDER BY n.canonical_name \
             LIMIT 200",
        )
        .fetch_all(self.repo.pool())
        .await?;

        let orphan_code: Vec<CoverageItem> = orphan_rows
            .into_iter()
            .map(|(id, name, ntype, path)| CoverageItem {
                node_id: id,
                name,
                node_type: ntype,
                file_path: if path.is_empty() { None } else { Some(path) },
                reason: "No PART_OF_COMPONENT edge to any Component".to_string(),
            })
            .collect();

        // Unimplemented specs: domain-class nodes from spec/design sources
        // with no inbound IMPLEMENTS_INTENT edges.
        let unimpl_rows: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(
            "SELECT DISTINCT n.id, n.canonical_name, n.node_type \
             FROM nodes n \
             JOIN extractions ex ON ex.entity_id = n.id \
             JOIN chunks c ON ex.chunk_id = c.id \
             JOIN sources s ON c.source_id = s.id \
             WHERE n.entity_class = 'domain' \
               AND s.domain IN ('spec', 'design') \
               AND NOT EXISTS ( \
                 SELECT 1 FROM edges e \
                 WHERE e.target_node_id = n.id \
                   AND e.rel_type = 'IMPLEMENTS_INTENT' \
               ) \
             ORDER BY n.canonical_name \
             LIMIT 200",
        )
        .fetch_all(self.repo.pool())
        .await?;

        let unimplemented_specs: Vec<CoverageItem> = unimpl_rows
            .into_iter()
            .map(|(id, name, ntype)| CoverageItem {
                node_id: id,
                name,
                node_type: ntype,
                file_path: None,
                reason: "Spec concept with no IMPLEMENTS_INTENT edge".to_string(),
            })
            .collect();

        // Coverage score: (spec concepts with implementation / total spec
        // concepts).
        let total_spec: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT n.id) \
             FROM nodes n \
             JOIN extractions ex ON ex.entity_id = n.id \
             JOIN chunks c ON ex.chunk_id = c.id \
             JOIN sources s ON c.source_id = s.id \
             WHERE n.entity_class = 'domain' \
               AND s.domain IN ('spec', 'design')",
        )
        .fetch_one(self.repo.pool())
        .await?;

        let implemented: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT n.id) \
             FROM nodes n \
             JOIN extractions ex ON ex.entity_id = n.id \
             JOIN chunks c ON ex.chunk_id = c.id \
             JOIN sources s ON c.source_id = s.id \
             WHERE n.entity_class = 'domain' \
               AND s.domain IN ('spec', 'design') \
               AND EXISTS ( \
                 SELECT 1 FROM edges e \
                 WHERE e.target_node_id = n.id \
                   AND e.rel_type = 'IMPLEMENTS_INTENT' \
               )",
        )
        .fetch_one(self.repo.pool())
        .await?;

        let coverage_score = if total_spec > 0 {
            implemented as f64 / total_spec as f64
        } else {
            0.0
        };

        tracing::info!(
            orphan_code = orphan_code.len(),
            unimplemented_specs = unimplemented_specs.len(),
            coverage_score,
            total_spec,
            implemented,
            "coverage analysis complete"
        );

        Ok(CoverageResult {
            orphan_code,
            unimplemented_specs,
            coverage_score,
        })
    }

    // ------------------------------------------------------------------
    // Capability 2: Architecture Erosion Detection
    // ------------------------------------------------------------------

    /// Detect components where code has drifted from design intent.
    ///
    /// For each Component with an embedding, compute the mean cosine
    /// distance between the component's embedding and all code nodes
    /// linked via PART_OF_COMPONENT. Components above the threshold are
    /// flagged.
    pub async fn detect_erosion(&self, threshold: f64) -> Result<ErosionResult> {
        // Fetch all component nodes with embeddings.
        let components: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(
            "SELECT id, canonical_name, COALESCE(description, '') \
             FROM nodes \
             WHERE node_type = 'component' AND embedding IS NOT NULL",
        )
        .fetch_all(self.repo.pool())
        .await?;

        let total_components = components.len() as u64;
        let mut eroded = Vec::new();

        for (comp_id, comp_name, description) in &components {
            // Find all code nodes linked to this component via
            // PART_OF_COMPONENT that have embeddings.
            let code_nodes: Vec<(uuid::Uuid, String, Option<String>, f64)> = sqlx::query_as(
                "SELECT n.id, n.canonical_name, \
                        n.properties->>'semantic_summary', \
                        (n.embedding <=> (SELECT embedding FROM nodes WHERE id = $1)) AS dist \
                 FROM nodes n \
                 JOIN edges e ON e.source_node_id = n.id \
                 WHERE e.target_node_id = $1 \
                   AND e.rel_type = 'PART_OF_COMPONENT' \
                   AND n.embedding IS NOT NULL \
                 ORDER BY dist DESC",
            )
            .bind(comp_id)
            .fetch_all(self.repo.pool())
            .await?;

            if code_nodes.is_empty() {
                continue;
            }

            let avg_dist: f64 =
                code_nodes.iter().map(|(_, _, _, d)| d).sum::<f64>() / code_nodes.len() as f64;
            let drift_score = avg_dist; // 0 = perfect alignment, 1 = orthogonal

            if drift_score < threshold {
                continue;
            }

            // Top 5 most divergent nodes.
            let divergent_nodes: Vec<DivergentNode> = code_nodes
                .iter()
                .take(5)
                .map(|(id, name, summary, dist)| DivergentNode {
                    node_id: *id,
                    name: name.clone(),
                    summary: summary.clone(),
                    distance: *dist,
                })
                .collect();

            eroded.push(ErosionItem {
                component_id: *comp_id,
                component_name: comp_name.clone(),
                spec_intent: description.clone(),
                drift_score,
                divergent_nodes,
            });
        }

        eroded.sort_by(|a, b| {
            b.drift_score
                .partial_cmp(&a.drift_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        tracing::info!(
            total = total_components,
            eroded = eroded.len(),
            threshold,
            "erosion detection complete"
        );

        Ok(ErosionResult {
            eroded_components: eroded,
            total_components,
        })
    }

    // ------------------------------------------------------------------
    // Capability 3: Whitespace Roadmap (Gap Analysis)
    // ------------------------------------------------------------------

    /// Maximum research gaps to return.
    const MAX_WHITESPACE_GAPS: usize = 50;

    /// Detect research areas with no corresponding implementation.
    ///
    /// Groups research-domain nodes by their source article and checks
    /// whether any node in each group is connected to a Component via
    /// THEORETICAL_BASIS edges. Sources with zero bridges are
    /// "whitespace" — theory we've studied but haven't acted on.
    pub async fn whitespace_roadmap(
        &self,
        min_cluster_size: usize,
        domain_filter: Option<&str>,
    ) -> Result<WhitespaceResult> {
        // Find research sources using the domain field instead of URI heuristics.
        let rows: Vec<(uuid::Uuid, String, Option<String>, i64, i64)> = sqlx::query_as(
            "SELECT s.id, s.title, s.uri, \
                    COUNT(DISTINCT n.id) AS node_count, \
                    COUNT(DISTINCT CASE WHEN e.id IS NOT NULL THEN n.id END) AS bridged \
             FROM sources s \
             JOIN chunks c ON c.source_id = s.id \
             JOIN extractions ex ON ex.chunk_id = c.id \
             JOIN nodes n ON n.id = ex.entity_id \
             LEFT JOIN edges e ON e.target_node_id = n.id \
                               AND e.rel_type = 'THEORETICAL_BASIS' \
             WHERE s.domain IN ('research', 'external') \
               AND ($3::text IS NULL \
                    OR LOWER(s.title) LIKE '%' || LOWER($3::text) || '%' \
                    OR LOWER(COALESCE(s.uri, '')) LIKE '%' || LOWER($3::text) || '%') \
             GROUP BY s.id, s.title, s.uri \
             HAVING COUNT(DISTINCT n.id) >= $1 \
             ORDER BY COUNT(DISTINCT n.id) DESC \
             LIMIT $2",
        )
        .bind(min_cluster_size as i64)
        .bind(Self::MAX_WHITESPACE_GAPS as i64)
        .bind(domain_filter)
        .fetch_all(self.repo.pool())
        .await?;

        let mut gaps = Vec::new();
        let mut total_research = 0u64;
        let mut total_unbridged = 0u64;

        for (source_id, title, uri, node_count, bridged_count) in &rows {
            // Domain filter is now applied in SQL ($3 parameter), so
            // all rows here already match the filter.
            total_research += 1;

            if *bridged_count > 0 {
                continue; // This source has at least one bridge edge.
            }

            total_unbridged += 1;

            // Fetch representative node names for this source.
            let node_names: Vec<(String, String)> = sqlx::query_as(
                "SELECT DISTINCT n.canonical_name, n.node_type \
                 FROM nodes n \
                 JOIN extractions ex ON ex.entity_id = n.id \
                 JOIN chunks c ON c.id = ex.chunk_id \
                 WHERE c.source_id = $1 \
                 ORDER BY n.canonical_name \
                 LIMIT 10",
            )
            .bind(source_id)
            .fetch_all(self.repo.pool())
            .await?;

            // Check for IMPLEMENTS_INTENT connections (spec coverage).
            let spec_connected: Vec<(String,)> = sqlx::query_as(
                "SELECT DISTINCT comp.canonical_name \
                 FROM nodes comp \
                 JOIN edges e ON e.source_node_id = comp.id \
                 WHERE e.rel_type = 'IMPLEMENTS_INTENT' \
                   AND comp.node_type = 'component' \
                   AND e.target_node_id IN ( \
                     SELECT n.id FROM nodes n \
                     JOIN extractions ex ON ex.entity_id = n.id \
                     JOIN chunks c ON c.id = ex.chunk_id \
                     WHERE c.source_id = $1 \
                   )",
            )
            .bind(source_id)
            .fetch_all(self.repo.pool())
            .await?;

            // Find Component nodes connected via THEORETICAL_BASIS
            // to any entity extracted from this source.
            let comp_connected: Vec<(String,)> = sqlx::query_as(
                "SELECT DISTINCT comp.canonical_name \
                 FROM nodes comp \
                 JOIN edges e ON e.source_node_id = comp.id \
                 WHERE e.rel_type = 'THEORETICAL_BASIS' \
                   AND comp.node_type = 'component' \
                   AND e.target_node_id IN ( \
                     SELECT n.id FROM nodes n \
                     JOIN extractions ex ON ex.entity_id = n.id \
                     JOIN chunks c ON c.id = ex.chunk_id \
                     WHERE c.source_id = $1 \
                   )",
            )
            .bind(source_id)
            .fetch_all(self.repo.pool())
            .await?;

            gaps.push(WhitespaceGap {
                source_id: *source_id,
                title: title.clone(),
                uri: uri.clone(),
                node_count: *node_count as u64,
                representative_nodes: node_names
                    .into_iter()
                    .map(|(name, ntype)| WhitespaceNode {
                        name,
                        node_type: ntype,
                    })
                    .collect(),
                connected_components: comp_connected.into_iter().map(|(name,)| name).collect(),
                connected_spec_topics: spec_connected.into_iter().map(|(name,)| name).collect(),
                assessment: if *node_count > 10 {
                    format!(
                        "Dense research cluster ({} entities) with zero \
                         THEORETICAL_BASIS bridge edges to any component.",
                        node_count
                    )
                } else {
                    format!(
                        "{} entities with no bridge edges to any component.",
                        node_count
                    )
                },
            });
        }

        // Sort by node count descending (densest unbridged clusters first).
        gaps.sort_by(|a, b| b.node_count.cmp(&a.node_count));

        let whitespace_score = if total_research > 0 {
            total_unbridged as f64 / total_research as f64
        } else {
            0.0
        };

        tracing::info!(
            total_research,
            total_unbridged,
            whitespace_score = format!("{:.1}%", whitespace_score * 100.0),
            gaps = gaps.len(),
            "whitespace roadmap analysis complete"
        );

        Ok(WhitespaceResult {
            gaps,
            total_research_sources: total_research,
            unbridged_sources: total_unbridged,
            whitespace_score,
        })
    }
}

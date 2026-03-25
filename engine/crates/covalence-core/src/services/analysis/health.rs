//! Coverage analysis, erosion detection, and whitespace roadmap.

use crate::error::Result;
use crate::storage::traits::AnalysisRepo;

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
        let orphan_rows = AnalysisRepo::get_orphan_code_nodes(&*self.repo).await?;

        let orphan_code: Vec<CoverageItem> = orphan_rows
            .into_iter()
            .map(|(id, name, ntype, path)| CoverageItem {
                node_id: id,
                name,
                node_type: ntype,
                file_path: if path.is_empty() { None } else { Some(path) },
                reason: format!(
                    "No {} edge to any Component",
                    self.bridges.part_of_component
                ),
            })
            .collect();

        // Unimplemented specs: domain-class nodes whose PRIMARY domain
        // is spec or design, with no inbound IMPLEMENTS_INTENT edges.
        // Uses primary_domain (from domain_entropy computation) to avoid
        // counting cross-cutting concepts that happen to be mentioned in
        // specs (e.g., "gpt-4o-mini", "MRL-E" are primarily research).
        let unimpl_rows = AnalysisRepo::get_unimplemented_specs(&*self.repo).await?;

        let unimplemented_specs: Vec<CoverageItem> = unimpl_rows
            .into_iter()
            .map(|(id, name, ntype)| CoverageItem {
                node_id: id,
                name,
                node_type: ntype,
                file_path: None,
                reason: format!(
                    "Spec concept with no {} edge",
                    self.bridges.implements_intent
                ),
            })
            .collect();

        // Coverage score: (spec concepts with implementation / total spec
        // concepts).
        let total_spec: i64 = AnalysisRepo::count_spec_concepts(&*self.repo).await?;

        let implemented: i64 = AnalysisRepo::count_implemented_specs(&*self.repo).await?;

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
        let components = AnalysisRepo::list_component_nodes(&*self.repo).await?;

        let total_components = components.len() as u64;
        let mut eroded = Vec::new();

        for (comp_id, comp_name, description) in &components {
            // Find all code nodes linked to this component via
            // PART_OF_COMPONENT that have embeddings.
            let code_nodes = AnalysisRepo::find_component_code_nodes(
                &*self.repo,
                *comp_id,
                &self.bridges.part_of_component,
            )
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
        let rows = AnalysisRepo::get_research_source_bridges(
            &*self.repo,
            min_cluster_size as i64,
            Self::MAX_WHITESPACE_GAPS as i64,
            domain_filter,
        )
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
            let node_names =
                AnalysisRepo::list_source_representative_nodes(&*self.repo, *source_id).await?;

            // Check for IMPLEMENTS_INTENT connections (spec coverage).
            let spec_connected = AnalysisRepo::find_connected_components(
                &*self.repo,
                *source_id,
                &self.bridges.implements_intent,
            )
            .await?;

            // Find Component nodes connected via THEORETICAL_BASIS
            // to any entity extracted from this source.
            let comp_connected = AnalysisRepo::find_connected_components(
                &*self.repo,
                *source_id,
                &self.bridges.theoretical_basis,
            )
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
                         {} bridge edges to any component.",
                        node_count, self.bridges.theoretical_basis
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

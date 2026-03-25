//! Health checks and data health reporting.

use crate::error::{Error, Result};

use super::AdminService;

/// Health status of the system.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HealthStatus {
    /// Whether the database is reachable.
    pub pg_healthy: bool,
    /// Number of nodes in the sidecar.
    pub sidecar_node_count: usize,
    /// Number of edges in the sidecar.
    pub sidecar_edge_count: usize,
}

/// Data health report — preview of what's stale, orphaned, or
/// duplicated without modifying anything.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DataHealthReport {
    /// Sources superseded by newer versions.
    pub superseded_sources: u64,
    /// Chunks belonging to superseded sources.
    pub superseded_chunks: u64,
    /// Nodes with no extraction provenance.
    pub orphan_nodes: u64,
    /// Orphan nodes that still have edges (load-bearing).
    pub orphan_nodes_with_edges: u64,
    /// Duplicate sources (same title, same domain).
    pub duplicate_sources: u64,
    /// Nodes with no embedding.
    pub unembedded_nodes: u64,
    /// Code entities missing semantic summaries.
    pub unsummarized_code_entities: u64,
    /// Sources missing summaries.
    pub unsummarized_sources: u64,
}

impl AdminService {
    /// Check system health.
    pub async fn health(&self) -> HealthStatus {
        let pg_healthy = sqlx::query("SELECT 1")
            .execute(self.repo.pool())
            .await
            .is_ok();

        let sidecar_node_count = self.graph.node_count().await.unwrap_or(0);
        let sidecar_edge_count = self.graph.edge_count().await.unwrap_or(0);
        HealthStatus {
            pg_healthy,
            sidecar_node_count,
            sidecar_edge_count,
        }
    }

    /// Run a full configuration audit.
    ///
    /// Checks sidecar health, summarizes the current configuration,
    /// and generates warnings for potential issues. Returns
    /// `Error::Config` if no configuration has been set.
    pub async fn config_audit(&self) -> Result<super::super::health::ConfigAudit> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| Error::Config("no configuration set on AdminService".into()))?;
        Ok(super::super::health::run_config_audit(config).await)
    }

    /// Preview data health — shows what's stale, orphaned, or
    /// duplicated without modifying anything.
    pub async fn data_health_report(&self) -> Result<DataHealthReport> {
        let row: (i64, i64, i64, i64, i64, i64, i64, i64) =
            sqlx::query_as("SELECT * FROM sp_data_health_report()")
                .fetch_one(self.repo.pool())
                .await?;

        Ok(DataHealthReport {
            superseded_sources: row.0 as u64,
            superseded_chunks: row.1 as u64,
            orphan_nodes: row.2 as u64,
            orphan_nodes_with_edges: row.3 as u64,
            duplicate_sources: row.4 as u64,
            unembedded_nodes: row.5 as u64,
            unsummarized_code_entities: row.6 as u64,
            unsummarized_sources: row.7 as u64,
        })
    }
}

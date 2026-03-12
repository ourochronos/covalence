//! Outbox-based sync between PostgreSQL and the petgraph sidecar.
//!
//! Listens for `graph_sync_ping` NOTIFY events and polls the `outbox_events`
//! table for changes. 5-second polling fallback ensures no events are missed.

use std::time::Duration;

use serde::Deserialize;
use sqlx::Row;
use uuid::Uuid;

use crate::error::Result;
use crate::types::causal::CausalLevel;

use super::sidecar::{EdgeMeta, GraphSidecar, NodeMeta, SharedGraph};

/// A single change record from the outbox_events table.
#[derive(Debug, Clone, Deserialize)]
pub struct OutboxEvent {
    /// Monotonically increasing sequence ID.
    pub seq_id: i64,
    /// Entity type: "node" or "edge".
    pub entity_type: String,
    /// The UUID of the affected entity.
    pub entity_id: Uuid,
    /// Operation: "INSERT", "UPDATE", or "DELETE".
    pub operation: String,
    /// JSON payload with the entity's current state (None for DELETE).
    pub payload: Option<serde_json::Value>,
}

/// Run the sync loop: listen for NOTIFY pings, poll outbox for changes.
///
/// This function runs indefinitely. It combines LISTEN/NOTIFY for low-latency
/// wake-up with a 5-second polling fallback.
pub async fn sync_loop(pool: &sqlx::PgPool, graph: SharedGraph) -> Result<()> {
    let mut last_seq: i64 = 0;
    let mut listener = sqlx::postgres::PgListener::connect_with(pool).await?;
    listener.listen("graph_sync_ping").await?;

    loop {
        // Wait for a NOTIFY ping or timeout after 5 seconds
        let _ = tokio::time::timeout(Duration::from_secs(5), listener.recv()).await;

        let rows = sqlx::query(
            "SELECT seq_id, entity_type, entity_id, operation, payload \
             FROM outbox_events \
             WHERE seq_id > $1 \
             ORDER BY seq_id ASC \
             LIMIT 1000",
        )
        .bind(last_seq)
        .fetch_all(pool)
        .await?;

        if !rows.is_empty() {
            let count = rows.len();
            let mut g = graph.write().await;

            for row in &rows {
                let event = OutboxEvent {
                    seq_id: row.get("seq_id"),
                    entity_type: row.get("entity_type"),
                    entity_id: row.get("entity_id"),
                    operation: row.get("operation"),
                    payload: row.get("payload"),
                };
                g.apply_event(&event);
                last_seq = event.seq_id;
            }

            tracing::debug!(count, last_seq, "applied outbox events");
        }
    }
}

/// Load all nodes and edges from PostgreSQL into the graph sidecar.
///
/// Used on startup and for admin-triggered full reloads.
pub async fn full_reload(pool: &sqlx::PgPool, graph: SharedGraph) -> Result<()> {
    // Fetch all nodes
    let node_rows = sqlx::query(
        "SELECT id, COALESCE(canonical_type, node_type) AS node_type, \
         canonical_name, clearance_level FROM nodes",
    )
    .fetch_all(pool)
    .await?;

    // Fetch all edges
    let edge_rows = sqlx::query(
        "SELECT id, source_node_id, target_node_id, \
         COALESCE(canonical_rel_type, rel_type) AS rel_type, \
         weight, confidence, clearance_level, is_synthetic, \
         properties->>'causal_level' as causal_level \
         FROM edges",
    )
    .fetch_all(pool)
    .await?;

    let mut g = graph.write().await;
    // Clear existing graph
    *g = GraphSidecar::new();

    // Insert nodes
    let mut node_errors = 0usize;
    for row in &node_rows {
        if let Err(e) = g.add_node(NodeMeta {
            id: row.get("id"),
            node_type: row.get("node_type"),
            canonical_name: row.get("canonical_name"),
            clearance_level: row.get("clearance_level"),
        }) {
            tracing::warn!(error = %e, "failed to add node during full reload");
            node_errors += 1;
        }
    }

    // Insert edges (add_edge populates both the graph and edge_index)
    let mut edge_errors = 0usize;
    for row in &edge_rows {
        let causal_str: Option<String> = row.get("causal_level");
        let causal_level = causal_str.as_deref().and_then(CausalLevel::from_str_opt);

        if let Err(e) = g.add_edge(
            row.get("source_node_id"),
            row.get("target_node_id"),
            EdgeMeta {
                id: row.get("id"),
                rel_type: row.get("rel_type"),
                weight: row.get("weight"),
                confidence: row.get("confidence"),
                causal_level,
                clearance_level: row.get("clearance_level"),
                is_synthetic: row.get("is_synthetic"),
            },
        ) {
            tracing::warn!(error = %e, "failed to add edge during full reload");
            edge_errors += 1;
        }
    }

    if node_errors > 0 || edge_errors > 0 {
        tracing::warn!(
            node_errors,
            edge_errors,
            "full reload completed with errors"
        );
    }

    tracing::info!(
        nodes = g.node_count(),
        edges = g.edge_count(),
        "full graph reload complete"
    );

    Ok(())
}

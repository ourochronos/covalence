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
///
/// **Debounce behavior:** When events arrive in rapid succession (e.g.,
/// during bulk ingestion), the loop buffers them and only applies once
/// events stop arriving for `SETTLE_SECS`. This prevents the graph
/// sidecar from thrashing during bulk operations.
pub async fn sync_loop(pool: &sqlx::PgPool, graph: SharedGraph) -> Result<()> {
    /// How long to wait with no new events before applying the batch.
    const SETTLE_SECS: u64 = 10;
    /// How long to wait between polls when idle.
    const POLL_SECS: u64 = 5;
    /// Max events to buffer before forcing an apply (prevents unbounded memory).
    const MAX_BUFFER: usize = 10_000;

    let mut last_seq: i64 = 0;
    let mut listener = sqlx::postgres::PgListener::connect_with(pool).await?;
    listener.listen("graph_sync_ping").await?;

    let mut buffer: Vec<OutboxEvent> = Vec::new();
    let mut last_event_time = std::time::Instant::now();

    loop {
        // Wait for a NOTIFY ping or timeout
        let _ = tokio::time::timeout(Duration::from_secs(POLL_SECS), listener.recv()).await;

        // Fetch new events via stored procedure.
        let rows = sqlx::query("SELECT * FROM sp_poll_outbox_events($1, $2)")
            .bind(last_seq)
            .bind(1000_i32)
            .fetch_all(pool)
            .await?;

        if !rows.is_empty() {
            for row in &rows {
                let event = OutboxEvent {
                    seq_id: row.get("seq_id"),
                    entity_type: row.get("entity_type"),
                    entity_id: row.get("entity_id"),
                    operation: row.get("operation"),
                    payload: row.get("payload"),
                };
                last_seq = event.seq_id;
                buffer.push(event);
            }
            last_event_time = std::time::Instant::now();
        }

        // Apply buffered events when settled (no new events for SETTLE_SECS)
        // or when the buffer is full.
        let settled = last_event_time.elapsed() >= Duration::from_secs(SETTLE_SECS);
        let buffer_full = buffer.len() >= MAX_BUFFER;

        if !buffer.is_empty() && (settled || buffer_full) {
            let count = buffer.len();
            let mut g = graph.write().await;

            for event in buffer.drain(..) {
                g.apply_event(&event);
            }

            tracing::info!(
                count,
                last_seq,
                settled,
                buffer_full,
                "applied buffered outbox events"
            );
        }
    }
}

/// Load all nodes and edges from PostgreSQL into the graph sidecar.
///
/// Used on startup and for admin-triggered full reloads.
pub async fn full_reload(pool: &sqlx::PgPool, graph: SharedGraph) -> Result<()> {
    // Fetch all nodes via stored procedure.
    let node_rows = sqlx::query("SELECT * FROM sp_load_all_nodes()")
        .fetch_all(pool)
        .await?;

    // Fetch all non-invalidated edges via stored procedure.
    let edge_rows = sqlx::query("SELECT * FROM sp_load_all_edges()")
        .fetch_all(pool)
        .await?;

    let mut g = graph.write().await;
    // Clear existing graph
    *g = GraphSidecar::new();

    // Insert nodes (SP returns canonical_type = COALESCE(canonical_type, node_type))
    let mut node_errors = 0usize;
    for row in &node_rows {
        if let Err(e) = g.add_node(NodeMeta {
            id: row.get("id"),
            node_type: row.get("canonical_type"),
            entity_class: row.get("entity_class"),
            canonical_name: row.get("canonical_name"),
            clearance_level: row.get("clearance_level"),
        }) {
            tracing::warn!(error = %e, "failed to add node during full reload");
            node_errors += 1;
        }
    }

    // Insert edges (add_edge populates both the graph and edge_index).
    // SP returns canonical_rel_type = COALESCE(canonical_rel_type, rel_type)
    // and causal_level as INT (0 = none/association, 1 = intervention,
    // 2 = counterfactual).
    let mut edge_errors = 0usize;
    for row in &edge_rows {
        let causal_int: i32 = row.get("causal_level");
        let causal_level = match causal_int {
            1 => Some(CausalLevel::Intervention),
            2 => Some(CausalLevel::Counterfactual),
            _ => None,
        };

        if let Err(e) = g.add_edge(
            row.get("source_node_id"),
            row.get("target_node_id"),
            EdgeMeta {
                id: row.get("id"),
                rel_type: row.get("canonical_rel_type"),
                weight: row.get("weight"),
                confidence: row.get("confidence"),
                causal_level,
                clearance_level: row.get("clearance_level"),
                is_synthetic: row.get("is_synthetic"),
                has_valid_from: row.get("has_valid_from"),
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

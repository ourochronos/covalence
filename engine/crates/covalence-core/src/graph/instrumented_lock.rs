//! Instrumented lock acquisition for the graph sidecar.
//!
//! Thin wrappers around `SharedGraph` read/write lock acquisition that
//! measure wait time and emit tracing events. Useful for detecting
//! contention without changing the lock strategy.

use std::time::Instant;

use tokio::sync::{RwLockReadGuard, RwLockWriteGuard};

use super::sidecar::{GraphSidecar, SharedGraph};

/// Acquire a read lock on the graph sidecar with timing instrumentation.
///
/// Logs at `warn` level if acquisition takes longer than 10 ms,
/// otherwise logs at `trace`.
pub async fn read_graph<'a>(
    graph: &'a SharedGraph,
    caller: &str,
) -> RwLockReadGuard<'a, GraphSidecar> {
    let start = Instant::now();
    let guard = graph.read().await;
    let elapsed_ms = start.elapsed().as_millis() as u64;

    if elapsed_ms > 10 {
        tracing::warn!(caller, elapsed_ms, "slow graph lock acquisition");
    } else {
        tracing::trace!(caller, elapsed_ms, "graph lock acquired");
    }

    guard
}

/// Acquire a write lock on the graph sidecar with timing instrumentation.
///
/// Logs at `warn` level if acquisition takes longer than 10 ms,
/// otherwise logs at `trace`.
pub async fn write_graph<'a>(
    graph: &'a SharedGraph,
    caller: &str,
) -> RwLockWriteGuard<'a, GraphSidecar> {
    let start = Instant::now();
    let guard = graph.write().await;
    let elapsed_ms = start.elapsed().as_millis() as u64;

    if elapsed_ms > 10 {
        tracing::warn!(caller, elapsed_ms, "slow graph lock acquisition");
    } else {
        tracing::trace!(caller, elapsed_ms, "graph lock acquired");
    }

    guard
}

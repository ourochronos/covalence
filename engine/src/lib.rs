//! Library entry-point that exposes internal modules for integration testing.
//! The binary entry-point remains `main.rs`.
//!
//! Only modules that are self-contained (no `crate::errors` / `crate::graph`
//! deps) are re-exported here so that integration tests can import them via
//! `covalence_engine::…` without pulling in the full binary-only dep graph.

pub mod models;
pub mod search;
pub mod worker;

/// Selective service re-exports for integration tests.
///
/// Only `search_service` is declared here; the other services depend on
/// `crate::errors` and `crate::graph` which are binary-only modules.
pub mod services {
    pub mod search_service;
}

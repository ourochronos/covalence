//! Covalence core library.
//!
//! Provides the domain model, storage layer, graph algorithms, search fusion,
//! ingestion pipeline, epistemic model, and consolidation logic for the
//! Covalence knowledge engine.

pub mod config;
pub mod config_loader;
pub mod consolidation;
pub mod epistemic;
pub mod error;
pub mod factory;
pub mod graph;
pub mod ingestion;
pub mod metrics;
pub mod models;
pub mod search;
pub mod services;
pub mod storage;
pub mod types;

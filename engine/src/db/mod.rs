//! Database access helpers — thin async wrappers around sqlx queries.
//!
//! Each sub-module owns the SQL for one table.  All functions receive a
//! `&PgPool` so callers can share a single pool across the application.

pub mod edge_causal_metadata;

pub use edge_causal_metadata as ecm;

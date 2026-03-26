//! Extension system -- loads community-shareable domain packs.
//!
//! Each extension is defined by an `extension.yaml` manifest that
//! declares ontology additions (entity types, relationship types,
//! domains, view edges, noise patterns), domain classification rules,
//! alignment rules, lifecycle hooks, and optional external service
//! definitions.
//!
//! The [`ExtensionLoader`] scans a directory of extension subdirectories
//! at startup and seeds/updates the database tables with their
//! declarations.  All inserts use `ON CONFLICT DO NOTHING` so loading
//! is idempotent.

pub mod loader;
pub mod manifest;
pub mod metadata;

pub use loader::{ExtensionLoader, LoadResult};
pub use manifest::ExtensionManifest;
pub use metadata::EnforcementLevel;

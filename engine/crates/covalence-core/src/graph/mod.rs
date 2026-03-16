//! Graph compute layer — in-memory petgraph sidecar and algorithms.
//!
//! The [`GraphEngine`] trait abstracts graph operations so callers
//! don't depend on a specific backend (petgraph, Apache AGE, etc.).

pub mod algorithms;
pub mod bridges;
pub mod community;
pub mod engine;
pub mod filtered;
pub mod petgraph_engine;
pub mod sidecar;
pub mod sync;
pub mod topology;
pub mod traversal;

pub use bridges::Bridge;
pub use community::Community;
pub use engine::{
    BfsNode, BfsOptions, Contention, GapCandidate, GraphEngine, GraphStats, Neighbor, ReloadResult,
};
pub use petgraph_engine::PetgraphEngine;
pub use sidecar::{EdgeMeta, GraphSidecar, NodeMeta, SharedGraph};
pub use sync::OutboxEvent;
pub use topology::{Domain, DomainLink, TopologyMap};

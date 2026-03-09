//! Graph compute layer — in-memory petgraph sidecar and algorithms.

pub mod algorithms;
pub mod bridges;
pub mod community;
pub mod filtered;
pub mod sidecar;
pub mod sync;
pub mod topology;
pub mod traversal;

pub use bridges::Bridge;
pub use community::Community;
pub use sidecar::{EdgeMeta, GraphSidecar, NodeMeta, SharedGraph};
pub use sync::OutboxEvent;
pub use topology::{Domain, DomainLink, TopologyMap};

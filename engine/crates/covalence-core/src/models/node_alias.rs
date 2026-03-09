//! NodeAlias model — alternative names that resolve to a canonical node.

use serde::{Deserialize, Serialize};

use crate::types::ids::{AliasId, ChunkId, NodeId};

/// An alternative name that resolves to a canonical node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeAlias {
    pub id: AliasId,
    pub node_id: NodeId,
    pub alias: String,
    pub source_chunk_id: Option<ChunkId>,
}

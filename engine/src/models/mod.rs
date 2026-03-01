use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    Article,
    Source,
    Entity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EdgeType {
    // Provenance
    Supersedes,
    SplitFrom,
    CompiledFrom,
    Confirms,
    Contradicts,
    Contends,
    // Semantic
    RelatesTo,
    Elaborates,
    Generalizes,
    // Temporal
    Precedes,
    Follows,
    // Entity
    Involves,
}

impl EdgeType {
    /// Edge types that affect what counts as "current" (chain tip calculation).
    pub fn is_versioning_edge(&self) -> bool {
        matches!(self, EdgeType::Supersedes | EdgeType::SplitFrom)
    }

    pub fn as_label(&self) -> &'static str {
        match self {
            EdgeType::Supersedes => "SUPERSEDES",
            EdgeType::SplitFrom => "SPLIT_FROM",
            EdgeType::CompiledFrom => "COMPILED_FROM",
            EdgeType::Confirms => "CONFIRMS",
            EdgeType::Contradicts => "CONTRADICTS",
            EdgeType::Contends => "CONTENDS",
            EdgeType::RelatesTo => "RELATES_TO",
            EdgeType::Elaborates => "ELABORATES",
            EdgeType::Generalizes => "GENERALIZES",
            EdgeType::Precedes => "PRECEDES",
            EdgeType::Follows => "FOLLOWS",
            EdgeType::Involves => "INVOLVES",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Confidence {
    pub overall: f32,
    pub source: f32,
    pub method: f32,
    pub consistency: f32,
    pub freshness: f32,
    pub corroboration: f32,
    pub applicability: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: Uuid,
    pub node_type: NodeType,
    pub title: Option<String>,
    pub content: Option<String>,
    pub status: String,
    pub confidence: Option<Confidence>,
    pub epistemic_type: Option<String>,
    pub domain_path: Vec<String>,
    pub metadata: serde_json::Value,
    pub version: i32,
    pub usage_score: f32,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: Uuid,
    pub source_node_id: Uuid,
    pub target_node_id: Uuid,
    pub edge_type: EdgeType,
    pub weight: f32,
    pub confidence: f32,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

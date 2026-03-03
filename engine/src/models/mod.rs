use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

// =============================================================================
// Node types
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    Article,
    Source,
    Session,
    Entity, // v1 — label exists in AGE but not used in v0
}

impl NodeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeType::Article => "article",
            NodeType::Source => "source",
            NodeType::Session => "session",
            NodeType::Entity => "entity",
        }
    }

    /// AGE vertex label name.
    pub fn age_label(&self) -> &'static str {
        match self {
            NodeType::Article => "Article",
            NodeType::Source => "Source",
            NodeType::Session => "Session",
            NodeType::Entity => "Entity",
        }
    }
}

impl fmt::Display for NodeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// =============================================================================
// Edge types — canonical vocabulary per spec §5.2 + 002 migration
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EdgeType {
    // ── Provenance ──────────────────────────────────────────────
    Originates,  // source directly contributed to article compilation
    Confirms,    // source corroborates an existing article
    Supersedes,  // this node replaces another (directional: new→old)
    Contradicts, // conflicting claims
    Contends,    // softer disagreement / alternative interpretation
    Extends,     // elaborates without superseding
    DerivesFrom, // article derived from another article
    MergedFrom,  // article produced by merging parents
    SplitInto,   // article was divided into fragments
    SplitFrom,   // fragment produced by splitting a parent

    // ── Temporal ────────────────────────────────────────────────
    Precedes,       // temporally before
    Follows,        // temporally after
    ConcurrentWith, // overlapping time periods

    // ── Causal / Logical (slow-path inferred) ──────────────────
    Causes,      // LLM-inferred causal relationship
    MotivatedBy, // decision motivated by this knowledge
    Implements,  // concrete artifact implements abstract concept

    // ── Semantic ────────────────────────────────────────────────
    RelatesTo,   // generic semantic relatedness
    Generalizes, // abstracts a more specific node

    // ── Session / Entity ────────────────────────────────────────
    CapturedIn, // source captured during session
    Involves,   // node references a named entity

    // ── Legacy aliases (from 001 schema, retained for migration) ─
    CompiledFrom, // alias for ORIGINATES
    Elaborates,   // alias for EXTENDS
}

impl EdgeType {
    /// The AGE edge label string (used in Cypher queries).
    pub fn as_label(&self) -> &'static str {
        match self {
            EdgeType::Originates => "ORIGINATES",
            EdgeType::Confirms => "CONFIRMS",
            EdgeType::Supersedes => "SUPERSEDES",
            EdgeType::Contradicts => "CONTRADICTS",
            EdgeType::Contends => "CONTENDS",
            EdgeType::Extends => "EXTENDS",
            EdgeType::DerivesFrom => "DERIVES_FROM",
            EdgeType::MergedFrom => "MERGED_FROM",
            EdgeType::SplitInto => "SPLIT_INTO",
            EdgeType::SplitFrom => "SPLIT_FROM",
            EdgeType::Precedes => "PRECEDES",
            EdgeType::Follows => "FOLLOWS",
            EdgeType::ConcurrentWith => "CONCURRENT_WITH",
            EdgeType::Causes => "CAUSES",
            EdgeType::MotivatedBy => "MOTIVATED_BY",
            EdgeType::Implements => "IMPLEMENTS",
            EdgeType::RelatesTo => "RELATES_TO",
            EdgeType::Generalizes => "GENERALIZES",
            EdgeType::CapturedIn => "CAPTURED_IN",
            EdgeType::Involves => "INVOLVES",
            EdgeType::CompiledFrom => "COMPILED_FROM",
            EdgeType::Elaborates => "ELABORATES",
        }
    }

    /// Edge types that affect what counts as "current" (chain tip calculation).
    #[allow(dead_code)]
    pub fn is_versioning_edge(&self) -> bool {
        matches!(
            self,
            EdgeType::Supersedes | EdgeType::SplitFrom | EdgeType::SplitInto | EdgeType::MergedFrom
        )
    }

    /// Provenance edges — used in provenance chain walks.
    #[allow(dead_code)]
    pub fn is_provenance_edge(&self) -> bool {
        matches!(
            self,
            EdgeType::Originates
                | EdgeType::CompiledFrom
                | EdgeType::Confirms
                | EdgeType::Supersedes
                | EdgeType::DerivesFrom
                | EdgeType::MergedFrom
                | EdgeType::SplitInto
                | EdgeType::SplitFrom
                | EdgeType::Extends
                | EdgeType::Elaborates
        )
    }

    /// Temporal edges — prioritized for temporal intent queries.
    #[allow(dead_code)]
    pub fn is_temporal_edge(&self) -> bool {
        matches!(
            self,
            EdgeType::Precedes | EdgeType::Follows | EdgeType::ConcurrentWith
        )
    }

    /// Causal edges — prioritized for causal intent queries.
    #[allow(dead_code)]
    pub fn is_causal_edge(&self) -> bool {
        matches!(
            self,
            EdgeType::Causes | EdgeType::MotivatedBy | EdgeType::Implements
        )
    }

    /// Resolve legacy aliases to canonical names.
    #[allow(dead_code)]
    pub fn canonical(&self) -> EdgeType {
        match self {
            EdgeType::CompiledFrom => EdgeType::Originates,
            EdgeType::Elaborates => EdgeType::Extends,
            other => *other,
        }
    }
}

impl fmt::Display for EdgeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_label())
    }
}

impl FromStr for EdgeType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ORIGINATES" => Ok(EdgeType::Originates),
            "CONFIRMS" => Ok(EdgeType::Confirms),
            "SUPERSEDES" => Ok(EdgeType::Supersedes),
            "CONTRADICTS" => Ok(EdgeType::Contradicts),
            "CONTENDS" => Ok(EdgeType::Contends),
            "EXTENDS" => Ok(EdgeType::Extends),
            "DERIVES_FROM" => Ok(EdgeType::DerivesFrom),
            "MERGED_FROM" => Ok(EdgeType::MergedFrom),
            "SPLIT_INTO" => Ok(EdgeType::SplitInto),
            "SPLIT_FROM" => Ok(EdgeType::SplitFrom),
            "PRECEDES" => Ok(EdgeType::Precedes),
            "FOLLOWS" => Ok(EdgeType::Follows),
            "CONCURRENT_WITH" => Ok(EdgeType::ConcurrentWith),
            "CAUSES" => Ok(EdgeType::Causes),
            "MOTIVATED_BY" => Ok(EdgeType::MotivatedBy),
            "IMPLEMENTS" => Ok(EdgeType::Implements),
            "RELATES_TO" => Ok(EdgeType::RelatesTo),
            "GENERALIZES" => Ok(EdgeType::Generalizes),
            "CAPTURED_IN" => Ok(EdgeType::CapturedIn),
            "INVOLVES" => Ok(EdgeType::Involves),
            "COMPILED_FROM" => Ok(EdgeType::CompiledFrom),
            "ELABORATES" => Ok(EdgeType::Elaborates),
            other => Err(format!("unknown edge type: {other}")),
        }
    }
}

// =============================================================================
// Node status
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Active,
    Archived,
    Tombstone,
}

impl NodeStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeStatus::Active => "active",
            NodeStatus::Archived => "archived",
            NodeStatus::Tombstone => "tombstone",
        }
    }
}

impl fmt::Display for NodeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// =============================================================================
// Traversal direction (for neighborhood queries)
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraversalDirection {
    Outbound,
    Inbound,
    Both,
}

// =============================================================================
// Confidence — single canonical representation per node (SPEC §6.2)
// =============================================================================

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Confidence {
    pub overall: f32,
    pub source: f32,
    pub method: f32,
    pub consistency: f32,
    pub freshness: f32,
    pub corroboration: f32,
    pub applicability: f32,
}

impl Default for Confidence {
    fn default() -> Self {
        Self {
            overall: 0.5,
            source: 0.5,
            method: 0.5,
            consistency: 1.0,
            freshness: 1.0,
            corroboration: 0.0,
            applicability: 1.0,
        }
    }
}

impl Confidence {
    /// Recompute overall from components.
    /// Formula: weighted mean matching Valence v2's proven weighting.
    #[allow(dead_code)]
    pub fn recompute_overall(&mut self) {
        self.overall = self.source * 0.30
            + self.method * 0.15
            + self.consistency * 0.20
            + self.freshness * 0.10
            + self.corroboration * 0.15
            + self.applicability * 0.10;
    }
}

// =============================================================================
// Epistemic type
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicType {
    Semantic,
    Episodic,
    Procedural,
    Declarative,
}

// =============================================================================
// Domain models
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: Uuid,
    pub age_id: Option<i64>,
    pub node_type: NodeType,
    pub title: Option<String>,
    pub content: Option<String>,
    pub status: NodeStatus,
    pub confidence: Confidence,
    pub epistemic_type: Option<EpistemicType>,
    pub domain_path: Vec<String>,
    pub metadata: serde_json::Value,
    pub source_type: Option<String>,
    pub reliability: Option<f32>,
    pub content_hash: Option<String>,
    pub fingerprint: Option<String>,
    pub size_tokens: Option<i32>,
    pub pinned: bool,
    pub version: i32,
    pub usage_score: f32,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    pub accessed_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: Uuid,
    pub age_id: Option<i64>,
    pub source_node_id: Uuid,
    pub target_node_id: Uuid,
    pub edge_type: EdgeType,
    pub weight: f32,
    pub confidence: f32,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<String>,
}

/// Result from a graph traversal — a node with its relationship context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNeighbor {
    pub node: Node,
    pub edge: Edge,
    pub depth: u32,
}

/// A link in a provenance chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceLink {
    pub source_node: Node,
    pub edge_type: EdgeType,
    pub confidence: f32,
    pub depth: u32,
}

/// Search intent for intent-aware retrieval routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchIntent {
    Factual,
    Temporal,
    Causal,
    Entity,
}

impl SearchIntent {
    /// Edge types prioritized for this intent in the graph dimension.
    #[allow(dead_code)]
    pub fn priority_edges(&self) -> &'static [EdgeType] {
        match self {
            SearchIntent::Factual => &[EdgeType::Confirms, EdgeType::Originates],
            SearchIntent::Temporal => &[
                EdgeType::Precedes,
                EdgeType::Follows,
                EdgeType::ConcurrentWith,
            ],
            SearchIntent::Causal => &[
                EdgeType::Causes,
                EdgeType::MotivatedBy,
                EdgeType::Implements,
            ],
            SearchIntent::Entity => &[EdgeType::Involves, EdgeType::CapturedIn],
        }
    }
}

/// Dimension weights for score fusion (SPEC §7.3).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DimensionWeights {
    pub vector: f32,
    pub lexical: f32,
    pub graph: f32,
}

impl Default for DimensionWeights {
    fn default() -> Self {
        Self {
            vector: 0.50,
            lexical: 0.30,
            graph: 0.20,
        }
    }
}

impl DimensionWeights {
    /// Normalize so weights sum to 1.0.
    #[allow(dead_code)]
    pub fn normalize(&mut self) {
        let sum = self.vector + self.lexical + self.graph;
        if sum > 0.0 {
            self.vector /= sum;
            self.lexical /= sum;
            self.graph /= sum;
        }
    }
}

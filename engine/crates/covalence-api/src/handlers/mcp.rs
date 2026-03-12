//! MCP (Model Context Protocol) tool interface handlers.

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

/// Definition of an MCP tool.
#[derive(Debug, Serialize, ToSchema)]
pub struct McpTool {
    /// Tool name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema describing the tool's input parameters.
    pub input_schema: serde_json::Value,
}

/// Request to call an MCP tool.
#[derive(Debug, Deserialize, ToSchema)]
pub struct McpToolCall {
    /// Name of the tool to invoke.
    pub name: String,
    /// Arguments as a JSON object.
    pub arguments: serde_json::Value,
}

/// Result from an MCP tool call.
#[derive(Debug, Serialize, ToSchema)]
pub struct McpToolResult {
    /// Content blocks in the result.
    pub content: Vec<McpContent>,
    /// Whether the tool call resulted in an error.
    pub is_error: bool,
}

/// A single content block in an MCP result.
#[derive(Debug, Serialize, ToSchema)]
pub struct McpContent {
    /// Content type (typically "text").
    #[serde(rename = "type")]
    pub content_type: String,
    /// Text payload (usually JSON-serialized data).
    pub text: String,
}

/// List all available MCP tools.
#[utoipa::path(
    post,
    path = "/mcp/tools/list",
    responses(
        (status = 200, description = "Available tools", body = Vec<McpTool>),
    ),
    tag = "mcp"
)]
pub async fn list_tools_handler() -> Json<Vec<McpTool>> {
    Json(tool_definitions())
}

/// Call an MCP tool by name.
#[utoipa::path(
    post,
    path = "/mcp/tools/call",
    request_body = McpToolCall,
    responses(
        (status = 200, description = "Tool result", body = McpToolResult),
    ),
    tag = "mcp"
)]
pub async fn call_tool(
    State(state): State<AppState>,
    Json(call): Json<McpToolCall>,
) -> Result<Json<McpToolResult>, ApiError> {
    let result = dispatch(&state, &call).await;
    match result {
        Ok(value) => Ok(Json(McpToolResult {
            content: vec![McpContent {
                content_type: "text".to_string(),
                text: serde_json::to_string(&value)
                    .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}).to_string()),
            }],
            is_error: false,
        })),
        Err(msg) => Ok(Json(McpToolResult {
            content: vec![McpContent {
                content_type: "text".to_string(),
                text: msg,
            }],
            is_error: true,
        })),
    }
}

/// Dispatch a tool call to the appropriate service method.
async fn dispatch(state: &AppState, call: &McpToolCall) -> Result<serde_json::Value, String> {
    match call.name.as_str() {
        "search" => dispatch_search(state, &call.arguments).await,
        "get_node" => dispatch_get_node(state, &call.arguments).await,
        "get_provenance" => dispatch_get_provenance(state, &call.arguments).await,
        "ingest_source" => dispatch_ingest_source(state, &call.arguments).await,
        "traverse" => dispatch_traverse(state, &call.arguments).await,
        "resolve_entity" => dispatch_resolve_entity(state, &call.arguments).await,
        "list_communities" => dispatch_list_communities(state).await,
        "get_contradictions" => dispatch_get_contradictions(state, &call.arguments).await,
        "memory_store" => dispatch_memory_store(state, &call.arguments).await,
        "memory_recall" => dispatch_memory_recall(state, &call.arguments).await,
        "memory_forget" => dispatch_memory_forget(state, &call.arguments).await,
        _ => Err(format!("unknown tool: {}", call.name)),
    }
}

async fn dispatch_search(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or("missing required parameter: query")?;

    if query.trim().is_empty() {
        return Err("query must not be empty".to_string());
    }

    let strategy = match args.get("strategy").and_then(|v| v.as_str()) {
        Some("balanced") => covalence_core::search::strategy::SearchStrategy::Balanced,
        Some("precise") => covalence_core::search::strategy::SearchStrategy::Precise,
        Some("exploratory") => covalence_core::search::strategy::SearchStrategy::Exploratory,
        Some("recent") => covalence_core::search::strategy::SearchStrategy::Recent,
        Some("graph_first") => covalence_core::search::strategy::SearchStrategy::GraphFirst,
        Some("global") => covalence_core::search::strategy::SearchStrategy::Global,
        _ => covalence_core::search::strategy::SearchStrategy::Auto,
    };

    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v.min(200) as usize)
        .unwrap_or(10);

    let results = state
        .search_service
        .search(query, strategy, limit, None)
        .await
        .map_err(|e| e.to_string())?;

    Ok(json!(
        results
            .into_iter()
            .map(|r| json!({
                "id": r.id,
                "fused_score": r.fused_score,
                "confidence": r.confidence,
                "entity_type": r.entity_type,
                "name": r.name,
                "snippet": r.snippet,
                "dimension_scores": r.dimension_scores,
                "dimension_ranks": r.dimension_ranks,
            }))
            .collect::<Vec<_>>()
    ))
}

async fn dispatch_get_node(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let id = parse_uuid_arg(args, "id")?;

    let node = state
        .node_service
        .get(id.into())
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("node not found: {id}"))?;

    Ok(json!({
        "id": node.id.into_uuid(),
        "canonical_name": node.canonical_name,
        "node_type": node.node_type,
        "description": node.description,
        "properties": node.properties,
        "clearance_level": node.clearance_level.as_i32(),
        "first_seen": node.first_seen.to_rfc3339(),
        "last_seen": node.last_seen.to_rfc3339(),
        "mention_count": node.mention_count,
    }))
}

async fn dispatch_get_provenance(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let node_id = parse_uuid_arg(args, "node_id")?;

    let chain = state
        .node_service
        .provenance(node_id.into())
        .await
        .map_err(|e| e.to_string())?;

    Ok(json!({
        "node_id": chain.node_id.into_uuid(),
        "extraction_count": chain.extractions.len(),
        "chunk_count": chain.chunks.len(),
        "source_count": chain.sources.len(),
    }))
}

async fn dispatch_ingest_source(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("missing required parameter: content")?;

    let source_type = args
        .get("source_type")
        .and_then(|v| v.as_str())
        .ok_or("missing required parameter: source_type")?;

    let mime = args
        .get("mime")
        .and_then(|v| v.as_str())
        .unwrap_or("text/plain");

    let id = state
        .source_service
        .ingest(
            content.as_bytes(),
            source_type,
            mime,
            None,
            serde_json::Value::Object(Default::default()),
        )
        .await
        .map_err(|e| e.to_string())?;

    Ok(json!({ "source_id": id.into_uuid() }))
}

async fn dispatch_traverse(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let start_node_id = parse_uuid_arg(args, "start_node_id")?;

    let hops = args
        .get("hops")
        .and_then(|v| v.as_u64())
        .map(|v| v.min(10) as usize)
        .unwrap_or(1);

    let nodes = state
        .node_service
        .neighborhood(start_node_id.into(), hops)
        .await
        .map_err(|e| e.to_string())?;

    Ok(json!(
        nodes
            .into_iter()
            .map(|n| json!({
                "id": n.id.into_uuid(),
                "canonical_name": n.canonical_name,
                "node_type": n.node_type,
                "description": n.description,
            }))
            .collect::<Vec<_>>()
    ))
}

async fn dispatch_resolve_entity(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("missing required parameter: name")?;

    let node = state
        .node_service
        .resolve(name)
        .await
        .map_err(|e| e.to_string())?;

    match node {
        Some(n) => Ok(json!({
            "id": n.id.into_uuid(),
            "canonical_name": n.canonical_name,
            "node_type": n.node_type,
            "description": n.description,
        })),
        None => Ok(json!(null)),
    }
}

async fn dispatch_list_communities(state: &AppState) -> Result<serde_json::Value, String> {
    let graph = state.graph.read().await;
    let communities = covalence_core::graph::community::detect_communities(graph.graph());

    Ok(json!(
        communities
            .into_iter()
            .map(|c| json!({
                "id": c.id,
                "node_ids": c.node_ids,
                "label": c.label,
                "coherence": c.coherence,
            }))
            .collect::<Vec<_>>()
    ))
}

async fn dispatch_get_contradictions(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let graph = state.graph.read().await;
    let contentions = covalence_core::consolidation::contention::detect_contentions(graph.graph());

    let node_filter = match args.get("node_id").and_then(|v| v.as_str()) {
        Some(s) => Some(
            s.parse::<Uuid>()
                .map_err(|_| format!("invalid UUID for node_id: {s}"))?,
        ),
        None => None,
    };

    let filtered: Vec<_> = match node_filter {
        Some(nid) => contentions
            .into_iter()
            .filter(|c| c.node_a == nid || c.node_b == nid)
            .collect(),
        None => contentions,
    };

    Ok(json!({
        "contradictions": filtered.iter().map(|c| json!({
            "node_a": c.node_a,
            "node_b": c.node_b,
            "edge_id": c.edge_id,
            "rel_type": c.rel_type,
            "confidence": c.confidence,
        })).collect::<Vec<_>>()
    }))
}

async fn dispatch_memory_store(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("missing required parameter: content")?;

    if content.trim().is_empty() {
        return Err("content must not be empty".to_string());
    }

    let topic = args.get("topic").and_then(|v| v.as_str());

    let metadata = match topic {
        Some(t) => json!({ "topic": t }),
        None => json!({}),
    };

    let source_id = state
        .source_service
        .ingest(
            content.as_bytes(),
            "observation",
            "text/plain",
            None,
            metadata,
        )
        .await
        .map_err(|e| e.to_string())?;

    Ok(json!({ "id": source_id.into_uuid().to_string(), "status": "stored" }))
}

async fn dispatch_memory_recall(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or("missing required parameter: query")?;

    if query.trim().is_empty() {
        return Err("query must not be empty".to_string());
    }

    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v.min(200) as usize)
        .unwrap_or(10);

    let topic = args.get("topic").and_then(|v| v.as_str());
    let min_confidence = args.get("min_confidence").and_then(|v| v.as_f64());

    // Prepend topic to query for topical relevance boost.
    let effective_query = match topic {
        Some(t) => format!("[{t}] {query}"),
        None => query.to_string(),
    };

    let filters = if min_confidence.is_some() {
        Some(covalence_core::services::search::SearchFilters {
            min_confidence,
            node_types: None,
            date_range: None,
            source_types: None,
        })
    } else {
        None
    };

    let results = state
        .search_service
        .search(
            &effective_query,
            covalence_core::search::strategy::SearchStrategy::Auto,
            limit,
            filters,
        )
        .await
        .map_err(|e| e.to_string())?;

    Ok(json!(
        results
            .into_iter()
            .map(|r| json!({
                "id": r.id,
                "content": r.snippet.unwrap_or_default(),
                "relevance": r.fused_score,
                "confidence": r.confidence.unwrap_or(1.0),
                "entity_type": r.entity_type,
                "name": r.name,
            }))
            .collect::<Vec<_>>()
    ))
}

async fn dispatch_memory_forget(
    state: &AppState,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let id = parse_uuid_arg(args, "id")?;

    let result = state
        .source_service
        .delete(id.into())
        .await
        .map_err(|e| e.to_string())?;

    Ok(json!({
        "deleted": result.deleted,
        "id": id.to_string(),
        "chunks_deleted": result.chunks_deleted,
    }))
}

/// Parse a UUID from a named argument.
fn parse_uuid_arg(args: &serde_json::Value, name: &str) -> Result<Uuid, String> {
    let raw = args
        .get(name)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing required parameter: {name}"))?;

    raw.parse::<Uuid>()
        .map_err(|_| format!("invalid UUID for {name}: {raw}"))
}

/// Build the list of MCP tool definitions.
fn tool_definitions() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "search".to_string(),
            description: "Search the knowledge graph using \
                multi-dimensional fused search."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query text"
                    },
                    "strategy": {
                        "type": "string",
                        "enum": [
                            "balanced",
                            "precise",
                            "exploratory",
                            "recent",
                            "graph_first",
                            "global"
                        ],
                        "description": "Search strategy (default: balanced)"
                    },
                    "limit": {
                        "type": "number",
                        "description": "Maximum number of results \
                            (default: 10)"
                    }
                },
                "required": ["query"]
            }),
        },
        McpTool {
            name: "get_node".to_string(),
            description: "Get a knowledge graph node by its UUID.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "format": "uuid",
                        "description": "Node UUID"
                    }
                },
                "required": ["id"]
            }),
        },
        McpTool {
            name: "get_provenance".to_string(),
            description: "Get the provenance chain for a node, \
                showing extraction, chunk, and source links."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "node_id": {
                        "type": "string",
                        "format": "uuid",
                        "description": "Node UUID"
                    }
                },
                "required": ["node_id"]
            }),
        },
        McpTool {
            name: "ingest_source".to_string(),
            description: "Ingest a new source into the knowledge \
                graph."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "Raw text content to ingest"
                    },
                    "source_type": {
                        "type": "string",
                        "description": "Source type (document, web_page, \
                            conversation, code, api, manual, tool_output, \
                            observation)"
                    },
                    "mime": {
                        "type": "string",
                        "description": "MIME type (default: text/plain)"
                    }
                },
                "required": ["content", "source_type"]
            }),
        },
        McpTool {
            name: "traverse".to_string(),
            description: "Traverse the graph from a starting node, \
                returning its neighborhood."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "start_node_id": {
                        "type": "string",
                        "format": "uuid",
                        "description": "Starting node UUID"
                    },
                    "hops": {
                        "type": "number",
                        "description": "Number of hops (default: 1)"
                    }
                },
                "required": ["start_node_id"]
            }),
        },
        McpTool {
            name: "resolve_entity".to_string(),
            description: "Resolve an entity name to a node, \
                returning null if not found."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Entity name to resolve"
                    }
                },
                "required": ["name"]
            }),
        },
        McpTool {
            name: "list_communities".to_string(),
            description: "List detected communities in the \
                knowledge graph."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpTool {
            name: "get_contradictions".to_string(),
            description: "Get contradictions involving a node \
                or across the entire graph."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "node_id": {
                        "type": "string",
                        "format": "uuid",
                        "description": "Optional node UUID to scope \
                            contradictions"
                    }
                }
            }),
        },
        McpTool {
            name: "memory_store".to_string(),
            description: "Store a memory in the knowledge engine.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "The memory content to store"
                    },
                    "topic": {
                        "type": "string",
                        "description": "Optional topic/category for \
                            the memory"
                    }
                },
                "required": ["content"]
            }),
        },
        McpTool {
            name: "memory_recall".to_string(),
            description: "Recall memories matching a query using \
                semantic search."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The recall query text"
                    },
                    "limit": {
                        "type": "number",
                        "description": "Maximum memories to return \
                            (default: 10)"
                    },
                    "topic": {
                        "type": "string",
                        "description": "Optional topic filter"
                    }
                },
                "required": ["query"]
            }),
        },
        McpTool {
            name: "memory_forget".to_string(),
            description: "Forget (delete) a memory by its ID.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "format": "uuid",
                        "description": "Memory UUID to forget"
                    }
                },
                "required": ["id"]
            }),
        },
    ]
}

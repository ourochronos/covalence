//! Ask service — LLM-powered knowledge synthesis over the graph.
//!
//! Takes a natural language question, searches across all dimensions
//! to gather relevant context, enriches it with provenance and
//! confidence metadata, sends it to an LLM for grounded synthesis,
//! and returns a structured answer with citations.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Result;
use crate::ingestion::ChatBackend;
use crate::ingestion::chat_backend::CliChatBackend;
use crate::models::node::{EntityClass, derive_entity_class};
use crate::models::session::Turn;
use crate::search::fusion::FusedResult;
use crate::search::strategy::SearchStrategy;
use crate::services::SearchService;
use crate::services::adapter_service::AdapterService;
use crate::services::hooks::HookService;
use crate::services::search::SearchFilters;
use crate::storage::postgres::PgRepo;

/// Options controlling ask behavior.
#[derive(Debug, Clone)]
pub struct AskOptions {
    /// Maximum search results to include as context (default 15).
    pub max_context: usize,
    /// Search strategy. Default: auto.
    pub strategy: Option<String>,
    /// Override the LLM model for this request.
    /// Format: "haiku", "sonnet", "opus", "gemini", "copilot".
    /// If None, uses the default configured backend.
    pub model: Option<String>,
    /// Optional session ID for multi-turn conversation context.
    /// When set, the last 10 turns are injected into the prompt
    /// and the question + answer are recorded as new turns.
    pub session_id: Option<Uuid>,
    /// Optional adapter ID to scope lifecycle hooks to a specific
    /// adapter. When set, only hooks registered for this adapter
    /// (or global hooks) are fired.
    pub adapter_id: Option<Uuid>,
}

impl Default for AskOptions {
    fn default() -> Self {
        Self {
            max_context: 15,
            strategy: None,
            model: None,
            session_id: None,
            adapter_id: None,
        }
    }
}

/// A citation from the knowledge graph backing the answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    /// Source name or URI.
    pub source: String,
    /// Relevant snippet from the source.
    pub snippet: String,
    /// Result type (chunk, statement, section, node, etc.).
    pub result_type: String,
    /// Confidence score.
    pub confidence: f64,
}

/// Structured response from the ask endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskResponse {
    /// The synthesized answer.
    pub answer: String,
    /// Citations from the knowledge graph.
    pub citations: Vec<Citation>,
    /// Number of search results used as context.
    pub context_used: usize,
}

/// A Server-Sent Event from the streaming ask endpoint.
///
/// Each variant is tagged with a `type` field for SSE routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AskStreamEvent {
    /// Emitted first: search context metadata and citations.
    Context {
        /// Number of context fragments used.
        context_used: usize,
        /// Citations from the knowledge graph.
        citations: Vec<Citation>,
    },
    /// A token (or line) of the synthesized answer.
    Token {
        /// The text content of this token/chunk.
        text: String,
    },
    /// The stream is complete.
    Done {
        /// Which LLM provider generated the answer.
        provider: String,
    },
    /// An error occurred during streaming.
    Error {
        /// Human-readable error description.
        message: String,
    },
}

/// Maximum number of code entities to enrich with graph edges.
const MAX_GRAPH_ENRICHED_ENTITIES: usize = 5;

/// Maximum number of edges to include per direction per entity.
const MAX_EDGES_PER_DIRECTION: usize = 10;

/// Edge relationship types to look up in outgoing direction.
const OUTGOING_REL_TYPES: &[&str] = &["calls", "uses_type", "contains"];

/// Edge relationship types to look up in incoming direction
/// (converted to `_by` suffix in display).
const INCOMING_REL_TYPES: &[&str] = &["calls", "contains"];

/// Service for answering questions via LLM synthesis over graph search.
pub struct AskService {
    search: Arc<SearchService>,
    chat: Arc<dyn ChatBackend>,
    /// Database repo for provenance enrichment and graph edge lookups.
    repo: Arc<PgRepo>,
    /// Optional lifecycle hook service for pipeline extensibility.
    hooks: Option<Arc<HookService>>,
    /// Optional session service for multi-turn conversation context.
    sessions: Option<Arc<super::SessionService>>,
    /// Optional adapter service for adapter-driven defaults.
    adapters: Option<Arc<AdapterService>>,
}

impl AskService {
    /// Create a new ask service.
    pub fn new(search: Arc<SearchService>, chat: Arc<dyn ChatBackend>, repo: Arc<PgRepo>) -> Self {
        Self {
            search,
            chat,
            repo,
            hooks: None,
            sessions: None,
            adapters: None,
        }
    }

    /// Attach a lifecycle hook service for pipeline extensibility.
    pub fn with_hooks(mut self, hooks: Arc<HookService>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// Wire a session service for multi-turn conversation support.
    pub fn with_sessions(mut self, sessions: Arc<super::SessionService>) -> Self {
        self.sessions = Some(sessions);
        self
    }

    /// Wire an adapter service for adapter-driven default strategy.
    pub fn with_adapters(mut self, adapters: Arc<AdapterService>) -> Self {
        self.adapters = Some(adapters);
        self
    }

    /// Resolve the search strategy: explicit user choice > adapter
    /// default > "auto".
    async fn resolve_strategy(
        &self,
        user_strategy: Option<&str>,
        adapter_id: Option<Uuid>,
    ) -> SearchStrategy {
        // 1. Explicit user strategy wins.
        if let Some(s) = user_strategy {
            if s != "auto" {
                return parse_strategy(Some(s));
            }
        }

        // 2. Adapter default strategy.
        if let (Some(aid), Some(svc)) = (adapter_id, &self.adapters) {
            use crate::storage::traits::AdapterRepo;
            if let Ok(Some(adapter)) = AdapterRepo::find_by_id(svc.repo(), aid).await {
                if let Some(ref default_strat) = adapter.default_search_strategy {
                    let parsed = parse_strategy(Some(default_strat));
                    tracing::info!(
                        adapter_id = %aid,
                        strategy = %default_strat,
                        "using adapter default search strategy"
                    );
                    return parsed;
                }
            }
        }

        // 3. Fall back to auto.
        SearchStrategy::Auto
    }

    /// Answer a question by searching, enriching, and synthesizing.
    pub async fn ask(&self, question: &str, options: AskOptions) -> Result<AskResponse> {
        let adapter_id = options.adapter_id;

        // 0a. Pre-search hooks: let external systems enrich the query.
        let (effective_query, hook_filters) = if let Some(ref hooks) = self.hooks {
            let pre = hooks.fire_pre_search(question, adapter_id).await?;
            let boosted = if let Some(ref terms) = pre.boost_terms {
                if !terms.is_empty() {
                    let q = format!("{} {}", question, terms.join(" "));
                    tracing::info!(
                        boost_terms = ?terms,
                        "pre_search hook enriched query with boost terms"
                    );
                    q
                } else {
                    question.to_string()
                }
            } else {
                question.to_string()
            };
            let filters = pre.metadata_filters.as_ref().map(build_filters_from_hook);
            (boosted, filters)
        } else {
            (question.to_string(), None)
        };

        // 0b. Load conversation history if a session is active.
        let history = if let (Some(sid), Some(svc)) = (options.session_id, &self.sessions) {
            svc.get_history(sid, Some(10)).await.unwrap_or_default()
        } else {
            Vec::new()
        };

        // 1. Search for context (using the boost-enriched query).
        let strategy = self
            .resolve_strategy(options.strategy.as_deref(), adapter_id)
            .await;
        let results = self
            .search
            .search(
                &effective_query,
                strategy,
                options.max_context,
                hook_filters,
            )
            .await?;

        if results.is_empty() {
            let answer = "No relevant information found in the knowledge \
                         graph to answer this question."
                .to_string();

            // Record turns even for empty results so the conversation
            // stays coherent.
            if let (Some(sid), Some(svc)) = (options.session_id, &self.sessions) {
                let _ = svc.add_turn(sid, "user", question, None).await;
                let _ = svc.add_turn(sid, "assistant", &answer, None).await;
            }

            return Ok(AskResponse {
                answer,
                citations: Vec::new(),
                context_used: 0,
            });
        }

        // 2. Post-search hooks: let external systems inject
        //    additional context before synthesis.
        let extra_context = if let Some(ref hooks) = self.hooks {
            let summary = format!("{} results found", results.len());
            let post = hooks
                .fire_post_search(question, &summary, adapter_id)
                .await?;
            post.additional_context.unwrap_or_default()
        } else {
            Vec::new()
        };

        // 3. Enrich with provenance and build context blocks.
        let mut context_blocks = self.build_context_blocks(&results).await;

        // Append hook-injected context as extra blocks.
        for (i, ctx_text) in extra_context.iter().enumerate() {
            context_blocks.push(ContextBlock {
                number: context_blocks.len() + 1,
                result_type: "hook_context".to_string(),
                confidence: 1.0,
                source_title: format!("hook_context_{}", i + 1),
                source_uri: String::new(),
                source_domain: "external".to_string(),
                snippet: ctx_text.clone(),
                _fused_score: 0.0,
                graph_edges: None,
            });
        }
        let context_used = context_blocks.len();

        // 4. Build the grounded prompt (with optional conversation history).
        let system_prompt = build_system_prompt();
        let user_prompt = build_user_prompt_with_history(question, &context_blocks, &history);

        // 5. Call LLM (use per-request model override if specified).
        let backend: Arc<dyn ChatBackend> = if let Some(ref model) = options.model {
            Arc::new(resolve_model_backend(model))
        } else {
            Arc::clone(&self.chat)
        };
        let chat_resp = backend
            .chat(&system_prompt, &user_prompt, false, 0.3)
            .await?;
        let answer = chat_resp.text;

        // 6. Build citations from the search results.
        let citations = build_citations(&results, &context_blocks);

        // 7. Post-synthesis hook: fire-and-forget notification.
        if let Some(ref hooks) = self.hooks {
            let citation_values: Vec<serde_json::Value> = citations
                .iter()
                .filter_map(|c| serde_json::to_value(c).ok())
                .collect();
            hooks.fire_post_synthesis(
                question.to_string(),
                answer.clone(),
                citation_values,
                adapter_id,
            );
        }

        // 8. Record turns if a session is active.
        if let (Some(sid), Some(svc)) = (options.session_id, &self.sessions) {
            let _ = svc.add_turn(sid, "user", question, None).await;
            let _ = svc.add_turn(sid, "assistant", &answer, None).await;
        }

        Ok(AskResponse {
            answer,
            citations,
            context_used,
        })
    }

    /// Stream an answer as Server-Sent Events.
    ///
    /// Performs the same search + context enrichment as [`ask()`],
    /// but streams the LLM synthesis token-by-token instead of
    /// buffering the full response.
    ///
    /// Event order: `Context` (once) -> `Token` (many) -> `Done`.
    /// On error the stream yields an `Error` event and terminates.
    pub async fn ask_stream(
        &self,
        question: &str,
        options: AskOptions,
    ) -> Result<Pin<Box<dyn Stream<Item = AskStreamEvent> + Send>>> {
        use futures::StreamExt;

        let adapter_id = options.adapter_id;

        // 0a. Pre-search hooks: enrich query with boost terms.
        let (effective_query, hook_filters) = if let Some(ref hooks) = self.hooks {
            let pre = hooks.fire_pre_search(question, adapter_id).await?;
            let boosted = if let Some(ref terms) = pre.boost_terms {
                if !terms.is_empty() {
                    let q = format!("{} {}", question, terms.join(" "));
                    tracing::info!(
                        boost_terms = ?terms,
                        "pre_search hook enriched query with boost terms"
                    );
                    q
                } else {
                    question.to_string()
                }
            } else {
                question.to_string()
            };
            let filters = pre.metadata_filters.as_ref().map(build_filters_from_hook);
            (boosted, filters)
        } else {
            (question.to_string(), None)
        };

        // 0b. Load conversation history if a session is active.
        let history = if let (Some(sid), Some(svc)) = (options.session_id, &self.sessions) {
            svc.get_history(sid, Some(10)).await.unwrap_or_default()
        } else {
            Vec::new()
        };

        // 1. Search for context (using the boost-enriched query).
        let strategy = self
            .resolve_strategy(options.strategy.as_deref(), adapter_id)
            .await;
        let results = self
            .search
            .search(
                &effective_query,
                strategy,
                options.max_context,
                hook_filters,
            )
            .await?;

        if results.is_empty() {
            let answer = "No relevant information found in the \
                          knowledge graph to answer this question."
                .to_string();

            if let (Some(sid), Some(svc)) = (options.session_id, &self.sessions) {
                let _ = svc.add_turn(sid, "user", question, None).await;
                let _ = svc.add_turn(sid, "assistant", &answer, None).await;
            }

            let stream = futures::stream::iter(vec![
                AskStreamEvent::Context {
                    context_used: 0,
                    citations: Vec::new(),
                },
                AskStreamEvent::Token { text: answer },
                AskStreamEvent::Done {
                    provider: "none".to_string(),
                },
            ]);
            return Ok(Box::pin(stream));
        }

        // 2. Post-search hooks.
        let extra_context = if let Some(ref hooks) = self.hooks {
            let summary = format!("{} results found", results.len());
            let post = hooks
                .fire_post_search(question, &summary, adapter_id)
                .await?;
            post.additional_context.unwrap_or_default()
        } else {
            Vec::new()
        };

        // 3. Enrich with provenance and build context blocks.
        let mut context_blocks = self.build_context_blocks(&results).await;

        for (i, ctx_text) in extra_context.iter().enumerate() {
            context_blocks.push(ContextBlock {
                number: context_blocks.len() + 1,
                result_type: "hook_context".to_string(),
                confidence: 1.0,
                source_title: format!("hook_context_{}", i + 1),
                source_uri: String::new(),
                source_domain: "external".to_string(),
                snippet: ctx_text.clone(),
                _fused_score: 0.0,
                graph_edges: None,
            });
        }
        let context_used = context_blocks.len();

        // 4. Build prompts.
        let system_prompt = build_system_prompt();
        let user_prompt = build_user_prompt_with_history(question, &context_blocks, &history);

        // 5. Build citations.
        let citations = build_citations(&results, &context_blocks);

        // 6. Resolve LLM backend.
        let backend: Arc<dyn ChatBackend> = if let Some(ref model) = options.model {
            Arc::new(resolve_model_backend(model))
        } else {
            Arc::clone(&self.chat)
        };

        // 7. Get the LLM token stream.
        let token_stream = backend
            .chat_stream(&system_prompt, &user_prompt, false, 0.3)
            .await?;

        // Capture state needed by the stream closure.
        let hooks = self.hooks.clone();
        let sessions = self.sessions.clone();
        let question_owned = question.to_string();
        let session_id = options.session_id;

        // 8. Compose the SSE stream:
        //    Context -> Token* -> Done
        let context_event = AskStreamEvent::Context {
            context_used,
            citations: citations.clone(),
        };

        // Chain: one context event, then mapped token events.
        let prefix = futures::stream::once(async move { context_event });

        // Map the raw token stream, accumulating text for session
        // recording and post-synthesis hooks.
        let answer_buf = Arc::new(tokio::sync::Mutex::new(String::new()));
        let answer_buf2 = Arc::clone(&answer_buf);

        let tokens = token_stream.filter_map(move |chunk_result| {
            let answer_buf = Arc::clone(&answer_buf2);
            let hooks = hooks.clone();
            let sessions = sessions.clone();
            let citations = citations.clone();
            let question_owned = question_owned.clone();
            async move {
                match chunk_result {
                    Ok(chunk) if chunk.done => {
                        let provider = chunk.provider;
                        // Fire post-synthesis hook (fire-and-forget).
                        if let Some(hooks) = hooks {
                            let answer = answer_buf.lock().await;
                            let cit_vals: Vec<serde_json::Value> = citations
                                .iter()
                                .filter_map(|c| serde_json::to_value(c).ok())
                                .collect();
                            hooks.fire_post_synthesis(
                                question_owned.clone(),
                                answer.clone(),
                                cit_vals,
                                adapter_id,
                            );
                        }
                        // Record session turns.
                        if let (Some(sid), Some(svc)) = (session_id, &sessions) {
                            let answer = answer_buf.lock().await;
                            let _ = svc.add_turn(sid, "user", &question_owned, None).await;
                            let _ = svc.add_turn(sid, "assistant", &answer, None).await;
                        }
                        Some(AskStreamEvent::Done { provider })
                    }
                    Ok(chunk) if chunk.text.is_empty() => None,
                    Ok(chunk) => {
                        let mut buf = answer_buf.lock().await;
                        buf.push_str(&chunk.text);
                        Some(AskStreamEvent::Token { text: chunk.text })
                    }
                    Err(e) => Some(AskStreamEvent::Error {
                        message: e.to_string(),
                    }),
                }
            }
        });

        Ok(Box::pin(prefix.chain(tokens)))
    }

    /// Build numbered context blocks from search results, enriching
    /// each with source provenance metadata and graph edges for code
    /// entities.
    async fn build_context_blocks(&self, results: &[FusedResult]) -> Vec<ContextBlock> {
        let mut blocks = Vec::with_capacity(results.len());
        for (i, result) in results.iter().enumerate() {
            let source_info = self.lookup_source_info(result).await.unwrap_or_default();

            let result_type = result.result_type.as_deref().unwrap_or("unknown");
            let confidence = result.confidence.unwrap_or(0.0);
            let snippet = result
                .content
                .as_deref()
                .or(result.snippet.as_deref())
                .unwrap_or("")
                .to_string();

            blocks.push(ContextBlock {
                number: i + 1,
                result_type: result_type.to_string(),
                confidence,
                source_title: source_info.title,
                source_uri: source_info.uri.clone(),
                source_domain: source_info.domain,
                snippet,
                _fused_score: result.fused_score,
                graph_edges: None,
            });
        }

        // Enrich code entities with call graph and structural edges.
        let mut enriched_count = 0usize;
        for (i, result) in results.iter().enumerate() {
            if enriched_count >= MAX_GRAPH_ENRICHED_ENTITIES {
                break;
            }
            if !is_code_entity(result) {
                continue;
            }
            let entity_name = result.name.clone().unwrap_or_default();
            if entity_name.is_empty() {
                continue;
            }
            if let Ok(edges) = self.lookup_graph_edges(result.id).await {
                if !edges.is_empty() {
                    blocks[i].graph_edges = Some(GraphEdgeContext { entity_name, edges });
                    enriched_count += 1;
                }
            }
        }

        blocks
    }

    /// Look up source title and URI for a search result.
    async fn lookup_source_info(&self, result: &FusedResult) -> Option<SourceInfo> {
        // Prefer pre-enriched source metadata from the result.
        let title = result.source_title.clone();
        let uri = result.source_uri.clone();
        if title.is_some() || uri.is_some() {
            let domain = result.source_domains.first().cloned().unwrap_or_default();
            return Some(SourceInfo {
                title: title.unwrap_or_default(),
                uri: uri.unwrap_or_default(),
                domain,
            });
        }

        // Fall back to DB lookup for node-type results that lack
        // source metadata (nodes aren't directly tied to a single
        // source). Use the name as a fallback.
        if result.result_type.as_deref().is_some_and(|rt| rt == "node") {
            return Some(SourceInfo {
                title: result
                    .name
                    .clone()
                    .unwrap_or_else(|| "knowledge graph node".to_string()),
                uri: String::new(),
                domain: String::new(),
            });
        }

        // Attempt a source lookup via the source_id embedded in the
        // result's source_uri field (if it looks like a UUID). This
        // handles edge cases where enrichment didn't populate the
        // source_title.
        None
    }

    /// Look up structural graph edges for a node (calls, uses_type,
    /// contains) from the database. Returns edges grouped by
    /// relationship type with direction suffix (e.g. "called_by").
    ///
    /// This is a lightweight SQL query — no graph algorithms, just
    /// direct edge lookups with a JOIN for the neighbor's name.
    async fn lookup_graph_edges(&self, node_id: Uuid) -> Result<Vec<GraphEdge>> {
        use crate::storage::traits::AskRepo;

        let outgoing_types: Vec<String> = OUTGOING_REL_TYPES
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let incoming_types: Vec<String> = INCOMING_REL_TYPES
            .iter()
            .map(|s| (*s).to_string())
            .collect();

        // Outgoing: node --rel_type--> target
        let out_rows = AskRepo::get_outgoing_edges(
            &*self.repo,
            node_id,
            &outgoing_types,
            MAX_EDGES_PER_DIRECTION as i64,
        )
        .await?;

        // Incoming: source --rel_type--> node (displayed as rel_type_by)
        let in_rows = AskRepo::get_incoming_edges(
            &*self.repo,
            node_id,
            &incoming_types,
            MAX_EDGES_PER_DIRECTION as i64,
        )
        .await?;

        let mut edges = Vec::with_capacity(out_rows.len() + in_rows.len());
        for (name, rel) in out_rows {
            edges.push(GraphEdge {
                neighbor_name: name,
                rel_type: rel,
            });
        }
        for (name, rel) in in_rows {
            edges.push(GraphEdge {
                neighbor_name: name,
                rel_type: format!("{rel}_by"),
            });
        }
        Ok(edges)
    }
}

/// Internal representation of a context fragment.
#[derive(Debug, Clone)]
struct ContextBlock {
    number: usize,
    result_type: String,
    confidence: f64,
    source_title: String,
    source_uri: String,
    /// Knowledge domain: code, spec, design, research, external.
    source_domain: String,
    snippet: String,
    /// Retained for potential ranking/filtering use.
    _fused_score: f64,
    /// Structural graph edges for code entities (calls, uses_type,
    /// contains, called_by, contained_by).
    graph_edges: Option<GraphEdgeContext>,
}

/// Graph edge context for a code entity in the knowledge graph.
#[derive(Debug, Clone)]
struct GraphEdgeContext {
    /// The entity name for the header line.
    entity_name: String,
    /// Edges grouped by relationship type.
    edges: Vec<GraphEdge>,
}

/// A single graph edge: a named neighbor and its relationship.
#[derive(Debug, Clone)]
struct GraphEdge {
    /// Name of the neighboring entity.
    neighbor_name: String,
    /// Relationship type (e.g. "calls", "uses_type", "called_by").
    rel_type: String,
}

/// Source provenance metadata.
#[derive(Debug, Default)]
struct SourceInfo {
    title: String,
    uri: String,
    domain: String,
}

/// Check if a fused result represents a code entity (function,
/// struct, trait, impl_block, etc.) that should be enriched with
/// graph call/structural edges.
fn is_code_entity(result: &FusedResult) -> bool {
    result
        .entity_type
        .as_deref()
        .is_some_and(|t| derive_entity_class(t) == EntityClass::Code)
}

/// Format graph edge context into a text block for the LLM prompt.
fn format_graph_edges(ctx: &GraphEdgeContext) -> String {
    // Group edges by rel_type for compact display.
    let mut by_type: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &ctx.edges {
        by_type
            .entry(&edge.rel_type)
            .or_default()
            .push(&edge.neighbor_name);
    }

    let mut lines = vec![format!("[Graph Context for {}]", ctx.entity_name)];
    // Sort relationship types for deterministic output.
    let mut types: Vec<&&str> = by_type.keys().collect();
    types.sort();
    for rel_type in types {
        let names = &by_type[*rel_type];
        lines.push(format!("- {}: {}", rel_type, names.join(", ")));
    }
    lines.join("\n")
}

/// Resolve a model name to a CLI chat backend.
fn resolve_model_backend(model: &str) -> CliChatBackend {
    match model {
        "haiku" | "sonnet" | "opus" => CliChatBackend::new("claude".to_string(), model.to_string()),
        "gemini" => CliChatBackend::new("gemini".to_string(), "gemini-2.5-flash".to_string()),
        "copilot" => CliChatBackend::new("copilot".to_string(), "claude-haiku-4.5".to_string()),
        // Default: treat as a claude model name.
        other => CliChatBackend::new("claude".to_string(), other.to_string()),
    }
}

/// Build [`SearchFilters`] from a pre_search hook's
/// `metadata_filters` JSON.
///
/// Recognized keys:
/// - `domains` — `Vec<String>`: restrict to source domains.
/// - `entity_classes` — `Vec<String>`: restrict to entity classes.
/// - `node_types` — `Vec<String>`: restrict to node types.
/// - `min_confidence` — `f64`: minimum epistemic confidence.
///
/// Unrecognized keys are logged as warnings and ignored.
fn build_filters_from_hook(metadata: &serde_json::Value) -> SearchFilters {
    let mut filters = SearchFilters::default();

    if let Some(obj) = metadata.as_object() {
        for (key, value) in obj {
            match key.as_str() {
                "domains" => {
                    if let Some(arr) = value.as_array() {
                        let vals: Vec<String> = arr
                            .iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                        if !vals.is_empty() {
                            filters.domains = Some(vals);
                        }
                    }
                }
                "entity_classes" => {
                    if let Some(arr) = value.as_array() {
                        let vals: Vec<String> = arr
                            .iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                        if !vals.is_empty() {
                            filters.entity_classes = Some(vals);
                        }
                    }
                }
                "node_types" => {
                    if let Some(arr) = value.as_array() {
                        let vals: Vec<String> = arr
                            .iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                        if !vals.is_empty() {
                            filters.node_types = Some(vals);
                        }
                    }
                }
                "min_confidence" => {
                    if let Some(v) = value.as_f64() {
                        filters.min_confidence = Some(v);
                    }
                }
                other => {
                    tracing::warn!(
                        key = %other,
                        "unrecognized metadata_filter key \
                         from pre_search hook — ignoring"
                    );
                }
            }
        }
    }

    filters
}

fn parse_strategy(s: Option<&str>) -> SearchStrategy {
    match s {
        Some("balanced") => SearchStrategy::Balanced,
        Some("precise") => SearchStrategy::Precise,
        Some("exploratory") => SearchStrategy::Exploratory,
        Some("recent") => SearchStrategy::Recent,
        Some("graph_first") => SearchStrategy::GraphFirst,
        Some("global") => SearchStrategy::Global,
        _ => SearchStrategy::Auto,
    }
}

/// Build the system prompt for grounded synthesis.
fn build_system_prompt() -> String {
    "You are a knowledge synthesis engine for the Covalence \
     project \u{2014} a hybrid GraphRAG knowledge engine. You answer \
     questions by synthesizing information from retrieved knowledge \
     fragments.\n\n\
     Rules:\n\
     - Base your answer ONLY on the provided context fragments. Do \
       not fabricate information.\n\
     - Cite sources using [1], [2], etc. matching the fragment \
       numbers.\n\
     - If the context is insufficient to fully answer the question, \
       say what you can answer and what's missing.\n\
     - Each context fragment is labeled with its knowledge domain: \
       code (source code), spec (specifications), design (ADRs, \
       design docs), research (academic papers), or external \
       (third-party docs). Use these labels to distinguish \
       provenance when synthesizing.\n\
     - Be specific and technical. Include exact names, numbers, and \
       terminology from the sources.\n\
     - Keep the answer focused and concise. Don't pad with generic \
       statements."
        .to_string()
}

/// Build the user prompt including the question and context blocks.
///
/// Convenience wrapper around [`build_user_prompt_with_history`] with
/// no conversation history.
#[cfg(test)]
fn build_user_prompt(question: &str, blocks: &[ContextBlock]) -> String {
    build_user_prompt_with_history(question, blocks, &[])
}

/// Build the user prompt with optional conversation history.
///
/// Layout:
/// 1. Question
/// 2. Retrieved context (numbered blocks)
/// 3. Conversation history (if any)
/// 4. Closing instruction
fn build_user_prompt_with_history(
    question: &str,
    blocks: &[ContextBlock],
    history: &[Turn],
) -> String {
    let mut prompt = format!("Question: {question}\n\nRetrieved context:\n");

    for block in blocks {
        let source_label = if block.source_title.is_empty() {
            block.source_uri.clone()
        } else if block.source_uri.is_empty() {
            block.source_title.clone()
        } else {
            format!("\"{}\" {}", block.source_title, block.source_uri)
        };

        let domain_tag = if block.source_domain.is_empty() {
            String::new()
        } else {
            format!(", domain: {}", block.source_domain)
        };

        prompt.push_str(&format!(
            "\n[{}] ({}, confidence: {:.2}{}, source: {})\n{}\n",
            block.number,
            block.result_type,
            block.confidence,
            domain_tag,
            source_label,
            truncate_context(&block.snippet, 2000),
        ));

        // Append graph edge context for code entities.
        if let Some(ref graph_ctx) = block.graph_edges {
            prompt.push_str(&format_graph_edges(graph_ctx));
            prompt.push('\n');
        }
    }

    // Inject conversation history between context and closing.
    if !history.is_empty() {
        prompt.push_str("\n## Conversation History\n");
        for turn in history {
            let role_label = match turn.role.as_str() {
                "user" => "User",
                "assistant" => "Assistant",
                "system" => "System",
                "tool" => "Tool",
                other => other,
            };
            prompt.push_str(&format!(
                "{}: {}\n",
                role_label,
                truncate_context(&turn.content, 1000)
            ));
        }
    }

    prompt.push_str("\nProvide a comprehensive answer based on the context above.");
    prompt
}

/// Build citations from search results and context blocks.
fn build_citations(results: &[FusedResult], blocks: &[ContextBlock]) -> Vec<Citation> {
    results
        .iter()
        .zip(blocks.iter())
        .map(|(result, block)| {
            let source = if !block.source_title.is_empty() {
                block.source_title.clone()
            } else if !block.source_uri.is_empty() {
                block.source_uri.clone()
            } else {
                result.name.clone().unwrap_or_else(|| "unknown".to_string())
            };

            Citation {
                source,
                snippet: truncate_context(&block.snippet, 500),
                result_type: block.result_type.clone(),
                confidence: block.confidence,
            }
        })
        .collect()
}

/// Truncate context text to a maximum character length, appending
/// an ellipsis if truncation occurred.
fn truncate_context(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        return s.to_string();
    }
    // Find a safe truncation point (avoid splitting multi-byte chars).
    let mut end = max_chars;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_options_default() {
        let opts = AskOptions::default();
        assert_eq!(opts.max_context, 15);
        assert!(opts.strategy.is_none());
        assert!(opts.session_id.is_none());
        assert!(opts.adapter_id.is_none());
    }

    #[test]
    fn parse_strategy_auto() {
        assert!(matches!(parse_strategy(None), SearchStrategy::Auto));
        assert!(matches!(parse_strategy(Some("auto")), SearchStrategy::Auto));
    }

    #[test]
    fn parse_strategy_precise() {
        assert!(matches!(
            parse_strategy(Some("precise")),
            SearchStrategy::Precise
        ));
    }

    #[test]
    fn parse_strategy_balanced() {
        assert!(matches!(
            parse_strategy(Some("balanced")),
            SearchStrategy::Balanced
        ));
    }

    #[test]
    fn parse_strategy_exploratory() {
        assert!(matches!(
            parse_strategy(Some("exploratory")),
            SearchStrategy::Exploratory
        ));
    }

    #[test]
    fn parse_strategy_recent() {
        assert!(matches!(
            parse_strategy(Some("recent")),
            SearchStrategy::Recent
        ));
    }

    #[test]
    fn parse_strategy_graph_first() {
        assert!(matches!(
            parse_strategy(Some("graph_first")),
            SearchStrategy::GraphFirst
        ));
    }

    #[test]
    fn parse_strategy_global() {
        assert!(matches!(
            parse_strategy(Some("global")),
            SearchStrategy::Global
        ));
    }

    #[test]
    fn parse_strategy_unknown() {
        assert!(matches!(
            parse_strategy(Some("bogus")),
            SearchStrategy::Auto
        ));
    }

    #[test]
    fn truncate_context_short() {
        let s = "hello world";
        assert_eq!(truncate_context(s, 100), "hello world");
    }

    #[test]
    fn truncate_context_long() {
        let s = "a".repeat(200);
        let result = truncate_context(&s, 50);
        assert_eq!(result.len(), 53); // 50 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_context_multibyte() {
        // Multi-byte UTF-8: each char is 4 bytes.
        let s = "\u{1F600}".repeat(10); // 10 emoji
        let result = truncate_context(&s, 5);
        // Should truncate at a valid boundary.
        assert!(result.ends_with("..."));
    }

    #[test]
    fn build_system_prompt_contains_rules() {
        let prompt = build_system_prompt();
        assert!(prompt.contains("knowledge synthesis engine"));
        assert!(prompt.contains("[1]"));
        assert!(prompt.contains("fabricate"));
    }

    #[test]
    fn build_user_prompt_formats_correctly() {
        let blocks = vec![
            ContextBlock {
                number: 1,
                result_type: "chunk".to_string(),
                confidence: 0.92,
                source_title: "HippoRAG Paper".to_string(),
                source_uri: "file://spec/05-ingestion.md".to_string(),
                source_domain: "spec".to_string(),
                snippet: "Entity resolution uses 5 tiers.".to_string(),
                _fused_score: 0.85,
                graph_edges: None,
            },
            ContextBlock {
                number: 2,
                result_type: "node".to_string(),
                confidence: 0.0,
                source_title: "Entity Resolution".to_string(),
                source_uri: String::new(),
                source_domain: String::new(),
                snippet: "HDBSCAN clustering approach.".to_string(),
                _fused_score: 0.6,
                graph_edges: None,
            },
        ];
        let prompt = build_user_prompt("How does entity resolution work?", &blocks);
        assert!(prompt.contains("Question: How does entity resolution work?"));
        assert!(prompt.contains("[1] (chunk, confidence: 0.92, domain: spec"));
        assert!(prompt.contains("HippoRAG Paper"));
        assert!(prompt.contains("[2] (node, confidence: 0.00"));
        assert!(prompt.contains("Entity Resolution"));
        assert!(prompt.contains("Provide a comprehensive answer"));
    }

    #[test]
    fn build_citations_maps_results() {
        let results = vec![FusedResult {
            id: uuid::Uuid::nil(),
            fused_score: 0.9,
            confidence: Some(0.85),
            entity_type: None,
            name: Some("Test Node".to_string()),
            snippet: Some("test snippet".to_string()),
            content: Some("test content".to_string()),
            source_uri: None,
            source_title: None,
            source_type: None,
            source_domains: Vec::new(),
            result_type: Some("chunk".to_string()),
            created_at: None,
            dimension_scores: Default::default(),
            dimension_ranks: Default::default(),
            graph_context: None,
        }];
        let blocks = vec![ContextBlock {
            number: 1,
            result_type: "chunk".to_string(),
            confidence: 0.85,
            source_title: "My Source".to_string(),
            source_uri: "file://test.md".to_string(),
            source_domain: "external".to_string(),
            snippet: "test content".to_string(),
            _fused_score: 0.9,
            graph_edges: None,
        }];
        let citations = build_citations(&results, &blocks);
        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].source, "My Source");
        assert_eq!(citations[0].result_type, "chunk");
        assert!((citations[0].confidence - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn citation_falls_back_to_name() {
        let results = vec![FusedResult {
            id: uuid::Uuid::nil(),
            fused_score: 0.5,
            confidence: None,
            entity_type: None,
            name: Some("Fallback Name".to_string()),
            snippet: None,
            content: None,
            source_uri: None,
            source_title: None,
            source_type: None,
            source_domains: Vec::new(),
            result_type: Some("node".to_string()),
            created_at: None,
            dimension_scores: Default::default(),
            dimension_ranks: Default::default(),
            graph_context: None,
        }];
        let blocks = vec![ContextBlock {
            number: 1,
            result_type: "node".to_string(),
            confidence: 0.0,
            source_title: String::new(),
            source_uri: String::new(),
            source_domain: String::new(),
            snippet: String::new(),
            _fused_score: 0.5,
            graph_edges: None,
        }];
        let citations = build_citations(&results, &blocks);
        assert_eq!(citations[0].source, "Fallback Name");
    }

    // --- Graph edge enrichment tests ---

    #[test]
    fn is_code_entity_detects_code_types() {
        let make_result = |entity_type: &str| FusedResult {
            id: Uuid::nil(),
            fused_score: 0.5,
            confidence: None,
            entity_type: Some(entity_type.to_string()),
            name: None,
            snippet: None,
            content: None,
            source_uri: None,
            source_title: None,
            source_type: None,
            source_domains: Vec::new(),
            result_type: Some("node".to_string()),
            created_at: None,
            dimension_scores: Default::default(),
            dimension_ranks: Default::default(),
            graph_context: None,
        };

        // Code entity types
        assert!(is_code_entity(&make_result("function")));
        assert!(is_code_entity(&make_result("struct")));
        assert!(is_code_entity(&make_result("trait")));
        assert!(is_code_entity(&make_result("impl_block")));
        assert!(is_code_entity(&make_result("enum")));
        assert!(is_code_entity(&make_result("module")));

        // Non-code entity types
        assert!(!is_code_entity(&make_result("concept")));
        assert!(!is_code_entity(&make_result("person")));
        assert!(!is_code_entity(&make_result("article")));
        assert!(!is_code_entity(&make_result("chunk")));

        // No entity_type
        let mut no_type = make_result("function");
        no_type.entity_type = None;
        assert!(!is_code_entity(&no_type));
    }

    #[test]
    fn format_graph_edges_single_type() {
        let ctx = GraphEdgeContext {
            entity_name: "SearchService".to_string(),
            edges: vec![
                GraphEdge {
                    neighbor_name: "search".to_string(),
                    rel_type: "calls".to_string(),
                },
                GraphEdge {
                    neighbor_name: "clear_cache".to_string(),
                    rel_type: "calls".to_string(),
                },
            ],
        };
        let output = format_graph_edges(&ctx);
        assert!(output.contains("[Graph Context for SearchService]"));
        assert!(output.contains("- calls: search, clear_cache"));
    }

    #[test]
    fn format_graph_edges_multiple_types() {
        let ctx = GraphEdgeContext {
            entity_name: "PipelineService".to_string(),
            edges: vec![
                GraphEdge {
                    neighbor_name: "run_search".to_string(),
                    rel_type: "calls".to_string(),
                },
                GraphEdge {
                    neighbor_name: "SearchFilters".to_string(),
                    rel_type: "uses_type".to_string(),
                },
                GraphEdge {
                    neighbor_name: "run_pipeline".to_string(),
                    rel_type: "called_by".to_string(),
                },
            ],
        };
        let output = format_graph_edges(&ctx);
        assert!(output.contains("[Graph Context for PipelineService]"));
        assert!(output.contains("- calls: run_search"));
        assert!(output.contains("- uses_type: SearchFilters"));
        assert!(output.contains("- called_by: run_pipeline"));
    }

    #[test]
    fn format_graph_edges_deterministic_order() {
        let ctx = GraphEdgeContext {
            entity_name: "Node".to_string(),
            edges: vec![
                GraphEdge {
                    neighbor_name: "z_func".to_string(),
                    rel_type: "uses_type".to_string(),
                },
                GraphEdge {
                    neighbor_name: "a_func".to_string(),
                    rel_type: "calls".to_string(),
                },
                GraphEdge {
                    neighbor_name: "m_func".to_string(),
                    rel_type: "contains".to_string(),
                },
            ],
        };
        let output = format_graph_edges(&ctx);
        let lines: Vec<&str> = output.lines().collect();
        // Header + 3 rel types, sorted alphabetically
        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains("[Graph Context for Node]"));
        assert!(lines[1].starts_with("- calls:"));
        assert!(lines[2].starts_with("- contains:"));
        assert!(lines[3].starts_with("- uses_type:"));
    }

    #[test]
    fn build_user_prompt_includes_graph_edges() {
        let blocks = vec![ContextBlock {
            number: 1,
            result_type: "node".to_string(),
            confidence: 0.75,
            source_title: "SearchService".to_string(),
            source_uri: String::new(),
            source_domain: "code".to_string(),
            snippet: "Multi-dimensional fused search.".to_string(),
            _fused_score: 0.8,
            graph_edges: Some(GraphEdgeContext {
                entity_name: "SearchService".to_string(),
                edges: vec![
                    GraphEdge {
                        neighbor_name: "search".to_string(),
                        rel_type: "calls".to_string(),
                    },
                    GraphEdge {
                        neighbor_name: "SearchFilters".to_string(),
                        rel_type: "contains".to_string(),
                    },
                    GraphEdge {
                        neighbor_name: "run_pipeline".to_string(),
                        rel_type: "called_by".to_string(),
                    },
                ],
            }),
        }];
        let prompt = build_user_prompt("What does SearchService do?", &blocks);
        assert!(prompt.contains("[Graph Context for SearchService]"));
        assert!(prompt.contains("- calls: search"));
        assert!(prompt.contains("- contains: SearchFilters"));
        assert!(prompt.contains("- called_by: run_pipeline"));
    }

    #[test]
    fn build_user_prompt_no_graph_edges_for_non_code() {
        let blocks = vec![ContextBlock {
            number: 1,
            result_type: "chunk".to_string(),
            confidence: 0.9,
            source_title: "Some Paper".to_string(),
            source_uri: "https://example.com".to_string(),
            source_domain: "research".to_string(),
            snippet: "Some research content.".to_string(),
            _fused_score: 0.8,
            graph_edges: None,
        }];
        let prompt = build_user_prompt("What is RRF?", &blocks);
        assert!(!prompt.contains("[Graph Context"));
    }

    #[test]
    fn build_user_prompt_includes_conversation_history() {
        let blocks = vec![ContextBlock {
            number: 1,
            result_type: "chunk".to_string(),
            confidence: 0.9,
            source_title: "Test".to_string(),
            source_uri: String::new(),
            source_domain: "spec".to_string(),
            snippet: "Some content.".to_string(),
            _fused_score: 0.8,
            graph_edges: None,
        }];
        let history = vec![
            Turn {
                id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
                role: "user".to_string(),
                content: "What is entity resolution?".to_string(),
                metadata: serde_json::json!({}),
                ordinal: 1,
                created_at: chrono::Utc::now(),
            },
            Turn {
                id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
                role: "assistant".to_string(),
                content: "Entity resolution is a 5-tier process.".to_string(),
                metadata: serde_json::json!({}),
                ordinal: 2,
                created_at: chrono::Utc::now(),
            },
        ];
        let prompt = build_user_prompt_with_history("How does tier 3 work?", &blocks, &history);
        assert!(prompt.contains("## Conversation History"));
        assert!(prompt.contains("User: What is entity resolution?"));
        assert!(prompt.contains("Assistant: Entity resolution is a 5-tier"));
        assert!(prompt.contains("Question: How does tier 3 work?"));
    }

    // --- AskStreamEvent serialization tests ---

    #[test]
    fn ask_stream_event_context_serialization() {
        let event = AskStreamEvent::Context {
            context_used: 5,
            citations: vec![Citation {
                source: "test.md".to_string(),
                snippet: "hello".to_string(),
                result_type: "chunk".to_string(),
                confidence: 0.9,
            }],
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "Context");
        assert_eq!(json["context_used"], 5);
        assert_eq!(json["citations"].as_array().unwrap().len(), 1);
        assert_eq!(json["citations"][0]["source"], "test.md");
    }

    #[test]
    fn ask_stream_event_token_serialization() {
        let event = AskStreamEvent::Token {
            text: "hello world".to_string(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "Token");
        assert_eq!(json["text"], "hello world");
    }

    #[test]
    fn ask_stream_event_done_serialization() {
        let event = AskStreamEvent::Done {
            provider: "claude(haiku)".to_string(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "Done");
        assert_eq!(json["provider"], "claude(haiku)");
    }

    #[test]
    fn ask_stream_event_error_serialization() {
        let event = AskStreamEvent::Error {
            message: "something went wrong".to_string(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "Error");
        assert_eq!(json["message"], "something went wrong");
    }

    #[test]
    fn ask_stream_event_roundtrip() {
        let events = vec![
            AskStreamEvent::Context {
                context_used: 3,
                citations: vec![],
            },
            AskStreamEvent::Token {
                text: "test".to_string(),
            },
            AskStreamEvent::Done {
                provider: "mock".to_string(),
            },
            AskStreamEvent::Error {
                message: "oops".to_string(),
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let back: AskStreamEvent = serde_json::from_str(&json).unwrap();
            // Verify roundtrip preserves type tag.
            let orig_type = serde_json::to_value(event).unwrap()["type"].clone();
            let back_type = serde_json::to_value(&back).unwrap()["type"].clone();
            assert_eq!(orig_type, back_type);
        }
    }

    #[test]
    fn build_user_prompt_no_history_omits_section() {
        let blocks = vec![ContextBlock {
            number: 1,
            result_type: "chunk".to_string(),
            confidence: 0.5,
            source_title: "Test".to_string(),
            source_uri: String::new(),
            source_domain: String::new(),
            snippet: "content".to_string(),
            _fused_score: 0.5,
            graph_edges: None,
        }];
        let prompt = build_user_prompt_with_history("query", &blocks, &[]);
        assert!(!prompt.contains("## Conversation History"));
    }

    // --- build_filters_from_hook tests ---

    #[test]
    fn build_filters_empty_object() {
        let meta = serde_json::json!({});
        let filters = build_filters_from_hook(&meta);
        assert!(filters.domains.is_none());
        assert!(filters.entity_classes.is_none());
        assert!(filters.node_types.is_none());
        assert!(filters.min_confidence.is_none());
    }

    #[test]
    fn build_filters_domains() {
        let meta = serde_json::json!({
            "domains": ["code", "spec"]
        });
        let filters = build_filters_from_hook(&meta);
        assert_eq!(
            filters.domains.as_deref(),
            Some(["code".to_string(), "spec".to_string()].as_slice())
        );
    }

    #[test]
    fn build_filters_entity_classes() {
        let meta = serde_json::json!({
            "entity_classes": ["code", "domain"]
        });
        let filters = build_filters_from_hook(&meta);
        assert_eq!(
            filters.entity_classes.as_deref(),
            Some(["code".to_string(), "domain".to_string()].as_slice())
        );
    }

    #[test]
    fn build_filters_node_types() {
        let meta = serde_json::json!({
            "node_types": ["concept", "function"]
        });
        let filters = build_filters_from_hook(&meta);
        assert_eq!(
            filters.node_types.as_deref(),
            Some(["concept".to_string(), "function".to_string()].as_slice())
        );
    }

    #[test]
    fn build_filters_min_confidence() {
        let meta = serde_json::json!({
            "min_confidence": 0.75
        });
        let filters = build_filters_from_hook(&meta);
        assert!((filters.min_confidence.unwrap() - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn build_filters_combined() {
        let meta = serde_json::json!({
            "domains": ["research"],
            "min_confidence": 0.5,
            "node_types": ["concept"]
        });
        let filters = build_filters_from_hook(&meta);
        assert_eq!(
            filters.domains.as_deref(),
            Some(["research".to_string()].as_slice())
        );
        assert!((filters.min_confidence.unwrap() - 0.5).abs() < f64::EPSILON);
        assert_eq!(
            filters.node_types.as_deref(),
            Some(["concept".to_string()].as_slice())
        );
        assert!(filters.entity_classes.is_none());
    }

    #[test]
    fn build_filters_ignores_unknown_keys() {
        let meta = serde_json::json!({
            "domains": ["code"],
            "unknown_key": "value",
            "another": 42
        });
        let filters = build_filters_from_hook(&meta);
        assert!(filters.domains.is_some());
        // Unknown keys are silently ignored (logged as warnings).
    }

    #[test]
    fn build_filters_non_object_returns_default() {
        let meta = serde_json::json!("not an object");
        let filters = build_filters_from_hook(&meta);
        assert!(filters.domains.is_none());
        assert!(filters.min_confidence.is_none());
    }

    #[test]
    fn build_filters_empty_array_ignored() {
        let meta = serde_json::json!({
            "domains": [],
            "node_types": []
        });
        let filters = build_filters_from_hook(&meta);
        // Empty arrays should not set the filter.
        assert!(filters.domains.is_none());
        assert!(filters.node_types.is_none());
    }
}

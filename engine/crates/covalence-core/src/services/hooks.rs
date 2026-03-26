//! Lifecycle hook service — external HTTP hooks for the /ask pipeline.
//!
//! Hooks are generic HTTP POST calls at three pipeline phases:
//! `pre_search`, `post_search`, and `post_synthesis`. The engine
//! doesn't know what's behind the URL — it just fires the request,
//! respects `fail_open` semantics, and merges the response.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::storage::postgres::PgRepo;
use crate::storage::traits::HookRepo;

/// Hook execution phase in the /ask pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPhase {
    /// Before the search call.
    PreSearch,
    /// After search, before LLM synthesis.
    PostSearch,
    /// After LLM synthesis (fire-and-forget).
    PostSynthesis,
}

impl HookPhase {
    /// Convert to the database column value.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PreSearch => "pre_search",
            Self::PostSearch => "post_search",
            Self::PostSynthesis => "post_synthesis",
        }
    }

    /// Parse from the database column value.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pre_search" => Some(Self::PreSearch),
            "post_search" => Some(Self::PostSearch),
            "post_synthesis" => Some(Self::PostSynthesis),
            _ => None,
        }
    }
}

impl std::fmt::Display for HookPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A lifecycle hook configuration row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleHook {
    /// Primary key.
    pub id: Uuid,
    /// Human-readable name (unique).
    pub name: String,
    /// Pipeline phase.
    pub phase: HookPhase,
    /// URL to POST to.
    pub hook_url: String,
    /// Optional adapter ID that scopes the hook.
    pub adapter_id: Option<Uuid>,
    /// Per-hook timeout in milliseconds.
    pub timeout_ms: i32,
    /// If true, timeout/error is logged and the pipeline continues.
    /// If false, the error propagates.
    pub fail_open: bool,
    /// Whether the hook is active.
    pub is_active: bool,
}

/// Response from a pre_search hook.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PreSearchHookResponse {
    /// Additional boost terms to enrich the query.
    #[serde(default)]
    pub boost_terms: Option<Vec<String>>,
    /// Additional metadata filters (opaque JSON).
    #[serde(default)]
    pub metadata_filters: Option<serde_json::Value>,
}

/// Response from a post_search hook.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PostSearchHookResponse {
    /// Additional context strings to inject before synthesis.
    #[serde(default)]
    pub additional_context: Option<Vec<String>>,
}

/// Request body sent to pre_search hooks.
#[derive(Debug, Serialize)]
struct PreSearchPayload<'a> {
    query: &'a str,
    adapter_id: Option<Uuid>,
}

/// Request body sent to post_search hooks.
#[derive(Debug, Serialize)]
struct PostSearchPayload<'a> {
    query: &'a str,
    results_summary: &'a str,
    adapter_id: Option<Uuid>,
}

/// Request body sent to post_synthesis hooks.
#[derive(Debug, Serialize)]
struct PostSynthesisPayload<'a> {
    query: &'a str,
    answer: &'a str,
    citations: &'a [serde_json::Value],
    adapter_id: Option<Uuid>,
}

/// Service for firing lifecycle hooks at pipeline phases.
pub struct HookService {
    client: reqwest::Client,
    repo: Arc<PgRepo>,
}

impl HookService {
    /// Create a new hook service.
    pub fn new(repo: Arc<PgRepo>) -> Self {
        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(4)
            .build()
            .unwrap_or_default();
        Self { client, repo }
    }

    /// Fire pre_search hooks and merge responses.
    pub async fn fire_pre_search(
        &self,
        query: &str,
        adapter_id: Option<Uuid>,
    ) -> Result<PreSearchHookResponse> {
        let hooks = self.active_hooks(HookPhase::PreSearch, adapter_id).await?;
        if hooks.is_empty() {
            return Ok(PreSearchHookResponse::default());
        }

        let payload = PreSearchPayload { query, adapter_id };
        let body = serde_json::to_value(&payload)?;

        let mut merged = PreSearchHookResponse::default();
        let results = self.fire_all(&hooks, &body).await;

        for (hook, result) in hooks.iter().zip(results) {
            match result {
                Ok(val) => {
                    if let Ok(resp) = serde_json::from_value::<PreSearchHookResponse>(val) {
                        if let Some(terms) = resp.boost_terms {
                            merged
                                .boost_terms
                                .get_or_insert_with(Vec::new)
                                .extend(terms);
                        }
                        if resp.metadata_filters.is_some() {
                            merged.metadata_filters = resp.metadata_filters;
                        }
                    }
                }
                Err(e) => {
                    if !hook.fail_open {
                        return Err(Error::Hook(format!("hook '{}' failed: {e}", hook.name)));
                    }
                    tracing::warn!(
                        hook = %hook.name,
                        error = %e,
                        "pre_search hook failed (fail_open)"
                    );
                }
            }
        }

        Ok(merged)
    }

    /// Fire post_search hooks and merge responses.
    pub async fn fire_post_search(
        &self,
        query: &str,
        results_summary: &str,
        adapter_id: Option<Uuid>,
    ) -> Result<PostSearchHookResponse> {
        let hooks = self.active_hooks(HookPhase::PostSearch, adapter_id).await?;
        if hooks.is_empty() {
            return Ok(PostSearchHookResponse::default());
        }

        let payload = PostSearchPayload {
            query,
            results_summary,
            adapter_id,
        };
        let body = serde_json::to_value(&payload)?;

        let mut merged = PostSearchHookResponse::default();
        let results = self.fire_all(&hooks, &body).await;

        for (hook, result) in hooks.iter().zip(results) {
            match result {
                Ok(val) => {
                    if let Ok(resp) = serde_json::from_value::<PostSearchHookResponse>(val) {
                        if let Some(ctx) = resp.additional_context {
                            merged
                                .additional_context
                                .get_or_insert_with(Vec::new)
                                .extend(ctx);
                        }
                    }
                }
                Err(e) => {
                    if !hook.fail_open {
                        return Err(Error::Hook(format!("hook '{}' failed: {e}", hook.name)));
                    }
                    tracing::warn!(
                        hook = %hook.name,
                        error = %e,
                        "post_search hook failed (fail_open)"
                    );
                }
            }
        }

        Ok(merged)
    }

    /// Fire post_synthesis hooks (fire-and-forget).
    ///
    /// Errors are logged but never propagated. This method spawns a
    /// background task and returns immediately.
    pub fn fire_post_synthesis(
        &self,
        query: String,
        answer: String,
        citations: Vec<serde_json::Value>,
        adapter_id: Option<Uuid>,
    ) {
        let client = self.client.clone();
        let repo = Arc::clone(&self.repo);
        tokio::spawn(async move {
            let hooks =
                match HookRepo::list_by_phase(&*repo, HookPhase::PostSynthesis.as_str()).await {
                    Ok(h) => h,
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "failed to load post_synthesis hooks"
                        );
                        return;
                    }
                };

            let active: Vec<_> = hooks
                .into_iter()
                .filter(|h| {
                    h.is_active
                        && (adapter_id.is_none()
                            || h.adapter_id.is_none()
                            || h.adapter_id == adapter_id)
                })
                .collect();

            if active.is_empty() {
                return;
            }

            let payload = PostSynthesisPayload {
                query: &query,
                answer: &answer,
                citations: &citations,
                adapter_id,
            };
            let body = match serde_json::to_value(&payload) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "failed to serialize post_synthesis payload"
                    );
                    return;
                }
            };

            for hook in &active {
                let timeout = Duration::from_millis(hook.timeout_ms as u64);
                match client
                    .post(&hook.hook_url)
                    .json(&body)
                    .timeout(timeout)
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        tracing::debug!(
                            hook = %hook.name,
                            "post_synthesis hook succeeded"
                        );
                    }
                    Ok(resp) => {
                        tracing::warn!(
                            hook = %hook.name,
                            status = %resp.status(),
                            "post_synthesis hook returned non-success"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            hook = %hook.name,
                            error = %e,
                            "post_synthesis hook failed"
                        );
                    }
                }
            }
        });
    }

    // ── Private helpers ─────────────────────────────────────────

    /// Load active hooks for a phase, optionally filtered by adapter.
    async fn active_hooks(
        &self,
        phase: HookPhase,
        adapter_id: Option<Uuid>,
    ) -> Result<Vec<LifecycleHook>> {
        let hooks = HookRepo::list_by_phase(&*self.repo, phase.as_str()).await?;
        Ok(hooks
            .into_iter()
            .filter(|h| {
                h.is_active
                    && (adapter_id.is_none()
                        || h.adapter_id.is_none()
                        || h.adapter_id == adapter_id)
            })
            .collect())
    }

    /// Fire all hooks concurrently, returning results in order.
    async fn fire_all(
        &self,
        hooks: &[LifecycleHook],
        body: &serde_json::Value,
    ) -> Vec<std::result::Result<serde_json::Value, String>> {
        let futs: Vec<_> = hooks
            .iter()
            .map(|hook| {
                let client = self.client.clone();
                let url = hook.hook_url.clone();
                let timeout = Duration::from_millis(hook.timeout_ms as u64);
                let body = body.clone();
                async move {
                    let resp = client
                        .post(&url)
                        .json(&body)
                        .timeout(timeout)
                        .send()
                        .await
                        .map_err(|e| e.to_string())?;

                    if !resp.status().is_success() {
                        return Err(format!("HTTP {}", resp.status()));
                    }

                    resp.json::<serde_json::Value>()
                        .await
                        .map_err(|e| e.to_string())
                }
            })
            .collect();

        futures::future::join_all(futs).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_search_response_default() {
        let resp = PreSearchHookResponse::default();
        assert!(resp.boost_terms.is_none());
        assert!(resp.metadata_filters.is_none());
    }

    #[test]
    fn post_search_response_default() {
        let resp = PostSearchHookResponse::default();
        assert!(resp.additional_context.is_none());
    }

    #[test]
    fn hook_phase_serialization() {
        let json = serde_json::to_string(&HookPhase::PreSearch).unwrap();
        assert_eq!(json, "\"pre_search\"");

        let json = serde_json::to_string(&HookPhase::PostSearch).unwrap();
        assert_eq!(json, "\"post_search\"");

        let json = serde_json::to_string(&HookPhase::PostSynthesis).unwrap();
        assert_eq!(json, "\"post_synthesis\"");
    }

    #[test]
    fn hook_phase_deserialization() {
        let phase: HookPhase = serde_json::from_str("\"pre_search\"").unwrap();
        assert_eq!(phase, HookPhase::PreSearch);

        let phase: HookPhase = serde_json::from_str("\"post_search\"").unwrap();
        assert_eq!(phase, HookPhase::PostSearch);

        let phase: HookPhase = serde_json::from_str("\"post_synthesis\"").unwrap();
        assert_eq!(phase, HookPhase::PostSynthesis);
    }

    #[test]
    fn hook_phase_as_str() {
        assert_eq!(HookPhase::PreSearch.as_str(), "pre_search");
        assert_eq!(HookPhase::PostSearch.as_str(), "post_search");
        assert_eq!(HookPhase::PostSynthesis.as_str(), "post_synthesis");
    }

    #[test]
    fn hook_phase_from_str() {
        assert_eq!(HookPhase::parse("pre_search"), Some(HookPhase::PreSearch));
        assert_eq!(HookPhase::parse("post_search"), Some(HookPhase::PostSearch));
        assert_eq!(
            HookPhase::parse("post_synthesis"),
            Some(HookPhase::PostSynthesis)
        );
        assert_eq!(HookPhase::parse("invalid"), None);
    }

    #[test]
    fn hook_phase_display() {
        assert_eq!(format!("{}", HookPhase::PreSearch), "pre_search");
        assert_eq!(format!("{}", HookPhase::PostSearch), "post_search");
        assert_eq!(format!("{}", HookPhase::PostSynthesis), "post_synthesis");
    }

    #[test]
    fn lifecycle_hook_serializes() {
        let hook = LifecycleHook {
            id: Uuid::nil(),
            name: "test-hook".to_string(),
            phase: HookPhase::PreSearch,
            hook_url: "http://example.com/hook".to_string(),
            adapter_id: None,
            timeout_ms: 2000,
            fail_open: true,
            is_active: true,
        };
        let json = serde_json::to_value(&hook).unwrap();
        assert_eq!(json["name"], "test-hook");
        assert_eq!(json["phase"], "pre_search");
        assert_eq!(json["timeout_ms"], 2000);
    }

    #[test]
    fn pre_search_response_deserializes_partial() {
        let json = r#"{"boost_terms": ["graph", "rag"]}"#;
        let resp: PreSearchHookResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            resp.boost_terms.as_deref(),
            Some(["graph".to_string(), "rag".to_string()].as_slice())
        );
        assert!(resp.metadata_filters.is_none());
    }

    #[test]
    fn post_search_response_deserializes_empty() {
        let json = "{}";
        let resp: PostSearchHookResponse = serde_json::from_str(json).unwrap();
        assert!(resp.additional_context.is_none());
    }
}

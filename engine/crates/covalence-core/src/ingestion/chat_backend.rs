//! Pluggable LLM chat backends.
//!
//! [`ChatBackend`] abstracts the transport layer for chat
//! completions so that extraction, compilation, and other
//! LLM-driven stages can run against either an OpenAI-compatible
//! HTTP API or a local CLI tool (e.g. `gemini`).

use std::pin::Pin;

use futures::Stream;
use reqwest_middleware::ClientWithMiddleware;
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Response from a chat backend, including the provider that handled it.
#[derive(Debug, Clone)]
pub struct ChatResponse {
    /// The assistant's response text.
    pub text: String,
    /// Label identifying which provider handled the request
    /// (e.g. "claude(haiku)", "gemini(2.5-flash)", "openai(gpt-4)").
    pub provider: String,
}

/// A single chunk from a streaming chat response.
#[derive(Debug, Clone)]
pub struct StreamChunk {
    /// The text content of this chunk (a token or line).
    pub text: String,
    /// Label identifying which provider is streaming.
    pub provider: String,
    /// Whether this is the final chunk in the stream.
    pub done: bool,
}

/// A backend for LLM chat completions.
///
/// Implementations handle the transport (HTTP, CLI subprocess, etc.)
/// while callers provide prompts and parse the response text.
#[async_trait::async_trait]
pub trait ChatBackend: Send + Sync {
    /// Send a chat completion request and return the response with
    /// provider attribution.
    ///
    /// When `json_mode` is true the backend should request
    /// structured JSON output (via `response_format` for HTTP, or
    /// by appending an instruction for CLI backends).
    async fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        json_mode: bool,
        temperature: f64,
    ) -> Result<ChatResponse>;

    /// Stream a chat completion token-by-token.
    ///
    /// Default implementation calls [`chat()`](ChatBackend::chat)
    /// and yields the full response as a single chunk.
    async fn chat_stream(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        json_mode: bool,
        temperature: f64,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>> {
        let resp = self
            .chat(system_prompt, user_prompt, json_mode, temperature)
            .await?;
        let provider = resp.provider.clone();
        let stream = tokio_stream::iter(vec![
            Ok(StreamChunk {
                text: resp.text,
                provider: provider.clone(),
                done: false,
            }),
            Ok(StreamChunk {
                text: String::new(),
                provider,
                done: true,
            }),
        ]);
        Ok(Box::pin(stream))
    }
}

// ── HTTP backend (OpenAI-compatible) ────────────────────────────

/// OpenAI-compatible HTTP chat backend.
///
/// Uses `reqwest-middleware` with exponential-backoff retry on
/// transient failures (5xx, timeouts, connection errors).
pub struct HttpChatBackend {
    client: ClientWithMiddleware,
    base_url: String,
    api_key: String,
    model: String,
}

impl HttpChatBackend {
    /// Create a new HTTP backend.
    ///
    /// `base_url` defaults to `https://api.openai.com/v1` when
    /// `None`.  The inner HTTP client is wrapped with retry
    /// middleware that retries transient failures up to 3 times
    /// with exponential backoff (100ms, 200ms, 400ms).
    pub fn new(model: String, api_key: String, base_url: Option<String>) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build();
        Self {
            client,
            base_url: base.trim_end_matches('/').to_string(),
            api_key,
            model,
        }
    }
}

#[async_trait::async_trait]
impl ChatBackend for HttpChatBackend {
    async fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        json_mode: bool,
        temperature: f64,
    ) -> Result<ChatResponse> {
        let body = HttpChatRequest {
            model: &self.model,
            messages: vec![
                HttpChatMessage {
                    role: "system",
                    content: system_prompt,
                },
                HttpChatMessage {
                    role: "user",
                    content: user_prompt,
                },
            ],
            response_format: if json_mode {
                Some(ResponseFormat {
                    r#type: "json_object",
                })
            } else {
                None
            },
            temperature,
        };

        let resp = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Ingestion(format!("chat backend request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(Error::Ingestion(format!(
                "chat backend API returned {status}: {body_text}"
            )));
        }

        let chat_resp: HttpChatResponse = resp
            .json()
            .await
            .map_err(|e| Error::Ingestion(format!("failed to parse chat backend response: {e}")))?;

        let content = chat_resp
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default();

        Ok(ChatResponse {
            text: content,
            provider: format!("http({})", self.model),
        })
    }
}

// ── CLI backend (gemini, etc.) ──────────────────────────────────

/// CLI-based chat backend that shells out to a command.
///
/// The command receives the user prompt on stdin and the system
/// prompt via the `-p` flag. Stdout is captured as the response.
///
/// Default command: `gemini -p <system+user> --model <model>`
pub struct CliChatBackend {
    /// CLI command name (e.g. "gemini").
    command: String,
    /// Model name passed via `--model` flag.
    model: String,
}

impl CliChatBackend {
    /// Create a new CLI backend.
    pub fn new(command: String, model: String) -> Self {
        Self { command, model }
    }

    /// Create a Gemini CLI backend with the default command.
    pub fn gemini(model: String) -> Self {
        Self::new("gemini".to_string(), model)
    }
}

/// Build the combined prompt for CLI backends.
///
/// CLI tools don't have separate system/user prompt channels,
/// so we combine them with a separator. Uses "====" instead of
/// "---" to avoid CLI argument parsing issues.
fn build_cli_prompt(
    system_prompt: &str,
    user_prompt: &str,
    json_mode: bool,
    temperature: f64,
) -> String {
    let mut prompt = if system_prompt.trim().is_empty() {
        user_prompt.to_string()
    } else {
        format!("{system_prompt}\n\n====\n\n{user_prompt}")
    };

    // Ensure the prompt never starts with a dash — CLI tools may
    // interpret it as a flag.
    if prompt.starts_with('-') {
        prompt.insert(0, '\n');
    }

    if json_mode {
        prompt.push_str(
            "\n\nIMPORTANT: Return ONLY valid JSON. No markdown fences, no explanation, no extra text.",
        );
    }

    if temperature < 0.1 {
        prompt.push_str("\n\nBe precise and deterministic in your response.");
    }

    prompt
}

/// Build a [`tokio::process::Command`] for the given CLI backend.
///
/// Each CLI tool has different flags for non-interactive prompt mode:
///   gemini:  --prompt=<text> --model <model>
///   copilot: --prompt=<text> --model <model>
///   claude:  --print --model <model> <prompt>
///
/// For gemini/copilot: use `--prompt=<value>` (equals syntax)
/// to avoid yargs misinterpreting prompt content containing
/// dashes (e.g. "---" markdown separators) as CLI flags.
fn build_cli_command(command: &str, model: &str, prompt: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(command);
    if command == "claude" {
        cmd.arg("--print").arg("--model").arg(model).arg(prompt);
    } else {
        cmd.arg(format!("--prompt={prompt}"))
            .arg("--model")
            .arg(model);
    }
    cmd
}

#[async_trait::async_trait]
impl ChatBackend for CliChatBackend {
    async fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        json_mode: bool,
        temperature: f64,
    ) -> Result<ChatResponse> {
        let prompt = build_cli_prompt(system_prompt, user_prompt, json_mode, temperature);
        let mut cmd = build_cli_command(&self.command, &self.model, &prompt);

        // Run from a neutral directory to prevent CLI agents from
        // picking up the repo cwd and entering agentic/tool-use
        // mode. Use $HOME instead of /tmp — some CLIs (gemini)
        // scan the cwd and fail on /tmp due to permission errors
        // on systemd dirs.
        let neutral_dir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let output = cmd
            .current_dir(&neutral_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| {
                Error::Ingestion(format!(
                    "failed to execute chat CLI '{}': {e}",
                    self.command
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Ingestion(format!(
                "chat CLI '{}' exited with {}: {}",
                self.command,
                output.status,
                stderr.trim()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();

        // Strip the "Loaded cached credentials." preamble that
        // Gemini CLI sometimes emits.
        let content = stdout
            .strip_prefix("Loaded cached credentials.\n")
            .unwrap_or(&stdout)
            .trim()
            .to_string();

        Ok(ChatResponse {
            text: content,
            provider: format!("{}({})", self.command, self.model),
        })
    }

    async fn chat_stream(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        json_mode: bool,
        temperature: f64,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>> {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let prompt = build_cli_prompt(system_prompt, user_prompt, json_mode, temperature);
        let mut cmd = build_cli_command(&self.command, &self.model, &prompt);

        let neutral_dir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());

        let mut child = cmd
            .current_dir(&neutral_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                Error::Ingestion(format!(
                    "failed to spawn streaming chat CLI '{}': {e}",
                    self.command
                ))
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Ingestion("chat CLI stdout not captured".to_string()))?;

        let provider = format!("{}({})", self.command, self.model);
        let cmd_name = self.command.clone();
        let lines = BufReader::new(stdout).lines();

        // State tuple: (lines, child, provider, cmd_name,
        //               seen_credentials_preamble, terminated)
        type State = (
            tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
            tokio::process::Child,
            String,
            String,
            bool,
            bool,
        );

        let stream = futures::stream::unfold(
            (lines, child, provider, cmd_name, false, false) as State,
            |state| async move {
                let (mut lines, mut child, provider, cmd_name, mut seen_cred, terminated) = state;
                if terminated {
                    return None;
                }
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        // Skip the Gemini credentials preamble.
                        if !seen_cred && line == "Loaded cached credentials." {
                            seen_cred = true;
                            return Some((
                                Ok(StreamChunk {
                                    text: String::new(),
                                    provider: provider.clone(),
                                    done: false,
                                }),
                                (lines, child, provider, cmd_name, seen_cred, false),
                            ));
                        }
                        Some((
                            Ok(StreamChunk {
                                text: format!("{line}\n"),
                                provider: provider.clone(),
                                done: false,
                            }),
                            (lines, child, provider, cmd_name, seen_cred, false),
                        ))
                    }
                    Ok(None) => {
                        // EOF — check exit status.
                        let status = child.wait().await;
                        let item = match status {
                            Ok(s) if s.success() => Ok(StreamChunk {
                                text: String::new(),
                                provider: provider.clone(),
                                done: true,
                            }),
                            Ok(s) => Err(Error::Ingestion(format!(
                                "chat CLI '{cmd_name}' \
                                 exited with {s}"
                            ))),
                            Err(e) => Err(Error::Ingestion(format!(
                                "failed to wait on chat CLI \
                                 '{cmd_name}': {e}"
                            ))),
                        };
                        Some((item, (lines, child, provider, cmd_name, seen_cred, true)))
                    }
                    Err(e) => Some((
                        Err(Error::Ingestion(format!(
                            "error reading chat CLI \
                             '{cmd_name}' stdout: {e}"
                        ))),
                        (lines, child, provider, cmd_name, seen_cred, true),
                    )),
                }
            },
        );

        Ok(Box::pin(stream))
    }
}

// ── Fallback backend (CLI → HTTP, legacy two-layer) ─────────────

/// Chat backend that tries the primary first and falls back to a
/// secondary on any error. Kept for backwards compatibility.
pub struct FallbackChatBackend {
    primary: Box<dyn ChatBackend>,
    fallback: Box<dyn ChatBackend>,
}

impl FallbackChatBackend {
    /// Create a new fallback backend.
    pub fn new(primary: Box<dyn ChatBackend>, fallback: Box<dyn ChatBackend>) -> Self {
        Self { primary, fallback }
    }
}

#[async_trait::async_trait]
impl ChatBackend for FallbackChatBackend {
    async fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        json_mode: bool,
        temperature: f64,
    ) -> Result<ChatResponse> {
        match self
            .primary
            .chat(system_prompt, user_prompt, json_mode, temperature)
            .await
        {
            Ok(response) => Ok(response),
            Err(primary_err) => {
                tracing::warn!(
                    error = %primary_err,
                    "primary chat backend failed, falling back to secondary"
                );
                self.fallback
                    .chat(system_prompt, user_prompt, json_mode, temperature)
                    .await
            }
        }
    }
}

// ── Chain backend (multi-provider failover) ─────────────────────

/// Chat backend that tries multiple providers in order, falling
/// back to the next on any error. Each provider is labelled for
/// logging.
pub struct ChainChatBackend {
    /// Ordered list of (label, backend) pairs.
    chain: Vec<(String, Box<dyn ChatBackend>)>,
}

impl ChainChatBackend {
    /// Create a chain backend from an ordered list of providers.
    /// The first provider is tried first; on failure, the next
    /// is tried, and so on.
    pub fn new(chain: Vec<(String, Box<dyn ChatBackend>)>) -> Self {
        Self { chain }
    }
}

#[async_trait::async_trait]
impl ChatBackend for ChainChatBackend {
    async fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        json_mode: bool,
        temperature: f64,
    ) -> Result<ChatResponse> {
        let call_start = std::time::Instant::now();
        let mut last_err = None;
        for (i, (label, backend)) in self.chain.iter().enumerate() {
            match backend
                .chat(system_prompt, user_prompt, json_mode, temperature)
                .await
            {
                Ok(response) => {
                    let elapsed = call_start.elapsed().as_secs_f64();
                    crate::metrics::record_llm_call(&response.provider);
                    crate::metrics::record_llm_latency(&response.provider, elapsed);
                    if i > 0 {
                        tracing::info!(
                            provider = %label,
                            attempt = i + 1,
                            "chat succeeded on fallback provider"
                        );
                    }
                    return Ok(response);
                }
                Err(e) => {
                    let remaining = self.chain.len() - i - 1;
                    tracing::warn!(
                        provider = %label,
                        error = %e,
                        remaining_providers = remaining,
                        "chat provider failed, trying next"
                    );
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| Error::Ingestion("no chat backends configured".to_string())))
    }
}

// ── HTTP serialization types ────────────────────────────────────

#[derive(Serialize)]
struct HttpChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ResponseFormat<'a> {
    r#type: &'a str,
}

#[derive(Serialize)]
struct HttpChatRequest<'a> {
    model: &'a str,
    messages: Vec<HttpChatMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat<'a>>,
    temperature: f64,
}

#[derive(Deserialize)]
struct HttpChatChoice {
    message: HttpChatResponseMessage,
}

#[derive(Deserialize)]
struct HttpChatResponseMessage {
    content: Option<String>,
}

#[derive(Deserialize)]
struct HttpChatResponse {
    choices: Vec<HttpChatChoice>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_backend_default_url() {
        let backend = HttpChatBackend::new("gpt-4".into(), "key".into(), None);
        assert_eq!(backend.base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn http_backend_custom_url_trailing_slash() {
        let backend = HttpChatBackend::new(
            "model".into(),
            "key".into(),
            Some("https://example.com/v1/".to_string()),
        );
        assert_eq!(backend.base_url, "https://example.com/v1");
    }

    #[test]
    fn cli_backend_gemini_constructor() {
        let backend = CliChatBackend::gemini("gemini-2.5-flash".into());
        assert_eq!(backend.command, "gemini");
        assert_eq!(backend.model, "gemini-2.5-flash");
    }

    /// Mock backend that returns a fixed result.
    struct MockBackend {
        label: String,
        result: std::sync::Mutex<std::result::Result<String, Error>>,
    }

    impl MockBackend {
        fn ok(s: &str) -> Self {
            Self {
                label: "mock".to_string(),
                result: std::sync::Mutex::new(Ok(s.to_string())),
            }
        }

        fn err(msg: &str) -> Self {
            Self {
                label: "mock".to_string(),
                result: std::sync::Mutex::new(Err(Error::Ingestion(msg.to_string()))),
            }
        }
    }

    #[async_trait::async_trait]
    impl ChatBackend for MockBackend {
        async fn chat(
            &self,
            _system_prompt: &str,
            _user_prompt: &str,
            _json_mode: bool,
            _temperature: f64,
        ) -> Result<ChatResponse> {
            let mut guard = self.result.lock().unwrap();
            let result = std::mem::replace(
                &mut *guard,
                Err(Error::Ingestion("already consumed".into())),
            );
            result.map(|text| ChatResponse {
                text,
                provider: self.label.clone(),
            })
        }
    }

    #[tokio::test]
    async fn fallback_returns_primary_on_success() {
        let fb = FallbackChatBackend::new(
            Box::new(MockBackend::ok("primary")),
            Box::new(MockBackend::ok("fallback")),
        );
        let result = fb.chat("sys", "user", false, 0.0).await.unwrap();
        assert_eq!(result.text, "primary");
    }

    #[tokio::test]
    async fn fallback_uses_secondary_on_primary_error() {
        let fb = FallbackChatBackend::new(
            Box::new(MockBackend::err("quota exhausted")),
            Box::new(MockBackend::ok("fallback response")),
        );
        let result = fb.chat("sys", "user", false, 0.0).await.unwrap();
        assert_eq!(result.text, "fallback response");
    }

    #[tokio::test]
    async fn fallback_returns_secondary_error_when_both_fail() {
        let fb = FallbackChatBackend::new(
            Box::new(MockBackend::err("primary failed")),
            Box::new(MockBackend::err("secondary failed")),
        );
        let err = fb.chat("sys", "user", false, 0.0).await.unwrap_err();
        assert!(err.to_string().contains("secondary failed"));
    }

    #[test]
    fn cli_prompt_no_dashes_separator() {
        let prompt = build_cli_prompt("system", "user", false, 0.5);
        assert!(
            !prompt.contains("\n---\n"),
            "separator must not contain dashes (breaks CLI arg parsing)"
        );
        assert!(prompt.contains("===="), "should use ==== separator");
    }

    #[test]
    fn cli_prompt_empty_system_no_separator() {
        let prompt = build_cli_prompt("", "user prompt here", false, 0.5);
        assert_eq!(prompt, "user prompt here");
        assert!(
            !prompt.contains("===="),
            "no separator when system prompt is empty"
        );
    }

    #[test]
    fn cli_prompt_never_starts_with_dash() {
        // Empty system prompt — prompt is just user text
        let p1 = build_cli_prompt("", "hello", false, 0.5);
        assert!(!p1.starts_with('-'), "prompt must not start with dash");

        // System prompt present — starts with system text
        let p2 = build_cli_prompt("You are helpful.", "hello", false, 0.5);
        assert!(!p2.starts_with('-'), "prompt must not start with dash");

        // Edge case: system prompt is just whitespace (treated as empty)
        let p3 = build_cli_prompt("   ", "hello", false, 0.5);
        assert!(
            !p3.contains("===="),
            "whitespace-only system prompt should skip separator"
        );

        // Edge case: user prompt starts with dash
        let p4 = build_cli_prompt("", "- list item", false, 0.5);
        assert!(
            !p4.starts_with('-'),
            "prompt starting with dash must be guarded"
        );
    }

    #[test]
    fn cli_prompt_json_mode_appends_instruction() {
        let prompt = build_cli_prompt("sys", "user", true, 0.5);
        assert!(prompt.contains("Return ONLY valid JSON"));
    }

    #[test]
    fn cli_prompt_low_temperature_appends_hint() {
        let prompt = build_cli_prompt("sys", "user", false, 0.05);
        assert!(prompt.contains("precise and deterministic"));
    }

    // --- StreamChunk tests ---

    #[test]
    fn stream_chunk_fields() {
        let chunk = StreamChunk {
            text: "hello".to_string(),
            provider: "test(model)".to_string(),
            done: false,
        };
        assert_eq!(chunk.text, "hello");
        assert_eq!(chunk.provider, "test(model)");
        assert!(!chunk.done);

        let done = StreamChunk {
            text: String::new(),
            provider: "test(model)".to_string(),
            done: true,
        };
        assert!(done.done);
        assert!(done.text.is_empty());
    }

    #[test]
    fn stream_chunk_clone() {
        let chunk = StreamChunk {
            text: "data".to_string(),
            provider: "p".to_string(),
            done: false,
        };
        let cloned = chunk.clone();
        assert_eq!(cloned.text, chunk.text);
        assert_eq!(cloned.provider, chunk.provider);
        assert_eq!(cloned.done, chunk.done);
    }

    // --- Default chat_stream tests ---

    /// Mock backend that always returns the same response (for
    /// chat_stream default impl testing).
    struct StableMockBackend {
        text: String,
        provider: String,
    }

    impl StableMockBackend {
        fn new(text: &str) -> Self {
            Self {
                text: text.to_string(),
                provider: "stable_mock".to_string(),
            }
        }
    }

    #[async_trait::async_trait]
    impl ChatBackend for StableMockBackend {
        async fn chat(
            &self,
            _system_prompt: &str,
            _user_prompt: &str,
            _json_mode: bool,
            _temperature: f64,
        ) -> Result<ChatResponse> {
            Ok(ChatResponse {
                text: self.text.clone(),
                provider: self.provider.clone(),
            })
        }
    }

    #[tokio::test]
    async fn default_chat_stream_yields_content_then_done() {
        use futures::StreamExt;

        let backend = StableMockBackend::new("full response");
        let stream = backend
            .chat_stream("sys", "user", false, 0.5)
            .await
            .unwrap();

        let chunks: Vec<_> = stream.collect::<Vec<_>>().await;

        assert_eq!(chunks.len(), 2);

        // First chunk: the full content.
        let first = chunks[0].as_ref().unwrap();
        assert_eq!(first.text, "full response");
        assert_eq!(first.provider, "stable_mock");
        assert!(!first.done);

        // Second chunk: done sentinel.
        let second = chunks[1].as_ref().unwrap();
        assert!(second.text.is_empty());
        assert!(second.done);
        assert_eq!(second.provider, "stable_mock");
    }

    #[test]
    fn build_cli_command_claude() {
        let cmd = build_cli_command("claude", "haiku", "hello world");
        let prog = cmd.as_std().get_program().to_str().unwrap();
        assert_eq!(prog, "claude");
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_str().unwrap().to_string())
            .collect();
        assert!(args.contains(&"--print".to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"haiku".to_string()));
        assert!(args.contains(&"hello world".to_string()));
    }

    #[test]
    fn build_cli_command_gemini() {
        let cmd = build_cli_command("gemini", "gemini-2.5-flash", "test prompt");
        let prog = cmd.as_std().get_program().to_str().unwrap();
        assert_eq!(prog, "gemini");
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_str().unwrap().to_string())
            .collect();
        assert!(args.contains(&"--prompt=test prompt".to_string()));
        assert!(args.contains(&"gemini-2.5-flash".to_string()));
    }
}

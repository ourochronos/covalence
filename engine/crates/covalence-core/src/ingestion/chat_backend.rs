//! Pluggable LLM chat backends.
//!
//! [`ChatBackend`] abstracts the transport layer for chat
//! completions so that extraction, compilation, and other
//! LLM-driven stages can run against either an OpenAI-compatible
//! HTTP API or a local CLI tool (e.g. `gemini`).

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// A backend for LLM chat completions.
///
/// Implementations handle the transport (HTTP, CLI subprocess, etc.)
/// while callers provide prompts and parse the response text.
#[async_trait::async_trait]
pub trait ChatBackend: Send + Sync {
    /// Send a chat completion request and return the assistant's
    /// response text.
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
    ) -> Result<String>;
}

// ── HTTP backend (OpenAI-compatible) ────────────────────────────

/// OpenAI-compatible HTTP chat backend.
pub struct HttpChatBackend {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl HttpChatBackend {
    /// Create a new HTTP backend.
    ///
    /// `base_url` defaults to `https://api.openai.com/v1` when
    /// `None`.
    pub fn new(model: String, api_key: String, base_url: Option<String>) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        Self {
            client: reqwest::Client::new(),
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
    ) -> Result<String> {
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

        Ok(content)
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

#[async_trait::async_trait]
impl ChatBackend for CliChatBackend {
    async fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        json_mode: bool,
        temperature: f64,
    ) -> Result<String> {
        use tokio::process::Command;

        // Build the prompt. The CLI doesn't have a separate system
        // prompt channel, so we combine them.
        let mut prompt = format!("{system_prompt}\n\n---\n\n{user_prompt}");

        // For JSON mode, reinforce the instruction since there's no
        // response_format parameter.
        if json_mode {
            prompt.push_str("\n\nIMPORTANT: Return ONLY valid JSON. No markdown fences, no explanation, no extra text.");
        }

        // Temperature hint (CLI may or may not support it — Gemini
        // CLI doesn't expose temperature, so we just note it in the
        // prompt for very low values).
        if temperature < 0.1 {
            prompt.push_str("\n\nBe precise and deterministic in your response.");
        }

        // Run from /tmp to prevent CLI agents (gemini, copilot) from
        // picking up the repo cwd and entering agentic/tool-use mode.
        let output = Command::new(&self.command)
            .arg("-p")
            .arg(&prompt)
            .arg("--model")
            .arg(&self.model)
            .current_dir("/tmp")
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

        Ok(content)
    }
}

// ── Fallback backend (CLI → HTTP) ───────────────────────────────

/// Chat backend that tries the CLI first and falls back to HTTP
/// on any error (quota exhaustion, command not found, etc.).
///
/// This is the recommended backend for production: it prefers
/// the free Gemini CLI quota, but seamlessly degrades to the
/// paid OpenRouter HTTP API when quota is exhausted.
pub struct FallbackChatBackend {
    /// Primary backend (typically CLI).
    primary: Box<dyn ChatBackend>,
    /// Fallback backend (typically HTTP).
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
    ) -> Result<String> {
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
        result: std::sync::Mutex<Result<String>>,
    }

    impl MockBackend {
        fn ok(s: &str) -> Self {
            Self {
                result: std::sync::Mutex::new(Ok(s.to_string())),
            }
        }

        fn err(msg: &str) -> Self {
            Self {
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
        ) -> Result<String> {
            let mut guard = self.result.lock().unwrap();
            // Take the result so the mock can only be called once.
            std::mem::replace(
                &mut *guard,
                Err(Error::Ingestion("already consumed".into())),
            )
        }
    }

    #[tokio::test]
    async fn fallback_returns_primary_on_success() {
        let fb = FallbackChatBackend::new(
            Box::new(MockBackend::ok("primary")),
            Box::new(MockBackend::ok("fallback")),
        );
        let result = fb.chat("sys", "user", false, 0.0).await.unwrap();
        assert_eq!(result, "primary");
    }

    #[tokio::test]
    async fn fallback_uses_secondary_on_primary_error() {
        let fb = FallbackChatBackend::new(
            Box::new(MockBackend::err("quota exhausted")),
            Box::new(MockBackend::ok("fallback response")),
        );
        let result = fb.chat("sys", "user", false, 0.0).await.unwrap();
        assert_eq!(result, "fallback response");
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
}

//! JSON-in, JSON-out STDIO transport for external service processes.
//!
//! Spawns a child process per call, writes JSON to stdin, reads JSON
//! from stdout. Stateless — each invocation is independent.
//!
//! Use for: format conversion, text preprocessing, entity extraction.
//! Not for: model inference (needs persistent process), graph algorithms.

use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::error::{Error, Result};

/// Transport type for external service communication.
///
/// Unifies HTTP services (persistent servers) and STDIO services
/// (stateless child processes) under a single enum so the registry
/// can manage both.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum ServiceTransport {
    /// HTTP service — a persistent server at the given URL.
    Http {
        /// Base URL of the HTTP service.
        url: String,
    },
    /// STDIO service — spawned per-call, JSON on stdin/stdout.
    Stdio {
        /// Command to execute.
        command: String,
        /// Arguments to pass to the command.
        args: Vec<String>,
    },
}

/// JSON-in, JSON-out STDIO transport.
///
/// Spawns a child process per call, writes JSON to stdin, reads JSON
/// from stdout. Stateless — each invocation is independent.
pub struct StdioTransport {
    /// Command to execute.
    command: String,
    /// Arguments to pass to the command.
    args: Vec<String>,
    /// Maximum time to wait for the child process to complete.
    timeout: Duration,
}

impl StdioTransport {
    /// Create a new STDIO transport with the given command and args.
    ///
    /// Default timeout is 30 seconds.
    pub fn new(command: String, args: Vec<String>) -> Self {
        Self {
            command,
            args,
            timeout: Duration::from_secs(30),
        }
    }

    /// Override the default timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Send `input` as JSON to stdin, read JSON from stdout.
    ///
    /// Spawns a fresh child process, serializes `input` to JSON,
    /// writes it to stdin, waits for exit (with timeout), then
    /// deserializes stdout as `O`.
    pub async fn call<I: Serialize, O: DeserializeOwned>(&self, input: &I) -> Result<O> {
        let mut child = Command::new(&self.command)
            .args(&self.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                Error::Ingestion(format!("failed to spawn service '{}': {e}", self.command))
            })?;

        // Write JSON to stdin, then close it.
        let stdin_bytes = serde_json::to_vec(input)
            .map_err(|e| Error::Ingestion(format!("failed to serialize service input: {e}")))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(&stdin_bytes)
                .await
                .map_err(|e| Error::Ingestion(format!("failed to write to service stdin: {e}")))?;
            // Drop stdin to signal EOF.
            drop(stdin);
        }

        // Wait for the child with timeout.
        let output = tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| {
                Error::Ingestion(format!(
                    "service '{}' timed out after {:?}",
                    self.command, self.timeout
                ))
            })?
            .map_err(|e| Error::Ingestion(format!("failed to read service output: {e}")))?;

        // Check exit status.
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let code = output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string());
            return Err(Error::Ingestion(format!(
                "service '{}' exited with code {code}: {stderr}",
                self.command
            )));
        }

        // Parse stdout as JSON.
        serde_json::from_slice::<O>(&output.stdout).map_err(|e| {
            let raw = String::from_utf8_lossy(&output.stdout);
            Error::Ingestion(format!(
                "service '{}' returned invalid JSON: {e} (raw: {})",
                self.command,
                &raw[..raw.len().min(200)]
            ))
        })
    }

    /// Validate that the service is reachable and returns valid JSON.
    ///
    /// Sends `{"ping": true}` and checks that any valid JSON comes
    /// back.
    pub async fn validate(&self) -> Result<()> {
        let ping = serde_json::json!({"ping": true});
        let _: Value = self.call(&ping).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_transport_serde_roundtrip_http() {
        let transport = ServiceTransport::Http {
            url: "http://localhost:8080".to_string(),
        };
        let json = serde_json::to_string(&transport).unwrap();
        let parsed: ServiceTransport = serde_json::from_str(&json).unwrap();
        match parsed {
            ServiceTransport::Http { url } => {
                assert_eq!(url, "http://localhost:8080");
            }
            _ => panic!("expected Http variant"),
        }
    }

    #[test]
    fn service_transport_serde_roundtrip_stdio() {
        let transport = ServiceTransport::Stdio {
            command: "my-tool".to_string(),
            args: vec!["--format".to_string(), "json".to_string()],
        };
        let json = serde_json::to_string(&transport).unwrap();
        let parsed: ServiceTransport = serde_json::from_str(&json).unwrap();
        match parsed {
            ServiceTransport::Stdio { command, args } => {
                assert_eq!(command, "my-tool");
                assert_eq!(args, vec!["--format", "json"]);
            }
            _ => panic!("expected Stdio variant"),
        }
    }

    #[tokio::test]
    async fn stdio_transport_call_with_cat() {
        // `cat` echoes stdin to stdout, so JSON-in should equal
        // JSON-out.
        let transport = StdioTransport::new("cat".to_string(), vec![]);
        let input = serde_json::json!({"hello": "world", "n": 42});
        let output: Value = transport.call(&input).await.unwrap();
        assert_eq!(output, input);
    }

    #[tokio::test]
    async fn stdio_transport_timeout() {
        let transport = StdioTransport::new("sleep".to_string(), vec!["10".to_string()])
            .with_timeout(Duration::from_millis(100));

        let input = serde_json::json!({"ping": true});
        let result = transport.call::<Value, Value>(&input).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("timed out"),
            "expected timeout error, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn stdio_transport_nonzero_exit() {
        let transport = StdioTransport::new("false".to_string(), vec![]);
        let input = serde_json::json!({"ping": true});
        let result = transport.call::<Value, Value>(&input).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("exited with code"),
            "expected exit code error, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn stdio_transport_invalid_json_response() {
        // `echo "not json"` writes to stdout and exits 0, but the
        // output is not valid JSON.
        let transport = StdioTransport::new("echo".to_string(), vec!["not json".to_string()]);
        let input = serde_json::json!({"ping": true});
        let result = transport.call::<Value, Value>(&input).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("invalid JSON"),
            "expected invalid JSON error, got: {err_msg}"
        );
    }
}

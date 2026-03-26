//! Service registry — manages named transport endpoints (HTTP and
//! STDIO) and validates them at startup.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::error::Error;

use super::stdio_transport::{ServiceTransport, StdioTransport};

/// Health status of a registered service.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ServiceHealth {
    /// Human-readable service name.
    pub name: String,
    /// Transport type: "stdio" or "http".
    pub transport_type: String,
    /// Whether the last health check passed.
    pub healthy: bool,
    /// When the last health check was performed.
    pub last_checked: Option<chrono::DateTime<chrono::Utc>>,
    /// Error message from the last failed check, if any.
    pub error: Option<String>,
}

/// Registry of named service transports.
///
/// Both HTTP and STDIO services can be registered under a human-
/// readable name. At startup the factory calls [`validate_all`] to
/// smoke-test every transport and log failures.
pub struct ServiceRegistry {
    /// Name -> transport mapping.
    transports: HashMap<String, ServiceTransport>,
}

impl ServiceRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            transports: HashMap::new(),
        }
    }

    /// Register a transport under the given name.
    ///
    /// Overwrites any existing transport with the same name.
    pub fn register(&mut self, name: &str, transport: ServiceTransport) {
        self.transports.insert(name.to_string(), transport);
    }

    /// Look up a transport by name.
    pub fn get(&self, name: &str) -> Option<&ServiceTransport> {
        self.transports.get(name)
    }

    /// List all registered transports as `(name, transport)` pairs.
    pub fn list(&self) -> Vec<(&str, &ServiceTransport)> {
        self.transports
            .iter()
            .map(|(k, v)| (k.as_str(), v))
            .collect()
    }

    /// Validate all registered transports.
    ///
    /// HTTP transports are checked via a HEAD/GET to the base URL.
    /// STDIO transports are checked via
    /// [`StdioTransport::validate`].
    ///
    /// Returns a vec of `(name, error)` for any transports that
    /// failed validation. An empty vec means all passed.
    pub async fn validate_all(&self) -> Vec<(String, Error)> {
        let mut failures = Vec::new();

        for (name, transport) in &self.transports {
            let result = match transport {
                ServiceTransport::Http { url } => validate_http(url).await,
                ServiceTransport::Stdio { command, args } => {
                    let t = StdioTransport::new(command.clone(), args.clone());
                    t.validate().await
                }
            };
            if let Err(e) = result {
                failures.push((name.clone(), e));
            }
        }

        failures
    }

    /// Run validate_all and return structured health status for all
    /// registered services.
    pub async fn health_status(&self) -> Vec<ServiceHealth> {
        let now = chrono::Utc::now();
        let failures = self.validate_all().await;
        let failure_map: HashMap<&str, &Error> =
            failures.iter().map(|(n, e)| (n.as_str(), e)).collect();

        self.transports
            .iter()
            .map(|(name, transport)| {
                let transport_type = match transport {
                    ServiceTransport::Http { .. } => "http",
                    ServiceTransport::Stdio { .. } => "stdio",
                };
                let error = failure_map.get(name.as_str()).map(|e| e.to_string());
                ServiceHealth {
                    name: name.clone(),
                    transport_type: transport_type.to_string(),
                    healthy: error.is_none(),
                    last_checked: Some(now),
                    error,
                }
            })
            .collect()
    }

    /// Spawn a background task that periodically validates all
    /// services.
    pub fn spawn_health_loop(self: &Arc<Self>, interval_secs: u64) {
        let registry = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(interval_secs)).await;
                let failures = registry.validate_all().await;
                for (name, err) in &failures {
                    tracing::warn!(
                        service = %name,
                        error = %err,
                        "service health check failed"
                    );
                }
            }
        });
    }
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Quick connectivity check for an HTTP service.
///
/// Sends a GET to the base URL and considers any non-error response
/// (even 4xx) as "reachable." Only connection/DNS failures count as
/// validation errors.
async fn validate_http(url: &str) -> crate::error::Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| Error::Ingestion(format!("failed to build HTTP client: {e}")))?;

    client
        .get(url)
        .send()
        .await
        .map_err(|e| Error::Ingestion(format!("HTTP service at {url} is unreachable: {e}")))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_get() {
        let mut registry = ServiceRegistry::new();
        registry.register(
            "pdf",
            ServiceTransport::Http {
                url: "http://localhost:9000".to_string(),
            },
        );
        let t = registry.get("pdf");
        assert!(t.is_some());
        match t.unwrap() {
            ServiceTransport::Http { url } => {
                assert_eq!(url, "http://localhost:9000");
            }
            _ => panic!("expected Http variant"),
        }
    }

    #[test]
    fn get_returns_none_for_unknown() {
        let registry = ServiceRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn list_returns_all_registered() {
        let mut registry = ServiceRegistry::new();
        registry.register(
            "pdf",
            ServiceTransport::Http {
                url: "http://localhost:9000".to_string(),
            },
        );
        registry.register(
            "converter",
            ServiceTransport::Stdio {
                command: "my-converter".to_string(),
                args: vec![],
            },
        );
        let list = registry.list();
        assert_eq!(list.len(), 2);

        let names: Vec<&str> = list.iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"pdf"));
        assert!(names.contains(&"converter"));
    }

    #[test]
    fn register_overwrites_existing() {
        let mut registry = ServiceRegistry::new();
        registry.register(
            "pdf",
            ServiceTransport::Http {
                url: "http://old:9000".to_string(),
            },
        );
        registry.register(
            "pdf",
            ServiceTransport::Http {
                url: "http://new:9000".to_string(),
            },
        );
        match registry.get("pdf").unwrap() {
            ServiceTransport::Http { url } => {
                assert_eq!(url, "http://new:9000");
            }
            _ => panic!("expected Http variant"),
        }
    }

    #[test]
    fn default_creates_empty_registry() {
        let registry = ServiceRegistry::default();
        assert!(registry.list().is_empty());
    }
}

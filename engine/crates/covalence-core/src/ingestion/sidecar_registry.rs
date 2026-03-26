//! Sidecar registry — manages named transport endpoints (HTTP and
//! STDIO) and validates them at startup.

use std::collections::HashMap;

use crate::error::Error;

use super::stdio_transport::{SidecarTransport, StdioTransport};

/// Registry of named sidecar transports.
///
/// Both HTTP and STDIO sidecars can be registered under a human-
/// readable name. At startup the factory calls [`validate_all`] to
/// smoke-test every transport and log failures.
pub struct SidecarRegistry {
    /// Name -> transport mapping.
    transports: HashMap<String, SidecarTransport>,
}

impl SidecarRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            transports: HashMap::new(),
        }
    }

    /// Register a transport under the given name.
    ///
    /// Overwrites any existing transport with the same name.
    pub fn register(&mut self, name: &str, transport: SidecarTransport) {
        self.transports.insert(name.to_string(), transport);
    }

    /// Look up a transport by name.
    pub fn get(&self, name: &str) -> Option<&SidecarTransport> {
        self.transports.get(name)
    }

    /// List all registered transports as `(name, transport)` pairs.
    pub fn list(&self) -> Vec<(&str, &SidecarTransport)> {
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
                SidecarTransport::Http { url } => validate_http(url).await,
                SidecarTransport::Stdio { command, args } => {
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
}

impl Default for SidecarRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Quick connectivity check for an HTTP sidecar.
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
        .map_err(|e| Error::Ingestion(format!("HTTP sidecar at {url} is unreachable: {e}")))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_get() {
        let mut registry = SidecarRegistry::new();
        registry.register(
            "pdf",
            SidecarTransport::Http {
                url: "http://localhost:9000".to_string(),
            },
        );
        let t = registry.get("pdf");
        assert!(t.is_some());
        match t.unwrap() {
            SidecarTransport::Http { url } => {
                assert_eq!(url, "http://localhost:9000");
            }
            _ => panic!("expected Http variant"),
        }
    }

    #[test]
    fn get_returns_none_for_unknown() {
        let registry = SidecarRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn list_returns_all_registered() {
        let mut registry = SidecarRegistry::new();
        registry.register(
            "pdf",
            SidecarTransport::Http {
                url: "http://localhost:9000".to_string(),
            },
        );
        registry.register(
            "converter",
            SidecarTransport::Stdio {
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
        let mut registry = SidecarRegistry::new();
        registry.register(
            "pdf",
            SidecarTransport::Http {
                url: "http://old:9000".to_string(),
            },
        );
        registry.register(
            "pdf",
            SidecarTransport::Http {
                url: "http://new:9000".to_string(),
            },
        );
        match registry.get("pdf").unwrap() {
            SidecarTransport::Http { url } => {
                assert_eq!(url, "http://new:9000");
            }
            _ => panic!("expected Http variant"),
        }
    }

    #[test]
    fn default_creates_empty_registry() {
        let registry = SidecarRegistry::default();
        assert!(registry.list().is_empty());
    }
}

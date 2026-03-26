//! Extractor that delegates to an external service via ServiceTransport.
//!
//! Routes extraction requests to named services registered in the
//! ServiceRegistry. Supports both STDIO (per-call child process) and
//! HTTP (persistent server) transports.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::ingestion::extractor::{
    ExtractedEntity, ExtractedRelationship, ExtractionContext, ExtractionResult, Extractor,
};
use crate::ingestion::stdio_transport::{ServiceTransport, StdioTransport};

/// Request payload sent to an external extraction service.
#[derive(Debug, Serialize)]
struct ServiceExtractRequest<'a> {
    /// Source code or text to extract from.
    source_code: &'a str,
    /// Programming language (if known from context).
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    /// File path (if known from context).
    #[serde(skip_serializing_if = "Option::is_none")]
    file_path: Option<String>,
}

/// Response from an external extraction service.
#[derive(Debug, Deserialize)]
struct ServiceExtractResponse {
    /// Extracted entities.
    #[serde(default)]
    entities: Vec<ExtractedEntity>,
    /// Extracted relationships.
    #[serde(default)]
    relationships: Vec<ExtractedRelationship>,
}

/// Extractor that delegates to a named external service.
///
/// The service receives JSON with `source_code`, `language`, and
/// `file_path` fields, and returns JSON with `entities` and
/// `relationships` arrays matching the standard extraction types.
pub struct ServiceExtractor {
    /// Human-readable service name (for logging).
    service_name: String,
    /// Transport configuration (STDIO or HTTP).
    transport: ServiceTransport,
}

impl ServiceExtractor {
    /// Create a new service extractor from a transport definition.
    pub fn new(service_name: String, transport: ServiceTransport) -> Self {
        Self {
            service_name,
            transport,
        }
    }

    /// Derive a language hint from the extraction context.
    fn language_from_context(context: &ExtractionContext) -> Option<String> {
        context.source_uri.as_ref().and_then(|uri| {
            let ext = uri.rsplit('.').next()?;
            match ext {
                "rs" => Some("rust".to_string()),
                "go" => Some("go".to_string()),
                "py" => Some("python".to_string()),
                "ts" | "tsx" => Some("typescript".to_string()),
                "js" | "jsx" => Some("javascript".to_string()),
                "java" => Some("java".to_string()),
                "c" | "h" => Some("c".to_string()),
                "cpp" | "cc" | "cxx" | "hpp" => Some("cpp".to_string()),
                "rb" => Some("ruby".to_string()),
                "swift" => Some("swift".to_string()),
                "kt" | "kts" => Some("kotlin".to_string()),
                "md" => Some("markdown".to_string()),
                _ => None,
            }
        })
    }
}

#[async_trait::async_trait]
impl Extractor for ServiceExtractor {
    /// Extract entities and relationships by delegating to the
    /// external service.
    async fn extract(&self, text: &str, context: &ExtractionContext) -> Result<ExtractionResult> {
        let request = ServiceExtractRequest {
            source_code: text,
            language: Self::language_from_context(context),
            file_path: context.source_uri.clone(),
        };

        let response: ServiceExtractResponse = match &self.transport {
            ServiceTransport::Stdio { command, args } => {
                let transport = StdioTransport::new(command.clone(), args.clone());
                transport.call(&request).await?
            }
            ServiceTransport::Http { url } => {
                let extract_url = format!("{}/extract", url.trim_end_matches('/'));
                let client = reqwest::Client::new();
                let resp = client
                    .post(&extract_url)
                    .json(&request)
                    .timeout(std::time::Duration::from_secs(60))
                    .send()
                    .await
                    .map_err(|e| {
                        Error::Ingestion(format!(
                            "service '{}' HTTP request failed: {e}",
                            self.service_name
                        ))
                    })?;

                if !resp.status().is_success() {
                    return Err(Error::Ingestion(format!(
                        "service '{}' returned HTTP {}",
                        self.service_name,
                        resp.status()
                    )));
                }

                resp.json::<ServiceExtractResponse>().await.map_err(|e| {
                    Error::Ingestion(format!(
                        "service '{}' returned invalid JSON: {e}",
                        self.service_name
                    ))
                })?
            }
        };

        tracing::debug!(
            service = %self.service_name,
            entities = response.entities.len(),
            relationships = response.relationships.len(),
            "service extraction complete"
        );

        Ok(ExtractionResult {
            entities: response.entities,
            relationships: response.relationships,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_from_rust_uri() {
        let ctx = ExtractionContext {
            source_uri: Some("file://engine/src/main.rs".to_string()),
            ..Default::default()
        };
        assert_eq!(
            ServiceExtractor::language_from_context(&ctx),
            Some("rust".to_string())
        );
    }

    #[test]
    fn language_from_go_uri() {
        let ctx = ExtractionContext {
            source_uri: Some("file://cli/main.go".to_string()),
            ..Default::default()
        };
        assert_eq!(
            ServiceExtractor::language_from_context(&ctx),
            Some("go".to_string())
        );
    }

    #[test]
    fn language_from_python_uri() {
        let ctx = ExtractionContext {
            source_uri: Some("file://scripts/build.py".to_string()),
            ..Default::default()
        };
        assert_eq!(
            ServiceExtractor::language_from_context(&ctx),
            Some("python".to_string())
        );
    }

    #[test]
    fn language_from_unknown_extension() {
        let ctx = ExtractionContext {
            source_uri: Some("file://data/config.toml".to_string()),
            ..Default::default()
        };
        assert_eq!(ServiceExtractor::language_from_context(&ctx), None);
    }

    #[test]
    fn language_from_no_uri() {
        let ctx = ExtractionContext::default();
        assert_eq!(ServiceExtractor::language_from_context(&ctx), None);
    }

    #[test]
    fn service_extract_response_deserializes_empty() {
        let json = r#"{"entities": [], "relationships": []}"#;
        let resp: ServiceExtractResponse = serde_json::from_str(json).unwrap();
        assert!(resp.entities.is_empty());
        assert!(resp.relationships.is_empty());
    }

    #[test]
    fn service_extract_response_defaults_missing_fields() {
        let json = "{}";
        let resp: ServiceExtractResponse = serde_json::from_str(json).unwrap();
        assert!(resp.entities.is_empty());
        assert!(resp.relationships.is_empty());
    }

    #[test]
    fn service_extract_response_with_entities() {
        let json = r#"{
            "entities": [
                {"name": "Foo", "entity_type": "struct", "confidence": 0.9}
            ],
            "relationships": [
                {"source_name": "Foo", "target_name": "Bar",
                 "rel_type": "uses", "confidence": 0.8}
            ]
        }"#;
        let resp: ServiceExtractResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.entities.len(), 1);
        assert_eq!(resp.entities[0].name, "Foo");
        assert_eq!(resp.relationships.len(), 1);
        assert_eq!(resp.relationships[0].rel_type, "uses");
    }

    #[tokio::test]
    async fn stdio_extractor_with_echo() {
        // `echo` produces a known JSON response on stdout,
        // regardless of stdin input.
        let transport = ServiceTransport::Stdio {
            command: "echo".to_string(),
            args: vec![r#"{"entities":[],"relationships":[]}"#.to_string()],
        };
        let extractor = ServiceExtractor::new("test-echo".to_string(), transport);
        let ctx = ExtractionContext::default();
        let result = extractor.extract("fn main() {}", &ctx).await.unwrap();
        assert!(result.entities.is_empty());
        assert!(result.relationships.is_empty());
    }
}

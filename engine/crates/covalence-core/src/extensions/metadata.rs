//! Metadata schema validation for extension-declared contracts.
//!
//! Extensions declare JSON Schemas for entity type metadata and
//! source metadata. This module validates metadata values against
//! those schemas at ingestion time, with configurable enforcement
//! levels.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// How strictly metadata schemas are enforced during ingestion.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnforcementLevel {
    /// No validation — schemas are ignored entirely.
    Ignore,
    /// Validate and log warnings, but don't reject.
    #[default]
    Warn,
    /// Validate and reject on failure.
    Strict,
}

impl fmt::Display for EnforcementLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ignore => write!(f, "ignore"),
            Self::Warn => write!(f, "warn"),
            Self::Strict => write!(f, "strict"),
        }
    }
}

impl EnforcementLevel {
    /// Parse an enforcement level from a string.
    ///
    /// Returns `None` for unrecognized values.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "ignore" => Some(Self::Ignore),
            "warn" => Some(Self::Warn),
            "strict" => Some(Self::Strict),
            _ => None,
        }
    }
}

/// Validate entity metadata against the schema for its type.
///
/// Looks up the schema for `entity_type` in the provided map.
/// If no schema exists, validation passes (schemas are opt-in).
///
/// Behavior depends on `level`:
/// - `Ignore`: returns `Ok(())` immediately.
/// - `Warn`: validates, logs warnings, returns `Ok(())`.
/// - `Strict`: validates, returns `Err` on failure.
pub fn validate_entity_metadata(
    entity_type: &str,
    metadata: &serde_json::Value,
    schemas: &HashMap<String, serde_json::Value>,
    level: EnforcementLevel,
) -> Result<()> {
    if level == EnforcementLevel::Ignore {
        return Ok(());
    }

    let schema = match schemas.get(entity_type) {
        Some(s) => s,
        None => return Ok(()), // No schema declared — pass.
    };

    validate_against_schema(
        &format!("entity_type:{entity_type}"),
        metadata,
        schema,
        level,
    )
}

/// Validate source metadata against schemas for its domains.
///
/// Checks the metadata against schemas for each domain in the
/// provided list. If any domain has a schema, the metadata is
/// validated against it.
///
/// Behavior depends on `level`:
/// - `Ignore`: returns `Ok(())` immediately.
/// - `Warn`: validates, logs warnings, returns `Ok(())`.
/// - `Strict`: validates, returns `Err` on first failure.
pub fn validate_source_metadata(
    domains: &[String],
    metadata: &serde_json::Value,
    schemas: &HashMap<String, serde_json::Value>,
    level: EnforcementLevel,
) -> Result<()> {
    if level == EnforcementLevel::Ignore {
        return Ok(());
    }

    for domain in domains {
        if let Some(schema) = schemas.get(domain) {
            validate_against_schema(&format!("source_domain:{domain}"), metadata, schema, level)?;
        }
    }

    Ok(())
}

/// Internal: validate a value against a JSON Schema and handle
/// the enforcement level.
fn validate_against_schema(
    scope_label: &str,
    value: &serde_json::Value,
    schema: &serde_json::Value,
    level: EnforcementLevel,
) -> Result<()> {
    let validator = match jsonschema::validator_for(schema) {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("invalid metadata schema for {scope_label}: {e}");
            tracing::warn!(%msg);
            // Bad schema is an author error — don't reject data.
            return Ok(());
        }
    };

    let errors: Vec<String> = validator
        .iter_errors(value)
        .map(|e| {
            let path = e.instance_path.to_string();
            if path.is_empty() {
                e.to_string()
            } else {
                format!("{path}: {e}")
            }
        })
        .collect();

    if errors.is_empty() {
        return Ok(());
    }

    let joined = errors.join("; ");
    match level {
        EnforcementLevel::Warn => {
            tracing::warn!(
                scope = %scope_label,
                errors = %joined,
                "metadata schema validation failed (warn mode)"
            );
            Ok(())
        }
        EnforcementLevel::Strict => Err(Error::InvalidInput(format!(
            "metadata validation failed for {scope_label}: {joined}"
        ))),
        EnforcementLevel::Ignore => Ok(()), // unreachable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_schemas() -> HashMap<String, serde_json::Value> {
        let mut schemas = HashMap::new();
        schemas.insert(
            "function".to_string(),
            serde_json::json!({
                "type": "object",
                "required": ["signature"],
                "properties": {
                    "signature": { "type": "string" },
                    "visibility": { "type": "string" },
                    "parameters": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                }
            }),
        );
        schemas.insert(
            "struct".to_string(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "fields": { "type": "array" },
                    "visibility": { "type": "string" }
                }
            }),
        );
        schemas
    }

    fn make_source_schemas() -> HashMap<String, serde_json::Value> {
        let mut schemas = HashMap::new();
        schemas.insert(
            "code".to_string(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "language": { "type": "string" },
                    "file_path": { "type": "string" }
                }
            }),
        );
        schemas.insert(
            "research".to_string(),
            serde_json::json!({
                "type": "object",
                "required": ["year"],
                "properties": {
                    "authors": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "year": { "type": "integer" },
                    "doi": { "type": "string" }
                }
            }),
        );
        schemas
    }

    // -- Entity metadata tests --

    #[test]
    fn entity_valid_metadata_passes_strict() {
        let schemas = make_schemas();
        let metadata = serde_json::json!({
            "signature": "fn hello()",
            "visibility": "pub"
        });
        let result =
            validate_entity_metadata("function", &metadata, &schemas, EnforcementLevel::Strict);
        assert!(result.is_ok());
    }

    #[test]
    fn entity_missing_required_fails_strict() {
        let schemas = make_schemas();
        let metadata = serde_json::json!({
            "visibility": "pub"
        });
        let result =
            validate_entity_metadata("function", &metadata, &schemas, EnforcementLevel::Strict);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("signature"),
            "error should mention 'signature': {err}"
        );
    }

    #[test]
    fn entity_missing_required_passes_warn() {
        let schemas = make_schemas();
        let metadata = serde_json::json!({
            "visibility": "pub"
        });
        let result =
            validate_entity_metadata("function", &metadata, &schemas, EnforcementLevel::Warn);
        assert!(result.is_ok());
    }

    #[test]
    fn entity_missing_required_passes_ignore() {
        let schemas = make_schemas();
        let metadata = serde_json::json!({
            "visibility": "pub"
        });
        let result =
            validate_entity_metadata("function", &metadata, &schemas, EnforcementLevel::Ignore);
        assert!(result.is_ok());
    }

    #[test]
    fn entity_no_schema_for_type_passes() {
        let schemas = make_schemas();
        let metadata = serde_json::json!({"anything": true});
        let result =
            validate_entity_metadata("concept", &metadata, &schemas, EnforcementLevel::Strict);
        assert!(result.is_ok());
    }

    #[test]
    fn entity_wrong_field_type_fails_strict() {
        let schemas = make_schemas();
        let metadata = serde_json::json!({
            "signature": 42
        });
        let result =
            validate_entity_metadata("function", &metadata, &schemas, EnforcementLevel::Strict);
        assert!(result.is_err());
    }

    #[test]
    fn entity_optional_schema_passes_empty() {
        let schemas = make_schemas();
        let metadata = serde_json::json!({});
        // struct has no required fields
        let result =
            validate_entity_metadata("struct", &metadata, &schemas, EnforcementLevel::Strict);
        assert!(result.is_ok());
    }

    // -- Source metadata tests --

    #[test]
    fn source_valid_metadata_passes_strict() {
        let schemas = make_source_schemas();
        let metadata = serde_json::json!({
            "language": "rust",
            "file_path": "src/main.rs"
        });
        let result = validate_source_metadata(
            &["code".to_string()],
            &metadata,
            &schemas,
            EnforcementLevel::Strict,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn source_missing_required_fails_strict() {
        let schemas = make_source_schemas();
        let metadata = serde_json::json!({
            "authors": ["Alice"]
        });
        let result = validate_source_metadata(
            &["research".to_string()],
            &metadata,
            &schemas,
            EnforcementLevel::Strict,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("year"), "error should mention 'year': {err}");
    }

    #[test]
    fn source_missing_required_passes_warn() {
        let schemas = make_source_schemas();
        let metadata = serde_json::json!({
            "authors": ["Alice"]
        });
        let result = validate_source_metadata(
            &["research".to_string()],
            &metadata,
            &schemas,
            EnforcementLevel::Warn,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn source_no_schema_for_domain_passes() {
        let schemas = make_source_schemas();
        let metadata = serde_json::json!({"anything": true});
        let result = validate_source_metadata(
            &["external".to_string()],
            &metadata,
            &schemas,
            EnforcementLevel::Strict,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn source_multiple_domains_checks_all() {
        let schemas = make_source_schemas();
        // This metadata is valid for code but not research (missing
        // required "year").
        let metadata = serde_json::json!({
            "language": "rust"
        });
        let result = validate_source_metadata(
            &["code".to_string(), "research".to_string()],
            &metadata,
            &schemas,
            EnforcementLevel::Strict,
        );
        assert!(result.is_err());
    }

    #[test]
    fn source_ignore_skips_validation() {
        let schemas = make_source_schemas();
        let metadata = serde_json::json!(42); // Not even an object
        let result = validate_source_metadata(
            &["research".to_string()],
            &metadata,
            &schemas,
            EnforcementLevel::Ignore,
        );
        assert!(result.is_ok());
    }

    // -- EnforcementLevel tests --

    #[test]
    fn enforcement_level_from_str() {
        assert_eq!(
            EnforcementLevel::from_str_opt("ignore"),
            Some(EnforcementLevel::Ignore)
        );
        assert_eq!(
            EnforcementLevel::from_str_opt("warn"),
            Some(EnforcementLevel::Warn)
        );
        assert_eq!(
            EnforcementLevel::from_str_opt("strict"),
            Some(EnforcementLevel::Strict)
        );
        assert_eq!(
            EnforcementLevel::from_str_opt("WARN"),
            Some(EnforcementLevel::Warn)
        );
        assert!(EnforcementLevel::from_str_opt("invalid").is_none());
    }

    #[test]
    fn enforcement_level_display() {
        assert_eq!(EnforcementLevel::Ignore.to_string(), "ignore");
        assert_eq!(EnforcementLevel::Warn.to_string(), "warn");
        assert_eq!(EnforcementLevel::Strict.to_string(), "strict");
    }

    #[test]
    fn enforcement_level_default_is_warn() {
        assert_eq!(EnforcementLevel::default(), EnforcementLevel::Warn);
    }

    // -- Invalid schema handling --

    #[test]
    fn invalid_schema_logs_warning_but_passes() {
        let mut schemas = HashMap::new();
        // Invalid JSON Schema — "required" should be an array.
        schemas.insert(
            "bad_type".to_string(),
            serde_json::json!({
                "type": "object",
                "required": "not_an_array"
            }),
        );
        let metadata = serde_json::json!({"anything": true});
        let result =
            validate_entity_metadata("bad_type", &metadata, &schemas, EnforcementLevel::Strict);
        // Should pass — bad schema is an author error, not a data
        // error.
        assert!(result.is_ok());
    }
}

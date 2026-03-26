//! Extension manifest types -- deserialized from `extension.yaml`.

use std::collections::HashMap;

use serde::Deserialize;

/// An extension manifest (`extension.yaml`).
///
/// Extensions are the shareable unit of Covalence functionality.
/// Each manifest declares ontology additions, domain rules, alignment
/// rules, noise patterns, lifecycle hooks, and optional external
/// service definitions.
#[derive(Debug, Clone, Deserialize)]
pub struct ExtensionManifest {
    /// Unique extension name (e.g. "code-analysis").
    pub name: String,

    /// Semantic version string (e.g. "1.0.0").
    pub version: String,

    /// Human-readable description.
    #[serde(default)]
    pub description: String,

    /// Domain definitions to register.
    #[serde(default)]
    pub domains: Vec<DomainDef>,

    /// Entity type definitions to register.
    #[serde(default)]
    pub entity_types: Vec<EntityTypeDef>,

    /// Relationship type definitions to register.
    #[serde(default)]
    pub relationship_types: Vec<RelTypeDef>,

    /// View-to-edge-type mappings: `{ view_name: [rel_type, ...] }`.
    #[serde(default)]
    pub view_edges: HashMap<String, Vec<String>>,

    /// Noise entity patterns to register.
    #[serde(default)]
    pub noise_patterns: Vec<NoisePatternDef>,

    /// Domain classification rules.
    #[serde(default)]
    pub domain_rules: Vec<DomainRuleDef>,

    /// Named domain groups: `{ group_name: [domain_id, ...] }`.
    #[serde(default)]
    pub domain_groups: HashMap<String, Vec<String>>,

    /// Alignment rules for cross-domain analysis.
    #[serde(default)]
    pub alignment_rules: Vec<AlignmentRuleDef>,

    /// Optional external service definition (singular, for backward
    /// compatibility).
    #[serde(default)]
    pub service: Option<ServiceDef>,

    /// Multiple external service definitions.
    #[serde(default)]
    pub services: Vec<ServiceDef>,

    /// Lifecycle hooks to register.
    #[serde(default)]
    pub hooks: Vec<HookDef>,

    /// Config schema: `{ key: { type, default, description } }`.
    #[serde(default)]
    pub config_schema: HashMap<String, ConfigFieldDef>,
}

impl ExtensionManifest {
    /// Return all service definitions, merging the singular `service`
    /// field with the plural `services` vec for backward
    /// compatibility.
    pub fn merged_services(&self) -> Vec<&ServiceDef> {
        let mut out: Vec<&ServiceDef> = Vec::new();
        if let Some(ref svc) = self.service {
            out.push(svc);
        }
        for svc in &self.services {
            out.push(svc);
        }
        out
    }
}

/// A domain definition.
#[derive(Debug, Clone, Deserialize)]
pub struct DomainDef {
    /// Domain identifier (e.g. "code").
    pub id: String,

    /// Human-readable label.
    pub label: String,

    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,

    /// Whether this domain is internal (for DDSS boost).
    #[serde(default)]
    pub is_internal: bool,
}

/// An entity type definition.
#[derive(Debug, Clone, Deserialize)]
pub struct EntityTypeDef {
    /// Entity type identifier (e.g. "function").
    pub id: String,

    /// Category this type belongs to (e.g. "process").
    pub category: String,

    /// Human-readable label.
    pub label: String,

    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
}

/// A relationship type definition.
#[derive(Debug, Clone, Deserialize)]
pub struct RelTypeDef {
    /// Relationship type identifier (e.g. "calls").
    pub id: String,

    /// Optional universal relationship type this maps to.
    #[serde(default)]
    pub universal: Option<String>,

    /// Human-readable label.
    pub label: String,

    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
}

/// A noise entity pattern definition.
#[derive(Debug, Clone, Deserialize)]
pub struct NoisePatternDef {
    /// The pattern string (literal or regex).
    pub pattern: String,

    /// Pattern type: "literal" or "regex".
    #[serde(default = "default_literal")]
    pub pattern_type: String,

    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
}

/// A domain classification rule.
#[derive(Debug, Clone, Deserialize)]
pub struct DomainRuleDef {
    /// Match type: "source_type", "uri_prefix", or "uri_regex".
    pub match_type: String,

    /// Value to match against.
    pub match_value: String,

    /// Domain ID this rule classifies sources into.
    pub domain_id: String,

    /// Priority (lower = higher priority, default 100).
    #[serde(default = "default_priority")]
    pub priority: i32,

    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
}

/// An alignment rule definition.
#[derive(Debug, Clone, Deserialize)]
pub struct AlignmentRuleDef {
    /// Unique rule name.
    pub name: String,

    /// Check type: "ahead", "contradiction", or "staleness".
    pub check_type: String,

    /// Source domain group name.
    pub source_group: String,

    /// Target domain group name.
    pub target_group: String,

    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,

    /// Additional parameters as JSON.
    #[serde(default = "default_empty_object")]
    pub parameters: serde_json::Value,
}

/// An external service definition.
#[derive(Debug, Clone, Deserialize)]
pub struct ServiceDef {
    /// Service name.
    pub name: String,

    /// Transport type: "stdio" or "http".
    pub transport: String,

    /// Command for stdio transport.
    #[serde(default)]
    pub command: Option<String>,

    /// Arguments for stdio transport.
    #[serde(default)]
    pub args: Vec<String>,

    /// URL for http transport.
    #[serde(default)]
    pub url: Option<String>,

    /// Domain name this service extracts for (e.g. "code").
    ///
    /// When set, the pipeline will route extraction requests for
    /// sources in this domain to this service instead of the
    /// default extractor.
    #[serde(default)]
    pub extractor_for: Option<String>,

    /// Other service names this service depends on.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// A lifecycle hook definition.
#[derive(Debug, Clone, Deserialize)]
pub struct HookDef {
    /// Pipeline phase: "pre_search", "post_search", etc.
    pub phase: String,

    /// URL to POST to when the hook fires.
    pub url: String,

    /// Per-hook timeout in milliseconds.
    #[serde(default = "default_timeout")]
    pub timeout_ms: i32,

    /// If true, errors are logged but the pipeline continues.
    #[serde(default = "default_true")]
    pub fail_open: bool,
}

/// A config field schema definition.
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigFieldDef {
    /// Field type: "string", "integer", "boolean", "float".
    #[serde(rename = "type")]
    pub field_type: String,

    /// Default value.
    #[serde(default)]
    pub default: Option<serde_json::Value>,

    /// Human-readable description.
    #[serde(default)]
    pub description: Option<String>,
}

fn default_literal() -> String {
    "literal".to_string()
}

fn default_priority() -> i32 {
    100
}

fn default_timeout() -> i32 {
    2000
}

fn default_true() -> bool {
    true
}

fn default_empty_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// JSON Schema for validating extension manifests.
///
/// This schema validates the structure and required fields of an
/// `extension.yaml` before serde deserialization. It catches
/// structural errors (missing fields, wrong types) with descriptive
/// messages rather than generic serde errors.
fn manifest_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["name", "version"],
        "properties": {
            "name": { "type": "string" },
            "version": { "type": "string" },
            "description": { "type": "string" },
            "domains": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "label"],
                    "properties": {
                        "id": { "type": "string" },
                        "label": { "type": "string" },
                        "description": { "type": "string" },
                        "is_internal": { "type": "boolean" }
                    }
                }
            },
            "entity_types": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "category", "label"],
                    "properties": {
                        "id": { "type": "string" },
                        "category": { "type": "string" },
                        "label": { "type": "string" },
                        "description": { "type": "string" }
                    }
                }
            },
            "relationship_types": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "label"],
                    "properties": {
                        "id": { "type": "string" },
                        "universal": { "type": "string" },
                        "label": { "type": "string" },
                        "description": { "type": "string" }
                    }
                }
            },
            "view_edges": {
                "type": "object",
                "additionalProperties": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            },
            "noise_patterns": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["pattern"],
                    "properties": {
                        "pattern": { "type": "string" },
                        "pattern_type": { "type": "string" },
                        "description": { "type": "string" }
                    }
                }
            },
            "domain_rules": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": [
                        "match_type", "match_value", "domain_id"
                    ],
                    "properties": {
                        "match_type": { "type": "string" },
                        "match_value": { "type": "string" },
                        "domain_id": { "type": "string" },
                        "priority": { "type": "integer" },
                        "description": { "type": "string" }
                    }
                }
            },
            "domain_groups": {
                "type": "object",
                "additionalProperties": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            },
            "alignment_rules": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": [
                        "name", "check_type",
                        "source_group", "target_group"
                    ],
                    "properties": {
                        "name": { "type": "string" },
                        "check_type": { "type": "string" },
                        "source_group": { "type": "string" },
                        "target_group": { "type": "string" },
                        "description": { "type": "string" },
                        "parameters": { "type": "object" }
                    }
                }
            },
            "service": {
                "type": "object",
                "required": ["name", "transport"],
                "properties": {
                    "name": { "type": "string" },
                    "transport": { "type": "string" },
                    "command": { "type": "string" },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "url": { "type": "string" },
                    "extractor_for": { "type": "string" },
                    "depends_on": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                }
            },
            "services": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["name", "transport"],
                    "properties": {
                        "name": { "type": "string" },
                        "transport": { "type": "string" },
                        "command": { "type": "string" },
                        "args": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "url": { "type": "string" },
                        "extractor_for": { "type": "string" },
                        "depends_on": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    }
                }
            },
            "hooks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["phase", "url"],
                    "properties": {
                        "phase": { "type": "string" },
                        "url": { "type": "string" },
                        "timeout_ms": { "type": "integer" },
                        "fail_open": { "type": "boolean" }
                    }
                }
            },
            "config_schema": {
                "type": "object",
                "additionalProperties": {
                    "type": "object",
                    "required": ["type"],
                    "properties": {
                        "type": { "type": "string" },
                        "default": {},
                        "description": { "type": "string" }
                    }
                }
            }
        }
    })
}

/// Validate a parsed JSON value against the extension manifest schema.
///
/// Returns `Ok(())` if valid, or an error with all validation failure
/// messages joined together.
pub fn validate_manifest_json(value: &serde_json::Value) -> std::result::Result<(), String> {
    let schema = manifest_schema();
    let validator =
        jsonschema::validator_for(&schema).map_err(|e| format!("invalid manifest schema: {e}"))?;
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
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_manifest() {
        let yaml = r#"
name: test-ext
version: "1.0.0"
"#;
        let manifest: ExtensionManifest =
            serde_yaml::from_str(yaml).expect("should parse minimal manifest");
        assert_eq!(manifest.name, "test-ext");
        assert_eq!(manifest.version, "1.0.0");
        assert!(manifest.description.is_empty());
        assert!(manifest.domains.is_empty());
        assert!(manifest.entity_types.is_empty());
        assert!(manifest.relationship_types.is_empty());
        assert!(manifest.view_edges.is_empty());
        assert!(manifest.noise_patterns.is_empty());
        assert!(manifest.domain_rules.is_empty());
        assert!(manifest.domain_groups.is_empty());
        assert!(manifest.alignment_rules.is_empty());
        assert!(manifest.service.is_none());
        assert!(manifest.services.is_empty());
        assert!(manifest.hooks.is_empty());
        assert!(manifest.config_schema.is_empty());
        assert!(manifest.merged_services().is_empty());
    }

    #[test]
    fn parse_full_manifest() {
        let yaml = r#"
name: full-ext
version: "2.0.0"
description: "A full test extension"

domains:
  - id: testdom
    label: "Test Domain"
    is_internal: true

entity_types:
  - id: widget
    category: concept
    label: Widget
    description: "A test widget"

relationship_types:
  - id: uses_widget
    universal: uses
    label: "Uses Widget"

view_edges:
  structural:
    - uses_widget

noise_patterns:
  - pattern: "TODO"
    pattern_type: literal
    description: "TODO marker"

domain_rules:
  - match_type: uri_prefix
    match_value: "file://test/"
    domain_id: testdom
    priority: 50
    description: "Test files"

domain_groups:
  test_group:
    - testdom

alignment_rules:
  - name: test_ahead
    check_type: ahead
    source_group: test_group
    target_group: specification
    description: "Test entities without spec"

service:
  name: test-svc
  transport: http
  url: "http://localhost:9999"
  extractor_for: testdom

hooks:
  - phase: pre_search
    url: "http://localhost:9999/hook"
    timeout_ms: 3000
    fail_open: false

config_schema:
  test.threshold:
    type: float
    default: 0.5
    description: "Test threshold"
"#;
        let manifest: ExtensionManifest =
            serde_yaml::from_str(yaml).expect("should parse full manifest");
        assert_eq!(manifest.name, "full-ext");
        assert_eq!(manifest.version, "2.0.0");
        assert_eq!(manifest.description, "A full test extension");
        assert_eq!(manifest.domains.len(), 1);
        assert_eq!(manifest.domains[0].id, "testdom");
        assert!(manifest.domains[0].is_internal);
        assert_eq!(manifest.entity_types.len(), 1);
        assert_eq!(manifest.entity_types[0].id, "widget");
        assert_eq!(manifest.relationship_types.len(), 1);
        assert_eq!(
            manifest.relationship_types[0].universal,
            Some("uses".to_string())
        );
        assert_eq!(manifest.view_edges.len(), 1);
        assert_eq!(manifest.noise_patterns.len(), 1);
        assert_eq!(manifest.noise_patterns[0].pattern_type, "literal");
        assert_eq!(manifest.domain_rules.len(), 1);
        assert_eq!(manifest.domain_rules[0].priority, 50);
        assert_eq!(manifest.domain_groups.len(), 1);
        assert_eq!(manifest.alignment_rules.len(), 1);
        assert!(manifest.service.is_some());
        let svc = manifest.service.as_ref().unwrap();
        assert_eq!(svc.extractor_for, Some("testdom".to_string()));
        assert_eq!(manifest.hooks.len(), 1);
        assert!(!manifest.hooks[0].fail_open);
        assert_eq!(manifest.config_schema.len(), 1);
    }

    #[test]
    fn defaults_applied_correctly() {
        let yaml = r#"
name: defaults-test
version: "1.0.0"

noise_patterns:
  - pattern: "test"

domain_rules:
  - match_type: source_type
    match_value: test
    domain_id: code

hooks:
  - phase: post_search
    url: "http://localhost:8080/hook"

alignment_rules:
  - name: test_rule
    check_type: ahead
    source_group: impl
    target_group: spec
"#;
        let manifest: ExtensionManifest =
            serde_yaml::from_str(yaml).expect("should parse with defaults");
        assert_eq!(manifest.noise_patterns[0].pattern_type, "literal");
        assert_eq!(manifest.domain_rules[0].priority, 100);
        assert_eq!(manifest.hooks[0].timeout_ms, 2000);
        assert!(manifest.hooks[0].fail_open);
        assert_eq!(
            manifest.alignment_rules[0].parameters,
            serde_json::Value::Object(serde_json::Map::new())
        );
    }

    #[test]
    fn merged_services_combines_singular_and_plural() {
        let yaml = r#"
name: svc-test
version: "1.0.0"

service:
  name: legacy-svc
  transport: http
  url: "http://localhost:9000"

services:
  - name: new-svc-a
    transport: stdio
    command: my-cmd
  - name: new-svc-b
    transport: http
    url: "http://localhost:9001"
"#;
        let manifest: ExtensionManifest =
            serde_yaml::from_str(yaml).expect("should parse services");
        assert!(manifest.service.is_some());
        assert_eq!(manifest.services.len(), 2);

        let merged = manifest.merged_services();
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].name, "legacy-svc");
        assert_eq!(merged[1].name, "new-svc-a");
        assert_eq!(merged[2].name, "new-svc-b");
    }

    #[test]
    fn merged_services_only_plural() {
        let yaml = r#"
name: svc-test
version: "1.0.0"

services:
  - name: only-svc
    transport: http
    url: "http://localhost:9000"
"#;
        let manifest: ExtensionManifest = serde_yaml::from_str(yaml).expect("should parse");
        assert!(manifest.service.is_none());
        let merged = manifest.merged_services();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].name, "only-svc");
    }

    // --- JSON Schema validation tests ---

    #[test]
    fn schema_validates_minimal_manifest() {
        let json = serde_json::json!({
            "name": "test-ext",
            "version": "1.0.0"
        });
        validate_manifest_json(&json).expect("minimal manifest should validate");
    }

    #[test]
    fn schema_rejects_missing_name() {
        let json = serde_json::json!({
            "version": "1.0.0"
        });
        let err = validate_manifest_json(&json).expect_err("should reject missing name");
        assert!(err.contains("name"), "error should mention 'name': {err}");
    }

    #[test]
    fn schema_rejects_missing_version() {
        let json = serde_json::json!({
            "name": "test-ext"
        });
        let err = validate_manifest_json(&json).expect_err("should reject missing version");
        assert!(
            err.contains("version"),
            "error should mention 'version': {err}"
        );
    }

    #[test]
    fn schema_rejects_wrong_type_for_name() {
        let json = serde_json::json!({
            "name": 123,
            "version": "1.0.0"
        });
        let err = validate_manifest_json(&json).expect_err("should reject numeric name");
        assert!(
            err.contains("name") || err.contains("string"),
            "error should mention the field or type: {err}"
        );
    }

    #[test]
    fn schema_rejects_domain_missing_id() {
        let json = serde_json::json!({
            "name": "test",
            "version": "1.0.0",
            "domains": [{"label": "Missing ID"}]
        });
        let err = validate_manifest_json(&json).expect_err("should reject domain without id");
        assert!(err.contains("id"), "error should mention 'id': {err}");
    }

    #[test]
    fn schema_rejects_entity_type_missing_category() {
        let json = serde_json::json!({
            "name": "test",
            "version": "1.0.0",
            "entity_types": [{
                "id": "widget",
                "label": "Widget"
            }]
        });
        let err =
            validate_manifest_json(&json).expect_err("should reject entity_type without category");
        assert!(
            err.contains("category"),
            "error should mention 'category': {err}"
        );
    }

    #[test]
    fn schema_rejects_hook_missing_url() {
        let json = serde_json::json!({
            "name": "test",
            "version": "1.0.0",
            "hooks": [{"phase": "pre_search"}]
        });
        let err = validate_manifest_json(&json).expect_err("should reject hook without url");
        assert!(err.contains("url"), "error should mention 'url': {err}");
    }

    #[test]
    fn schema_validates_full_manifest() {
        let yaml = r#"
name: full-ext
version: "2.0.0"
description: "A full test extension"
domains:
  - id: testdom
    label: "Test Domain"
    is_internal: true
entity_types:
  - id: widget
    category: concept
    label: Widget
relationship_types:
  - id: uses_widget
    label: "Uses Widget"
domain_rules:
  - match_type: uri_prefix
    match_value: "file://test/"
    domain_id: testdom
hooks:
  - phase: pre_search
    url: "http://localhost:9999/hook"
"#;
        let value: serde_json::Value = serde_yaml::from_str(yaml).expect("parse yaml");
        validate_manifest_json(&value).expect("full manifest should validate");
    }

    /// Validate the 4 default extensions against the schema.
    #[test]
    fn default_extensions_validate_against_schema() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent());

        let ext_dir = match repo_root {
            Some(root) => root.join("extensions"),
            None => return,
        };

        if !ext_dir.is_dir() {
            return;
        }

        let mut validated = 0;
        for entry in std::fs::read_dir(&ext_dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let manifest_path = path.join("extension.yaml");
            if path.is_dir() && manifest_path.exists() {
                let content = std::fs::read_to_string(&manifest_path)
                    .unwrap_or_else(|e| panic!("failed to read {}: {e}", manifest_path.display()));
                let value: serde_json::Value = serde_yaml::from_str(&content)
                    .unwrap_or_else(|e| panic!("failed to parse {}: {e}", manifest_path.display()));
                validate_manifest_json(&value).unwrap_or_else(|e| {
                    panic!(
                        "extension {} failed schema validation: \
                             {e}",
                        manifest_path.display()
                    )
                });
                validated += 1;
            }
        }
        assert!(
            validated >= 4,
            "expected at least 4 default extensions, \
             found {validated}"
        );
    }
}

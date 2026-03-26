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
}

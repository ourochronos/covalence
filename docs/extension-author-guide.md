# Extension Author Guide

## What is an Extension?

An extension is the shareable unit of Covalence functionality. It adds domain-specific ontology types, classification rules, alignment checks, external services, and lifecycle hooks to the core engine -- without modifying any Rust code or recompiling.

An extension is defined by a single YAML manifest file. The engine loads manifests at startup, seeds the database with declared types and rules, registers services, and routes data accordingly.

Extensions are **not** plugins in the dynamic-linking sense. There is no WASM, no shared libraries, no ABI concerns. An extension is declarative config plus optional external processes.

## Directory Structure

Extensions live under the `extensions/` directory at the repo root. Each extension gets its own subdirectory containing an `extension.yaml` manifest:

```
extensions/
  core/extension.yaml              # Universal primitives (categories, universals)
  code-analysis/extension.yaml     # Code entity types, structural edges
  spec-design/extension.yaml       # Spec/design domains, bridge types
  research/extension.yaml          # Research domains, epistemic edges
  your-domain/extension.yaml       # Your extension
```

The engine scans `extensions/` at startup and loads every subdirectory that contains an `extension.yaml`.

## Manifest Schema Reference

The manifest is a YAML file with the following top-level fields. Only `name` and `version` are required -- all other sections default to empty.

### name (required)

Unique extension identifier. Use lowercase with hyphens.

```yaml
name: code-analysis
```

### version (required)

Semantic version string.

```yaml
version: "1.0.0"
```

### description

Human-readable description of the extension's purpose.

```yaml
description: >
  Source code analysis domain pack. Defines code entity types,
  structural relationship types, and domain classification rules.
```

### domains

Domains are visibility scopes for sources. Each domain has:

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `id` | string | yes | -- | Domain identifier (e.g. `"code"`) |
| `label` | string | yes | -- | Human-readable label |
| `description` | string | no | `null` | Optional description |
| `is_internal` | bool | no | `false` | Whether this domain is internal (affects DDSS self-referential boost) |

```yaml
domains:
  - id: code
    label: Code
    description: "Source code and implementation artifacts"
    is_internal: true
```

### entity_types

Entity types define the kinds of nodes that can exist in the graph. Each type belongs to a MAGMA category defined by the `core` extension (concept, process, artifact, agent, property, collection).

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `id` | string | yes | -- | Entity type identifier (e.g. `"function"`) |
| `category` | string | yes | -- | MAGMA category (concept, process, artifact, agent, property, collection) |
| `label` | string | yes | -- | Human-readable label |
| `description` | string | no | `null` | Optional description |

```yaml
entity_types:
  - id: function
    category: process
    label: Function
    description: "A named function"
  - id: struct
    category: concept
    label: Struct
    description: "A struct or data type"
```

### relationship_types

Relationship types define the kinds of edges that can exist in the graph. Each type can optionally map to a universal relationship defined by the `core` extension.

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `id` | string | yes | -- | Relationship type identifier (e.g. `"calls"`) |
| `universal` | string | no | `null` | Universal relationship this maps to (e.g. `"uses"`, `"part_of"`, `"is_a"`) |
| `label` | string | yes | -- | Human-readable label |
| `description` | string | no | `null` | Optional description |

```yaml
relationship_types:
  - id: calls
    universal: uses
    label: Calls
    description: "Function/method call dependency"
  - id: contains
    universal: part_of
    label: Contains
    description: "Module or scope containment"
```

### view_edges

Maps view names to lists of relationship type IDs. Views group edge types for filtered graph queries (e.g., show only structural edges, or only causal edges).

```yaml
view_edges:
  structural:
    - calls
    - uses_type
    - contains
    - imports
  causal:
    - enables
    - supports
    - contradicts
```

### noise_patterns

Patterns for filtering out noise entities during extraction. Each pattern is either a literal string or a regex.

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `pattern` | string | yes | -- | The pattern string |
| `pattern_type` | string | no | `"literal"` | `"literal"` for exact match, `"regex"` for regular expression |
| `description` | string | no | `null` | Optional description |

```yaml
noise_patterns:
  - pattern: "TODO"
    pattern_type: literal
    description: "TODO marker"
  - pattern: "^[A-Z]{2,4}$"
    pattern_type: regex
    description: "Short all-caps abbreviations"
```

### domain_rules

Rules for automatically classifying sources into domains. Sources are matched against rules in priority order (lower number = higher priority). The first matching rule wins.

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `match_type` | string | yes | -- | `"source_type"`, `"uri_prefix"`, or `"uri_regex"` |
| `match_value` | string | yes | -- | Value to match against |
| `domain_id` | string | yes | -- | Domain ID to assign on match |
| `priority` | int | no | `100` | Priority (lower = higher priority) |
| `description` | string | no | `null` | Optional description |

```yaml
domain_rules:
  - match_type: source_type
    match_value: code
    domain_id: code
    priority: 10
    description: "Code sources"
  - match_type: uri_prefix
    match_value: "file://engine/"
    domain_id: code
    priority: 40
    description: "Engine source files"
```

### domain_groups

Named groups of domain IDs used by alignment rules. A group aggregates multiple domains for cross-domain analysis.

```yaml
domain_groups:
  implementation:
    - code
  specification:
    - spec
    - design
  evidence:
    - research
    - external
```

### alignment_rules

Cross-domain alignment checks. Each rule compares entities between a source domain group and a target domain group to detect drift.

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | string | yes | -- | Unique rule name |
| `check_type` | string | yes | -- | `"ahead"`, `"contradiction"`, or `"staleness"` |
| `source_group` | string | yes | -- | Source domain group name |
| `target_group` | string | yes | -- | Target domain group name |
| `description` | string | no | `null` | Optional description |
| `parameters` | object | no | `{}` | Additional parameters as JSON (e.g. `source_domain`, `target_domain`) |

Check types:
- **ahead** -- entities exist in source group but have no counterpart in target group
- **contradiction** -- entities in source group contradict entities in target group
- **staleness** -- entities in source group have diverged from their target counterparts

```yaml
alignment_rules:
  - name: code_ahead
    check_type: ahead
    source_group: implementation
    target_group: specification
    description: "Code entities with no matching spec concept"
  - name: stale_design
    check_type: staleness
    source_group: specification
    target_group: implementation
    description: "Design docs whose descriptions diverge from code reality"
    parameters:
      source_domain: design
```

### service

An optional external service definition. Services are registered in the `ServiceRegistry` and validated at startup.

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | string | yes | -- | Service name |
| `transport` | string | yes | -- | `"stdio"` or `"http"` |
| `command` | string | no | `null` | Command to execute (STDIO transport only) |
| `args` | list | no | `[]` | Command arguments (STDIO transport only) |
| `url` | string | no | `null` | Base URL (HTTP transport only) |

STDIO services follow the [STDIO Service Contract](stdio-service-contract.md). HTTP services must respond to a GET on their base URL for validation.

```yaml
# STDIO service
service:
  name: ast-extractor
  transport: stdio
  command: covalence-ast-extractor
  args: []

# HTTP service
service:
  name: pdf-converter
  transport: http
  url: "http://localhost:9000"
```

### hooks

Lifecycle hooks are HTTP POST callbacks at pipeline points. See [Lifecycle Hooks](lifecycle-hooks.md) for the full specification.

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `phase` | string | yes | -- | Pipeline phase (see lifecycle-hooks.md) |
| `url` | string | yes | -- | URL to POST to |
| `timeout_ms` | int | no | `2000` | Per-hook timeout in milliseconds |
| `fail_open` | bool | no | `true` | If true, errors are logged but the pipeline continues |

```yaml
hooks:
  - phase: post_synthesis
    url: "http://localhost:9090/on-synthesis"
    timeout_ms: 3000
    fail_open: true
```

### config_schema

Declares configuration knobs that operators can override via `covalence.conf.d/` files. Each key becomes available under the `extensions.<name>.<key>` namespace.

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `type` | string | yes | -- | `"string"`, `"integer"`, `"boolean"`, or `"float"` |
| `default` | any | no | `null` | Default value |
| `description` | string | no | `null` | Human-readable description |

```yaml
config_schema:
  min_entity_size:
    type: integer
    default: 3
    description: "Minimum entity name length"
  threshold:
    type: float
    default: 0.5
    description: "Extraction confidence threshold"
```

## Operator Configuration Overrides

Operators customize extension behavior via files in `covalence.conf.d/`:

```yaml
# covalence.conf.d/10-code-analysis.conf
extensions.code-analysis.enabled: true
extensions.code-analysis.min_entity_size: 5
extensions.code-analysis.service.command: /usr/local/bin/ast-extractor
```

Config precedence (lowest to highest):
1. Hardcoded defaults in code
2. `covalence.conf` (instance base)
3. `covalence.conf.d/*.conf` (alphabetical order, last value wins)
4. Environment variables (`COVALENCE_*`)

## Example: Creating a Minimal Extension

Here is a complete walkthrough for creating a `security-audit` extension that adds a security domain with vulnerability entity types.

### 1. Create the directory

```bash
mkdir extensions/security-audit
```

### 2. Write the manifest

```yaml
# extensions/security-audit/extension.yaml
name: security-audit
version: "1.0.0"
description: "Security vulnerability tracking and audit domain"

domains:
  - id: security
    label: Security
    description: "Security audit findings and vulnerability reports"
    is_internal: true

entity_types:
  - id: vulnerability
    category: concept
    label: Vulnerability
    description: "A known security vulnerability (CVE, CWE, etc.)"
  - id: mitigation
    category: process
    label: Mitigation
    description: "A remediation or mitigation strategy"

relationship_types:
  - id: mitigates
    universal: supports
    label: Mitigates
    description: "A mitigation addresses a vulnerability"
  - id: affects
    universal: uses
    label: Affects
    description: "A vulnerability affects a component"

domain_rules:
  - match_type: uri_prefix
    match_value: "file://security/"
    domain_id: security
    priority: 15
    description: "Security audit reports"

domain_groups:
  security_scope:
    - security

alignment_rules:
  - name: unmitigated_vulns
    check_type: ahead
    source_group: security_scope
    target_group: implementation
    description: "Vulnerabilities with no corresponding mitigation in code"

config_schema:
  severity_threshold:
    type: string
    default: "medium"
    description: "Minimum severity to track (low, medium, high, critical)"
```

### 3. Verify loading

Start the engine and check that the extension was loaded:

```bash
make run

# In another terminal:
curl -s http://localhost:8431/api/v1/admin/extensions | jq
```

The response should list `security-audit` among the loaded extensions.

### 4. Configure (optional)

```yaml
# covalence.conf.d/10-security-audit.conf
extensions.security-audit.severity_threshold: high
```

## Loading Behavior

- All inserts use `ON CONFLICT DO NOTHING`, so loading is **idempotent**. Re-starting the engine with the same manifest does not create duplicates.
- If a manifest fails to parse, the engine logs a warning and continues loading other extensions.
- If database seeding fails for an extension, the engine logs a warning and continues.
- Extensions are loaded in alphabetical order by directory name.

# ADR-0023: Extension Model and Layered Configuration

**Status:** Proposed

**Date:** 2026-03-26

## Context

Covalence's core is becoming domain-agnostic infrastructure (ADR-0020, ADR-0022). Features currently baked into the Rust binary — AST extraction, spec/design analysis, research ingestion, cross-domain alignment — should be decomposable into independent, community-shareable units that extend the core without modifying it.

The infrastructure for this exists: configurable ontology tables, domain groups, alignment rules, domain classification rules, a service registry (HTTP + STDIO), and lifecycle hooks. What's missing is the **composition model** — how these pieces are declared, packaged, configured, and loaded.

Additionally, configuration is currently split across `.env` files, DB tables, and hardcoded defaults with no clear precedence model. Operators need IaC-friendly configuration that's version-controllable and diffable, while also supporting UI-driven changes.

## Decision

### Extensions

An **extension** is the shareable unit of Covalence functionality. It declares:

- **Ontology additions** — entity types, relationship types, domains, view edge mappings
- **Domain rules** — how to classify sources into this extension's domains
- **Alignment rules** — domain-specific quality checks
- **Services** — external processes (STDIO or HTTP) for ingestion, extraction, or transformation
- **Lifecycle hooks** — callbacks at pipeline points (pre_search, post_search, post_synthesis)
- **Configuration schema** — what knobs are available to operators, with types and defaults

An extension is defined by a manifest file (`extension.yaml`) that declares all of the above declaratively. The core engine loads the manifest, seeds/updates the appropriate DB tables, registers services, and routes data based on domain rules. **No Rust recompilation required.**

Extensions are external processes + config. They are not plugins in the dynamic-linking sense — no WASM, no shared libraries, no Rust ABI concerns.

### Layered Configuration

Configuration uses a `covalence.conf` + `covalence.conf.d/` pattern:

```
covalence.conf              # instance-level settings
covalence.conf.d/
  10-code-analysis.conf     # extension operator config
  20-research-papers.conf   # extension operator config
  99-ui-overrides.conf      # managed by UI (auto-generated)
```

**Composition rules:**
- All `.conf` files merge into a single config tree
- Files load in alphabetical order within `covalence.conf.d/`
- `covalence.conf` loads first (base)
- **Last value wins** — later files override earlier ones for the same key
- On conflict: log a warning at startup identifying the overriding file and both values
- Environment variables override file config (for containers, CI, secrets injection)

**Precedence (lowest to highest):**
1. Hardcoded defaults in code
2. `covalence.conf` (instance base)
3. `covalence.conf.d/*.conf` (alphabetical order)
4. Environment variables (`COVALENCE_*`)

**UI persistence:** The UI writes changes to `covalence.conf.d/99-ui-overrides.conf`. Because `99-` sorts last, UI changes override file-defined defaults. Operators can see exactly what the UI changed by reading this one file. Deleting it reverts to file-defined config.

**DB role:** The `config` table becomes a runtime cache of the merged config, not a separate authority. The loader reads all files, merges, and writes the result to the DB for services to read at runtime.

### Extension Manifest

```yaml
# extension.yaml — published by extension author
name: code-analysis
version: "1.0"
description: "AST-aware code entity extraction and structural analysis"

# Ontology additions
domains:
  - id: code
    label: Code
    is_internal: true

entity_types:
  - id: function
    category: process
    label: Function
  - id: struct
    category: concept
    label: Struct
  # ...

relationship_types:
  - id: calls
    universal: uses
    label: Calls
  - id: contains
    universal: part_of
    label: Contains
  # ...

view_edges:
  structural: [calls, contains, imports, uses_type]

# Domain classification rules
domain_rules:
  - match_type: source_type
    match_value: code
    priority: 10

# Alignment rules
alignment_rules:
  - name: code_ahead
    check_type: ahead
    source_group: implementation
    target_group: specification

# Service definition
service:
  name: ast-extractor
  transport: stdio
  command: covalence-ast-extractor
  args: []

# Lifecycle hooks
hooks:
  - phase: post_synthesis
    url: "${service.url}/on-synthesis"

# Configuration schema (operator-facing knobs)
config_schema:
  languages:
    type: array
    default: [rust, python, go]
    description: "Languages to extract AST from"
  min_entity_size:
    type: integer
    default: 3
    description: "Minimum entity name length"
```

### Extension Operator Config

```yaml
# covalence.conf.d/10-code-analysis.conf
extensions.code-analysis.enabled: true
extensions.code-analysis.languages: [rust, python, go, typescript]
extensions.code-analysis.service.command: /usr/local/bin/ast-extractor
```

### Config File Format

YAML with dotted keys for namespacing. Flat key-value pairs within sections:

```yaml
# covalence.conf
database.url: postgres://covalence:covalence@localhost:5432/covalence
bind_addr: "0.0.0.0:8080"
embedding.provider: voyage
embedding.model: voyage-3-large
search.cache_ttl: 300
graph.engine: petgraph
```

### Extension Loader Lifecycle

At startup:
1. Read `covalence.conf` → base config
2. Scan `covalence.conf.d/*.conf` alphabetically → merge (last wins, warn on override)
3. Apply environment variable overrides
4. For each enabled extension:
   a. Read its `extension.yaml` manifest
   b. Seed/update ontology tables (entity types, rel types, domains, views)
   c. Seed/update domain rules, alignment rules, domain groups
   d. Register services in ServiceRegistry
   e. Register lifecycle hooks
   f. Apply operator config overrides from merged config
5. Write merged config to DB `config` table (runtime cache)
6. Validate all registered services

On config reload (hot or via API):
- Re-read files, re-merge, diff against current state
- Apply changes to DB tables and service registry
- Log what changed

### Permissions

Deferred. Config keys are namespaced (`extensions.code-analysis.*`), which makes RBAC layerable later without restructuring. No permission model is designed or implemented in this ADR.

## Consequences

### Positive

- **Community extensions** — anyone can package and share domain-specific functionality without forking Covalence
- **IaC-friendly** — all config is file-based, version-controllable, diffable
- **UI and CLI coexist** — UI writes to `99-ui-overrides.conf`, operators edit files directly, both are valid
- **Composability** — extensions declare orthogonal types and rules that compose at config time
- **No recompilation** — extensions are external processes + declarative config
- **Self-describing** — extension manifests can be ingested into the graph, enabling Covalence to reason about its own extension ecosystem

### Negative

- **File management complexity** — operators need to understand the layered config model
- **Startup cost** — scanning directories, parsing YAML, reconciling with DB adds boot time
- **Migration from .env** — existing deployments need to transition from env-var-only config
- **Manifest validation** — malformed extension manifests could break startup; needs robust error handling
- **No dynamic loading** — extensions must be registered at startup (or via hot reload), not discovered at runtime

## Alternatives Considered

### WASM plugin system
Rejected. Rust WASM tooling is immature for this use case, ABI stability is fragile, and the target users (infra engineers) are more comfortable with external processes + config than WASM modules.

### Dynamic Rust libraries (cdylib)
Rejected. Same ABI stability issues. Requires matching Rust toolchain versions between core and plugins. Defeats the "no recompilation" goal.

### DB-only configuration
Rejected. Not IaC-friendly. Can't `git diff` config changes. Hard to reproduce deployments. The DB serves as a runtime cache, not the source of truth.

### Single config file (no conf.d)
Rejected. Doesn't scale to multiple extensions. Forces operators to manage one growing file. The `conf.d/` pattern is proven in infrastructure tooling.

### First-value-wins for conflicts
Rejected. Last-wins with warnings is more intuitive for layered config (base → overrides → UI). Operators expect later files to override earlier ones. First-wins would require careful file ordering to prevent accidental lockout.

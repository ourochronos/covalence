# ADR-0019: Generalization Over Hardcoding

**Status:** Superseded by [ADR-0023](0023-extensions-and-config.md)

**Date:** 2026-03-18

## Context

Covalence is currently used to analyze itself — its own code, specs, and research. This works well and has produced real improvements. However, the system is intended to be general-purpose: other codebases, other teams, other domains.

During Session 40, we identified several places where Covalence-specific assumptions are hardcoded:

- Component names in `constants.rs` ("Ingestion Pipeline", "Search Fusion")
- Module path mappings in `constants.rs` (hardcoded file paths like `src/ingestion/`)
- Domain classification rules in `source.rs` (`derive_domain` checks for `file://spec/`, `file://engine/`)
- DDSS internal domain list (`["spec", "design", "code"]`)
- Default project name `"covalence"` in migration 014

These work fine for self-analysis but would need to be different for any other project.

## Decision

**Keep building features at full speed, but stop adding new hardcoded Covalence-specific values.** Anything that references Covalence by name, assumes a specific file layout, or hardcodes domain classification rules should take those values from configuration or from source/project metadata.

Specifically:

1. **New features** must not introduce Covalence-specific assumptions. Use config, environment variables, or project-level metadata.

2. **Existing hardcoding** will be refactored incrementally — not all at once, but as each area is touched for other reasons. Don't create a dedicated "remove hardcoding" project; fix it when you're already in the file.

3. **The `project` field** on sources (added in migration 014) is the mechanism for multi-project support. Domain classification, component definitions, and path mappings should eventually be per-project configuration, not global constants.

4. **Config over constants**: when adding domain rules, component definitions, or path patterns, prefer `COVALENCE_*` environment variables or a config file over `const` arrays in Rust code.

## Consequences

### Positive

- Every feature we build is usable by others without forking
- Multi-project support (ingesting two codebases) becomes straightforward
- Open-source release doesn't require stripping Covalence-specific code

### Negative

- Slightly more work per feature (config lookup vs hardcoded value)
- Need to document what config a new project needs to provide
- Some features (like DDSS) are simpler with hardcoded assumptions

## What NOT to do

- Don't prematurely abstract. If there's only one project using Covalence, a hardcoded default with a config override is fine.
- Don't create a massive refactoring PR to remove all existing hardcoding at once. That's risky and the current code works.
- Don't add a plugin system or extensibility framework yet. Simple config and per-project metadata are sufficient.

# ADR-0013: Three-Tier Clearance Levels for Federation

**Status:** Accepted

**Date:** 2026-03-07

**Spec Reference:** spec/09-federation.md

## Context

The system must support sharing knowledge with trusted peers while preventing private data from leaking. Every node, edge, and chunk needs a classification.

## Decision

Three clearance levels:

| Level | Name | Scope |
|-------|------|-------|
| 0 | `local_strict` | Never leaves the local node |
| 1 | `federated_trusted` | Shared with whitelisted peers |
| 2 | `federated_public` | Fully sharable in zero-trust broadcast |

All data defaults to `local_strict` (secure by default). Promotion requires explicit `POST /admin/publish/:source_id?clearance_level=N`. Nodes inherit the most restrictive clearance of any contributing source. Edges inherit `min(source_node, target_node)`.

## Consequences

### Positive

- Secure by default — nothing leaks without explicit promotion
- Simple model (3 levels) is easy to reason about
- petgraph `Filtered` trait provides zero-copy filtered views for egress
- Inheritance rules prevent accidental exposure

### Negative

- Three tiers may be too coarse for complex organizations
- Promoting a source requires recursive update of all derivatives
- Can't share a node publicly if any contributing source is private (conservative but safe)

## Alternatives Considered

- **Binary (public/private):** Too coarse, no trusted-peer tier
- **Role-based access control:** Too complex for v1, adds user management
- **Per-field clearance:** Extremely granular but massive implementation overhead

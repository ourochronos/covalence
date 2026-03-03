# ADR-001: covalence-network as Separate Project

## Status
Accepted (2026-03-03)

## Context
With the Covalence engine reaching stability (127+ tests, 4 search dimensions, temporal edges, reconsolidation), the question arose whether federation (covalence-network) should live inside the engine repo or as a standalone project.

## Decision
covalence-network is a **separate project/repo**, not part of the covalence engine.

## Alternatives Considered

### Monorepo (engine + network in one repo)
- **Pro**: Easier coordinated changes, shared types, single CI pipeline
- **Con**: Couples deployment lifecycles, makes the engine repo more complex, federation is optional functionality

### Network as engine module
- **Pro**: Maximum code sharing, single binary
- **Con**: Every engine deployment includes federation code even when unused, testing complexity, different operational concerns (networking vs. local knowledge)

## Reasoning
- **Deployment independence**: Federation is optional. Most Covalence instances will run standalone. Coupling federation into the engine means shipping networking code to users who don't need it.
- **Dependency direction**: Network depends on engine's HTTP API, not on engine internals. The API is already versioned and stable. This is a clean dependency boundary.
- **Operational concerns**: Federation involves networking, peer discovery, sync protocols, conflict resolution — fundamentally different from local knowledge operations. Mixing them adds complexity to both.
- **Development pace**: The engine is shipping features rapidly. Federation needs careful design (CRDTs, consensus, trust). Different timelines, different risk profiles.

## Consequences
- **covalence-network** repo handles: federation protocol, peer sync, namespace federation, cross-node trust
- **covalence** (engine) provides: HTTP API as the contract, no internal knowledge of federation
- Integration testing will require a multi-process test harness
- Shared types (if any) go in a separate crate or are duplicated with the API spec as source of truth

## Conditions to Revisit
- If coordinated changes across both repos happen more than twice per week → reconsider merging
- If the API contract becomes insufficient and network needs engine internals → reconsider architecture
- If federation becomes non-optional (every instance must federate) → reconsider embedding

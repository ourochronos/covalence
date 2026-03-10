# Design: Federation

## Status: schema-ready, not implemented

## Spec Sections: 09-federation.md, 07-epistemic-model.md

## Architecture Overview

Federation enables multiple Covalence instances to share knowledge selectively while maintaining epistemic independence. Each instance maintains local control over what it shares (clearance levels), how it trusts remote sources (trust discounting), and how it handles conflicting evidence from peers (epistemic isolation).

## Implemented Components

### Schema-Ready ✅ (data model exists, no runtime logic)

| Component | Location | Notes |
|-----------|----------|-------|
| **ClearanceLevel enum** | `types/clearance.rs` | Three levels: LocalStrict (default), FederatedTrusted, FederatedPublic |
| **Clearance on nodes** | `models/node.rs` | `clearance_level: ClearanceLevel` field on all nodes |
| **Clearance filtering** | `graph/filtered.rs` | `filtered_view()` can filter by clearance level |
| **Synthetic edge flag** | `models/edge.rs` | `is_synthetic` flag for federation-generated edges |
| **Outbox events table** | DB schema | `outbox_events` exists for reliable event publishing |

### Not Implemented ❌

| Component | Spec Reference | Priority |
|-----------|---------------|----------|
| **Peer discovery & registration** | Spec 09: peer node management | Blocked on federation scope decision |
| **Egress filtering** | Spec 09: "egress-filtered subgraph" | Not started |
| **Ingress quarantine** | Spec 09: "Standard quarantine" for received data | Not started |
| **Trust discounting** | Spec 09: opinion discounting by peer trust level | Not started |
| **HMAC attestation** | Spec 09: "HMAC-based attestation" for provenance | Not started |
| **Proof hashing** | Spec 09: "proof_hash" for zero-knowledge claims | Not started |
| **Publish endpoint** | Spec 08: `POST /admin/publish/:source_id` | Not started |
| **Federated views** | Spec 09: Local/Federated/Public view layers | Not started |
| **Epistemic isolation** | Spec 09: "Epistemic Algorithm Isolation" | Not started |
| **PROVEN_LINK edges** | Spec 09: verified federation relationships | Schema only |

## Key Design Decisions

### Why clearance levels over ACLs
Three-level clearance (local/trusted/public) is simpler than per-entity ACLs and maps naturally to federation trust tiers. All new data defaults to LocalStrict — explicit promotion required for sharing.

### Why epistemic isolation
When a peer shares evidence, their confidence scores should NOT directly overwrite local opinions. Instead, remote opinions are discounted by the peer's trust level (Subjective Logic discount operator) and then fused. This prevents a single unreliable peer from corrupting the local knowledge base.

### Why "The Airgap"
No raw database access across federation boundaries. All data exchange happens through structured API endpoints with clearance checks. This prevents side-channel attacks where graph traversal patterns leak private information.

### Why this might not be part of Covalence (#35)
Federation is architecturally separable — it's a layer on top of the core knowledge engine. It could be a separate service that coordinates multiple Covalence instances rather than being built into each instance. Decision deferred.

## Gaps Identified by Graph Analysis

1. **Lowest grounding of any section** — 39.2% of federation concepts have paper backing (up from 16.5% after TrustRank/EigenTrust ingestion, but still lowest)

2. **Federation spec references "ZK-SNARK"** — zero-knowledge proofs for federated claims. This is cutting-edge and may be over-specified for current needs.

3. **Trust model undefined** — how is peer trust initially established? What's the bootstrap? TrustRank paper provides the algorithm but not the peer authentication.

4. **No conflict resolution protocol** — what happens when peers disagree? DS combination handles evidence fusion, but what about schema conflicts (different entity types for the same entity)?

## Academic Foundations

| Concept | Paper | Status in KB |
|---------|-------|-------------|
| TrustRank | Gyöngyi et al. 2004 | ✅ Ingested |
| EigenTrust | Kamvar et al. 2003 | ✅ Ingested |
| Subjective Logic trust transitivity | Jøsang 2016 | ✅ Ingested |
| Beta reputation | Jøsang & Ismail 2002 | ✅ Just ingested |
| Zero-knowledge proofs | — | ❌ Not ingested (may be over-scoped) |
| Byzantine fault tolerance | — | ❌ Not ingested (needed for adversarial peers) |
| Federated learning | — | ❌ Tangentially relevant |

## Next Actions

1. Decide: is federation part of Covalence or a separate coordination layer? (#35)
2. If in-scope: implement egress filtering and publish endpoint first (minimal viable federation)
3. Trust bootstrap: define how peer trust is initially established (manual registration? challenge-response?)
4. Ingest papers on distributed trust and Byzantine fault tolerance

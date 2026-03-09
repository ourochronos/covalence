# 09 — Federation & Compartmentalization

**Status:** Draft

## Overview

The system supports federation — sharing knowledge with trusted peer nodes — while maintaining strict compartmentalization of private data. The core challenge is **epistemic leakage**: ensuring the public graph doesn't accidentally expose private information through connecting edges, synthesis, or inference.

## Clearance Levels

Every node, edge, and chunk carries a `clearance_level` — an atomic-level classification tag:

| Level | Name | Scope |
|-------|------|-------|
| 0 | `local_strict` | Never leaves the local node. Personal notes, API keys, private conversations. |
| 1 | `federated_trusted` | Shared only with explicitly whitelisted peer nodes. |
| 2 | `federated_public` | Fully sharable in zero-trust broadcast. |

### Clearance Rules

- **Source-level default:** The `clearance_level` on a source record is the default for all chunks and extractions from that source.
- **Chunk override:** Individual chunks within a public document can be marked `local_strict` (e.g., a redacted paragraph). A chunk's clearance is `min(source_clearance, chunk_clearance)` — i.e., a chunk in a private source is always private regardless of its own tag.
- **Node/edge inheritance:** Extracted nodes and edges inherit the **most restrictive** clearance of any source they were extracted from. If a node is mentioned in both a public and a private source, it inherits `local_strict` until explicitly reclassified.
- **Edge clearance:** An edge's clearance is `min(source_node_clearance, target_node_clearance)`. An edge connecting a public and a private node is private.

## Egress Filtering (The Airgap)

When the system prepares data for federation broadcast, it never queries the raw database directly. Instead:

1. **Build filtered subgraph** — Using petgraph's `Filtered` trait, create a zero-copy view of the graph that masks any node or edge where `clearance_level < required_level`.
2. **Package assertion payload** — The broadcast payload is built exclusively from the filtered view. Private data is mathematically invisible to the broadcast function.
3. **Validate before send** — A final check ensures no `local_strict` entity IDs appear in the payload.

```rust
use petgraph::visit::NodeFiltered;

fn egress_view(graph: &GraphSidecar, min_clearance: ClearanceLevel) -> NodeFiltered<&DiGraph<NodeMeta, EdgeMeta>, impl Fn(NodeIndex) -> bool> {
    NodeFiltered::from_fn(graph, |idx| {
        graph[idx].clearance_level >= min_clearance
    })
}
```

## Dual Synthesis (Private vs Public Views)

When the system synthesizes articles or summaries from a topic cluster that contains mixed-clearance data, it produces **two versions**:

### Private Article (Local View)
- Fed the full cluster: public + trusted + private nodes
- Generates a detailed article with all available information
- Tagged `clearance_level: local_strict`
- Only visible to the local user

### Public Article (Federated View)
- Fed only the filtered public subgraph
- Generates a coherent, self-contained article from public data only
- Tagged `clearance_level: federated_public`
- Safe for broadcast

The key constraint: the public article must be **coherent on its own**, not a redacted version of the private article. The LLM generates them independently from different input sets.

## Zero-Knowledge Edges

> **Implementation status: Deferred to post-v1.** The cryptographic scheme (HMAC-based attestation vs ZK-SNARK) must be selected before implementation. For v1, private intermediate nodes simply create a gap in the federated graph — no synthetic bridging. This is the safer default.

Sometimes two public nodes are connected through a private intermediate node:

```
Node A (Public) → Node B (Private) → Node C (Public)
```

If we just filter out Node B, the federated network sees A and C as unrelated. The local node loses authority on the subject.

**Solution:** Generate a synthetic **zero-knowledge edge** during egress filtering:

```
(Node A) -[PROVEN_LINK {proof_hash: "0xABC..."}]-> (Node C)
```

**Properties:**
- The edge carries a cryptographic proof that the local node possesses a valid, unbroken semantic path between A and C
- Federated peers can verify the proof without learning what Node B is
- The proof hash is computed from the private path's content hashes (blake3)
- The synthetic edge does not carry relationship type or properties from the private path — only existence

**Constraints:**
- ZK edges are only generated when both endpoints are `federated_public`
- The private intermediate must be a single hop (multi-hop private paths are not bridged — too much structural leakage)
- ZK edges are tagged as synthetic and are not included in local graph algorithm computations

## Federation Protocol

### Outbound (Sharing)

1. Trigger: scheduled sync, manual push, or event-driven (significant epistemic delta)
2. Build egress-filtered subgraph at the target clearance level
3. Generate ZK edges for public nodes connected through private intermediates
4. Package assertion payload: new/updated nodes, edges, chunks, and their metadata
5. Sign payload with local node's identity key
6. Broadcast to appropriate peers based on clearance level

### Inbound (Receiving)

1. Receive signed assertion payload from peer
2. Verify signature against known peer identity
3. Tag all incoming data with `federation_origin: peer_node_id` in metadata
4. **Quarantine:** Incoming data enters a staging area before integration
   - Confidence is initially reduced (federated claims start at 0.7× local confidence)
   - Entity resolution runs against local graph
   - Contradictions with local data are flagged, not auto-resolved
5. Integrate into local graph after quarantine processing
6. Incoming nodes/edges retain their `federated_public` clearance — they don't automatically become `local_strict` just because they're now local

### Trust Tiers

| Tier | Trust Level | Behavior |
|------|-------------|----------|
| Unknown | 0.3 | Maximum quarantine, heavy confidence penalty |
| Recognized | 0.5 | Standard quarantine, moderate confidence penalty |
| Trusted | 0.7 | Light quarantine, minimal confidence penalty |
| Verified | 0.9 | Auto-integrate, near-local confidence |

Trust is built over time: peers whose claims consistently align with local knowledge (and other peers) have their trust tier increased.

## Data Model Additions

### Source metadata for federation

```json
{
  "federation_origin": "local | peer_node_id",
  "federation_received_at": "2026-03-07T10:29:20Z",
  "federation_trust_tier": "unknown | recognized | trusted | verified",
  "quarantine_status": "pending | integrated | rejected"
}
```

### Node/edge clearance

```sql
ALTER TABLE nodes ADD COLUMN clearance_level INT NOT NULL DEFAULT 0;  -- local_strict (secure by default)
ALTER TABLE edges ADD COLUMN clearance_level INT NOT NULL DEFAULT 0;
ALTER TABLE chunks ADD COLUMN clearance_level INT NOT NULL DEFAULT 0;

CREATE INDEX idx_nodes_clearance ON nodes(clearance_level);
CREATE INDEX idx_edges_clearance ON edges(clearance_level);
```

## Epistemic Algorithm Isolation

**Critical constraint:** Epistemic algorithms (PageRank, TrustRank, topological confidence) must be computed **twice** when federation is active:

1. **Full graph** — For local queries. Includes all nodes/edges regardless of clearance.
2. **Egress-filtered graph** — For federation broadcast. Computed exclusively on the `federated_public` subgraph.

**Why:** If TrustRank runs on the full graph, private edges inflate the topological confidence of public nodes. Broadcasting those inflated scores leaks information about the existence of private evidence — a side-channel attack.

Federated confidence scores must be strictly derived from public data. The Rust sidecar must maintain two sets of cached scores, or recompute on the filtered view before each broadcast.

## Clearance Promotion

Data defaults to `local_strict` (clearance = 0). Promoting to `federated_public` requires an explicit action:

```
POST /admin/publish/:source_id?clearance_level=2
```

This endpoint recursively upgrades the clearance of a source and all its derivative chunks and extractions. An audit log entry records the promotion. Nodes/edges extracted from the promoted source have their clearance recalculated based on the most restrictive remaining source.

## Open Questions

- [x] Clearance reclassification → Yes, would need ZK edge regeneration, but ZK edges deferred to post-v1. Moot for now.
- [x] Quarantine: auto vs manual → Auto for Trusted/Verified tiers. Manual review for Unknown/Recognized. See Zero Trust Architecture in Covalence KB.
- [x] Peer correction of our claims → Accept as `CONTENDS` edge (0.3× attack weight). Peer corrections don't auto-override local claims. Enter DF-QuAD argumentation framework. Local operator can promote to `CORRECTS` if warranted.
- [x] ZK edge crypto → Deferred to post-v1. HMAC-based attestation is simpler and sufficient. Full ZK-SNARK is overkill.
- [x] Clearance granularity → Three tiers sufficient for v1. More granularity adds complexity without clear benefit.
- [x] Multi-source clearance → Most restrictive. Already specified: node inherits lowest clearance of any contributing source.
- [x] Peer trust revocation → Quarantine all contributions from revoked peer. Re-evaluate each claim: sole provenance → mark inactive, multi-provenance → remove peer's contribution from confidence calculation. Log in audit_logs. See EigenTrust + Zero Trust articles in Covalence KB.

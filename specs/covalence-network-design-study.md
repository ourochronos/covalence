# Covalence Network: Federated Knowledge Design Study

**Issue:** tracking#102  
**Date:** 2026-03-02  
**Status:** Design Study — No Implementation  
**Author:** Architecture Research Agent (subagent, depth 1/3)

---

## Table of Contents

1. [Problem Statement](#1-problem-statement)
2. [Architecture Options](#2-architecture-options)
3. [Recommended Architecture](#3-recommended-architecture)
4. [The Seven Research Questions](#4-the-seven-research-questions)
5. [What Valence-v2 Got Right and Wrong](#5-what-valence-v2-got-right-and-wrong)
6. [Phased Roadmap](#6-phased-roadmap)
7. [Open Problems](#7-open-problems)

---

## 1. Problem Statement

### What Problem Does Federation Solve?

A single-node Covalence instance is fundamentally limited by the knowledge it can ingest. An agent that can only recall what its own node has seen is epistemically parochial — it cannot benefit from sources ingested by a colleague's node, a team's shared instance, or the wider research community's accumulated knowledge. The knowledge graph stays siloed even as the agents using it collaborate.

More specifically, federation addresses three distinct failure modes:

**1. Knowledge locality.** When two agents on separate Covalence instances independently ingest overlapping sources, they produce independent compilations, independent confidence scores, and independent contention resolutions — none of which benefit from the other's work. Every node re-does the same epistemic labor.

**2. Single point of failure.** A single Covalence node is a single point of both failure and trust. All confidence scores, all PageRank computations, all contention resolutions are local to one operator. For use cases requiring shared epistemic ground — multi-agent teams, organizational knowledge bases, research consortia — there is no way to disagree, corroborate, or verify.

**3. Compute concentration.** LLM compilation, embedding, and PageRank are expensive. Distributing knowledge across nodes allows distributing the cost, and allows specialization (a node with a powerful GPU handles compilations; a lightweight edge node handles retrieval).

### For Whom?

The primary beneficiaries are, in order of near-term relevance:

- **Multi-agent teams at a single organization** sharing a federated cluster of Covalence nodes (e.g., one per developer, one per project, one organizational root node).
- **Research consortia** pooling domain knowledge across institutions without centralizing control.
- **Long-running personal agents** whose knowledge outlives their current machine — federation enables multi-device continuity.
- **Eventually, open networks** where strangers contribute knowledge under an incentive model (the hard case; addressed in §4.1).

### The Lineage Argument

The project genealogy is instructive:

```
EKB:        "knowledge should understand itself"
VKB:        "...and that understanding should be distributed"   ← right, too early
valence-v2: "let's make the graph atomic first"                ← right step, too lossy
Valence:    "keep the language, add the graph"                  ← right direction
Covalence:  "make it work on one machine first"                 ← right now
Future:     "now distribute it"                                 ← VKB was right all along
```

VKB was not wrong about federation. It was wrong about sequence. You cannot federate a system you haven't proven locally. Covalence's single-node architecture is now proven; this study asks: how do we distribute it?

Critically, Covalence's architecture is *inherently distributable* in ways that valence-v2's triple graph was not:

- **Immutable sources** replicate without conflict (content-addressed, immutable by design)
- **Graph computations** partition naturally (PageRank, confidence propagation)
- **Confidence scores** can be computed locally and merged globally
- **Contentions** are already a consensus mechanism in disguise — "these two things disagree; here is the resolution" is the fundamental operation of distributed epistemic systems

---

## 2. Architecture Options

Three fundamentally different approaches, each with distinct properties:

---

### Option A: Gossip-Federated P2P (Full Mesh)

**What it is:** All Covalence nodes participate as peers in a shared P2P network. Sources, articles, and scores propagate via gossip protocols. No central authority. This is what valence-v2's federation module built (libp2p + gossipsub + Kademlia + bloom filter sync).

**How it works:**
- Each node has a DID-based identity (Ed25519 keypair)
- Sources are content-addressed; sharing is conflict-free
- BloomSync (3-step: filter exchange → header request → full payload) handles set reconciliation
- Trust computed per-peer via TrustPhase model (Unknown → Provisional → Established → Trusted)
- Articles and confidence scores propagated via gossipsub with corroboration boosting

**Pros:**
- No single point of failure or control
- Scales horizontally without coordination overhead
- Valence-v2's federation code (~1,100 lines) is directly reusable for transport and sync
- Eventual consistency is acceptable for epistemic systems (confidence is approximate by definition)

**Cons:**
- **Economics unsolved**: why do strangers run nodes and share compute? valence-network-rs spent enormous effort on reputation/token models and never closed this loop
- **Trust bootstrapping**: who do you trust when you've never met? Cold start is hard
- **LLM non-determinism**: two nodes compiling the same sources may produce divergent articles — the graph becomes inconsistent across the mesh
- **Privacy is adversarial**: in a true P2P network, you don't control who your peers are
- **Complexity**: sybil resistance, partition detection, governance — all hard problems

**Verdict:** Correct end-state for open networks. Wrong starting point for Covalence.

---

### Option B: Hub-and-Spoke Institutional Federation

**What it is:** Organizations (or individuals) run authoritative Covalence nodes. Nodes federate with explicit, named peers — much like email servers or ActivityPub/Mastodon instances. No global P2P mesh; just bilateral or small-group peering agreements.

**How it works:**
- Each node has a stable identity (DID + human-readable name: `covalence://research.acme.com`)
- Peering is explicitly configured: Node A declares "I trust node B; here are its credentials"
- Source replication is pull-based: nodes subscribe to source feeds from peers
- Articles are locally compiled; peers share their article summaries, not compilation
- Confidence scores from remote nodes are received but discounted by a configurable trust factor
- Contention resolution is local: remote article contradicts local article → standard contention flow

**Pros:**
- **Economics trivially solved**: institutional nodes exist because organizations benefit from their own instance; peering is bilateral value exchange (you share yours, I share mine)
- **Trust model is simple**: peer list is explicit, human-curated, auditable
- **Privacy is tractable**: you know exactly who your peers are and can restrict sharing
- **Protocol complexity is low**: no Sybil resistance, no governance, no VDF
- **This is how the internet actually scales**: email, Mastodon, Matrix all use this model
- **Incremental path**: start with 2 nodes, grow organically

**Cons:**
- Does not achieve full decentralization
- Network effect is limited to explicit peer relationships
- No incentive for strangers to peer
- Requires manual trust establishment (but this is usually a feature, not a bug)

**Verdict:** The pragmatic correct answer for Covalence in the next 2–3 years.

---

### Option C: Hierarchical / Layered Federation

**What it is:** A three-tier architecture: personal nodes → organizational nodes → global backbone. Personal nodes sync to their org node; org nodes peer with each other and optionally with a global backbone. Like a corporate DNS hierarchy but for knowledge.

**How it works:**
- Tier 1 (personal): lightweight Covalence instances, sync upstream to org node
- Tier 2 (organizational): full Covalence instances, authoritative for their domain
- Tier 3 (global): high-availability nodes providing discovery and cross-org bootstrap
- Each tier trusts its parent more than peers; trust decays with hop count

**Pros:**
- Clean trust hierarchy (parallels existing organizational trust)
- Personal nodes can be lightweight (no LLM compilation, just ingest and sync)
- Global backbone solves discovery without requiring full P2P
- Supports "private to org, shared globally" access levels naturally

**Cons:**
- The global backbone is a centralization point and a governance problem
- Tier boundaries complicate the provenance model (which node's article is canonical?)
- Highest implementation complexity of the three options
- Requires answering governance questions before you can build

**Verdict:** Correct long-term architecture for large-scale deployment. Wrong first step.

---

## 3. Recommended Architecture

### Recommendation: Option B — Institutional Hub-and-Spoke Federation

**Rationale:**

The core lesson from the VKB/valence-network-rs experience is that economic complexity killed momentum before the system proved its epistemic value. VKB's failure mode was not technical — it was the attempt to solve Sybil resistance, token economics, and governance simultaneously with knowledge management. The right sequence is:

1. Prove the single-node epistemic engine (✅ done — Covalence)
2. Prove two-node federation with known, trusted peers (next step)
3. Grow to small networks of known institutions
4. Only then address stranger-facing economics if the use case demands it

Hub-and-spoke federation is also the historically correct answer for distributed knowledge systems. Email federated the world's messaging without a token economy. ActivityPub federated social media without Sybil resistance. Matrix federated communications without blockchain. The pattern works because participants have inherent reasons to run nodes (their own data, their own users) and bilateral peering creates mutual value without requiring strangers to trust each other.

### Core Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                  Covalence Network Layer                      │
│                                                               │
│   Node A (org-a.example.com)    Node B (org-b.example.com)  │
│   ┌─────────────────────┐       ┌─────────────────────────┐ │
│   │  Covalence Engine   │◄─────►│   Covalence Engine      │ │
│   │  Sources (CAS)      │ sync  │   Sources (CAS)         │ │
│   │  Articles (local)   │       │   Articles (local)      │ │
│   │  Confidence (local) │       │   Confidence (local)    │ │
│   │  Contentions        │       │   Contentions           │ │
│   └─────────────────────┘       └─────────────────────────┘ │
│            │                              │                   │
│            └──────────── Peer ───────────┘                   │
│                    (explicit, configured)                      │
└─────────────────────────────────────────────────────────────┘
```

**Federated objects:**
- **Sources**: replicated across all peered nodes (immutable, conflict-free)
- **Article summaries**: shared as read-only reference (not authoritative, not re-compiled)
- **Confidence signals**: shared as advisory, discounted by trust factor
- **Contentions**: cross-node contentions raised when remote article contradicts local article

**Not federated (in v1):**
- LLM compilation (each node compiles independently)
- Private sources (LOCAL_ONLY flag respected)
- Confidence authority (scores are local; remote scores are input, not ground truth)

### Identity Model

Each Covalence node has:
- An Ed25519 keypair (node identity)
- A human-readable name (e.g., `research.acme.com`)
- A list of explicitly configured peers (names + public keys + endpoints)

All transmitted objects are signed by the originating node's key. Receivers verify signatures before ingestion.

### Wire Protocol

Re-use valence-v2's transport layer:
- **Transport**: libp2p with Noise+Yamux (authenticated, encrypted)
- **Sync protocol**: BloomSync adapted for Covalence's source model (not triples)
- **Topic**: `/covalence/sources/v1` for source gossip, `/covalence/signals/v1` for confidence signals
- **Message format**: JSON with 4-byte length prefix (consistent with valence-v2's TripleSyncCodec)

---

## 4. The Seven Research Questions

### 4.1 Economics: What Incentivizes Nodes to Contribute Compute?

**The honest answer:** for institutional federation, economics is solved by alignment of interests, not incentive design.

An organization runs a Covalence node because they benefit from it. When they peer with another organization, they gain access to the remote node's knowledge in exchange for sharing their own. This is the same economics as email peering, BGP routing, or academic paper sharing: bilateral value exchange between parties who each have standing reasons to participate.

**Token economies:** valence-network-rs built a sophisticated reputation/token system (velocity-limited scores, quartic scarcity pricing, capability ramps, storage rent). It solved the mechanism design problems correctly but introduced enormous complexity before the system had proven value. The lesson: don't design the economy of strangers until strangers are actually using your system.

**Game-theoretic properties of institutional federation:**
- **Dominant strategy:** contribute if you receive proportional value. Bilateral peering makes this explicit.
- **Defection:** a node that consumes without contributing gets de-peered. No blockchain needed — social enforcement works at this scale.
- **Free-rider problem:** constrained by the fact that you only peer with nodes you explicitly trust. Strangers can't free-ride.
- **Scalability:** this breaks down past ~100 nodes or when strangers need to interact. That is when economics design becomes necessary — but not before.

**For a future open network (when needed):**
The valence-network-rs model is the right starting point: reputation as a first-class score (not a token), capability ramps (fresh DIDs have limited access), storage as a reputation transfer market (uploaders pay ongoing rent to providers; adoption creates reputation). The key fix: don't require reputation to be earned before getting *any* value — start with read access and gate write/compile on earned trust.

**Recommendation:** Phase 1–3 uses bilateral trust and explicit peering. Reputation economics are Phase 4+.

---

### 4.2 Zero-Trust Verification: How Do You Trust Derived Values from an Untrusted Remote Node?

The problem: Covalence's confidence scores, PageRank values, and compiled articles are derived via computation (LLM compilation, iterative graph algorithms). How do you know a remote node's scores are honest?

**Three-layer approach:**

**Layer 1: Source verification (always on)**
Sources are immutable and content-addressed. A received source's SHA-256 hash must match its declared content ID. This is free and catches all data corruption and tampering at the source level. Since everything in Covalence's provenance graph traces back to sources, this is the primary trust anchor.

**Layer 2: Compilation commitment (for articles)**
A compiled article's trustworthiness depends on:
1. The source set it was compiled from (verifiable: check source hashes)
2. The LLM model version used (declarable: include in article metadata)
3. The compilation prompt/template (declarable: include hash of prompt template)

A node can verify a remote article by: (a) confirming it has all the declared source inputs, (b) confirming the model version is known, and (c) optionally re-compiling locally and comparing. Full re-compilation is expensive but definitive. For high-stakes articles, this is the right approach. For routine sharing, (a) and (b) provide reasonable assurance.

**Layer 3: Score verification (statistical)**
Confidence scores and PageRank values cannot be efficiently ZK-proven without purpose-built circuits. The practical approach is statistical sampling: periodically re-run the computation on a random 5–10% of received data and check that scores are consistent within a tolerance (e.g., ±0.05 for confidence, ±10% for PageRank). Outliers trigger full re-computation and potentially a contention.

**LLM non-determinism is the hardest problem:**
Two nodes running the same LLM on the same sources will produce different articles. This is not a trust violation; it's inherent to the compilation process. The correct response is: treat remote articles as additional sources, not as authoritative compilations. When a remote compiled article and a local compiled article cover the same topic, this is a natural input to the contention system.

**Practical recommendation:**
- Verify source content hashes always
- Include model version + source set in article metadata
- Discount remote confidence scores by a configurable trust factor (default: 0.7 × remote_confidence)
- For contentions involving remote articles, re-run local compilation before resolution

---

### 4.3 Graph Partitioning: How Do You Shard the Knowledge Graph?

**The constraint unique to Covalence:**
Covalence's edges are typed provenance relationships: `originates`, `confirms`, `supersedes`, `contradicts`, `contends`. These edges are not random — they follow semantic and temporal structure. Any partitioning strategy must preserve the traversability of these edges.

**Why consistent hashing fails:**
Standard consistent hashing (e.g., Chord, Cassandra's token ring) distributes nodes by hash of their ID. This destroys semantic locality: a source and the article compiled from it end up on different partitions, breaking the `originates` edge. PageRank traversal requires multi-hop graph walks; cross-partition edges become network calls.

**Recommended approach: Topic-based semantic partitioning**

Assign nodes to partitions based on their semantic domain. Articles about `python/stdlib` live on the same partition. Articles about `climate/policy` live on another. Sources are replicated to all partitions that contain articles citing them (since sources are immutable, this is safe).

Partition assignment:
1. Each article has a `domain_path` (already exists in Covalence's schema, e.g., `['python', 'stdlib']`)
2. Partition 0 owns `['python', *]`, Partition 1 owns `['climate', *]`, etc.
3. Sources are replicated to every partition that references them (via `originates` edges)
4. Cross-partition edges (e.g., a Python article `related` to a climate article) are stored as **stub edges** — the edge exists on both partitions, pointing to a remote node identifier

**Distributed PageRank:**
Use the Pregel/BSP (Bulk Synchronous Parallel) model:
1. Each partition computes local PageRank for its subgraph
2. After each iteration, border-node scores are exchanged with neighboring partitions
3. Iterations continue until global convergence (Δscore < ε across all partitions)

Covalence's existing `pagerank_filtered()` function is the natural starting point. The federation layer adds: (a) border node identification, (b) score exchange protocol, (c) convergence detection across nodes.

**Replication factor:**
For a 2-node proof-of-concept, full replication (every source on both nodes) is acceptable. Partitioning only becomes necessary at hundreds of nodes or when storage becomes the bottleneck.

**Stub edge resolution:**
When a query traverses a stub edge, the local node sends a federated graph query to the owning node. This is the same as a DNS lookup: slow but rare, and cacheable with TTL.

---

### 4.4 Consensus on Derived Values: How Do Nodes Agree on Confidence Scores, Reliability, PageRank?

**The key insight:** most derived values in Covalence are monotonically refineable. More evidence → higher confidence, never lower (unless a contention is resolved against an article). This means CRDT-based approaches apply naturally.

**CRDT model for each value type:**

| Value | CRDT Type | Merge Operation |
|-------|-----------|-----------------|
| Source reliability | G-Counter (observation count) → deterministic function | `max(local_count, remote_count)` → recompute score |
| Article confidence | LWW Register (Last-Write-Wins, by version) | Accept higher version; recompute locally with remote evidence |
| PageRank | Gossip-based convergence | Exchange delta vectors; iterate to fixed point |
| Source corroboration count | G-Counter | Additive merge |

**Eventual consistency is acceptable:**
Confidence scores are already approximate (Covalence's existing confidence formula combines corroboration, freshness, method, consistency, applicability, source scores). A confidence score of 0.73 vs. 0.71 does not change an agent's behavior. The tolerance for staleness in epistemic values is high — much higher than, say, a financial ledger.

**Gossip propagation:**
Adapt valence-v2's BloomSync to carry confidence signal payloads:
1. Each node maintains a vector of `(node_id, article_id, confidence, version)` tuples
2. BloomSync exchange identifies which confidence updates the peer is missing
3. Updates are transmitted as small payloads (not full articles)
4. On receipt, local node updates its trust-discounted view of remote confidence

**PageRank convergence:**
For the federated case, use a simplified version of Pregel:
1. Nodes exchange PageRank scores for shared nodes (nodes that appear in both knowledge graphs via replicated sources)
2. Each node runs its local PageRank with the shared nodes' scores as boundary conditions
3. Repeat at a configurable interval (e.g., every sync cycle)

**What requires strong consistency?**
Only contention resolutions that change article status (superseded, archived) require coordination. For these, see §4.5.

---

### 4.5 Contention as Consensus: Can Covalence's Contention System Function as a Distributed Consensus Protocol?

**Short answer: yes, with materiality gating.**

**The structural parallel:**
Covalence's contention system is: detect contradiction → classify → resolve with typed outcome. This is structurally identical to Byzantine fault-tolerant (BFT) consensus: detect disagreement → assess evidence → commit to a resolution.

The existing resolution types (`supersede_a`, `supersede_b`, `accept_both`, `dismiss`) map directly to consensus outcomes:
- `supersede_a`: local knowledge wins; remote is noted but article unchanged
- `supersede_b`: remote evidence wins; article is replaced (distributed commit)
- `accept_both`: both perspectives are valid (multi-valued consensus — legitimate in epistemic systems)
- `dismiss`: not material; no coordination needed

**Materiality-gated coordination:**

```
Contention materiality score = f(confidence delta, provenance depth, node count)

Low materiality  (score < 0.3): resolve locally; no coordination required
Med materiality  (score 0.3–0.7): notify peers; accept their resolutions as advisory
High materiality (score > 0.7): broadcast to all peers; require quorum acknowledgment
```

**Distributed contention flow:**

1. Node A ingests source S that contradicts local article X
2. `handle_contention_check` fires; materiality is computed
3. If high-materiality: Node A broadcasts a `ContentionNotice` to peers
4. Peers that have article X in their knowledge graph respond with their local resolution
5. Node A collects responses; uses majority vote as advisory input to its local resolution
6. Resolution is broadcast as a `ContentionResolution` message; peers apply it to their local state

**This is literally distributed consensus wearing epistemic clothes:**
The "transaction" is a knowledge resolution. The "commit" is an article supersession or annotation. The "quorum" is the set of trusted peers. The only difference from classical BFT is that the system can legitimately `accept_both` — epistemic truth is sometimes multi-valued.

**What valence-v2 missed:**
valence-v2 operated on triples, not compiled articles. There was no contention system — just gossip and corroboration boosting. Covalence's contention infrastructure is the key innovation that makes distributed consensus tractable without a blockchain.

---

### 4.6 Privacy and Selective Disclosure: Access Control for Federated Nodes

**Existing foundation:**
valence-v2's federation module already implements privacy predicates:
- `LOCAL_ONLY`: triple never leaves the node
- `SHAREABLE_WITH`: list of DIDs that may receive this triple

This is the right abstraction. For Covalence (which operates on sources and articles, not triples), the model adapts to:

**Source-level privacy tiers:**

| Tier | Label | Behavior |
|------|-------|----------|
| 0 | `private` | Never replicated; LOCAL_ONLY |
| 1 | `org-internal` | Shared only with peers in configured org group |
| 2 | `peered` | Shared with all explicitly configured peers |
| 3 | `public` | Shared with any federated node |

**Article privacy:**
Articles compiled from private or org-internal sources inherit the most restrictive classification of their source inputs. An article that cites a `private` source cannot be shared without leaking information about that source (even if the article text seems innocuous — the provenance graph reveals the source's existence).

**Practical mechanism:**
1. Each source has a `federation_tier` field (0–3)
2. Before BloomSync, sources below the peer's authorized tier are excluded from the bloom filter
3. Articles that cite below-tier sources are shared as **opaque stubs**: the article title and domain_path are shared, but content and provenance edges are withheld
4. Confidence signals from private articles are still shared (as floating-point scores with no provenance trail)

**Access control for graph queries:**
Stub edges (see §4.3) point to nodes on remote partitions. The owning node enforces access control: stub resolution requests include the requester's peer identity, and the owning node returns content only if the tier permits.

**Differential privacy for confidence signals:**
If a node wants to share aggregate confidence signals without revealing individual sources, add calibrated Laplace noise before transmission: `signal_shared = signal_true + Laplace(0, λ)` where λ is calibrated to the privacy budget. This allows peers to benefit from confidence signals without inferring which specific sources drove them.

**Federated contention privacy:**
A `ContentionNotice` must not reveal the content of private sources. Protocol: include only the article ID and the contention's vector similarity score; not the source content. Peers can decide whether to participate in quorum resolution based on whether they have local knowledge about the relevant article.

---

### 4.7 Practical First Step: What Would You Build in a 2-Node PoC Today?

**Build: federated source replication via adapted BloomSync.**

Nothing else. Not confidence gossip. Not distributed contention. Not partitioned PageRank. Just: two Covalence instances that share immutable sources.

**Why this is the right first step:**
- Sources are immutable → no conflict resolution needed
- Content-addressing → correctness is trivially verifiable (hash check)
- BloomSync is already implemented in valence-v2 → adaptation, not new code
- Demonstrates end-to-end value immediately: Node B can query sources that Node A ingested
- Validates the wire protocol, identity model, and trust configuration

**Concrete implementation:**

```rust
// New endpoint on Covalence Engine:
POST /federation/sync_request
  body: { "peer_bloom_filter": <bytes>, "peer_source_count": u64 }
  response: { "source_headers": [{ "id": uuid, "hash": bytes32, "title": str }] }

POST /federation/source_pull  
  body: { "source_ids": [uuid] }
  response: { "sources": [{ "id": uuid, "content": str, "metadata": json, "signature": bytes }] }

GET /federation/bloom
  response: { "filter": <bytes>, "source_count": u64 }

GET /federation/identity
  response: { "node_id": str, "public_key": bytes, "name": str }
```

**Configuration addition to `covalence.toml`:**
```toml
[federation]
enabled = true
listen_addr = "/ip4/0.0.0.0/tcp/9430"
node_name = "research.acme.com"

[[federation.peers]]
name = "team.acme.com"
endpoint = "https://team.acme.com:9430"
public_key = "ed25519:abc123..."
trust_tier = 2  # peered
```

**Sync flow:**
1. Node A calls `GET /federation/bloom` on Node B → gets bloom filter of B's sources
2. Node A identifies sources it has that B is missing
3. Node A calls `POST /federation/sync_request` on B with its own bloom filter
4. B identifies sources A is missing; returns headers
5. Both nodes selectively pull the sources they're missing via `POST /federation/source_pull`
6. Each node verifies content hashes on receipt before ingesting

**Timeline:** 2–3 weeks for two experienced Rust developers, given valence-v2's BloomSync as reference implementation. The bloom filter logic, set reconciliation, and wire format are already solved. The work is: (1) adapt from triple model to source model, (2) add HTTP endpoints to Covalence Engine, (3) add peer configuration, (4) implement signature verification.

**What you learn:**
- Whether the sync protocol handles real Covalence workloads
- Wire performance characteristics
- Whether signature verification overhead is acceptable
- Configuration ergonomics for node operators

---

## 5. What Valence-v2 Got Right and Wrong

### What It Got Right

**The transport stack is correct.**
libp2p with Noise+Yamux (authenticated, encrypted transport), Gossipsub (pub-sub for announcements), Kademlia (DHT for peer discovery), mDNS (local discovery), and RequestResponse (direct request-reply sync) is the right combination. It's what IPFS, Ethereum, and Polkadot all use. Don't reinvent this.

**Bloom filter gossip for set reconciliation is elegant.**
The 3-step BloomSync protocol (exchange filters → exchange headers → selective full-payload pull) minimizes bandwidth by transferring only what's missing. It handles 100,000-item sets efficiently (BLOOM_EXPECTED_ITEMS = 100,000, FP_RATE = 1%). The protocol is directly reusable for Covalence's source model.

**Trust phases are the right model.**
Unknown → Provisional → Established → Trusted, with configurable thresholds (0.3, 0.6, 0.8), is a clean and operationally useful abstraction. It maps naturally to the hub-and-spoke model: new peers start at Provisional; established bilateral peers reach Trusted.

**Privacy predicates (LOCAL_ONLY, SHAREABLE_WITH) are the right abstraction level.**
Per-object privacy, not per-connection privacy. This is more granular and more useful than TLS-level access control.

**Corroboration as a first-class concept.**
`MergeResult` distinguishes inserted (new objects) from corroborated (existing objects confirmed by a peer). The `CORROBORATION_BOOST` (+0.05 to base_weight) is how independent confirmation strengthens knowledge. This maps directly to Covalence's confidence scoring.

**PageRank-derived trust.**
Computing peer trust scores via PageRank of DID nodes in the graph — not a separate trust manager — is elegant and correct. Trust derives from the knowledge graph's topology, which is exactly what you want in an epistemic system.

### What It Got Wrong

**Operating at the triple level.**
valence-v2's knowledge model is triples (subject-predicate-object), which suited its RDF-heritage design. But Covalence's model is documents (sources) and compiled articles — semantically richer, computationally more expensive. The triple-level bloom sync cannot be directly applied; the sync unit should be sources and article summaries, not individual triples. However, the *protocol structure* is directly portable; only the object model changes.

**No concept of LLM-compiled articles.**
The federation layer in valence-v2 treated all knowledge as first-class triples — there was no notion that some knowledge is derived (compiled from sources by an LLM). This means the system couldn't distinguish between "raw source" (should always replicate) and "compiled article" (each node should compile independently, sharing only summaries and signals). This distinction is critical for handling LLM non-determinism correctly.

**No contention system.**
valence-v2's federation handles conflicts via corroboration weighting: if two peers disagree, the triple with higher aggregate weight wins. This is too blunt — it can silently suppress legitimate minority views and cannot represent the epistemic state "these two things genuinely disagree and the disagreement is meaningful." Covalence's formal contention system is the right answer to distributed disagreement.

**Attempted too much simultaneously.**
The valence-v2 federation module implemented: libp2p transport, gossipsub, Kademlia, mDNS, trust phases, bloom sync, privacy predicates, corroboration boosting, PageRank trust — all at once, all while the base knowledge engine was not yet stable. The ordering was wrong: federate a proven system, don't prove a federated system.

**Peer store was in-memory only.**
`InMemoryPeerStore` means peer state is lost on restart. For a production federation system, peer state (trust phases, successful sync counts, last-seen timestamps) must be persisted. This is a small but important omission that would have caused real operational pain.

**The manager's event loop was never completed.**
`handle_next_event()` exists but the actual event dispatch logic was incomplete when Covalence was greenlit. The skeleton was right; the integration was missing — exactly the pattern seen in valence-network-rs as well. The moral: federation modules need to be built against a stable base system, not in parallel with it.

---

## 6. Phased Roadmap

### Phase 1: Source Replication (4–6 weeks)
**Goal:** Two Covalence nodes can exchange immutable sources.

**Deliverables:**
- Federation configuration in `covalence.toml` (peer list, enable/disable, sync interval)
- `FederationConfig` struct in engine (adapted from valence-v2's `config.rs`)
- `PeerStore` with PostgreSQL persistence (new `federation_peers` table in Covalence schema)
- REST endpoints: `GET /federation/bloom`, `POST /federation/sync_request`, `POST /federation/source_pull`, `GET /federation/identity`
- Ed25519 node keypair generation and persistence at first startup
- Source signing on ingest; signature verification on receipt before ingestion
- Periodic sync task (configurable interval, default 5 minutes)
- `federation_tier` field on sources (`private`/`org-internal`/`peered`/`public`, default `peered`)
- Basic metrics: sources_replicated_in, sources_replicated_out, sync_errors, last_sync_at

**What this enables:** Node B queries sources that Node A ingested. Agents on Node B have access to Node A's raw knowledge base. Bilateral.

**Open question resolved in this phase:** Does the sync protocol handle real Covalence source sizes (some sources are megabytes of text)? What is the bloom filter false positive rate in practice?

---

### Phase 2: Peer Identity and Trust Phases (3–4 weeks, after Phase 1)
**Goal:** Formalize peer identity, implement TrustPhase state machine, secure all transmissions.

**Deliverables:**
- Peer DID format: `did:covalence:<pubkey_hex>` (or `did:key:` for interoperability)
- TrustPhase state machine (Unknown → Provisional → Established → Trusted) stored in `federation_peers`
- Transition rules: Provisional after first successful sync; Established after N successful syncs; Trusted after sustained high sync rate
- Per-peer trust factor in config (default: 0.7, range: 0.0–1.0)
- Handshake protocol: exchange identities, verify keys, negotiate capabilities
- Federation status endpoint: `GET /federation/status` (connected peers, trust phases, sync stats)
- Peer de-peering and blocklist support
- 2-node integration test suite using two in-process Covalence instances

**What this enables:** Federation is secure against impersonation. Trust is graduated and observable.

---

### Phase 3: Federated Confidence Signals (4–6 weeks, after Phase 2)
**Goal:** Nodes share confidence signals; local confidence incorporates remote corroboration.

**Deliverables:**
- `ConfidenceSignal` message type: `{ article_id, confidence, version, source_node_id, timestamp, source_hashes[] }`
- Confidence signal sync endpoints: `GET /federation/signals/bloom`, `POST /federation/signals/sync`
- BloomSync adapted for confidence signals (higher frequency: every minute vs. every 5 minutes for sources)
- Trust-discounted confidence merging: `effective_conf = local_conf × w + Σ(remote_conf × trust_factor × (1-w)/N)`
- Corroboration detection: when remote confidence signal shares source hashes with local article, boost local corroboration score
- Article metadata records contributing remote nodes (for provenance audit)
- Confidence version increment on signal incorporation

**What this enables:** Two nodes that independently compiled the same sources end up with more confident knowledge than either would have alone. Distributed corroboration works.

---

### Phase 4: Distributed Contention (6–8 weeks, after Phase 3)
**Goal:** Cross-node contentions are detected, broadcast, and resolved via peer quorum.

**Deliverables:**
- Materiality scoring function: `materiality = f(confidence_delta, provenance_depth, affected_node_count)`
- `ContentionNotice` message: `{ article_id, similarity_score, materiality, originating_node }` (no source content for privacy)
- Contention broadcast to peers above materiality threshold (>0.7 requires quorum)
- Peer contention resolution response protocol: peers respond with their local resolution (if they have the article)
- Majority-vote integration into local resolution decision
- `ContentionResolution` broadcast: nodes apply remote-initiated resolutions to their local state
- `accept_both` enrichment: dual-node provenance for multi-valued resolutions
- Audit trail: contention resolution records include participating nodes and votes

**What this enables:** The distributed epistemic ground becomes genuinely shared — not just replicated, but contested and resolved collaboratively. Federation has semantics beyond data sync.

---

### Phase 5: Graph Partitioning and Distributed PageRank (8–12 weeks, after Phase 4)
**Goal:** Support large-scale federation where full replication is impractical.

**Deliverables:**
- Partition ownership map: `domain_path[0]` → owning node (stored in a shared config or DHT)
- Stub edge support in Covalence's graph layer: `{ edge_type, local_node_id, remote_node_id, remote_node_endpoint }`
- Stub resolution endpoint: `POST /federation/graph_query` (resolve stub edges from authenticated peers)
- Pregel-style distributed PageRank: border node score exchange every sync cycle
- Partition rebalancing protocol: migrate source sets when new nodes join, with backfill period
- Storage-aware bloom filter: include only partition-local sources in primary bloom; separate cross-partition index

**What this enables:** Federation scales beyond storage and compute limits of individual nodes. Domain-specialized nodes are possible.

---

### Phase 6: Open Network Economics (when needed, no timeline)
**Goal:** Support federation between strangers under an economic model.

**Design inputs from valence-network-rs:**
- Reputation scoring with velocity limits (prevent rapid score inflation)
- Capability ramps: new nodes read-only until trust established
- Storage as reputation transfer market: uploaders pay ongoing rent; adoption creates reputation
- Quartic scarcity pricing: `price = 1 + 99 × utilization⁴`

**Prerequisites:** Phases 1–5 proven in institutional settings. Actual demand for stranger-facing federation. A governance model for protocol evolution.

---

## 7. Open Problems

These are genuinely hard problems this study does not fully resolve. They are not reasons to delay building — they are research questions to carry alongside the implementation.

---

### 7.1 LLM Non-Determinism Across Nodes

If Node A and Node B independently compile the same source set using the same model, they will produce different articles. This is inherent to language models. The implications:

- The "same" knowledge has multiple independent representations
- Confidence scores are not directly comparable (different text → different embedding → different score)
- Contention fires on genuinely equivalent articles that happen to be phrased differently

**Partial solutions:**
- Include model version + prompt hash in article metadata; nodes with matching metadata have structurally comparable articles
- Treat all remote articles as additional sources (not authoritative compilations) — feed them into the local compilation pipeline as strong evidence
- Accept multi-valued representations as the epistemic norm; Covalence already supports `accept_both`

**Unsolved:** Can two nodes converge on a canonical representation of the same knowledge without running the same LLM at the same time with the same random seed? Probably not. The right frame may be: federation manages divergence rather than eliminating it. This is an open research question in distributed AI systems.

---

### 7.2 Compilation Cost Coordination

Who should compile what? In a naive federation, every node re-compiles every source it receives. For a network with 10 nodes and 100,000 sources, that is 10× the compilation work. 

**Approaches under investigation:**
- **Compilation specialization:** assign source domains to specific nodes for authoritative compilation; others receive the article as a high-trust input
- **Lazy compilation:** don't compile received sources until a local agent queries them
- **Compilation sharing with independence preservation:** share compiled articles directly and use them as strong (but not definitive) evidence in local compilation

**Problem with compilation sharing:** you lose provenance independence. If Node B uses Node A's article as a source, its confidence is causally downstream of A's compilation. For high-stakes epistemic use cases, this matters.

---

### 7.3 Adversarial PageRank

A malicious node can inflate the PageRank of its own articles by constructing a dense internal graph of self-referential sources and articles. In a trusted network, this is a policy violation. In an open network, it is an attack vector.

**Known mitigations:**
- Trust-discounting: PageRank contributions from untrusted nodes are weighted by their trust factor
- Topological anomaly detection: unusual internal clustering patterns trigger a trust review
- Quorum-validated PageRank: for high-value nodes, require agreement from N peers on their rank before accepting

**Unsolved:** Detecting subtle PageRank inflation (small boosts distributed across many articles) without global graph visibility. This is the web spam problem; it has no clean solution. The institutional federation model sidesteps it (you only accept PageRank from trusted peers), but it re-emerges in open network Phase 6.

---

### 7.4 Schema and Protocol Evolution

Covalence's schema is actively evolving (Amendment 001, Amendment 002, graph layer redesign in progress). In a federated network, a schema change on one node creates protocol incompatibility with peers on the old schema.

**Needed:**
- Explicit protocol versioning in all endpoints (`/covalence/sources/v1`, `/v2`)
- Schema negotiation during handshake: "I support v1 and v2; what do you support?"
- Backward compatibility period during transitions (minimum: one full sync cycle)
- Deprecation timeline for old protocol versions

This is the HTTP/2 rollout problem — solved, but requires discipline. The earlier federation protocol design is locked in, the less costly schema evolution becomes. **Implication: the Phase 1 wire protocol should be designed to be versioned from day one.**

---

### 7.5 Cold Start for New Nodes

A new node joining a federation has no sources, no articles, no confidence history, and no PageRank. It provides no immediate value to peers. 

**For institutional federation:** not a problem. Peering is manual and trust is established out-of-band. A new node declares its intent to share; existing peers accept it based on organizational relationship.

**For open networks (Phase 6):** valence-network-rs's capability ramp is the right architecture. New nodes start read-only. They earn the right to contribute by demonstrating value. The VDF (Verifiable Delay Function) provides sybil resistance — you can't instantly spin up 1,000 new nodes with full capabilities.

---

### 7.6 Privacy of the Provenance Graph

Even when source content is marked private, the provenance graph leaks information. If Node A shares an article that cites source S, and Node B knows source S relates to topic X, then B knows A has knowledge about X — even if the source content is never transmitted.

**This is the correlation problem in privacy research.** Mitigations:
- Share only article titles and confidence signals, not provenance edges, for private-tier knowledge
- Add synthetic stub sources as cover traffic in provenance graphs (expensive; weakens epistemic guarantees)
- Accept that federation unavoidably leaks metadata and design the privacy model around what *is* safe to share rather than what isn't

**The honest answer:** federation and perfect privacy are in tension. The right model is not "prevent all leakage" but "control what leaks and to whom." The four-tier privacy model in §4.6 is the practical answer; users must understand its limits.

---

### 7.7 The "Articles as Scaffolding" Intersection

Covalence's long-term trajectory is toward treating articles as temporary scaffolding: when compute is cheap enough, queries are answered directly from sources, not from pre-compiled articles. Live synthesis replaces cached compilation.

If federation is built while articles are primary, the federated protocol is article-centric. If federation happens after live synthesis is primary, the protocol should be source-centric.

**Implication:** invest more in federated source replication (Phase 1) than federated article sharing. The source layer is durable; the article layer is transitional. Phase 3's confidence signals are the article layer's contribution to federation — they distill article-level epistemic state into lightweight signals that survive the transition to live synthesis.

---

## Appendix: Reusable Assets from Prior Work

### From `~/projects/valence-v2/engine/src/federation/`

| File | Est. Lines | Reuse Status | Notes |
|------|-----------|--------------|-------|
| `config.rs` | ~80 | ✅ Direct port | Replace `Multiaddr` with HTTP endpoint config; keep `TrustThresholds` |
| `peer.rs` | ~80 | ✅ Direct port | `TrustPhase` model and `Peer` struct minimal changes |
| `protocol.rs` | ~100 | ⚠️ Adapt | Replace `Triple`/`TripleHeader` with `Source`/`SourceHeader`; keep message envelope and codec |
| `sync.rs` | ~100 | ⚠️ Adapt | BloomSync logic directly reusable; object model swap only |
| `manager.rs` | ~100 | ⚠️ Adapt | Event loop skeleton; needs completion + Covalence integration |
| `transport.rs` | ~80 | ⚠️ Optional | Full libp2p stack; Phase 1 can use HTTP transport; upgrade in Phase 2 |

**Estimated reuse: ~540 lines directly applicable. Primary new work: source-model adaptation, PostgreSQL persistence, HTTP endpoint layer, and integration tests.**

### From `~/projects/valence-network-rs/`

| Component | Applicability | Phase |
|-----------|---------------|-------|
| Ed25519 identity + signing | High | Phase 1 |
| Reputation formulas (velocity-limited) | Medium | Phase 6 |
| Quorum evaluation (standard/constitutional) | Medium | Phase 4 (simplified) |
| VDF Sybil resistance | Low | Phase 6 |
| Gossipsub + Kademlia transport | High | Phase 1 (if using libp2p) |
| BloomSync set reconciliation | High | Phase 1 |
| Partition detection + merge | Medium | Phase 5 |
| Anti-gaming modules | Low | Phase 6 |

---

## Summary

Federation is feasible. The path is clear. The sequence is everything.

**Do:** institutional hub-and-spoke federation, starting with source replication, building on valence-v2's proven transport and sync infrastructure.

**Don't:** attempt full P2P, token economics, or Sybil resistance before proving two-node sync. VKB tried that. It was right about the destination; wrong about the route.

The deepest insight from the VKB–Covalence arc: **contentions are consensus**. Covalence already has the distributed agreement primitive — it's just running locally. Federating it is the act of connecting those primitives across nodes. The hard problem was never protocol design; it was proving the epistemic engine first. That work is done.

*Build the two-node PoC. Everything else follows.*

---

*Design study complete. tracking#102.*

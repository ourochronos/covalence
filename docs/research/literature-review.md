# Covalence Literature Review
## Graph-Native Agent Memory: Research Foundations

**Prepared:** 2026-03-01  
**Agent:** Literature Review Subagent (depth 1)  
**Scope:** Six recent ArXiv papers reviewed for architectural relevance to the Covalence knowledge substrate.

---

## Table of Contents

1. [Paper 1 — Graph-based Agent Memory Survey (2602.05665)](#paper-1)
2. [Paper 2 — MAGMA: Multi-Graph Agentic Memory (2601.03236)](#paper-2)
3. [Paper 3 — Memory in the Age of AI Agents (2512.13564)](#paper-3)
4. [Paper 4 — TopER: Topological Embeddings (2410.01778)](#paper-4)
5. [Paper 5 — DEG-RAG: Denoising KGs for RAG (2510.14271)](#paper-5)
6. [Paper 6 — Efficient KG Construction and Retrieval (2507.03226)](#paper-6)
7. [SYNTHESIS: What These Papers Tell Us About Building Covalence](#synthesis)

---

## Paper 1 — Graph-based Agent Memory: Taxonomy, Techniques, and Applications {#paper-1}

**Citation:** Yang et al. (2026). *Graph-based Agent Memory: Taxonomy, Techniques, and Applications.* arXiv:2602.05665v1. Hong Kong Polytechnic University et al.  
**Type:** Comprehensive survey (Feb 2026, cutting-edge)  
**Resources:** https://github.com/DEEP-PolyU/Awesome-GraphMemory

---

### Core Contribution / Key Insight

This is the field's most current and comprehensive survey of graph-based agent memory, establishing a canonical taxonomy and lifecycle framework. The central thesis is powerful: **graph-based memory is not a special case of memory — it is the general case.** Linear buffers, vector stores, and key-value logs are all degenerate graphs (chains, fully-connected graphs with similarity-weighted edges, etc.). By adopting graph structure from the start, you get a framework that subsumes all simpler paradigms while enabling capabilities they cannot achieve.

The survey formally defines a **Memory Lifecycle** with four stages:
1. **Extraction** — transforming raw observations into structured memory units
2. **Storage** — organizing units into graph structure with appropriate indexing
3. **Retrieval** — retrieving relevant subsets via queries
4. **Evolution** — updating, consolidating, abstracting, and pruning over time

This lifecycle framing is highly actionable for Covalence: it maps directly onto the write/read/update/delete primitives of a knowledge substrate API.

---

### Architecture / Data Model

**Six cognitive memory types** are identified and mapped to agent use cases:

| Type | Description | Graph Role |
|------|-------------|-----------|
| **Semantic** | Decontextualized world facts ("Paris is the capital of France") | Stable ontological backbone |
| **Procedural** | Skills, rules, routines ("how-to" knowledge) | Reusable procedure nodes |
| **Associative** | Latent links between concepts | Cross-domain edge layer |
| **Working** | Active reasoning scratchpad | Transient session subgraph |
| **Episodic** | Chronological session history | Timestamped event chains |
| **Sentiment** | Emotional tone / user feedback history | Qualitative edge metadata |

**Two primary memory categories** for implementation:
- **Knowledge Memory (passive/static):** Pre-loaded facts, rules, domain knowledge. Slowly updated, context-independent.
- **Experience Memory (proactive/dynamic):** Interaction logs, trial-and-error trajectories, personalized histories. Continuously evolving.

**Graph types surveyed:** knowledge graphs, temporal graphs, hypergraphs, hierarchical trees/graphs, hybrid multigraphs.

---

### Relevance to Covalence

Covalence's design (rich property nodes + typed edges + multi-dimensional retrieval on PostgreSQL + AGE + PGVector) maps almost perfectly onto this survey's recommendations:

- The **typed edge model** in Covalence directly corresponds to the survey's insistence on explicit relationship types as first-class data
- **PGVector** addresses the embedding-based retrieval dimension
- **Apache AGE (openCypher)** enables the traversal-based retrieval the survey identifies as essential for complex reasoning
- The survey validates that **hybrid retrieval** (semantic similarity + graph traversal) is the frontier approach — exactly what Covalence's multi-dimensional retrieval layer should implement

The **memory lifecycle** framework suggests Covalence needs explicit APIs for all four stages, not just read/write.

---

### Techniques to Adopt

1. **Formal lifecycle operations** — expose Write, Read, Update, Delete as atomic primitives, then compose into lifecycle stages. Don't let this be implicit.
2. **Multi-type memory architecture** — Covalence's node schema should support flagging nodes as semantic/procedural/episodic/working/sentinel types so retrieval can filter by cognitive role.
3. **Graph-as-generalization principle** — design the substrate so that flat retrieval (pure vector similarity) is a degenerate special case of graph traversal with no edge constraints. This ensures backwards compatibility with RAG-style usage.
4. **Evolution as a first-class operation** — memory consolidation, abstraction, and pruning must be part of the system, not afterthoughts. The survey identifies this as what distinguishes sophisticated agent memory from simple RAG.
5. **Relationship modeling over flat similarity** — invest in rich edge typing: CAUSES, FOLLOWS, CONTRADICTS, SUMMARIZES, DERIVES_FROM, etc. These are what enable causal and temporal reasoning.

---

### Limitations / Inapplicable Aspects

- Being a survey, it does not prescribe implementation details (DB choice, query languages, indexing strategies). Covalence must translate the conceptual framework into concrete engineering choices.
- The survey focuses heavily on LLM agent use cases; Covalence needs to also serve structured programmatic access patterns (CLI, API), which requires additional schema discipline.
- Hypergraph representations (multi-node edges) are identified as powerful but complex — not a priority for Covalence v1 given AGE's property graph model.

---

## Paper 2 — MAGMA: A Multi-Graph based Agentic Memory Architecture {#paper-2}

**Citation:** Jiang, Li, Li, Li (2026). *A Multi-Graph based Agentic Memory Architecture for AI Agents.* arXiv:2601.03236v1. University of Texas at Dallas, University of Florida.  
**Type:** Novel architecture with experimental validation  
**Code:** https://github.com/FredJiang0324/MAMGA  
**Benchmarks:** LoCoMo, LongMemEval (outperforms SOTA)

---

### Core Contribution / Key Insight

MAGMA's central insight is that **existing memory systems fail because they entangle orthogonal relational dimensions.** When temporal, causal, entity, and semantic relationships are all mixed into a single graph or vector store, retrieval becomes a blunt instrument that cannot reason about *why* something happened or *who* was involved across time.

The solution: **four orthogonal relation graphs** maintained in parallel over the same node set:

1. **Temporal Graph (ℰ_temp):** Strictly ordered event chains. Answers "When?" queries.
2. **Causal Graph (ℰ_causal):** Explicitly inferred cause→effect edges. Answers "Why?" queries.
3. **Semantic Graph (ℰ_sem):** Undirected similarity-weighted edges. Answers "What is related?" queries.
4. **Entity Graph (ℰ_ent):** Events linked to persistent entity nodes. Solves object permanence across disjoint timelines.

Each memory item (node) has a **unified representation**: `nᵢ = ⟨cᵢ, τᵢ, vᵢ, 𝒜ᵢ⟩`
- `cᵢ` — content (event, observation, action)
- `τᵢ` — discrete timestamp
- `vᵢ ∈ ℝᵈ` — dense embedding (for semantic search)
- `𝒜ᵢ` — structured attribute set (entity refs, temporal cues, metadata)

This is an elegant hybrid: each node lives in a vector database *and* in four typed edge spaces simultaneously.

---

### Architecture

**Three-layer system:**

```
Query Process Layer
  ├── Intent-Aware Router (classifies: Why / When / Entity)
  ├── Adaptive Topological Retrieval (heuristic beam search)
  └── Context Synthesizer (narrative linearization with provenance)

Data Structure Layer
  ├── Vector Database (dense embeddings for all nodes)
  └── Four Relation Graphs (Semantic, Temporal, Causal, Entity)

Write/Update Layer
  ├── Fast Path: Synaptic Ingestion (non-blocking, latency-critical)
  │     segment event → vector index → append temporal edge → enqueue
  └── Slow Path: Structural Consolidation (async, LLM-powered)
        dequeue → analyze 2-hop neighborhood → infer causal/entity edges
```

**Adaptive Traversal Policy (the core algorithm):**

Transition score: `S(nⱼ|nᵢ, q) = exp(λ₁·ϕ(edge_type, query_intent) + λ₂·sim(nⱼ, q))`

- `ϕ` rewards edge types matching query intent (causal edges weighted high for "Why" queries)
- `sim` maintains semantic focus during traversal
- Heuristic beam search with decay factor `γ` prevents runaway expansion

**Multi-signal anchor identification** via Reciprocal Rank Fusion (RRF) of:
- Dense vector search
- Sparse keyword matching
- Temporal window filtering

**Context linearization** with provenance tokens: `<t:τᵢ> content <ref:nᵢ.id>`

---

### Relevance to Covalence

MAGMA is the closest existing architecture to what Covalence aims to be. Key alignment points:

| MAGMA Concept | Covalence Equivalent |
|---------------|---------------------|
| Four typed edge spaces | Typed edges in AGE (edge labels + properties) |
| Unified node `⟨content, timestamp, embedding, attributes⟩` | Rich property nodes in Covalence article schema |
| Vector DB + Graph DB dual storage | PGVector + AGE in same PostgreSQL instance |
| Intent-aware query routing | Multi-dimensional retrieval API layer (to be designed) |
| RRF anchor fusion | Planned hybrid search in Covalence |
| Dual-stream ingestion | Source ingest (fast) + article compile (async/slow) separation |

The **dual-stream write model** is particularly instructive: Covalence's existing separation of `source_ingest` (fast, raw) and `article_compile` (slow, LLM-synthesized) IS this pattern. MAGMA provides theoretical validation and implementation guidance.

The **orthogonal graph decomposition** suggests Covalence should use typed edge labels rather than a single undifferentiated graph. In AGE/openCypher, this means distinct relationship types like `:FOLLOWS`, `:CAUSES`, `:RELATES_TO`, `:INVOLVES` rather than a generic `:LINKED_TO`.

---

### Techniques to Adopt

1. **Orthogonal edge-type decomposition** — explicitly partition the Covalence edge schema into temporal, causal, semantic, and entity dimensions. Query routing can then prioritize edge types based on intent.

2. **Intent classification on queries** — implement a lightweight classifier (or prompt) that maps incoming queries to intent categories (factual/temporal/causal/entity). Route to different traversal strategies accordingly.

3. **RRF anchor fusion** — when identifying starting nodes for graph traversal, fuse vector similarity, full-text search (pg_textsearch), and temporal filtering via RRF before traversing. This is directly implementable in Covalence's Go CLI.

4. **Adaptive beam search traversal** — implement a scored BFS/beam search in AGE Cypher queries: start from anchor nodes, traverse edges weighted by `(edge_type_alignment × query_intent) + (neighbor_embedding_similarity × query_embedding)`, prune to top-K at each hop.

5. **Provenance tokens in context** — when serializing retrieved subgraphs for LLM consumption, include structured provenance (`<ref:article_id> <t:timestamp>`) to enable hallucination detection and attribution.

6. **Salience-based token budgeting** — when context window is limited, use traversal scores to prioritize which nodes get full content vs. compressed summaries.

7. **Slow-path consolidation queue** — implement a background worker that processes the mutation queue (already exists in Covalence's schema) to infer causal/entity links asynchronously.

---

### Limitations / Inapplicable Aspects

- MAGMA stores events at granular interaction level; Covalence operates at the article/source level (higher abstraction). The mapping is: MAGMA events ≈ Covalence sources; MAGMA semantic summaries ≈ Covalence articles.
- The LLM-inferred causal edge construction in the slow path requires careful cost management at scale. For Covalence, this should be opt-in or triggered only when confidence thresholds are met.
- MAGMA does not address multi-agent or multi-tenant memory isolation; Covalence will need namespace/workspace scoping.
- The four-graph decomposition creates graph synchronization complexity. Covalence can achieve similar effect using edge property metadata and typed labels within a single AGE multigraph rather than physically separate graphs.

---

## Paper 3 — Memory in the Age of AI Agents {#paper-3}

**Citation:** Hu et al. (2025/2026). *Memory in the Age of AI Agents.* arXiv:2512.13564v2. 40+ authors across Fudan, Chinese Academy of Sciences, Oxford, and others.  
**Type:** Comprehensive survey + taxonomy  
**Published:** Dec 2025, updated Jan 2026

---

### Core Contribution / Key Insight

This survey attacks a fragmentation problem: the term "agent memory" means wildly different things across papers, with inconsistent terminology, incompatible taxonomies, and no shared evaluation protocols. The paper proposes a **unified tripartite taxonomy** viewed through three orthogonal lenses:

**By Form:**
- **Token-level memory** — in-context storage (conversation history, scratchpad)
- **Parametric memory** — encoded in model weights (fine-tuning, LoRA adaptation)
- **Latent memory** — compressed state in activations, memory tokens, or KV-cache

**By Function:**
- **Factual memory** — world knowledge, domain facts, reference information
- **Experiential memory** — task trajectories, interaction outcomes, behavioral patterns
- **Working memory** — immediate task context, active reasoning state

**By Dynamics:**
- **Formation** — how memories are created (extraction, encoding, summarization)
- **Evolution** — how memories change over time (consolidation, decay, update)
- **Retrieval** — how memories are accessed (similarity, traversal, hybrid)

The paper explicitly **distinguishes agent memory from RAG and context engineering**, arguing these are related but distinct: RAG retrieves from static external corpora; agent memory is dynamic, personalized, and evolves through the agent's lived experience.

---

### Architecture / Data Model

No single novel architecture, but strong design principles:

- Memory should be treated as a **first-class primitive** in agent system design, not bolted on
- The traditional long/short-term taxonomy is "proven insufficient" for modern systems
- Emerging frontiers: memory automation, RL integration, multimodal memory, multi-agent memory sharing, trustworthiness/privacy

Key observation: most existing systems collapse the "by function" dimension, treating all memory as uniform. Distinguishing factual vs. experiential vs. working memory enables **function-specific retrieval strategies** and **lifecycle management policies** (facts persist longer; working memory is ephemeral; experiential memory evolves via consolidation).

---

### Relevance to Covalence

1. Covalence's existing source/article distinction maps roughly to the **formation/evolution** dimension:
   - Sources = raw memory formation events
   - Articles = consolidated/evolved representations

2. The **factual vs. experiential** distinction suggests Covalence nodes should be typed by function, not just by content. An article about "the Covalence schema" (factual) should be retrieved and decayed differently than an article recording "the decision to use AGE over Neo4j" (experiential/episodic).

3. The **trustworthiness** dimension flags an important gap: Covalence's confidence/reliability scoring (already in the schema) is directly relevant here. The survey calls this an open research frontier.

4. The survey reinforces that Covalence is building **agent memory**, not just a knowledge base or RAG system — this framing matters for API design and documentation.

---

### Techniques to Adopt

1. **Memory-type tagging** — annotate articles/sources with their function category: `factual | experiential | working`. Apply differential decay rates and retrieval weights.

2. **Explicit lifecycle management** — implement formation → evolution → retrieval as explicit system operations, not side effects. The mutation queue in Covalence is the evolution pipeline.

3. **Trustworthiness as retrieval signal** — the existing `confidence` and `reliability_score` fields should be first-class retrieval ranking factors, especially for high-stakes reasoning tasks.

4. **Multi-agent memory scoping** — design the namespace/workspace system to support shared vs. private memory partitions across agents.

---

### Limitations / Inapplicable Aspects

- The paper is primarily a taxonomic survey; it does not provide implementation guidance or empirical comparisons. Use it for vocabulary and framing, not engineering.
- The parametric memory form (fine-tuning) is out of scope for Covalence as an external substrate.
- The latent memory form (KV-cache, memory tokens) is also out of scope — Covalence is explicitly external/persistent memory.

---

## Paper 4 — TopER: Topological Embeddings in Graph Representation Learning {#paper-4}

**Citation:** Coskunuzer (2024/2025). *Topological Embeddings in Graph Representation Learning.* arXiv:2410.01778v3. NeurIPS 2025.  
**Type:** Novel embedding method  
**Datasets:** Molecular, biological, social networks

---

### Core Contribution / Key Insight

TopER introduces the **Topological Evolution Rate** — a low-dimensional, interpretable graph embedding derived from Persistent Homology (algebraic topology). Rather than using opaque high-dimensional GNN embeddings, TopER computes how quickly topological substructures (connected components, loops, voids) appear and disappear as edges are added in order of weight. This "evolution rate" is condensed into a compact numerical signature for each graph.

Key properties:
- **Low-dimensional** — 2D or small-d embeddings, human-visualizable
- **Interpretable** — the embedding captures structural complexity, not just spectral properties
- **Competitive** — matches or surpasses SOTA GNNs on graph classification/clustering benchmarks
- **Domain-agnostic** — works on molecular graphs, social networks, biological networks equally

The technique leverages Persistent Homology without requiring full persistence diagrams — it computes only the *rate of topological change*, making it significantly cheaper.

---

### Relevance to Covalence

This is the most speculative connection of the six papers, but important for future development:

1. **Graph-level structural signatures** — TopER could provide a cheap structural fingerprint for each article's local neighborhood subgraph. Two articles that "feel similar" in terms of how they connect to the knowledge graph would have similar TopER signatures, even if their textual content differs. This enables **structural retrieval** as a third retrieval dimension alongside semantic (PGVector) and traversal (AGE).

2. **Memory evolution monitoring** — as Covalence's knowledge graph grows, TopER can track *how the topology is changing*. Rapid topological changes in a subgraph (e.g., a cluster suddenly gaining many new connections) could signal a knowledge domain undergoing restructuring — a useful signal for triggering recompilation.

3. **Clustering and visualization** — TopER's low-dimensional embeddings make knowledge graph clusters human-visualizable without dimensionality reduction. Useful for Covalence workspace exploration UIs.

4. **Anomaly detection** — topologically unusual nodes (high evolution rate relative to their neighborhood) may indicate low-quality or noisy knowledge imports — connecting to Paper 5's denoising theme.

---

### Techniques to Consider (Future Work / v2+)

1. **Subgraph TopER signatures** — for each article node, compute a TopER signature over its 2-hop neighborhood. Store as a small float array column. Enable structural similarity search.

2. **Topology change monitoring** — run TopER on the full knowledge graph periodically. Alert when topological evolution rate spikes in a domain, indicating rapid knowledge restructuring.

3. **Structural clustering** — use TopER embeddings to cluster articles by structural role (hubs, bridges, leaves, cluster cores) independent of semantic content.

---

### Limitations / Inapplicable Aspects

- TopER is designed for *graph-level* classification, not *node-level* retrieval. Adapting it to per-node use requires computing subgraph signatures, which adds overhead.
- Persistent Homology computation is O(n³) in theory, though TopER's simplification makes it practical for small subgraphs.
- PostgreSQL has no native topology computation. This would require Rust extension work or an external computation pipeline.
- **Not a priority for Covalence v1.** Flag for v2 or research prototype.

---

## Paper 5 — DEG-RAG: Denoising Knowledge Graphs For Retrieval Augmented Generation {#paper-5}

**Citation:** Zheng et al. (2025). *Denoising Knowledge Graphs For Retrieval Augmented Generation.* arXiv:2510.14271v1.  
**Type:** Novel framework with empirical evaluation  
**Tagline:** "Less is More"

---

### Core Contribution / Key Insight

When knowledge graphs are constructed from LLMs (as in Microsoft GraphRAG and similar systems), they accumulate two types of noise:

1. **Redundant entities** — the same real-world concept appears under multiple surface forms ("Machine Learning", "ML", "machine learning") creating node explosion and retrieval dilution
2. **Erroneous relations** — hallucinated or spurious triples that degrade retrieval precision

DEG-RAG's insight: **a smaller, cleaner graph consistently outperforms a larger, noisier one for RAG tasks.** Quality beats quantity.

**Two-stage denoising pipeline:**

**Stage 1 — Entity Resolution:**
- Blocking strategies to avoid O(n²) comparison (e.g., n-gram/token-based blocking, embedding clustering)
- Multiple embedding choices evaluated (OpenAI Ada, domain-specific encoders)
- Similarity metrics compared (cosine, Jaccard, edit distance)
- Entity merging: canonical node selection, attribute merging

**Stage 2 — Triple Reflection:**
- For each triple (subject, predicate, object), apply a reflection check: "Does this relation make semantic sense given the entities?"
- LLM-based or rule-based filtering of spurious edges
- Removes low-confidence relations that the original extraction hallucinated

**Results:** Drastic graph size reduction + consistent QA improvement across multiple GraphRAG variants.

---

### Relevance to Covalence

Covalence's existing architecture has a partial answer to this problem through its `contention` system and `confidence` scoring. But DEG-RAG reveals that Covalence needs **explicit deduplication and quality filtering** at the knowledge graph level, not just at the article level.

The **entity resolution problem** in Covalence surfaces when:
- Multiple sources reference the same real-world concept with variant names
- The same article exists under multiple titles after splits/merges
- Tags and domain paths drift over time

The **triple reflection** concept maps to Covalence's contention resolution mechanism — but DEG-RAG suggests this should be more systematic and proactive, not just reactive to detected contradictions.

---

### Techniques to Adopt

1. **Entity normalization pipeline** — before inserting new nodes, run fuzzy deduplication against existing nodes in the same domain. If similarity (embedding + edit distance) exceeds a threshold, merge rather than create.

2. **Edge confidence scoring** — all relationships in Covalence (provenance links, article-to-article references) should carry confidence scores. Below a threshold, mark for review or prune automatically.

3. **Proactive deduplication maintenance** — add a `deduplicate` operation to `admin_maintenance`. Periodically cluster nodes by embedding similarity within domains; flag or merge near-duplicates.

4. **Canonical entity registry** — maintain a master entity registry mapping variant surface forms to canonical IDs. This solves the object permanence problem (a concept referenced 10 different ways still maps to one node).

5. **Size ≠ quality principle** — track graph density metrics. A Covalence workspace with 10K high-quality articles may outperform one with 100K noisy articles. Implement organic eviction that prioritizes quality over raw coverage.

---

### Limitations / Inapplicable Aspects

- DEG-RAG is designed for static KG construction pipelines. Covalence is *dynamic and continuously evolving* — denoising must be incremental, not batch-only.
- Entity resolution at scale is expensive (even with blocking). For Covalence, domain partitioning provides natural blocking — only compare entities within the same `domain_path`.
- Triple reflection via LLM adds latency and cost; use selectively on low-confidence edges rather than the full graph.

---

## Paper 6 — Efficient Knowledge Graph Construction and Retrieval from Unstructured Text {#paper-6}

**Citation:** [SAP Research] (2025). *Efficient Knowledge Graph Construction and Retrieval from Unstructured Text for Large-Scale RAG Systems.* arXiv:2507.03226v2. CIKM 2025.  
**Type:** Industrial system paper with enterprise-scale experiments  
**Domain:** Legacy code migration (SAP S/4HANA)

---

### Core Contribution / Key Insight

This paper solves the **practical deployment problem** that most GraphRAG research ignores: LLM-based KG construction is wildly expensive at enterprise scale (the paper calculates ~65 days of API calls for 550 documents). The solution: **dependency-parser-based triple extraction** using SpaCy, eliminating LLM calls from the construction pipeline entirely.

Key finding: dependency parsing achieves **94% of GPT-4o performance** at a fraction of the cost.

The retrieval strategy is deliberately simple and effective:
- **One-hop traversal** from seed nodes (not multi-hop, which blows up complexity)
- **Hybrid seed identification**: SpaCy noun phrase extraction + vector similarity search → merge → seed nodes
- **Dense re-ranking** of one-hop neighbors via cosine similarity
- **Context = {chunks, relations, entities}** — richer than standard RAG alone

Dual storage: vector DB (Milvus for embeddings) + in-memory graph (iGraph for traversal). The paper advocates keeping these separate rather than forcing one system to handle both.

**Results:**
- 15% improvement in context precision over dense-only RAG
- 32% reduction in "no coverage" responses
- Code migration quality: GraphRAG wins 78.5% of pairwise comparisons vs. dense retrieval

---

### Relevance to Covalence

This paper validates Covalence's core architectural decision: **use both a graph database and a vector database, but use them for different things.** Covalence's PostgreSQL + AGE + PGVector stack is exactly this separation, just integrated within one PostgreSQL instance (a significant advantage over the paper's Milvus + iGraph setup — no cross-database synchronization overhead).

The **dependency-parsing approach** is directly relevant to how Covalence extracts knowledge from raw sources. Currently, Covalence relies on LLM compilation for everything. A dependency-parser-based extraction pass could:
- Pre-populate entity/relation structure cheaply before LLM refinement
- Provide a "fast path" structural skeleton (mirroring MAGMA's synaptic ingestion)
- Dramatically reduce LLM API costs for large-scale ingestion

The **cascaded retrieval** model (broad one-hop recall → dense re-ranking) maps to Covalence's retrieval pipeline:
1. AGE Cypher: identify anchor articles, traverse 1-hop neighbors → candidate set
2. PGVector: re-rank candidates by embedding similarity to query
3. pg_textsearch: optional BM25-style lexical boost via RRF

The **context = {chunks, relations, entities}** formulation is powerful: retrieved context should include not just article content, but the relationship structure explaining *why* these articles are related.

---

### Techniques to Adopt

1. **Cascaded retrieval architecture** — explicitly implement a two-stage retrieval: (1) graph traversal for high-recall candidate generation, (2) vector re-ranking for precision. Don't try to do both in one query.

2. **Hybrid anchor identification** — use RRF to fuse text-based entity extraction (from query) + vector ANN search + keyword search to identify anchor nodes before traversal begins.

3. **One-hop-first discipline** — resist the temptation to do deep multi-hop traversal by default. Start with one-hop + re-ranking. Add additional hops only when shallow retrieval demonstrably fails (e.g., bridge queries that require two hops).

4. **Dependency-parser pre-extraction** — for large-scale source ingestion, use spaCy or equivalent to extract subject-relation-object triples as a cheap structural skeleton before LLM compilation. Enables the "fast path" in dual-stream ingestion.

5. **Entity normalization in extraction** — apply canonicalization during extraction to prevent the duplicate entity problem (synergy with Paper 5).

6. **Context packaging** — when returning results to an agent, package as structured context: `{articles: [...], relationships: [...], entities: [...]}` rather than flat text. This gives the LLM more signal for multi-hop reasoning.

---

### Limitations / Inapplicable Aspects

- Dependency parsing misses implicit, context-dependent relations — a known limitation acknowledged by the paper. For Covalence's higher-level knowledge (design decisions, conceptual relationships), LLM extraction remains necessary.
- One-hop retrieval is insufficient for complex reasoning chains (the paper acknowledges this as future work). Covalence needs configurable hop depth.
- The paper uses iGraph (in-memory) — Apache AGE's persistent, ACID-compliant graph storage is a better fit for Covalence's durability requirements, at the cost of some query speed.

---

## SYNTHESIS: What These Papers Collectively Tell Us About Building Covalence {#synthesis}

### Finding 1: The Field Has Converged on Graph + Vector as the Correct Substrate

All six papers, from different angles, arrive at the same architectural conclusion: **a knowledge substrate for AI agents needs both a graph database and a vector database, used for complementary purposes.** The graph handles structural/relational queries (traversal, multi-hop reasoning, causal chains, entity tracking); the vector store handles semantic similarity (ANN search, embedding-based retrieval). Neither alone is sufficient.

**Covalence's PostgreSQL + AGE + PGVector architecture is directly validated by this convergence.** The additional advantage Covalence has over systems like MAGMA and the SAP paper is that all three capabilities live in one ACID-transactional PostgreSQL instance — no cross-database synchronization overhead.

**Implication:** Double down on this architecture. Make the graph + vector duality a first-class design principle, not an implementation detail.

---

### Finding 2: Typed Edges Are Not Optional — They Are the Core Value Proposition

Every paper that proposes a specific architecture (MAGMA, DEG-RAG, the SAP paper) emphasizes that **undifferentiated edges are the root cause of poor retrieval quality.** When you can't distinguish "A caused B" from "A is semantically similar to B" from "A followed B in time," your retrieval system is flying blind.

MAGMA decomposes this into four orthogonal graph layers. Paper 1's survey validates this across the literature. The SAP paper shows that even simple entity-to-entity vs. entity-to-chunk edge typing improves performance.

**Implication for Covalence:** The existing provenance relationship types (`originates`, `confirms`, `supersedes`, `contradicts`, `contends`) are a good start but insufficient. The schema needs a richer edge taxonomy:

```
# Structural/provenance (existing — keep)
:ORIGINATES, :CONFIRMS, :SUPERSEDES, :CONTRADICTS, :CONTENDS

# Temporal (new)
:PRECEDES, :FOLLOWS, :CONCURRENT_WITH

# Causal/logical (new)
:CAUSES, :ENABLES, :PREVENTS, :MOTIVATED_BY, :IMPLEMENTS

# Semantic/thematic (new)
:RELATES_TO, :ELABORATES, :EXEMPLIFIES, :GENERALIZES

# Entity linkage (new)
:INVOLVES, :AUTHORED_BY, :LOCATED_IN, :PART_OF
```

This does not require rewriting the system — it is a schema evolution. But it unlocks query routing by intent (the MAGMA technique) and structured context packaging.

---

### Finding 3: Retrieval Must Be Intent-Aware, Not One-Size-Fits-All

MAGMA's most important engineering insight is that **query intent classification drives retrieval strategy.** A "Why did X happen?" query should prioritize causal edges. A "When did Y occur?" query should traverse temporal edges. An "What is Z related to?" query should use semantic similarity. Using the same retrieval path for all queries is provably suboptimal.

Papers 1, 2, and 3 all reach this conclusion from different angles. The SAP paper demonstrates it empirically.

**Implication for Covalence:** The Go CLI's `knowledge_search` must evolve toward intent-aware retrieval. Minimally:
1. Detect query intent (lightweight classifier or keyword heuristics)
2. Adjust the AGE Cypher traversal to weight relevant edge types
3. Apply RRF to fuse graph traversal results with PGVector ANN results and pg_textsearch BM25

This can be phased: start with explicit intent parameters in the API; eventually auto-detect intent from the query text.

---

### Finding 4: Dual-Stream Ingestion (Fast + Slow) Is the Right Write Architecture

Both MAGMA and the existing Covalence design independently arrive at the same write pattern:

| Path | Operation | Latency | LLM Required? |
|------|-----------|---------|---------------|
| **Fast path** | Source ingestion, vector indexing, temporal edge creation | Milliseconds | No |
| **Slow path** | Article compilation, causal/entity edge inference, consolidation | Seconds to minutes | Yes |

MAGMA calls these "Synaptic Ingestion" and "Structural Consolidation." Covalence calls them `source_ingest` and `article_compile`. The names differ; the architecture is the same.

**Implication:** This is validation, not a gap. However, the slow path should be more explicitly managed:
- The mutation queue should expose visibility into pending consolidation work
- Agents should know when memory is "fresh but uncompiled" vs. "fully consolidated"
- The slow path should proactively infer edges (causal, entity) using LLM reasoning, not just compile text summaries

---

### Finding 5: Graph Quality Beats Graph Size — Denoising Is an Ongoing Discipline

DEG-RAG (Paper 5) and the SAP paper (Paper 6) both provide empirical evidence that **a smaller, higher-quality graph outperforms a larger, noisier one** for retrieval tasks. The "more data is always better" intuition fails in knowledge graphs.

**Covalence has an organic eviction mechanism, but lacks a systematic quality maintenance pipeline.** The right model is continuous, incremental quality enforcement, not occasional bulk eviction.

**Implication — Three quality pillars to implement:**

1. **Entity deduplication** (Paper 5): Fuzzy merge of near-duplicate nodes within domains, using embedding similarity + edit distance + domain_path blocking. Run as part of `admin_maintenance`.

2. **Edge confidence scoring** (Papers 2, 5): All edges should carry confidence/weight scores. Low-confidence edges are pruned first during quality maintenance; high-confidence edges drive retrieval.

3. **Triple reflection / contention resolution** (Papers 5, existing Covalence): The existing `contention_list` and `contention_resolve` tools are on the right track. Make contention resolution more proactive — detect potential contradictions during ingestion, not just after the fact.

---

### Finding 6: Topology and Structure Are Underexplored Retrieval Signals

Paper 4 (TopER) represents an emerging research direction that none of the other papers address: **using graph topology itself as a retrieval signal.** The structural role of a node (hub, bridge, leaf, cluster center) is information that semantic embeddings cannot capture.

**This is an open research question for Covalence**, not a deployment recommendation. But the insight is valuable: in a mature knowledge graph, a "hub" article (connected to many domains) behaves differently in retrieval than a "leaf" article (connected to only one cluster). Retrieval ranking should account for this.

**Potential future signal:** Node degree, betweenness centrality, or TopER structural signature as one component of the multi-factor `usage_score`.

---

### Finding 7: The Memory Lifecycle Is the Right API Surface

Paper 1's lifecycle framework (Extract → Store → Retrieve → Evolve) and Paper 3's dynamics framework (Formation → Evolution → Retrieval) converge on the same API surface. Every operation in a knowledge substrate falls into one of these categories, and they have different performance, consistency, and cost characteristics.

**Covalence's current API already covers all four lifecycle stages.** The gap is not in what's available but in how they're composed and tuned:

| Lifecycle Stage | Covalence Operations |
|----------------|---------------------|
| **Extraction** | `source_ingest` |
| **Storage** | `source_ingest` + `article_create/compile` |
| **Retrieval** | `knowledge_search` + `source_search` + `article_get` |
| **Evolution** | `article_update` + `article_merge/split` + `admin_maintenance` + `contention_resolve` |

**Implication:** The API surface is approximately right. Investment should go into making each stage smarter (intent-aware retrieval, richer edge typing, dual-stream efficiency) rather than adding new operations.

---

### Open Questions for Covalence

1. **Traversal depth strategy:** MAGMA uses heuristic beam search; the SAP paper recommends one-hop-first. Covalence needs a configurable, empirically-tuned hop depth strategy. What should the default be?

2. **Causal/temporal edge inference:** MAGMA uses LLM reasoning in the slow path. What is the right trigger, cost model, and confidence threshold for Covalence's article-level graph?

3. **Canonical entity registry:** The entity resolution problem (Paper 5) requires a canonical entity registry. Where does this live in Covalence's schema, and how does it interact with `domain_path` tagging?

4. **Multi-agent memory isolation:** Paper 3 identifies this as a frontier; MAGMA doesn't address it. Covalence needs a principled namespace design for shared vs. private memory partitions.

5. **Topological signals:** When does graph topology become a useful retrieval signal? The prerequisite is a stable, mature knowledge graph with consistent edge typing — probably a v2+ consideration.

6. **Differential memory decay:** Paper 3's functional taxonomy implies different retention policies for factual vs. experiential vs. working memory. Covalence's current scoring doesn't distinguish memory function type — should it?

---

### Consensus Summary Table

| Design Decision | Consensus Level | Evidence |
|----------------|----------------|----------|
| Graph + Vector dual substrate | **Very High** | Papers 1, 2, 4, 5, 6 |
| Typed edges as first-class data | **Very High** | Papers 1, 2, 3, 6 |
| Hybrid retrieval (graph + vector + text) via RRF | **High** | Papers 2, 5, 6 |
| Dual-stream ingestion (fast/slow) | **High** | Papers 2, 6; validated by Covalence existing design |
| Intent-aware query routing | **High** | Papers 1, 2, 3 |
| Graph quality > graph size | **High** | Papers 5, 6 |
| Memory lifecycle as primary API framing | **High** | Papers 1, 3 |
| Richer edge taxonomy (causal, temporal, entity) | **High** | Papers 1, 2 |
| LLM-free extraction as fast path | **Medium** | Paper 6 |
| Entity deduplication as ongoing discipline | **Medium** | Papers 5, 6 |
| Topology as retrieval signal | **Low (emerging)** | Paper 4 |

---

## Priority Recommendations for Covalence

Based on this literature review, ranked by impact vs. implementation cost:

### Tier 1 — High Impact, Achievable Now (v1 scope)
1. **Expand edge taxonomy** in AGE schema to include typed temporal/causal/entity/semantic labels
2. **Implement RRF** in `knowledge_search` to fuse vector + text + graph scores
3. **Add intent parameters** to `knowledge_search` API (intent=factual|temporal|causal|entity)
4. **Proactive entity deduplication** in `admin_maintenance` using embedding clustering within domain_path

### Tier 2 — High Impact, Needs Design Work (v1.5 / v2)
5. **Slow-path edge inference** — background worker that infers causal/entity links during consolidation
6. **Structured context packaging** — return `{articles, relationships, entities}` from `knowledge_search`
7. **Memory function tagging** — annotate articles with `factual | experiential | working` type
8. **Dependency-parser fast path** — pre-extraction skeleton before LLM compilation for large ingestion batches

### Tier 3 — Research / Future (v2+)
9. **Topological signatures** per node for structural retrieval dimension
10. **Differential decay** by memory function type
11. **Multi-agent namespace isolation** design

---

*End of literature review.*  
*Last updated: 2026-03-01 by literature-review subagent.*  
*Source papers: arXiv 2602.05665, 2601.03236, 2512.13564, 2410.01778, 2510.14271, 2507.03226*

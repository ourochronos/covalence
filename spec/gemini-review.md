Loaded cached credentials.
This review analyzes the **GraphRAG Specification** across its 9 component documents. The architecture is sophisticated, blending hippocampal-neocortical memory models (CLS) with formal epistemic logic (Subjective Logic, Pearl's Hierarchy).

---

### 1. Holes & Critical Risks

#### **A. The Sync Bottleneck (01-Architecture & 04-Graph)**
*   **Hole:** The spec proposes `LISTEN/NOTIFY` for syncing PostgreSQL to the Rust `petgraph` sidecar. 
*   **Risk:** `LISTEN/NOTIFY` has an 8000-byte payload limit. During a "Batch Consolidation" or a large PDF ingestion, a single transaction could modify thousands of edges. Notifications will be dropped, or the sidecar will be forced into a "Full Reload" loop, killing performance.
*   **Solution:** Implement the **Outbox Pattern**. Use a `graph_changes` table in PG. `LISTEN/NOTIFY` should send a "poke" (no payload). The Rust sidecar then queries the `graph_changes` table for all IDs since its last `last_processed_txid`.

#### **B. Entity Resolution Race Conditions (05-Ingestion)**
*   **Hole:** Stage 7 (Resolution) describes matching against existing nodes. In a parallel ingestion pipeline (multiple Axum workers), two workers may simultaneously decide that "Apple" doesn't exist and attempt to create two canonical "Apple" nodes with different UUIDs.
*   **Solution:** Use a PostgreSQL `INSERT ... ON CONFLICT (canonical_name, node_type) DO UPDATE` or a dedicated "Global Name Service" actor in Rust to serialize creation of new canonical entities.

#### **C. Epistemic Oscillation (07-Epistemic-Model)**
*   **Hole:** You use a 5-stage propagation pipeline: Dempster-Shafer $\rightarrow$ Subjective Logic $\rightarrow$ DF-QuAD $\rightarrow$ Decay $\rightarrow$ TrustRank.
*   **Risk:** These frameworks have different mathematical axioms. Applying them sequentially without a "Convergence Guard" can lead to **belief oscillation**. For example, a `CONTRADICTS` edge (Stage 3) might lower confidence, which then triggers a TrustRank recalibration (Stage 5) that erroneously boosts it back up because of structural connectivity.
*   **Solution:** Define Stage 1-2 as **Local Evidence Aggregation** (Atomic) and Stage 3-5 as **Structural Belief Revision** (Global). Global updates must be damped (e.g., using a learning rate) and should only run after Local Aggregation has reached a steady state for a transaction.

#### **D. The "Secure by Default" Failure (03-Storage & 09-Federation)**
*   **Hole:** `03-storage.md` sets `clearance_level DEFAULT 2` (Public). 
*   **Risk:** If a parser fails to extract a clearance level or a developer forgets to set it, sensitive data becomes `federated_public` by default.
*   **Solution:** Change all schema defaults to `0` (`local_strict`). Force an explicit "Promotion" step during the consolidation pipeline to move data to `federated_public`.

---

### 2. Cross-Document Inconsistencies

#### **A. Causal Level Mapping**
*   **04-Graph:** Lists `CausalLevel` as an Enum: `Association`, `Intervention`, `Counterfactual`.
*   **07-Epistemic:** Maps these to Pearl’s Hierarchy (L0, L1, L2) but adds `ORIGINATES`, `CONFIRMS`, `SUPERSEDES` as edge types.
*   **Mismatch:** `03-storage.md` (SQL) doesn't have a `causal_level` column on `edges`; it hides it in a `JSONB` properties blob.
*   **Solution:** Promote `causal_level` to a first-class `SMALLINT` or `ENUM` column on the `edges` table. It is too central to the search/reasoning engine to be buried in JSONB.

#### **B. Vector Dimension Heterogeneity**
*   **03-Storage:** Uses `halfvec(768)` for Chunks, Nodes, Aliases, and Articles.
*   **Real-world clash:** Node aliases often benefit from small, fast models (BGE-small, 384d), while Articles require high-capacity models (OpenAI/Voyage, 1536d+). 
*   **Solution:** Decouple dimensions. Use a separate `embeddings` table with `(entity_id, entity_type, model_id, vector)` or use `vector` (without dimension constraint) if your PG version/extension supports it.

#### **C. "Article" vs "Chunk" Retrieval**
*   **06-Search:** States "Articles are the primary retrieval target."
*   **03-Storage:** Shows the HNSW index on the `chunks` table, not the `articles` table.
*   **Consistency Fix:** Ensure `articles` table in `03-storage.md` includes an HNSW index definition matching the one on `chunks`.

---

### 3. Implementation Risks & Prototyping Priorities

1.  **HIGH RISK: Subjective Logic Implementation.** Jøsang’s operators (cumulative vs. average fusion) are non-trivial to implement correctly in a graph context. 
    *   *Prototype first:* A standalone Rust crate for Subjective Logic arithmetic.
2.  **MEDIUM RISK: Markdown Normalization.** Preserving table semantics and heading hierarchies during PDF-to-MD conversion is notoriously "leaky."
    *   *Prototype first:* The ingestion parser for complex, multi-page tables.
3.  **LOW RISK: pgvector Search.** This is well-understood and stable.

---

### 4. Missing Pieces

1.  **The "Correction" Loop:** There is no API for a human user to manually "Split" a node that was incorrectly resolved (e.g., "Apple" the fruit vs "Apple" the company) or "Merge" two nodes. This is an operational requirement for any GraphRAG system.
2.  **Cost Budgeting:** The spec mentions LLM calls for prefixes, extraction, and compilation. A single document could trigger 50+ LLM calls. You need a `Budget` or `Priority` field in the `SourceInput` to prevent runaway API costs.
3.  **Audit Logs:** While `provenance` is tracked, there is no `audit_log` for *system decisions* (e.g., "Why did the Deep Consolidation tier prune this edge?").

---

### 5. Proposed Design Decisions (Resolving TBDs)

*   **Decision (02): Property Graph vs Triples?** 
    *   **Recommendation:** **Hybrid (Option C).** Use Property Graph for the primary store (performance/simplicity) but implement a `triples_view` for the Epistemic Model to perform its DS/SL math on atomic claims.
*   **Decision (04): Sync Mechanism?** 
    *   **Recommendation:** **Outbox + Logical Replication.** If you want <1s latency and high reliability, use `pg_recvlogical` to stream changes directly into the Rust sidecar.
*   **Decision (05): Contextual Prefixes?** 
    *   **Recommendation:** **One per Section.** One per document is too broad; one per chunk is too expensive. Section-level prefixes provide the best cost/context balance.

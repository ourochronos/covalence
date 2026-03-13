# V2 Architecture Migration Plan (Statement-First & Gemini Flash 3.0)

**Date:** March 2026
**Target:** Convert the existing hybrid/chunk-based ingestion pipeline into the uncompromising "Statement-First" architecture defined in `spec/05-ingestion.md`, leveraging Gemini Flash 3.0, Offset Projection Ledgers, and HDBSCAN.

This document outlines the efficient, step-by-step technical path to migrate the existing Rust codebase to the new architecture without breaking the operational integrity of the graph engine.

## Phase 1: Storage & Schema Foundation

Before we touch the pipeline logic, the database must be ready to accept the new data primitives.

1.  **Drop Chunk-Specific Columns (Optional but Recommended):**
    *   Deprecate columns in the `chunks` table that relate to the legacy embedding landscape analysis (e.g., `parent_alignment`, `adjacent_similarity`, `sibling_outlier_score`).
    *   *Note:* The `chunks` table will stick around solely for code AST chunking, but prose will no longer use it.
2.  **Create `statements` Table:**
    *   Create a new table for atomic statements.
    *   Fields: `id`, `source_id`, `content`, `canonical_byte_start`, `canonical_byte_end`, `embedding` (halfvec 1024), `section_id` (for HAC clustering).
3.  **Create `offset_projection_ledgers` Table:**
    *   Fields: `id`, `source_id`, `canonical_span_start`, `canonical_span_end`, `canonical_token`, `mutated_span_start`, `mutated_span_end`, `mutated_token`.
4.  **Create HDBSCAN Residual Pool:**
    *   Create `unresolved_entities` table to catch Tier 5 entity resolution failures.
    *   Fields: `id`, `statement_id`, `extracted_name`, `entity_type`, `embedding`, `created_at`.
5.  **Update `extractions` Table:**
    *   Ensure `statement_id` is supported alongside (or replacing) `chunk_id` for provenance tracking.

## Phase 2: Pipeline Component Isolation

Build the new tools as isolated Rust modules before wiring them into `SourceService::ingest()`.

1.  **Fastcoref Sidecar Integration (`covalence-core/src/ingestion/coref.rs`):**
    *   Implement an HTTP client to a local `fastcoref` sidecar.
    *   Input: Canonical Markdown text.
    *   Output: Mutated text + `Vec<LedgerEntry>`.
2.  **Offset Projection Engine (`covalence-core/src/ingestion/projection.rs`):**
    *   Implement the mathematical reverse-projection logic.
    *   Function: `fn reverse_project(mutated_span: (usize, usize), ledger: &[LedgerEntry]) -> (usize, usize)`
3.  **Gemini Flash 3.0 Client (`covalence-core/src/ingestion/extractor/gemini.rs`):**
    *   Implement the Gemini API client utilizing Flash 3.0's massive context window.
    *   Configure two strict JSON-schema prompts:
        *   **Pass 1:** Windowed Statement Extraction.
        *   **Pass 2:** Triples Extraction from Statements.
4.  **HDBSCAN Clustering Service (`covalence-core/src/consolidation/hdbscan.rs`):**
    *   Implement a background job that pulls from `unresolved_entities`, clusters their embeddings via HDBSCAN, and proposes new `Canonical Entities`.

## Phase 3: Wiring the Prose Pipeline

Replace the `chunk -> landscape analysis -> extract` flow in `SourceService::ingest()` for prose documents.

1.  **Diverge Pipeline by Type:**
    *   In `ingest()`, check `source_type`. If `Code`, route to the existing AST pipeline. Otherwise, route to the new Prose Pipeline.
2.  **Implement Stage 5-10 (The Core Loop):**
    *   Call `fastcoref` -> save ledger.
    *   Call Gemini Flash 3.0 (Pass 1) -> generate statements.
    *   Embed statements (Voyage or local).
    *   Run HAC clustering on statement embeddings -> compile sections -> compile source summary via LLM.
    *   Call Gemini Flash 3.0 (Pass 2) on statements -> extract entities/triples.
    *   Map entity spans backward using the Offset Projection Engine.
3.  **Update Entity Resolution (Stage 11):**
    *   Modify `PgResolver`.
    *   Ensure Tiers 1-4 execute as normal.
    *   **Crucial Change:** If an entity misses all 4 tiers, DO NOT create a new node immediately. Insert it into the `unresolved_entities` pool (Tier 5) for HDBSCAN.

## Phase 4: Eradicating Legacy Tech Debt

1.  **Remove PG Graph Traversal Fallbacks:**
    *   *Status:* Completed. The `graph_traverse` CTE was successfully removed from Postgres migrations. Ensure all API endpoints strict-depend on the `petgraph` sidecar for traversal queries.
2.  **Remove Landscape Analysis Code:**
    *   Delete the Rust modules responsible for semantic valley detection, cross-document novelty checks, and extraction budgeting (`covalence-core/src/ingestion/landscape.rs`). They are obsolete under the statement-first paradigm.

## Phase 5: Data Migration & Testing

Because the foundational extraction philosophy has changed, old extractions are fundamentally incompatible with the new paradigm.

1.  **Blue/Green Re-Ingestion:**
    *   We cannot perform an in-place mutation of legacy chunks to statements.
    *   Wipe the existing graph (`make reset-db`), or spin up a parallel V2 instance.
    *   Re-ingest all raw sources through the new pipeline.
2.  **Evaluate:**
    *   Use the existing `covalence-eval` framework. Expect Faithfulness and Citation Accuracy to skyrocket due to the elimination of coreference ambiguities and noisy chunks.
# 12 — Code Ingestion

**Status:** Proposed

Code sources require a fundamentally different ingestion pipeline than prose. Splitting a function at a token boundary destroys its meaning. Embedding raw syntax produces vectors that live in a different semantic space than natural language. This spec defines the AST-aware code ingestion pipeline that produces code entities which exist in the same vector space as prose concepts.

## Design Goals

1. **AST-aware chunking** — Split code at logical boundaries (functions, structs, impl blocks, modules), never at arbitrary token counts
2. **Semantic bridging** — Generate natural language summaries of code, embed the summaries, so code and prose share a vector space
3. **Structural fidelity** — Preserve call graphs, type references, module hierarchy, and trait implementations as graph edges
4. **Incremental updates** — Detect which functions changed (via AST hash) and only re-summarize those
5. **Language-extensible** — Start with Rust and Go, design for adding languages

## Pipeline Overview

```
Source (code file)
  │
  ├──→ Tree-sitter parse → AST
  │         │
  │         ├──→ Chunk by AST boundary → code chunks (functions, structs, traits, impls)
  │         │         │
  │         │         ├──→ LLM semantic summary → natural language description
  │         │         │         │
  │         │         │         └──→ Embed summary → vector (same space as prose)
  │         │         │
  │         │         └──→ AST hash → structural change detection
  │         │
  │         └──→ Extract structural edges → CALLS, USES_TYPE, IMPLEMENTS, CONTAINS, DEPENDS_ON
  │
  └──→ Statement pipeline (on semantic summaries) → statements, sections, source summary
```

## Stage 1: AST Parsing

Use Tree-sitter to parse source files into their Abstract Syntax Tree. Tree-sitter provides:
- Language-agnostic API with per-language grammars
- Incremental parsing (only re-parse changed regions)
- S-expression queries for extracting specific node types

### Tree-sitter Queries

**Rust:**
```scheme
;; Functions (including async, pub, etc.)
(function_item
  name: (identifier) @fn_name
  parameters: (parameters) @fn_params
  return_type: (_)? @fn_return
  body: (block) @fn_body) @function

;; Structs
(struct_item
  name: (type_identifier) @struct_name
  body: (field_declaration_list)? @struct_body) @struct

;; Traits
(trait_item
  name: (type_identifier) @trait_name
  body: (declaration_list) @trait_body) @trait

;; Impl blocks
(impl_item
  trait: (type_identifier)? @impl_trait
  type: (type_identifier) @impl_type
  body: (declaration_list) @impl_body) @impl

;; Modules
(mod_item
  name: (identifier) @mod_name) @module

;; Use statements (for dependency edges)
(use_declaration
  argument: (_) @use_path) @use
```

**Go:**
```scheme
;; Functions
(function_declaration
  name: (identifier) @fn_name
  parameters: (parameter_list) @fn_params
  result: (_)? @fn_return
  body: (block) @fn_body) @function

;; Methods
(method_declaration
  receiver: (parameter_list) @method_receiver
  name: (field_identifier) @method_name
  parameters: (parameter_list) @method_params
  body: (block) @method_body) @method

;; Structs
(type_declaration
  (type_spec
    name: (type_identifier) @struct_name
    type: (struct_type) @struct_body)) @struct

;; Interfaces
(type_declaration
  (type_spec
    name: (type_identifier) @interface_name
    type: (interface_type) @interface_body)) @interface
```

### AST Chunk Output

Each AST chunk captures:
```rust
struct AstChunk {
    node_type: CodeNodeType,      // code_function, code_struct, code_trait, etc.
    name: String,                 // qualified name (module::function)
    language: String,             // "rust", "go"
    file_path: String,            // relative to repo root
    line_start: u32,
    line_end: u32,
    raw_source: String,           // full source text of this AST node
    signature: Option<String>,    // function signature, struct definition (no body)
    visibility: String,           // "public", "private", "crate"
    ast_hash: Vec<u8>,            // SHA-256 of AST structure (ignoring whitespace/comments)
    parent_module: Option<String>, // containing module path
    doc_comment: Option<String>,  // /// doc comments
}
```

The `ast_hash` is computed from the AST structure, not the raw text. This means whitespace changes, comment edits, and formatting don't trigger re-summarization — only structural code changes do.

## Stage 2: Semantic Summary (The Translation Pass)

Raw code syntax creates poor vector embeddings when compared against natural language. The semantic summary bridges this gap.

For each AST chunk, pass the raw source to an LLM with this prompt pattern:

```
System: You are a code documentation engine. Read the following {language} code
and output a concise natural language summary of its business logic, inputs,
outputs, and error handling. Focus on WHAT the code does and WHY, not HOW
(implementation details). Write as if explaining to someone who understands
the domain but hasn't read the code.

User: ```{language}
{raw_source}
```

Context: This is part of module `{parent_module}` in file `{file_path}`.
{doc_comment if present}
```

**Model selection:** This is a low-cognitive-load task — a fast model (Gemini Flash, Claude Haiku, or a local 14B) is sufficient. The summary is typically 50-150 words.

The summary is stored as `semantic_summary` in the Node's `properties` JSONB, and the Node's `embedding` is computed from the summary text. This places code entities in the same semantic vector space as prose concepts — enabling cross-domain search.

### Summary Quality

A good semantic summary:
- Uses domain language, not code syntax ("calculates trust scores" not "calls compute_trust_alpha()")
- Mentions inputs and outputs in business terms ("takes a source document and returns extracted entities")
- Notes error conditions in human terms ("fails if the embedding service is unavailable")
- References domain concepts that would appear in specs or research ("implements the Reciprocal Rank Fusion algorithm")

A bad semantic summary:
- Repeats the function signature in English
- Lists every line of code
- Uses implementation details as the description ("iterates over a Vec<Node> and calls .embedding()")

## Entity Class Constraints

All code entities created by this pipeline receive `entity_class = 'code'`. The `node_type` values (`code_function`, `code_struct`, `code_trait`, `code_module`, `code_impl`, `code_type`, `code_test`) all map to the `code` entity class via the `derive_entity_class()` function defined in [spec/02-data-model](02-data-model.md#entity-classification).

Structural code edges (CALLS, USES_TYPE, IMPLEMENTS, CONTAINS, DEPENDS_ON) are constrained to `code` → `code` entity class pairs. Cross-domain edges (PART_OF_COMPONENT, IMPLEMENTS_INTENT, THEORETICAL_BASIS) bridge between entity classes and follow the constraints documented in [spec/02-data-model](02-data-model.md#traceability-edge-types).

## Stage 3: Structural Edge Extraction

Tree-sitter provides structural relationships for free. These become typed edges in the graph:

### Call Graph (CALLS)
Identify function calls within function bodies:
```scheme
;; Rust
(call_expression
  function: (identifier) @callee)
(call_expression
  function: (field_expression
    value: (_) @receiver
    field: (field_identifier) @callee))

;; Go
(call_expression
  function: (identifier) @callee)
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @callee))
```

Resolution: Match `@callee` against known function names in the same crate/package. Cross-crate calls are tracked but not resolved to external nodes.

### Type References (USES_TYPE)
Functions reference types through parameters, return types, and local variables. Extract these as edges linking `code_function` → `code_struct`/`code_type`.

### Module Hierarchy (CONTAINS)
Every function, struct, trait belongs to a module. The `CONTAINS` edge preserves this hierarchy: `code_module` → `code_function`.

### Trait Implementations (IMPLEMENTS)
Impl blocks that name a trait create an `IMPLEMENTS` edge: `code_impl` → `code_trait`.

### Dependencies (DEPENDS_ON)
`use` statements create `DEPENDS_ON` edges between modules. These enable dependency-aware queries ("what depends on the embedder module?").

## Stage 4: Statement Extraction from Code

After semantic summaries are generated, the standard statement pipeline (ADR-0015) runs on the summaries — not the raw code. This produces atomic statements about what the code does, which cluster into topics and compile into source summaries.

For code sources, the statement extraction window is the semantic summary of a single function or struct. This is already a self-contained unit, so windowing is trivial.

The statements reference the code entity they describe (via the code Node's ID), providing full provenance: `statement → code_function → file:line → raw source`.

## Stage 5: Component Linking

Components (design doc bridge nodes) are created either:
1. **Manually** — a human defines logical components ("Ingestion Pipeline", "Search Fusion", "Entity Resolution")
2. **From design docs** — the statement pipeline extracts topics from design docs/specs; high-level topics become Component candidates
3. **By clustering** — code entities cluster by semantic summary similarity; clusters become Component candidates

Once Components exist, bridge edges are created:

### Upward (to intent):
- `IMPLEMENTS_INTENT`: link Component to the spec/design doc topic nodes that describe what this component should do
- Detected by: semantic similarity between Component description and spec topic statements

### Downward (to code):
- `PART_OF_COMPONENT`: link code_function/code_struct nodes to their parent Component
- Detected by: module path matching (all functions in `src/ingestion/` → "Ingestion Pipeline" component) + semantic similarity fallback

### Lateral (to research):
- `THEORETICAL_BASIS`: link Component to the research paper entities that describe the theory behind the approach
- Detected by: semantic similarity between Component description and research paper statements

## Incremental Updates

When a code file is re-ingested:

1. **Parse** the new AST
2. **Compare** AST hashes against stored `ast_hash` values
3. For unchanged chunks: **skip** (no re-summarization, no re-embedding)
4. For changed chunks: **re-summarize** → re-embed → update Node properties
5. **Re-extract** structural edges (call graph may have changed)
6. **Re-link** Components if semantic summaries shifted significantly
7. For deleted chunks: **evict** (mark is_evicted, remove from active search)

This makes re-ingestion proportional to the size of the change, not the size of the codebase.

## Supported Languages

### Phase 1: Rust and Go
These are the languages used in Covalence itself. Rust has complex AST patterns (impl blocks, traits, macros, async) that stress-test the pipeline. Go is simpler but covers the CLI codebase.

### Phase 2: Python, TypeScript
Natural extensions for broader applicability. Tree-sitter grammars are mature for both.

### Adding a Language
To add a new language:
1. Add the Tree-sitter grammar as a build dependency
2. Write the S-expression queries for the language's AST patterns
3. Map AST node types to `CodeNodeType` variants
4. Test with representative files

The semantic summary prompt, embedding, graph storage, and Component linking are language-agnostic — only the AST parsing layer is language-specific.

## Integration with Existing Pipeline

Code ingestion uses the same `Source`, `Node`, `Edge`, `Statement`, `Section` entities as prose ingestion. The differences are:

| Concern | Prose Pipeline | Code Pipeline |
|---------|---------------|---------------|
| Chunking | Overlapping windows on normalized text | AST boundary detection via Tree-sitter |
| Embedding source | Chunk text (with contextual prefix) | Semantic summary (natural language) |
| Structural edges | LLM-extracted entities and relationships | AST-extracted call graph, type refs, module hierarchy |
| Entity resolution | Vector + fuzzy matching | Qualified name matching + vector fallback |
| Re-ingestion | Content hash comparison | AST hash comparison (structural change detection) |

Both pipelines produce Nodes, Edges, Statements, Sections, and Source Summaries that participate equally in search and graph algorithms.

## Cross-Domain Search

Once code entities share a vector space with prose concepts, cross-domain queries work naturally:

- **"How does entity resolution work?"** → returns spec statements, research paper statements, AND code function summaries about entity resolution
- **"What depends on the embedder?"** → graph traversal via DEPENDS_ON + CALLS edges
- **"What code implements the ingestion spec?"** → traversal: Spec Topic ←IMPLEMENTS_INTENT← Component ←PART_OF_COMPONENT← code_function

See [spec/13-cross-domain-analysis](13-cross-domain-analysis.md) for the full analysis capabilities this enables.

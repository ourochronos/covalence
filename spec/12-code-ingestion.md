# 12 — Code Ingestion

**Status:** Implemented

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
  │         │         ├──→ AST hash → structural change detection
  │         │         │
  │         │         └──→ Extract entities (including methods from impl blocks)
  │         │                   │
  │         │                   └──→ Extract structural edges (CALLS, USES_TYPE, IMPLEMENTS,
  │         │                        CONTAINS, DEPENDS_ON) with noise filtering
  │         │
  │         └──→ Per-entity async jobs (SummarizeEntity):
  │                   │
  │                   ├──→ Definition-pattern chunk matching → find entity's source code
  │                   │
  │                   ├──→ LLM semantic summary → natural language description
  │                   │         │
  │                   │         └──→ Embed summary → vector (same space as prose)
  │                   │
  │                   └──→ Fan-in trigger (ComposeSourceSummary):
  │                             │
  │                             └──→ Bottom-up file summary from entity summaries
  │                                       │
  │                                       └──→ Re-embed file summary → Source.embedding
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

Each entity extracted from the AST gets its own semantic summary. The pipeline locates the entity's source code via **definition-pattern chunk matching**, then sends it to an LLM for summarization.

### Definition-Pattern Chunk Matching

To find the right chunk for summarization, the pipeline constructs a definition pattern from the entity's type and name:

| Entity Type | Pattern |
|------------|---------|
| function | `fn {name}` |
| struct | `struct {name}` |
| enum | `enum {name}` |
| trait | `trait {name}` |
| impl_block | `impl {name}` |
| module | `mod {name}` |
| constant | `const {name}` |
| macro | `macro_rules! {name}` |

The pattern is matched against chunks in two stages:
1. **Extraction-linked search** — Find chunks linked to the entity via the `extractions` table whose content contains the definition pattern. This is the most precise match.
2. **Source-wide fallback** — If no extraction-linked chunk matches, search all chunks from the same source for the definition pattern, preferring the shortest matching chunk (most focused).

If neither produces a match, the entity's description (from AST extraction) is used as input. Chunks shorter than 50 characters are skipped as too short to produce useful summaries.

### Summary Prompt

For each entity, pass the matched source code to an LLM:

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

**Model selection:** This is a low-cognitive-load task — Claude Haiku is the default, with copilot and Gemini as fallbacks via the `ChainChatBackend`. The summary is typically 50-150 words.

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
The AST extractor recursively walks function bodies to find `call_expression` nodes and extracts call targets in three forms:

- **Direct calls**: `foo()` — extracted from `identifier` nodes
- **Method calls**: `self.foo()` or `obj.foo()` — extracted from `field_expression` nodes (the `field` child)
- **Scoped calls**: `Module::foo()` — extracted from `scoped_identifier` nodes (the `name` child)

Call targets are deduplicated per function. A **noise filter** excludes common methods that produce uninformative edges:

| Category | Excluded Names |
|----------|---------------|
| Conversion | `clone`, `to_string`, `into`, `from` |
| Option/Result | `unwrap`, `expect`, `ok`, `err`, `map`, `and_then`, `unwrap_or`, `unwrap_or_default`, `unwrap_or_else`, `is_some`, `is_none`, `is_ok`, `is_err` |
| Reference | `as_ref`, `as_str`, `as_deref` |
| Collection | `len`, `is_empty`, `push`, `insert`, `get`, `contains`, `iter`, `collect`, `filter` |
| IO/Logging | `format`, `println`, `eprintln`, `write`, `debug`, `info`, `warn`, `error` |
| sqlx | `bind`, `execute`, `fetch_one`, `fetch_all`, `fetch_optional` |
| Async | `await` |

Resolution: Match `@callee` against known function names in the same crate/package. Cross-crate calls are tracked but not resolved to external nodes.

### Type References (USES_TYPE)
Functions reference types through parameters and return types. The extractor walks the `parameters` and `return_type` fields of function nodes to find `type_identifier` AST nodes.

A **primitive/common type filter** excludes types that would create noisy edges:

| Category | Excluded Types |
|----------|---------------|
| Primitives | `bool`, `u8`–`u128`, `i8`–`i128`, `usize`, `isize`, `f32`, `f64`, `str` |
| Standard types | `String`, `Vec`, `Option`, `Result`, `Box`, `Arc`, `Rc`, `HashMap`, `HashSet`, `BTreeMap`, `BTreeSet`, `Cow`, `Pin`, `Future` |
| Marker traits | `Send`, `Sync`, `Clone`, `Debug`, `Display`, `Default` |
| Iterator traits | `Iterator`, `IntoIterator` |
| Serde | `Serialize`, `Deserialize` |
| Self | `Self` |

Only domain-specific types survive the filter, producing edges like `code_function` → `code_struct`/`code_type`.

### Module Hierarchy and Method Containment (CONTAINS)
Every function, struct, trait belongs to a module. The `CONTAINS` edge preserves this hierarchy: `code_module` → `code_function`.

**Method extraction from impl blocks:** The AST extractor walks into impl block bodies and extracts each method as a **separate function entity** with its own `CONTAINS` edge back to the impl block. This applies to:

- **Rust impl blocks** — `function_item` children of the impl body are extracted via `extract_rust_function_full`, producing full function entities with their own CALLS and USES_TYPE relationships, plus a `contains` edge from the impl block to the method.
- **Python classes** — `function_definition` children of the class body are extracted as individual function entities with `contains` edges from the class.
- **Go methods** — `method_declaration` nodes automatically get a `contains` edge from the receiver type (e.g., `Server` → `Server.Start`). Go method names are qualified with the receiver type: `ReceiverType.MethodName`.

Each extracted method gets its own AST hash, enabling incremental re-summarization when only a single method changes. The method entity also gets its own semantic summary and embedding in Stage 2, placing it independently in the shared vector space alongside prose concepts.

### Trait Implementations (IMPLEMENTS)
Impl blocks that name a trait create an `IMPLEMENTS` edge: `code_impl` → `code_trait`.

### Dependencies (DEPENDS_ON)
`use` statements create `DEPENDS_ON` edges between modules. These enable dependency-aware queries ("what depends on the embedder module?").

## Stage 4: Bottom-Up File Summary Composition

Code file summaries are composed from individual entity summaries using a bottom-up pattern that mirrors the prose pipeline's statement → section → source structure. Instead of running the statement pipeline on raw code, the pipeline composes entity-level summaries into a file-level summary.

### Composition Flow

```
Entity summaries (function, struct, trait, ...)
  │
  └──→ SourceSummaryCompiler.compile_source_summary()
         │
         └──→ File-level summary (stored on Source.summary)
               │
               └──→ Re-embed file summary → Source.embedding
```

Each entity's semantic summary is treated as a "section summary entry" with a title of the form `{node_type}: {entity_name}`. The `SourceSummaryCompiler` trait (backed by `LlmSectionCompiler`) takes these section entries and compiles them into a coherent file-level summary — the same trait used by the prose pipeline for source summary compilation.

The composed summary is stored on the `Source.summary` field, and the source embedding is recomputed from this summary text. This means a code file's embedding captures the semantics of all its entities, enabling file-level search across both code and prose sources.

### Async Per-Entity Summarization

Entity summarization runs as **individual retry queue jobs** rather than a synchronous batch pass. This enables:
- **Independent retry** — If one entity's summary fails (LLM timeout, rate limit), only that entity retries, not the entire file.
- **Parallelism** — Multiple entity summaries from different sources can run concurrently across queue workers.
- **Fan-in coordination** — When all entity summaries for a source complete, the pipeline automatically advances to file summary composition.

The job flow:

```
ExtractChunk (per chunk)
  │
  └──→ fan-in: try_advance_to_summarize()
         │  (advisory lock prevents duplicate triggers)
         │
         ├──→ SummarizeEntity (per code entity without summary)
         │     │
         │     └──→ fan-in: try_advance_to_compose()
         │            │
         │            └──→ ComposeSourceSummary (per source)
         │
         └──→ [skips test entities and entities starting with test_]
```

- **`SummarizeEntity`** jobs carry `{node_id, source_id}` payloads. Each job locates the entity's source code via definition-pattern chunk matching, generates a summary via the chat backend, and stores it on the node. Processing metadata (model, provider, timing, prompt version) is recorded in `nodes.processing`.
- **`ComposeSourceSummary`** jobs carry `{source_id}` payloads. They collect all entity summaries for the source, pass them through `SourceSummaryCompiler`, and update the source summary and embedding.
- **Advisory locks** prevent race conditions: when multiple `SummarizeEntity` jobs for the same source complete concurrently, `pg_try_advisory_xact_lock` ensures only one worker enqueues the compose job.
- A **watchdog** in the queue service periodically checks for stalled sources (all entities summarized but no compose job pending) and enqueues missing compose jobs.

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

### Phase 1: Rust, Go, and Python
Rust, Go, and Python are fully supported. Rust has complex AST patterns (impl blocks, traits, macros, async) that stress-test the pipeline. Go covers the CLI codebase. Python covers external dependencies and tooling.

### Phase 2: TypeScript
Natural extension for broader applicability. Tree-sitter grammar is mature.

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
| Source summary | Statement → section → source compilation | Entity summary → file summary (bottom-up composition) |
| Summarization | Synchronous per-source | Async per-entity jobs with fan-in triggers |
| Re-ingestion | Content hash comparison | AST hash comparison (structural change detection) |

Both pipelines produce Nodes, Edges, and Source Summaries that participate equally in search and graph algorithms. The code pipeline skips the statement pipeline entirely — entity summaries serve the same role as statements for building the source-level summary.

## Cross-Domain Search

Once code entities share a vector space with prose concepts, cross-domain queries work naturally:

- **"How does entity resolution work?"** → returns spec statements, research paper statements, AND code function summaries about entity resolution
- **"What depends on the embedder?"** → graph traversal via DEPENDS_ON + CALLS edges
- **"What code implements the ingestion spec?"** → traversal: Spec Topic ←IMPLEMENTS_INTENT← Component ←PART_OF_COMPONENT← code_function

See [spec/13-cross-domain-analysis](13-cross-domain-analysis.md) for the full analysis capabilities this enables.

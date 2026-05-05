# ISSUE-0002: AST-extracted entities all attributed to a single chunk's byte range

**Status:** open
**Filed:** 2026-05-05
**Filed during:** change 0002-restore-code-extraction-data (verification of sanity reprocess)
**Kind:** observation (correctness / quality concern, not blocking)

## What

After reprocessing `engine/crates/covalence-core/src/services/hooks.rs` (one of the previously-broken code sources, change 0002 sanity step), all 14 code-class nodes that the AST extractor produced share the **same chunk's byte range**: chunk `959f7f25-284c-48a9-8cb1-8eea545cc49a`, byte_start=826, byte_end=1333.

The source has 72 chunks total spanning bytes 171–28069. Only that single chunk has code-class extractions; the other 71 chunks have zero. But the 14 named entities (`PreSearchPayload`, `PostResolvePayload`, `LifecycleHook`, `fire_post_search`, `fire_post_resolve`, `fire_post_synthesis`, `fire_post_extract`, `fire_pre_ingest`, `active_hooks`, `parse`, `PreSearchHookResponse`, etc.) clearly live throughout the 837-line file — `fire_post_search` is around line 100+, `LifecycleHook` is near the top, etc. The single chunk at bytes 826-1333 (≈ lines 25-50, ~507 bytes) physically cannot contain all 14 entities.

## Why it matters

INV-2 in Covalence's invariant catalog: "Every fact has pristine canonical provenance." Strict reading: a provenance link exists and points to valid offsets in the canonical source — satisfied. Looser reading: the provenance offsets should reflect *the entity's* actual location in the source, not *the extraction context's* location — **not satisfied**.

Practical impact:
- Code search "where is this function defined?" returns a misleading byte range (the chunk where AST extraction was anchored, not where the function lives).
- Cross-references between code entities can't use byte-offset proximity meaningfully.
- Future tooling that relies on entity-level provenance (e.g., "show me the source code of this struct") would have to map from chunk-level back to AST-level somehow.

## Context

Discovered while verifying INV-2 for the change-0002 sanity reprocess. Quote from the verification spot-check:

> All 14 code-class nodes share the same chunk's byte range (826-1333). The source has 72 chunks spanning bytes 171-28069, but only that one chunk holds code-extractions.

The most likely mechanism: the AST extractor is invoked once per source (or once per "code chunk" detected by the chunking pipeline) on the *full source text*. When entities are returned, they all get associated with the chunk that triggered the extraction call, rather than being mapped back to their AST byte spans.

This is hypothesized; needs investigation.

## Proposed action

Treat as candidate future change. Likely scope:

1. Read the AST extractor's response shape — does it include byte spans (`start_byte`, `end_byte`) per entity? The earlier discovery showed the response has fields like `name`, `entity_type`, `confidence`, `description`, `metadata.ast_hash` — but no spans were called out. Verify whether tree-sitter's per-node spans are available in the extractor output.
2. If spans are present: update the resolver (or entity-creation path in `services/pipeline/entity_resolution.rs`) to either:
   - (a) Create per-entity provenance records that override chunk-level offsets with AST-level spans, or
   - (b) Find or create the chunk that contains the entity's span and link there.
3. If spans are not present: extend the AST extractor's response to include them. The binary at `engine/crates/covalence-ast-extractor/src/main.rs` would gain span fields; the engine would carry them through.
4. Migration: existing entities (post-fix, including the 14 from the change-0002 sanity reprocess) would have coarse provenance until reprocessed under the fix. Backfill via a future managed change.

Estimated single-session work for steps 1-3 if spans are already in the extractor; multi-session if span extraction needs to be added to the binary.

## Related

- Change 0002 (this change) restored code-class node coverage but did not address per-entity provenance granularity.
- spec/12-code-ingestion.md may need updates if the resolution introduces new behavior.

## Resolution (filled when closed)

_Pending — file when the architectural change lands._

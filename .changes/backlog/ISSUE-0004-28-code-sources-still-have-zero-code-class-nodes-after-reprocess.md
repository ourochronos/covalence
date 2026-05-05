# ISSUE-0004: 28 code sources still have zero code-class nodes after reprocess

**Status:** open
**Filed:** 2026-05-05
**Filed during:** change 0002-restore-code-extraction-data (reconciliation phase)
**Kind:** observation (correctness regression sub-class)

## What

Of the 172 code sources reprocessed in change 0002 (after the AST-dispatch fix #186 had merged), **28 sources** still have zero `code`-class nodes despite having chunks (and in some cases, many chunks) and non-zero extractions.

Sample:

| Source | Chunks | Extractions | Code-class nodes |
|---|---:|---:|---:|
| `engine/crates/covalence-api/src/handlers/metrics.rs` | 2 | 1 | 0 |
| `engine/crates/covalence-core/src/consolidation/deep.rs` | 5 | 4 | 0 |
| `engine/crates/covalence-core/src/search/dimensions/vector.rs` | 44 | 12 | 0 |

The 44-chunk + 12-extraction case (`vector.rs`) is the most suspicious — the file is sizeable, has many top-level items, and chunks/extractions exist. But the resolver did not produce a single `code`-class node from those extractions.

## Why it matters

Change 0002's headline outcome is "235 of 250-ish previously-broken code sources now have code-class nodes." A 28-source residual is an obvious sub-class with a different failure mode that's worth diagnosing rather than ignoring. The most likely cause is the demotion rule at `models/node.rs:117-121`:

```rust
if base == EntityClass::Code {
    match source_domain {
        Some("code") | None => base,
        Some(_) => EntityClass::Domain,  // demoted!
    }
}
```

If `source_domain` is set to anything other than `"code"` or `None` (e.g., a domain-classifier added a different domain assignment to these specific sources at ingest time), every code-typed entity gets demoted to Domain. The sources with `source_type = 'code'` AND `domains = '{code}'` should not trigger this — but `reprocess.rs:101` passes `source.domains.first().cloned()` as `source_domain`. If `domains` happens to start with something other than `"code"` (e.g., `{external,code}` or `{config,code}`), the first element is non-code and demotion fires.

## Proposed action

Investigate the 28 sources to determine the root cause:

1. Run a diagnostic query to see each source's `source_type`, `domains` (full array), and `domain_groups` if any:

```sql
SELECT s.uri, s.source_type, s.domains, s.domains[1] AS first_domain
FROM sources s
LEFT JOIN chunks c ON c.source_id = s.id
LEFT JOIN extractions e ON e.chunk_id = c.id
LEFT JOIN nodes n ON n.id = e.entity_id AND n.entity_class = 'code'
WHERE s.source_type = 'code' AND s.superseded_by IS NULL
GROUP BY s.id, s.uri, s.source_type, s.domains
HAVING COUNT(DISTINCT n.id) FILTER (WHERE n.entity_class = 'code') = 0
   AND COUNT(DISTINCT e.id) > 0
ORDER BY s.uri;
```

2. If `first_domain != 'code'` for these 28: confirm the demotion rule is the cause. Either fix the rule (use `'code' = ANY(domains)` instead of `domains[1] = 'code'`) or fix the domain ordering (ensure `code` comes first in the array).

3. If `first_domain == 'code'`: dig deeper. Possibilities include the AST extractor returning entities with non-code `entity_type` values (e.g., an unknown new tree-sitter tag), or a transactional failure that committed extractions but not nodes.

Estimated single-session work.

## Related

- This is the same demotion rule that was almost the headline cause of #176. Discovery for change 0002 ruled it out for the 172 main sources (which had `domains = '{code}'`), but it may still be firing for these 28.
- Likely tied to the chosen approach in `reprocess.rs:101` of using `domains.first()` as the canonical source-domain. A `domains.contains('code')` test would be more robust.

## Resolution (filled when closed)

_Pending — file when the residual 28 are diagnosed and either fixed or accepted as a separate edge case._

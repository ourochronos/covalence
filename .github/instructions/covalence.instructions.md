# Covalence — Copilot Review Instructions

## Project Context
Covalence is a graph-native knowledge substrate for AI agent persistent memory. Rust engine, PostgreSQL (AGE + PGVector + pg_textsearch). It's used as the primary memory store for an autonomous AI agent system.

## Architecture Patterns

### Database
- All tables live in the `covalence` schema
- Edge columns are `source_node_id` and `target_node_id` (not `source_id`/`target_id`)
- Temporal edges: `valid_from`/`valid_to` on edges table. `NULL valid_to` = active. Always filter with `WHERE valid_to IS NULL` unless explicitly querying history.
- Migrations go in `engine/migrations/` as sequential `NNN_description.sql` files

### Error Handling
- Use `AppResult<T>` / `AppError` for service-level errors
- Use `GraphResult<T>` / `GraphError` for graph repository errors
- Workers should handle missing data gracefully (return skip result, not panic)

### Testing
- Integration tests in `engine/tests/integration/`
- Test DB: localhost:5434, database `covalence_test`
- Use `TestFixture::new()` for test setup — it truncates shared tables
- Tests that share state MUST use `#[serial]` to prevent race conditions
- Never test against production databases

### Worker/Queue System
- Tasks go through `slow_path_queue` table
- Task types: `compile`, `embed`, `infer_edges`, `reconsolidate`, `consolidate_article`, `decay_check`, `divergence_scan`, `recompute_graph_embeddings`
- Workers must handle missing metadata gracefully (e.g., missing `tree_index`)
- Queue tasks should be idempotent where possible

### API
- All endpoints under `/api/v1/`
- Namespace isolation via `X-Namespace` header (defaults to "default")
- API key auth via `X-API-Key` header (when `COVALENCE_API_KEY` is set)

## Review Priorities
1. **Data safety** — Does this change risk data loss or corruption? Are migrations additive?
2. **Test coverage** — Are edge cases covered? Are tests isolated?
3. **Coherence** — Does this change affect behavior described in docs or other files? Flag if so.
4. **Performance** — Does this add latency to the search path? Search must stay fast.

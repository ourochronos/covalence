# error_handling

`thiserror` typed errors in library crates (`covalence-core`), `anyhow` only in binary crates (`covalence-api`, `covalence-migrations`, etc.). Exit codes, error surfaces returned to CLI/HTTP, recovery paths.

No `unwrap()` or `expect()` in library code; use `?` or explicit error handling.

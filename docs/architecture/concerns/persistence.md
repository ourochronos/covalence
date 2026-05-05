# persistence

PostgreSQL schema, sqlx migrations, file formats, serialization. For engine modules: what migrations are introduced, do indexes change, are there backfill implications. For other modules: file-format changes (TOML, YAML, JSON manifests).

Migrations are runtime sqlx queries with `SQLX_OFFLINE=true` for tests. Always run `make migrate` (dev) before promoting to prod.

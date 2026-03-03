#!/usr/bin/env bash
# seed-sqlx-migrations.sh — DEPRECATED / NO-OP
#
# This script was written in tracking#106 to pre-register historical migrations
# (001-017) in the _sqlx_migrations table. It turned out to be incorrect:
# sqlx::migrate!("./migrations") embeds migrations from engine/migrations/ at
# COMPILE TIME. If a version appears in _sqlx_migrations but no corresponding
# file exists in engine/migrations/, SQLx panics on startup with:
#   "migration N was previously applied but is missing in the resolved migrations"
#
# The correct approach:
# - Historical schema (001-017 SQL files in sql/) was applied manually via
#   docker exec and is not tracked by SQLx.
# - _sqlx_migrations should remain EMPTY until actual migration files appear in
#   engine/migrations/ (future schema changes, 018+).
# - sqlx::migrate!() in main.rs will apply/track those future migrations only.
#
# DO NOT RUN THIS SCRIPT. It is kept for historical reference only.
echo "This script is deprecated and must not be run. See comments for details."
echo "Historical migrations (001-017) are not tracked by SQLx by design."
exit 1

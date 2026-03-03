#!/usr/bin/env bash
# seed-sqlx-migrations.sh — Register pre-SQLx migrations (001-017) in the
# _sqlx_migrations table so the engine's sqlx::migrate!() call does not try
# to re-apply them.
#
# Run this ONCE on any existing Covalence instance before deploying the version
# of the engine that includes the sqlx::migrate!() startup call (tracking#106).
# The script is fully idempotent — safe to re-run.
#
# Prerequisites:
#   • psql must be on $PATH
#   • openssl must be on $PATH  (for SHA-384 checksums)
#   • DATABASE_URL or individual PG* variables must be set
#
# Usage:
#   DATABASE_URL=postgres://covalence:covalence@localhost:5434/covalence \
#     ./scripts/seed-sqlx-migrations.sh
#
# Or rely on the default dev URL:
#   ./scripts/seed-sqlx-migrations.sh

set -euo pipefail

# ── Resolve connection ──────────────────────────────────────────────────────
DATABASE_URL="${DATABASE_URL:-postgres://covalence:covalence@localhost:5434/covalence}"
export DATABASE_URL

# Locate repo root regardless of where the script is invoked from
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
SQL_DIR="${REPO_ROOT}/sql"

echo "==> Covalence migration seeder"
echo "    Repo root : ${REPO_ROOT}"
echo "    SQL dir   : ${SQL_DIR}"
echo "    DB        : ${DATABASE_URL}"
echo ""

# ── Ensure _sqlx_migrations table exists ───────────────────────────────────
# SQLx creates this table on first migrate!() run.  We pre-create it here so
# the seeding script can run before the first engine boot.
psql "${DATABASE_URL}" <<'SQL'
CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version        BIGINT      NOT NULL PRIMARY KEY,
    description    TEXT        NOT NULL,
    installed_on   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    success        BOOLEAN     NOT NULL,
    checksum       BYTEA       NOT NULL,
    execution_time BIGINT      NOT NULL
);
SQL
echo "==> _sqlx_migrations table ready"

# ── Seed each historical migration ─────────────────────────────────────────
# We iterate over exactly the 17 files that were applied before SQLx tracking
# was introduced.  The loop is bounded to prevent accidentally seeding any
# 018+ files that belong to SQLx-managed territory.

SEEDED=0
SKIPPED=0

for sql_file in $(ls "${SQL_DIR}"/0[01][0-9]_*.sql "${SQL_DIR}"/01[0-7]_*.sql 2>/dev/null | sort -u); do
    filename="$(basename "${sql_file}")"

    # ── Extract version (integer, leading-zero-stripped) ───────────────────
    # Filename format: NNN_description_words.sql
    raw_version="${filename%%_*}"          # e.g. "017"
    version=$((10#${raw_version}))        # strip leading zeros → 17

    # Guard: only process 001-017
    if [[ ${version} -lt 1 || ${version} -gt 17 ]]; then
        echo "    [skip] ${filename} — outside 001-017 range"
        continue
    fi

    # ── Extract description (underscores → spaces, drop .sql) ──────────────
    stem="${filename%.sql}"               # e.g. "017_namespace_isolation"
    desc_raw="${stem#${raw_version}_}"    # e.g. "namespace_isolation"
    description="${desc_raw//_/ }"       # e.g. "namespace isolation"

    # ── Compute SHA-384 checksum (raw bytes → hex for psql decode) ─────────
    checksum_hex="$(openssl dgst -sha384 -hex "${sql_file}" | awk '{print $NF}')"

    # ── Insert (idempotent) ─────────────────────────────────────────────────
    psql "${DATABASE_URL}" --quiet --tuples-only <<SQL
INSERT INTO _sqlx_migrations (version, description, installed_on, success, checksum, execution_time)
VALUES (
    ${version},
    '${description}',
    NOW(),
    TRUE,
    decode('${checksum_hex}', 'hex'),
    0
)
ON CONFLICT (version) DO NOTHING;
SQL

    echo "    [ok]   v${version}  ${description}"
    SEEDED=$((SEEDED + 1))
done

echo ""
echo "==> Done. ${SEEDED} migration(s) processed (${SKIPPED} skipped)."
echo "    You can now start the Covalence engine — it will skip these 17"
echo "    migrations and only apply any new ones in engine/migrations/."

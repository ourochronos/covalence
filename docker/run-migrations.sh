#!/usr/bin/env bash
# run-migrations.sh — wait for Postgres, apply sql/*.sql in order, then exec
# the Covalence engine binary.
#
# This script is the ENTRYPOINT of the engine container.  It intentionally
# keeps all logic in plain POSIX-ish bash with no extra dependencies.

set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration (all overridable via env vars inherited from docker-compose)
# ---------------------------------------------------------------------------
DB_HOST="${DB_HOST:-postgres}"
DB_PORT="${DB_PORT:-5432}"
DB_NAME="${DB_NAME:-covalence}"
DB_USER="${DB_USER:-covalence}"
DB_PASSWORD="${DB_PASSWORD:-covalence}"
SQL_DIR="${SQL_DIR:-/app/sql}"
ENGINE_BIN="${ENGINE_BIN:-/app/covalence-engine}"
MAX_WAIT="${MAX_WAIT:-60}"   # seconds before giving up on Postgres

export PGPASSWORD="${DB_PASSWORD}"

# ---------------------------------------------------------------------------
# 1. Wait for Postgres to accept connections
# ---------------------------------------------------------------------------
echo "[migrations] Waiting for PostgreSQL at ${DB_HOST}:${DB_PORT} ..."

elapsed=0
until pg_isready -h "${DB_HOST}" -p "${DB_PORT}" -U "${DB_USER}" -d "${DB_NAME}" -q; do
    if [ "${elapsed}" -ge "${MAX_WAIT}" ]; then
        echo "[migrations] ERROR: Postgres did not become ready within ${MAX_WAIT}s. Aborting." >&2
        exit 1
    fi
    echo "[migrations]   still waiting... (${elapsed}s elapsed)"
    sleep 2
    elapsed=$((elapsed + 2))
done

echo "[migrations] PostgreSQL is ready."

# ---------------------------------------------------------------------------
# 2. Apply migrations in lexicographic order
# ---------------------------------------------------------------------------
if [ ! -d "${SQL_DIR}" ]; then
    echo "[migrations] WARNING: SQL directory '${SQL_DIR}' not found — skipping migrations."
else
    shopt -s nullglob
    sql_files=("${SQL_DIR}"/*.sql)
    shopt -u nullglob

    if [ ${#sql_files[@]} -eq 0 ]; then
        echo "[migrations] No .sql files found in '${SQL_DIR}' — skipping."
    else
        # Create a simple tracking table so we can skip already-applied files
        psql -h "${DB_HOST}" -p "${DB_PORT}" -U "${DB_USER}" -d "${DB_NAME}" \
            -c "CREATE TABLE IF NOT EXISTS _migration_log (
                    filename TEXT PRIMARY KEY,
                    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
                );" -q

        for sql_file in "${sql_files[@]}"; do
            filename="$(basename "${sql_file}")"

            # Check whether this migration has already been applied
            already_applied=$(psql -h "${DB_HOST}" -p "${DB_PORT}" -U "${DB_USER}" \
                -d "${DB_NAME}" -tAq \
                -c "SELECT COUNT(*) FROM _migration_log WHERE filename = '${filename}';")

            if [ "${already_applied}" -gt "0" ]; then
                echo "[migrations] SKIP  ${filename} (already applied)"
                continue
            fi

            echo "[migrations] APPLY ${filename} ..."
            psql -h "${DB_HOST}" -p "${DB_PORT}" -U "${DB_USER}" -d "${DB_NAME}" \
                -f "${sql_file}" -q

            # Record successful application
            psql -h "${DB_HOST}" -p "${DB_PORT}" -U "${DB_USER}" -d "${DB_NAME}" \
                -c "INSERT INTO _migration_log (filename) VALUES ('${filename}');" -q

            echo "[migrations] OK    ${filename}"
        done

        echo "[migrations] All migrations applied."
    fi
fi

# ---------------------------------------------------------------------------
# 3. Exec the engine binary (replaces this shell — clean signal handling)
# ---------------------------------------------------------------------------
echo "[migrations] Starting Covalence engine..."
exec "${ENGINE_BIN}"

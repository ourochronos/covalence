#!/usr/bin/env bash
# scripts/dev-up.sh — start the Covalence dev database and wait for it to be healthy.
#
# Usage:  scripts/dev-up.sh
#
# What it does:
#   1. Verifies Docker daemon is running.
#   2. Starts (or no-ops if already running) the `postgres` service defined in
#      docker-compose.yml.
#   3. Polls the container's healthcheck until it reports "healthy" (or times out).
#   4. Prints the connection string.
#
# Idempotent — safe to run when the container is already up.
#
# The Covalence engine itself runs natively via the macOS LaunchAgent
# `ai.ourochronos.covalence-engine` (see DEPLOY.md).  Use this script to
# ensure the backing database is available before running the engine.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COMPOSE_FILE="$REPO_ROOT/docker-compose.yml"
SERVICE="postgres"
MAX_WAIT=60   # seconds before giving up on healthcheck

# ---------------------------------------------------------------------------
# Load .env for credential defaults (if present)
# ---------------------------------------------------------------------------
if [ -f "$REPO_ROOT/.env" ]; then
    # shellcheck disable=SC1090
    set -o allexport
    source "$REPO_ROOT/.env"
    set +o allexport
fi

DB_USER="${POSTGRES_USER:-covalence}"
DB_NAME="${POSTGRES_DB:-covalence}"
DB_HOST="127.0.0.1"
DB_PORT="5434"

# ---------------------------------------------------------------------------
# 1. Check Docker daemon
# ---------------------------------------------------------------------------
if ! docker info > /dev/null 2>&1; then
    echo "✗ Docker daemon is not running."
    echo "  Start Docker Desktop (or colima / docker daemon) and retry."
    exit 1
fi

echo "✓ Docker daemon is running."

# ---------------------------------------------------------------------------
# 2. Start the postgres service (idempotent)
# ---------------------------------------------------------------------------
echo "Starting $SERVICE service..."
docker compose -f "$COMPOSE_FILE" up -d "$SERVICE"

# ---------------------------------------------------------------------------
# 3. Wait for healthy status
# ---------------------------------------------------------------------------
echo "Waiting for $SERVICE to be healthy (timeout: ${MAX_WAIT}s)..."

elapsed=0
while true; do
    status=$(docker inspect --format='{{.State.Health.Status}}' covalence-pg 2>/dev/null || echo "not_found")

    case "$status" in
        healthy)
            echo "✓ covalence-pg is healthy."
            break
            ;;
        not_found)
            echo "✗ Container covalence-pg not found — did the service start correctly?"
            exit 1
            ;;
        unhealthy)
            echo "✗ Container covalence-pg reported unhealthy after ${elapsed}s."
            echo "  Check logs: docker logs covalence-pg"
            exit 1
            ;;
        starting|*)
            if [ "$elapsed" -ge "$MAX_WAIT" ]; then
                echo "✗ Timed out after ${MAX_WAIT}s waiting for healthy status (current: $status)."
                echo "  Check logs: docker logs covalence-pg"
                exit 1
            fi
            printf "  [%ds] status=%s — still waiting...\n" "$elapsed" "$status"
            sleep 2
            elapsed=$((elapsed + 2))
            ;;
    esac
done

# ---------------------------------------------------------------------------
# 4. Print connection string
# ---------------------------------------------------------------------------
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  Covalence PostgreSQL is ready."
echo ""
echo "  Connection string:"
echo "  postgres://${DB_USER}:***@${DB_HOST}:${DB_PORT}/${DB_NAME}"
echo ""
echo "  DATABASE_URL=postgres://${DB_USER}:covalence@${DB_HOST}:${DB_PORT}/${DB_NAME}"
echo ""
echo "  psql: psql postgres://${DB_USER}:covalence@${DB_HOST}:${DB_PORT}/${DB_NAME}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "  Engine (native LaunchAgent): http://127.0.0.1:8430"
echo "  Health: curl http://127.0.0.1:8430/health"
echo ""

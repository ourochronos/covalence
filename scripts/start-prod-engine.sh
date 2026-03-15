#!/bin/bash
# Start the Covalence prod engine after ensuring PG is ready.
# Used by launchd (com.ourochronos.covalence-engine.plist).

set -euo pipefail

REPO="/Users/zonk1024/projects/covalence"
ENV_FILE="$REPO/.env.prod"
LOG="$REPO/logs/engine.log"
BINARY="$REPO/engine/target/release/covalence-api"

# Load env vars from .env.prod
set -a
source "$ENV_FILE"
set +a

# Override for prod
export DATABASE_URL="postgres://covalence:covalence@localhost:5437/covalence_prod"
export BIND_ADDR="0.0.0.0:8441"
export RUST_LOG="info,covalence_core=debug"
export COVALENCE_CHAT_CLI_COMMAND="${COVALENCE_CHAT_CLI_COMMAND:-gemini}"

# Ensure Docker containers are running
cd "$REPO"
/opt/homebrew/bin/docker compose --profile prod up -d prod-pg 2>/dev/null || true
/opt/homebrew/bin/docker compose up -d dev-pg 2>/dev/null || true

# Wait for PG to be ready (max 60s)
for i in $(seq 1 60); do
    if /opt/homebrew/bin/docker exec covalence-prod-pg pg_isready -U covalence -d covalence_prod >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

# Start the engine
exec "$BINARY" >> "$LOG" 2>&1

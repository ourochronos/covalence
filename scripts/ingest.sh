#!/usr/bin/env bash
# Ingest files into a Covalence engine instance.
# Usage: ./scripts/ingest.sh [API_URL]
# Default API_URL: http://localhost:8441

set -euo pipefail

API="${1:-http://localhost:8441}"
CONCURRENCY=4
SUCCESS=0
FAIL=0
SKIP=0

ingest_file() {
    local file="$1"
    local source_type="$2"
    local mime="$3"
    local uri="file://$file"

    local b64
    b64=$(base64 < "$file")

    local response
    response=$(curl -sf -w '%{http_code}' -o /dev/null -X POST "${API}/api/v1/sources" \
        -H 'Content-Type: application/json' \
        -d "{\"content\": \"${b64}\", \"source_type\": \"${source_type}\", \"mime\": \"${mime}\", \"uri\": \"${uri}\"}" \
        2>/dev/null) || true

    if [ "$response" = "201" ]; then
        echo "  OK  $file"
        return 0
    elif [ "$response" = "200" ] || [ "$response" = "409" ]; then
        echo "  SKIP $file (dedup)"
        return 2
    else
        echo "  FAIL $file (HTTP $response)"
        return 1
    fi
}

echo "=== Covalence Ingestion ==="
echo "Target: $API"
echo ""

# Verify engine is reachable
if ! curl -sf "${API}/health" > /dev/null 2>&1; then
    echo "ERROR: Engine not reachable at $API"
    echo "Start it with: make run-prod"
    exit 1
fi

# --- Rust source files ---
echo "--- Rust source files ---"
while IFS= read -r f; do
    rel="${f#/Users/zonk1024/projects/covalence/}"
    if ingest_file "$f" "code" "text/x-rust"; then
        ((SUCCESS++))
    elif [ $? -eq 2 ]; then
        ((SKIP++))
    else
        ((FAIL++))
    fi
done < <(find /Users/zonk1024/projects/covalence/engine/crates -name '*.rs' -not -path '*/target/*' | sort)

# --- Go source files ---
echo ""
echo "--- Go source files ---"
while IFS= read -r f; do
    if ingest_file "$f" "code" "text/plain"; then
        ((SUCCESS++))
    elif [ $? -eq 2 ]; then
        ((SKIP++))
    else
        ((FAIL++))
    fi
done < <(find /Users/zonk1024/projects/covalence/cli -name '*.go' | sort)

# --- Spec documents ---
echo ""
echo "--- Spec documents ---"
for f in /Users/zonk1024/projects/covalence/spec/*.md; do
    [ -f "$f" ] || continue
    if ingest_file "$f" "document" "text/markdown"; then
        ((SUCCESS++))
    elif [ $? -eq 2 ]; then
        ((SKIP++))
    else
        ((FAIL++))
    fi
done

# --- ADR documents ---
echo ""
echo "--- ADR documents ---"
for f in /Users/zonk1024/projects/covalence/docs/adr/*.md; do
    [ -f "$f" ] || continue
    if ingest_file "$f" "document" "text/markdown"; then
        ((SUCCESS++))
    elif [ $? -eq 2 ]; then
        ((SKIP++))
    else
        ((FAIL++))
    fi
done

echo ""
echo "=== Done ==="
echo "  Success: $SUCCESS"
echo "  Skipped: $SKIP"
echo "  Failed:  $FAIL"
echo "  Total:   $((SUCCESS + SKIP + FAIL))"

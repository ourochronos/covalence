#!/usr/bin/env bash
# =============================================================================
# Covalence Cranfield Search-Quality Harness  (Phase 2)
# =============================================================================
#
# Runs every query in golden_queries.json against the live Covalence search
# endpoint and checks that at least one result exceeds the query's
# minimum_expected_score threshold.
#
# Metrics computed per query (and aggregated):
#   Recall@5  — fraction of top-5  results with score >= min_score  (0.0–1.0)
#   Recall@10 — fraction of top-10 results with score >= min_score  (0.0–1.0)
#   MRR       — 1/(rank of first result with score >= min_score), 0 if none
#
# Usage:
#   ./run_harness.sh [OPTIONS]
#
# Options:
#   -u, --url URL       Covalence base URL  (default: http://localhost:8430)
#   -k, --api-key KEY   Bearer token        (default: $COVALENCE_API_KEY or empty)
#   -q, --queries FILE  Path to golden_queries.json
#                       (default: same directory as this script)
#   -v, --verbose       Print full JSON response for every query
#   -f, --fail-fast     Stop on the first failing query
#   -h, --help          Show this message
#
# Exit codes:
#   0  — all queries passed (>=1 result above min_score)
#   1  — one or more queries failed
#   2  — configuration / dependency error
# =============================================================================

set -euo pipefail

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE_URL="${COVALENCE_BASE_URL:-http://localhost:8430}"
API_KEY="${COVALENCE_API_KEY:-}"
QUERIES_FILE="${SCRIPT_DIR}/golden_queries.json"
VERBOSE=false
FAIL_FAST=false

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
  case "$1" in
    -u|--url)       BASE_URL="${2:-}";     shift 2 ;;
    -k|--api-key)   API_KEY="${2:-}";      shift 2 ;;
    -q|--queries)   QUERIES_FILE="${2:-}"; shift 2 ;;
    -v|--verbose)   VERBOSE=true;          shift   ;;
    -f|--fail-fast) FAIL_FAST=true;        shift   ;;
    -h|--help)
      sed -n '2,/^# ===\+/p' "$0" | grep '^#' | sed 's/^# \?//'
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 2
      ;;
  esac
done

# ---------------------------------------------------------------------------
# Dependency checks
# ---------------------------------------------------------------------------
for cmd in curl jq python3; do
  if ! command -v "$cmd" &>/dev/null; then
    echo "ERROR: '$cmd' is required but not found on PATH." >&2
    exit 2
  fi
done

if [[ ! -f "$QUERIES_FILE" ]]; then
  echo "ERROR: golden_queries.json not found at: $QUERIES_FILE" >&2
  exit 2
fi

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
SEARCH_URL="${BASE_URL%/}/search"

# Build curl auth flags
auth_flags=()
if [[ -n "$API_KEY" ]]; then
  auth_flags=(-H "Authorization: Bearer ${API_KEY}")
fi

# ANSI colours (suppressed when not a TTY)
if [[ -t 1 ]]; then
  RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
  CYAN='\033[0;36m'; BOLD='\033[1m'; RESET='\033[0m'
else
  RED=''; GREEN=''; YELLOW=''; CYAN=''; BOLD=''; RESET=''
fi

# Floating-point division via python3 (returns value rounded to 4 dp)
py_div() {
  python3 -c "print(round($1 / $2, 4))"
}

# ---------------------------------------------------------------------------
# Health-check
# ---------------------------------------------------------------------------
echo -e "${BOLD}Covalence Cranfield Harness — Phase 2${RESET}"
echo "  Engine : ${BASE_URL}"
echo "  Queries: ${QUERIES_FILE}"
echo ""

if ! curl -sf "${BASE_URL%/}/health" -o /dev/null; then
  echo -e "${RED}ERROR: Engine not reachable at ${BASE_URL}${RESET}" >&2
  echo "  Make sure the Covalence engine is running (docker compose up -d)" >&2
  exit 2
fi

# ---------------------------------------------------------------------------
# Read query count
# ---------------------------------------------------------------------------
QUERY_COUNT=$(jq '.queries | length' "$QUERIES_FILE")
echo -e "Running ${CYAN}${QUERY_COUNT}${RESET} golden queries against ${SEARCH_URL}..."
echo ""

# Print table header
printf "${BOLD}%-6s %-12s %-12s %-8s %-10s %-7s %-7s %-6s %-6s %-8s %s${RESET}\n" \
  "ID" "MODE" "STRATEGY" "THRESH" "TOP_SCORE" "R@5" "R@10" "MRR" "HITS" "RESULT" "DESCRIPTION"
echo "$(printf '%0.s-' {1..115})"

# ---------------------------------------------------------------------------
# Accumulators for aggregate metrics
# ---------------------------------------------------------------------------
PASS=0
FAIL=0
ERROR=0
FAILED_IDS=()

# Sums for aggregate computation (accumulated as strings; python does the math)
SUM_R5="0"
SUM_R10="0"
SUM_MRR="0"

# ---------------------------------------------------------------------------
# Main loop
# ---------------------------------------------------------------------------
while IFS= read -r ROW; do
  ID=$(echo "$ROW"        | jq -r '.id')
  QUERY=$(echo "$ROW"     | jq -r '.query')
  MODE=$(echo "$ROW"      | jq -r '.mode // "standard"')
  STRATEGY=$(echo "$ROW"  | jq -r '.strategy // "balanced"')
  INTENT=$(echo "$ROW"    | jq -r '.intent // empty')
  LIMIT=$(echo "$ROW"     | jq -r '.limit // 5')
  MIN_SCORE=$(echo "$ROW" | jq -r '.min_score')
  DESCRIPTION=$(echo "$ROW" | jq -r '.description')

  # Always fetch at least 10 results so we can compute R@10 and MRR
  FETCH_LIMIT=$(( LIMIT > 10 ? LIMIT : 10 ))

  # Build JSON payload
  PAYLOAD=$(jq -n \
    --arg  q  "$QUERY" \
    --arg  m  "$MODE" \
    --arg  s  "$STRATEGY" \
    --argjson l "$FETCH_LIMIT" \
    '{query: $q, mode: $m, strategy: $s, limit: $l}')

  if [[ -n "$INTENT" ]]; then
    PAYLOAD=$(echo "$PAYLOAD" | jq --arg i "$INTENT" '. + {intent: $i}')
  fi

  # Execute search
  HTTP_RESPONSE=$(curl -s -w "\n__HTTP_STATUS__%{http_code}" \
    -X POST "$SEARCH_URL" \
    -H "Content-Type: application/json" \
    "${auth_flags[@]+"${auth_flags[@]}"}" \
    -d "$PAYLOAD" 2>&1) || true

  HTTP_STATUS=$(echo "$HTTP_RESPONSE" | grep '__HTTP_STATUS__' | sed 's/__HTTP_STATUS__//')
  BODY=$(echo "$HTTP_RESPONSE" | grep -v '__HTTP_STATUS__')

  # Handle HTTP errors
  if [[ "$HTTP_STATUS" != "200" ]]; then
    ((ERROR++)) || true
    FAILED_IDS+=("$ID")
    printf "${RED}%-6s %-12s %-12s %-8s %-10s %-7s %-7s %-6s %-6s %-8s %s${RESET}\n" \
      "$ID" "$MODE" "$STRATEGY" "$MIN_SCORE" "N/A" "N/A" "N/A" "N/A" "0" "ERROR" "HTTP ${HTTP_STATUS}"
    if $FAIL_FAST; then
      echo ""
      echo -e "${RED}Stopping on first failure (--fail-fast).${RESET}"
      break
    fi
    continue
  fi

  # Validate JSON
  if ! echo "$BODY" | jq -e . &>/dev/null; then
    ((ERROR++)) || true
    FAILED_IDS+=("$ID")
    printf "${RED}%-6s %-12s %-12s %-8s %-10s %-7s %-7s %-6s %-6s %-8s %s${RESET}\n" \
      "$ID" "$MODE" "$STRATEGY" "$MIN_SCORE" "N/A" "N/A" "N/A" "N/A" "0" "ERROR" "Invalid JSON response"
    if $FAIL_FAST; then break; fi
    continue
  fi

  if $VERBOSE; then
    echo ""
    echo -e "${CYAN}--- ${ID}: ${QUERY} ---${RESET}"
    echo "$BODY" | jq .
  fi

  # Normalise response to plain array
  RESULTS_JSON=$(echo "$BODY" | jq 'if type == "array" then . elif .data != null then .data else [] end')

  # Scored arrays (bounded to top 5 and top 10)
  TOP5_JSON=$(echo "$RESULTS_JSON"  | jq '.[0:5]')
  TOP10_JSON=$(echo "$RESULTS_JSON" | jq '.[0:10]')

  TOP_SCORE=$(echo "$RESULTS_JSON" | jq '[.[].score] | if length > 0 then max else 0 end')

  # Hits at various cut-offs
  HITS5=$(echo "$TOP5_JSON"  | jq --argjson t "$MIN_SCORE" '[.[] | select(.score >= $t)] | length')
  HITS10=$(echo "$TOP10_JSON" | jq --argjson t "$MIN_SCORE" '[.[] | select(.score >= $t)] | length')

  # Total hits across all returned results (for pass/fail: >=1 anywhere)
  HITS_ANY=$(echo "$RESULTS_JSON" | jq --argjson t "$MIN_SCORE" '[.[] | select(.score >= $t)] | length')

  # Recall@5 = hits_in_top5 / 5
  R5=$(python3 -c "print(round(${HITS5} / 5, 4))")
  # Recall@10 = hits_in_top10 / 10
  R10=$(python3 -c "print(round(${HITS10} / 10, 4))")

  # MRR — find rank (1-based) of first result in top-10 that meets min_score
  # Returns 0 if no result meets threshold in the top 10
  MRR_RAW=$(echo "$TOP10_JSON" | jq --argjson t "$MIN_SCORE" '
    to_entries
    | map(select(.value.score >= $t) | .key + 1)
    | if length > 0 then (1.0 / (.[0] | tonumber)) else 0 end
  ')
  MRR=$(python3 -c "print(round(${MRR_RAW}, 4))")

  # Accumulate sums
  SUM_R5=$(python3  -c "print(round(${SUM_R5}  + ${R5},  6))")
  SUM_R10=$(python3 -c "print(round(${SUM_R10} + ${R10}, 6))")
  SUM_MRR=$(python3 -c "print(round(${SUM_MRR} + ${MRR}, 6))")

  # Determine pass/fail (>=1 hit anywhere in returned results)
  if [[ "$HITS_ANY" -ge 1 ]]; then
    ((PASS++)) || true
    STATUS_LABEL="${GREEN}PASS${RESET}"
  else
    ((FAIL++)) || true
    FAILED_IDS+=("$ID")
    STATUS_LABEL="${RED}FAIL${RESET}"
  fi

  # Short description for table (truncate)
  SHORT_DESC=$(echo "$DESCRIPTION" | cut -c1-38)

  printf "%-6s %-12s %-12s %-8s %-10s %-7s %-7s %-6s %-6s " \
    "$ID" "$MODE" "$STRATEGY" "$MIN_SCORE" "$TOP_SCORE" "$R5" "$R10" "$MRR" "$HITS_ANY"
  printf "${STATUS_LABEL}"
  printf "  %s\n" "$SHORT_DESC"

  if $FAIL_FAST && [[ "$HITS_ANY" -lt 1 ]]; then
    echo ""
    echo -e "${RED}Stopping on first failure (--fail-fast).${RESET}"
    break
  fi

done < <(jq -c '.queries[]' "$QUERIES_FILE")

# ---------------------------------------------------------------------------
# Aggregate metrics
# ---------------------------------------------------------------------------
TOTAL=$((PASS + FAIL + ERROR))

# Avoid division by zero if all queries errored before contributing
if [[ "$TOTAL" -gt 0 ]]; then
  AGG_R5=$(python3  -c "print(round(${SUM_R5}  / ${TOTAL}, 4))")
  AGG_R10=$(python3 -c "print(round(${SUM_R10} / ${TOTAL}, 4))")
  AGG_MRR=$(python3 -c "print(round(${SUM_MRR} / ${TOTAL}, 4))")
else
  AGG_R5="N/A"; AGG_R10="N/A"; AGG_MRR="N/A"
fi

echo ""
echo "$(printf '%0.s-' {1..115})"
echo -e "${BOLD}Results: ${GREEN}${PASS} passed${RESET}  ${RED}${FAIL} failed${RESET}  ${YELLOW}${ERROR} errored${RESET}  (${TOTAL} total)"
echo ""
echo -e "${BOLD}Aggregate Metrics (over ${TOTAL} queries):${RESET}"
printf "  %-18s %s\n" "Mean Recall@5:"  "${AGG_R5}"
printf "  %-18s %s\n" "Mean Recall@10:" "${AGG_R10}"
printf "  %-18s %s\n" "Mean MRR:"       "${AGG_MRR}"

if [[ ${#FAILED_IDS[@]} -gt 0 ]]; then
  echo ""
  echo -e "${RED}Failed query IDs: ${FAILED_IDS[*]}${RESET}"
fi

echo ""
if [[ "$FAIL" -gt 0 || "$ERROR" -gt 0 ]]; then
  echo -e "${RED}HARNESS RESULT: FAIL${RESET}"
  echo ""
  echo "Possible causes:"
  echo "  • The knowledge base is empty (no content ingested yet)"
  echo "  • The query threshold is too aggressive for the current corpus"
  echo "  • A search dimension (vector/lexical/graph) is not functioning"
  echo "  • Embeddings have not been generated (run: POST /admin/embed-all)"
  echo ""
  exit 1
else
  echo -e "${GREEN}HARNESS RESULT: PASS${RESET}"
  echo ""
  exit 0
fi

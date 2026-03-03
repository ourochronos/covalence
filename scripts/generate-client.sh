#!/usr/bin/env bash
# scripts/generate-client.sh — Generate the Covalence Python client from the OpenAPI spec.
#
# Usage:
#   ./scripts/generate-client.sh [--fetch] [--docker] [--dry-run]
#
# Flags:
#   --fetch     Re-fetch the spec from the live engine before generating
#               (requires the engine to be running on ENGINE_URL).
#   --docker    Force using Docker even if a local generator is available.
#   --dry-run   Print what would be run without actually running it.
#
# Environment:
#   ENGINE_URL  Base URL of the running engine (default: http://localhost:8430)
#   SPEC_FILE   Path to the OpenAPI spec JSON (default: openapi.json at repo root)
#   OUT_DIR     Output directory for the generated client (default: clients/python)
#
# Generator resolution order (first found wins):
#   1. `openapi-generator` / `openapi-generator-cli` on PATH
#   2. Docker with openapitools/openapi-generator-cli (GENERATOR_IMAGE)
#   3. `npx @openapitools/openapi-generator-cli`  ← requires Java on PATH
#
# The generated client is committed to clients/python/ and must be regenerated
# whenever the OpenAPI spec changes (see scripts/check-client-fresh.sh).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENGINE_URL="${ENGINE_URL:-http://localhost:8430}"
SPEC_FILE="${SPEC_FILE:-${REPO_ROOT}/openapi.json}"
OUT_DIR="${OUT_DIR:-${REPO_ROOT}/clients/python}"
# Pin to the same image version used during initial generation
GENERATOR_IMAGE="${GENERATOR_IMAGE:-openapitools/openapi-generator-cli:v7.10.0}"

FETCH=false
FORCE_DOCKER=false
DRY_RUN=false

for arg in "$@"; do
  case "$arg" in
    --fetch)      FETCH=true ;;
    --docker)     FORCE_DOCKER=true ;;
    --dry-run)    DRY_RUN=true ;;
    -h|--help)
      sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "Unknown flag: $arg" >&2
      exit 1
      ;;
  esac
done

run() {
  if $DRY_RUN; then
    echo "[dry-run] $*"
  else
    "$@"
  fi
}

# ─── 1. Optionally refresh the spec ───────────────────────────────────────────

if $FETCH; then
  echo "→ Fetching spec from ${ENGINE_URL}/openapi.json …"
  TEMP_SPEC="$(mktemp)"
  if ! curl -fsSL "${ENGINE_URL}/openapi.json" \
       | python3 -m json.tool --indent 2 \
       > "${TEMP_SPEC}"; then
    echo "✗ Failed to fetch spec from ${ENGINE_URL}" >&2
    rm -f "${TEMP_SPEC}"
    exit 1
  fi
  run cp "${TEMP_SPEC}" "${SPEC_FILE}"
  rm -f "${TEMP_SPEC}"
  echo "  Saved to ${SPEC_FILE} ($(wc -c < "${SPEC_FILE}" | tr -d ' ') bytes)"
fi

if [[ ! -f "${SPEC_FILE}" ]]; then
  echo "✗ Spec file not found: ${SPEC_FILE}" >&2
  echo "  Run with --fetch or start the engine and try again." >&2
  exit 1
fi

# ─── 2. Validate it's parseable JSON ─────────────────────────────────────────

echo "→ Validating spec JSON …"
if ! python3 -c "import json, sys; json.load(open(sys.argv[1]))" "${SPEC_FILE}"; then
  echo "✗ Spec is not valid JSON: ${SPEC_FILE}" >&2
  exit 1
fi
echo "  ✓ Valid JSON"

# ─── 3. Locate the generator ──────────────────────────────────────────────────

USE_DOCKER=false
GENERATOR_CMD=""

if ! $FORCE_DOCKER; then
  for candidate in openapi-generator openapi-generator-cli; do
    if command -v "${candidate}" &>/dev/null; then
      GENERATOR_CMD="${candidate}"
      break
    fi
  done
fi

# Docker is the most reliable fallback (no Java required on host)
if [[ -z "${GENERATOR_CMD}" ]] || $FORCE_DOCKER; then
  if command -v docker &>/dev/null; then
    USE_DOCKER=true
  fi
fi

# Last resort: npx (requires Java on PATH)
if [[ -z "${GENERATOR_CMD}" ]] && ! $USE_DOCKER; then
  if command -v npx &>/dev/null; then
    GENERATOR_CMD="npx --yes @openapitools/openapi-generator-cli"
  else
    echo "✗ No generator found. Install one of:" >&2
    echo "  brew install openapi-generator" >&2
    echo "  brew install --cask docker  (then start Docker)" >&2
    echo "  npm install -g @openapitools/openapi-generator-cli  (requires Java)" >&2
    exit 1
  fi
fi

# ─── 4. Generator config ──────────────────────────────────────────────────────

PACKAGE_NAME="covalence"
PACKAGE_VERSION="0.1.0"

ADDITIONAL_PROPS="packageName=${PACKAGE_NAME},packageVersion=${PACKAGE_VERSION},projectName=covalence-client,library=urllib3"

# ─── 5. Run generation ────────────────────────────────────────────────────────

echo "→ Generating Python client …"
echo "  spec    : ${SPEC_FILE}"
echo "  output  : ${OUT_DIR}"
echo "  package : ${PACKAGE_NAME} ${PACKAGE_VERSION}"
echo ""

run mkdir -p "${OUT_DIR}"

if $USE_DOCKER; then
  echo "  tool    : docker (${GENERATOR_IMAGE})"
  echo ""
  # Paths inside the container are /local/…
  CONTAINER_SPEC="/local/$(realpath --relative-to="${REPO_ROOT}" "${SPEC_FILE}")"
  CONTAINER_OUT="/local/$(realpath --relative-to="${REPO_ROOT}" "${OUT_DIR}")"
  run docker run --rm \
    -v "${REPO_ROOT}:/local" \
    "${GENERATOR_IMAGE}" generate \
    -i "${CONTAINER_SPEC}" \
    -g python \
    -o "${CONTAINER_OUT}" \
    --additional-properties="${ADDITIONAL_PROPS}" \
    --skip-validate-spec
else
  echo "  tool    : ${GENERATOR_CMD%% *}"
  echo ""
  # shellcheck disable=SC2086
  run ${GENERATOR_CMD} generate \
    -i "${SPEC_FILE}" \
    -g python \
    -o "${OUT_DIR}" \
    --additional-properties="${ADDITIONAL_PROPS}" \
    --skip-validate-spec
fi

echo ""
echo "✓ Client generated in ${OUT_DIR}"
echo ""
echo "Install with:"
echo "  pip install -e ${OUT_DIR}"
echo ""
echo "Quick smoke test:"
echo "  python3 -c 'import ${PACKAGE_NAME}; print(\"covalence client OK\")'"

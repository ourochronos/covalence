# Covalence — top-level Makefile
# Run `make help` for an overview of targets.

.PHONY: help build test fmt lint \
        spec spec-fetch \
        client client-fetch client-check \
        docker-up docker-down

SHELL := /usr/bin/env bash
REPO_ROOT := $(shell pwd)
ENGINE_URL ?= http://localhost:8430

# ─── Help ─────────────────────────────────────────────────────────────────────

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*##' $(MAKEFILE_LIST) | \
	  awk 'BEGIN{FS=":.*## "} {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}' | sort

# ─── Engine ───────────────────────────────────────────────────────────────────

build: ## Build the Rust engine (debug)
	cargo build --manifest-path engine/Cargo.toml

build-release: ## Build the Rust engine (release)
	cargo build --release --manifest-path engine/Cargo.toml

test: ## Run Rust tests
	cargo test --manifest-path engine/Cargo.toml

fmt: ## Format Rust code
	cargo fmt --manifest-path engine/Cargo.toml

lint: ## Lint Rust code with Clippy
	cargo clippy --manifest-path engine/Cargo.toml -- -D warnings

# ─── OpenAPI spec ─────────────────────────────────────────────────────────────

spec: ## Validate the committed openapi.json (no engine required)
	@echo "→ Validating openapi.json …"
	@python3 -c "import json, sys; json.load(open('openapi.json')); print('✓ openapi.json is valid JSON')"

spec-fetch: ## Fetch a fresh openapi.json from the running engine
	@echo "→ Fetching spec from $(ENGINE_URL)/openapi.json …"
	curl -fsSL $(ENGINE_URL)/openapi.json | python3 -m json.tool --indent 2 > openapi.json
	@echo "✓ openapi.json updated ($(shell wc -c < openapi.json | tr -d ' ') bytes)"

# ─── Python client generation ─────────────────────────────────────────────────

client: ## Generate the Python client from the committed openapi.json
	./scripts/generate-client.sh

client-fetch: ## Fetch spec from live engine, then regenerate the Python client
	./scripts/generate-client.sh --fetch

client-check: ## CI gate — fail if committed client is stale vs openapi.json
	./scripts/check-client-fresh.sh

# ─── Docker ───────────────────────────────────────────────────────────────────

docker-up: ## Start Postgres + engine via docker compose
	docker compose up -d

docker-down: ## Stop all docker compose services
	docker compose down

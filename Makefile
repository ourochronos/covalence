.PHONY: build test fmt lint clippy check run watch \
       dev-db test-db prod-db migrate migrate-prod reset-db reset-prod-db \
       run-dev run-prod deploy promote ingest-changes \
       spec spec-fetch \
       cli-build cli-install \
       docker-up docker-down \
       ingest-codebase ingest-specs ingest-adrs ingest-prod \
       reprocess-statements \
       eval-search

# === Engine ===

build:
	cd engine && cargo build --workspace

test:
	cd engine && cargo test --workspace

test-unit:
	cd engine && SQLX_OFFLINE=true cargo test --workspace

test-integration:
	cd engine && cargo test --workspace -- --ignored

fmt:
	cd engine && cargo fmt --all

fmt-check:
	cd engine && cargo fmt --all -- --check

lint: clippy

clippy:
	cd engine && cargo clippy --workspace -- -D warnings

check: fmt-check clippy test-unit

# === Search Regression ===

EVAL_API ?= http://covalence-wsl:8441

eval-search:
	cd engine && cargo run -p covalence-eval -- \
		--layer search-regression \
		--input crates/covalence-eval/fixtures/search_precision_baseline.json \
		--api-url $(EVAL_API)

# === Database: Dev (port 5435) ===

dev-db:
	@docker compose up -d dev-pg
	@echo "Waiting for dev PG to be ready..."
	@until docker exec covalence-dev-pg pg_isready -U covalence -d covalence_dev 2>/dev/null; do sleep 1; done
	@docker exec covalence-dev-pg psql -U covalence -d covalence_dev \
		-c "CREATE EXTENSION IF NOT EXISTS vector; CREATE EXTENSION IF NOT EXISTS pg_trgm; CREATE EXTENSION IF NOT EXISTS ltree;" \
		2>/dev/null || true
	@echo "Dev database ready on port 5435"

dev-db-stop:
	@docker compose stop dev-pg

# === Database: Test (port 5436) ===

test-db:
	@docker compose --profile test up -d test-pg
	@echo "Waiting for test PG to be ready..."
	@until docker exec covalence-test-pg pg_isready -U covalence -d covalence_test 2>/dev/null; do sleep 1; done
	@docker exec covalence-test-pg psql -U covalence -d covalence_test \
		-c "CREATE EXTENSION IF NOT EXISTS vector; CREATE EXTENSION IF NOT EXISTS pg_trgm; CREATE EXTENSION IF NOT EXISTS ltree;" \
		2>/dev/null || true
	@echo "Test database ready on port 5436"

test-db-stop:
	@docker compose --profile test stop test-pg
	@docker compose --profile test rm -f test-pg

# === Database: Prod (covalence-wsl) ===

prod-db:
	@echo "Prod PG runs on derptop (covalence-wsl:5432). Checking connectivity..."
	@ssh $(PROD_HOST) 'pg_isready -U covalence -d covalence_prod' || \
		(echo "ERROR: Prod PG not reachable on covalence-wsl" && exit 1)
	@echo "Prod database ready on covalence-wsl:5432"

prod-db-stop:
	@echo "Prod PG runs on derptop. Use: ssh $(PROD_HOST) 'sudo systemctl stop postgresql'"

# === Migrations ===

migrate:
	cd engine && DATABASE_URL=postgres://covalence:covalence@localhost:5435/covalence_dev cargo run -p covalence-migrations

migrate-prod:
	cd engine && DATABASE_URL=postgres://covalence:covalence@covalence-wsl:5432/covalence_prod cargo run -p covalence-migrations

reset-db:
	@echo "Dropping and recreating covalence_dev..."
	@docker exec covalence-dev-pg psql -U covalence -d postgres \
		-c "DROP DATABASE IF EXISTS covalence_dev;" \
		-c "CREATE DATABASE covalence_dev OWNER covalence;"
	@docker exec covalence-dev-pg psql -U covalence -d covalence_dev \
		-c "CREATE EXTENSION IF NOT EXISTS vector;" \
		-c "CREATE EXTENSION IF NOT EXISTS pg_trgm;" \
		-c "CREATE EXTENSION IF NOT EXISTS ltree;"
	@echo "Running migrations..."
	@cd engine && cargo run -p covalence-migrations
	@echo "Dev database reset complete."

reset-prod-db:
	@echo "WARNING: This will destroy all prod data on covalence-wsl!"
	@echo "Press Ctrl+C to abort, or wait 5 seconds..."
	@sleep 5
	@echo "Stopping engine..."
	@ssh $(PROD_HOST) 'sudo systemctl stop covalence-engine' || true
	@echo "Dropping and recreating covalence_prod..."
	@ssh $(PROD_HOST) 'psql postgres://covalence:covalence@localhost:5432/postgres \
		-c "DROP DATABASE IF EXISTS covalence_prod;" \
		-c "CREATE DATABASE covalence_prod OWNER covalence;"'
	@ssh $(PROD_HOST) 'psql postgres://covalence:covalence@localhost:5432/covalence_prod \
		-c "CREATE EXTENSION IF NOT EXISTS vector;" \
		-c "CREATE EXTENSION IF NOT EXISTS pg_trgm;" \
		-c "CREATE EXTENSION IF NOT EXISTS ltree;" \
		-c "CREATE EXTENSION IF NOT EXISTS age;"'
	@echo "Running migrations on prod..."
	@$(MAKE) migrate-prod
	@echo "Restarting engine..."
	@ssh $(PROD_HOST) 'sudo systemctl start covalence-engine'
	@echo "Prod database reset complete."

# (promote target is below, after deploy)

# === Run ===

run: run-dev

run-dev:
	cd engine && DATABASE_URL=postgres://covalence:covalence@localhost:5435/covalence_dev BIND_ADDR=0.0.0.0:8431 cargo run -p covalence-api

run-prod:
	@echo "Prod runs on derptop (covalence-wsl:8441). Use 'make deploy' to push changes."

# === Deploy to prod (derptop) ===

PROD_HOST ?= covalence@covalence-wsl
PROD_DIR  ?= /home/covalence/covalence

deploy:
	@echo "=== Deploying to $(PROD_HOST) ==="
	ssh $(PROD_HOST) 'cd $(PROD_DIR) && git pull'
	@echo "=== Building release ==="
	ssh $(PROD_HOST) 'source $$HOME/.cargo/env && cd $(PROD_DIR)/engine && cargo build --release 2>&1 | tail -3'
	@echo "=== Running migrations ==="
	ssh $(PROD_HOST) 'source $$HOME/.cargo/env && cd $(PROD_DIR)/engine && touch crates/covalence-migrations/src/main.rs && DATABASE_URL=postgres://covalence:covalence@localhost:5432/covalence_prod cargo run -p covalence-migrations 2>&1 | tail -3'
	@echo "=== Restarting services ==="
	ssh $(PROD_HOST) 'sudo systemctl restart covalence-engine && sleep 2 && curl -sf http://localhost:8441/api/v1/admin/health'
	ssh $(PROD_HOST) 'sudo systemctl restart covalence-worker 2>/dev/null || echo "  (worker not installed yet — install with: sudo cp deploy/covalence-worker.service /etc/systemd/system/ && sudo systemctl enable covalence-worker)"'
	@echo "=== Ingesting changes ==="
	@$(MAKE) ingest-changes || echo "  (ingestion skipped or failed — non-fatal)"
	@echo "=== Deploy complete ==="

# Promote: check locally, migrate prod, deploy
promote: check migrate-prod deploy
	@echo "=== Promotion complete ==="

watch:
	cd engine && cargo watch -x 'run -p covalence-api'

# === Ingestion ===
# These targets ingest content into an engine instance.
# Default: prod on :8441. Override with INGEST_API=http://localhost:8431 for dev.

INGEST_API ?= http://covalence-wsl:8441

ingest-codebase:
	@echo "Ingesting Rust source files..."
	@find engine/crates -name '*.rs' -not -path '*/target/*' | while read f; do \
		echo "  $$f"; \
		b64=$$(base64 < "$$f"); \
		curl -sf -X POST $(INGEST_API)/api/v1/sources \
			-H 'Content-Type: application/json' \
			-d "{\"content\": \"$$b64\", \"source_type\": \"code\", \"mime\": \"text/x-rust\", \"uri\": \"file://$$f\"}" \
			> /dev/null || echo "    FAILED: $$f"; \
	done
	@echo "Ingesting Go CLI files..."
	@find cli -name '*.go' | while read f; do \
		echo "  $$f"; \
		b64=$$(base64 < "$$f"); \
		curl -sf -X POST $(INGEST_API)/api/v1/sources \
			-H 'Content-Type: application/json' \
			-d "{\"content\": \"$$b64\", \"source_type\": \"code\", \"mime\": \"text/x-go\", \"uri\": \"file://$$f\"}" \
			> /dev/null || echo "    FAILED: $$f"; \
	done
	@echo "Ingesting dashboard files..."
	@for f in dashboard/index.html dashboard/style.css dashboard/dashboard.js; do \
		[ -f "$$f" ] || continue; \
		echo "  $$f"; \
		b64=$$(base64 < "$$f"); \
		mime=$$(case "$$f" in *.html) echo "text/html";; *.css) echo "text/css";; *.js) echo "application/javascript";; esac); \
		curl -sf -X POST $(INGEST_API)/api/v1/sources \
			-H 'Content-Type: application/json' \
			-d "{\"content\": \"$$b64\", \"source_type\": \"code\", \"mime\": \"$$mime\", \"uri\": \"file://$$f\"}" \
			> /dev/null || echo "    FAILED: $$f"; \
	done
	@echo "Codebase ingestion complete."

ingest-specs:
	@echo "Ingesting spec documents..."
	@for f in spec/*.md; do \
		[ -f "$$f" ] || continue; \
		echo "  $$f"; \
		b64=$$(base64 < "$$f"); \
		curl -sf -X POST $(INGEST_API)/api/v1/sources \
			-H 'Content-Type: application/json' \
			-d "{\"content\": \"$$b64\", \"source_type\": \"document\", \"mime\": \"text/markdown\", \"uri\": \"file://$$f\"}" \
			> /dev/null || echo "    FAILED: $$f"; \
	done
	@echo "Spec ingestion complete."

ingest-adrs:
	@echo "Ingesting ADR documents..."
	@for f in docs/adr/*.md; do \
		[ -f "$$f" ] || continue; \
		echo "  $$f"; \
		b64=$$(base64 < "$$f"); \
		curl -sf -X POST $(INGEST_API)/api/v1/sources \
			-H 'Content-Type: application/json' \
			-d "{\"content\": \"$$b64\", \"source_type\": \"document\", \"mime\": \"text/markdown\", \"uri\": \"file://$$f\"}" \
			> /dev/null || echo "    FAILED: $$f"; \
	done
	@echo "ADR ingestion complete."

ingest-prod: ingest-codebase ingest-specs ingest-adrs
	@echo "=== All ingestion complete ==="

# Ingest only files changed since last ingestion marker.
# Uses .last-ingest-commit to track what was previously ingested.
INGEST_MARKER ?= .last-ingest-commit

ingest-changes:
	@if [ ! -f $(INGEST_MARKER) ]; then \
		echo "No ingestion marker found. Run 'make ingest-prod' for initial ingestion."; \
		git rev-parse HEAD > $(INGEST_MARKER); \
		exit 0; \
	fi; \
	LAST=$$(cat $(INGEST_MARKER)); \
	HEAD=$$(git rev-parse HEAD); \
	if [ "$$LAST" = "$$HEAD" ]; then \
		echo "No changes since last ingestion ($$LAST)."; \
		exit 0; \
	fi; \
	echo "Ingesting changes: $$LAST..$$HEAD"; \
	git diff --name-only "$$LAST..$$HEAD" -- '*.rs' '*.go' | grep -v '/target/' > /tmp/cov-ingest-code.txt || true; \
	git diff --name-only "$$LAST..$$HEAD" -- 'spec/*.md' 'docs/adr/*.md' 'design/*.md' 'CLAUDE.md' 'VISION.md' 'MILESTONES.md' 'README.md' > /tmp/cov-ingest-docs.txt || true; \
	while IFS= read -r f; do \
		[ -f "$$f" ] || continue; \
		echo "  [code] $$f"; \
		b64=$$(base64 < "$$f"); \
		curl -sf -X POST $(INGEST_API)/api/v1/sources \
			-H 'Content-Type: application/json' \
			-d "{\"content\": \"$$b64\", \"source_type\": \"code\", \"uri\": \"file://$$f\"}" \
			> /dev/null 2>&1 || echo "    FAILED: $$f"; \
	done < /tmp/cov-ingest-code.txt; \
	while IFS= read -r f; do \
		[ -f "$$f" ] || continue; \
		echo "  [doc] $$f"; \
		b64=$$(base64 < "$$f"); \
		curl -sf --max-time 600 -X POST $(INGEST_API)/api/v1/sources \
			-H 'Content-Type: application/json' \
			-d "{\"content\": \"$$b64\", \"source_type\": \"document\", \"uri\": \"file://$$f\"}" \
			> /dev/null 2>&1 || echo "    FAILED: $$f"; \
	done < /tmp/cov-ingest-docs.txt; \
	rm -f /tmp/cov-ingest-code.txt /tmp/cov-ingest-docs.txt; \
	echo "Ingestion complete. Running edge synthesis..."; \
	curl -sf -X POST $(INGEST_API)/api/v1/admin/edges/synthesize \
		-H 'Content-Type: application/json' -d '{"min_cooccurrences": 1}' > /dev/null 2>&1 || true; \
	curl -sf -X POST $(INGEST_API)/api/v1/admin/graph/reload > /dev/null 2>&1 || true; \
	curl -sf -X POST $(INGEST_API)/api/v1/admin/cache/clear > /dev/null 2>&1 || true; \
	echo "$$HEAD" > $(INGEST_MARKER); \
	echo "Marker updated to $$HEAD"

REPROCESS_BATCH ?= 5

reprocess-statements:
	@echo "Finding document sources without statements..."
	@ssh $(PROD_HOST) 'psql postgres://covalence:covalence@localhost:5432/covalence_prod -t -c \
		"SELECT s.id FROM sources s WHERE s.source_type = '\''document'\'' \
		 AND NOT EXISTS (SELECT 1 FROM statements st WHERE st.source_id = s.id) \
		 ORDER BY s.created_date"' \
		| tr -d ' ' | grep -v '^$$' > /tmp/cov-reprocess-ids.txt; \
	total=$$(wc -l < /tmp/cov-reprocess-ids.txt | tr -d ' '); \
	echo "Found $$total unprocessed document sources"; \
	while IFS= read -r id; do \
		echo "  enqueuing $$id..."; \
		curl -sf -X POST $(INGEST_API)/api/v1/sources/$$id/queue-reprocess > /dev/null 2>&1 || echo "    FAILED"; \
	done < /tmp/cov-reprocess-ids.txt; \
	rm -f /tmp/cov-reprocess-ids.txt
	@echo "Final edge synthesis..."
	@curl -sf -X POST $(INGEST_API)/api/v1/admin/edges/synthesize \
		-H 'Content-Type: application/json' -d '{"min_cooccurrences": 1}' > /dev/null 2>&1 || true
	@curl -sf -X POST $(INGEST_API)/api/v1/admin/graph/reload > /dev/null 2>&1 || true
	@curl -sf -X POST $(INGEST_API)/api/v1/admin/cache/clear > /dev/null 2>&1 || true
	@echo "=== Statement reprocessing complete ==="

# === OpenAPI ===

spec:
	@echo "Start the engine first, then run: make spec-fetch"

spec-fetch:
	curl -s http://localhost:8431/openapi.json | python3 -m json.tool > openapi.json
	@echo "NOTE: Swagger UI at http://localhost:8431/docs, API at /api/v1/*"
	@echo "OpenAPI spec saved to openapi.json"

# === CLI ===

cli-build:
	cd cli && go build -o cove .

cli-install:
	cd cli && go install .

cli-test:
	cd cli && go test ./...

cli-vet:
	cd cli && go vet ./...

# === Docker ===

docker-up:
	@docker compose up -d dev-pg

docker-down:
	@docker compose down --remove-orphans

docker-status:
	@echo "=== Covalence Containers ==="
	@docker ps --format 'table {{.Names}}\t{{.Status}}\t{{.Ports}}' --filter 'name=covalence'

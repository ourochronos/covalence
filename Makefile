.PHONY: build test fmt lint clippy check run watch \
       dev-db test-db prod-db migrate migrate-prod reset-db reset-prod-db \
       run-dev run-prod promote \
       spec spec-fetch \
       cli-build cli-install \
       docker-up docker-down \
       ingest-codebase ingest-specs ingest-adrs ingest-prod \
       reprocess-statements

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

# === Database: Prod (port 5437) ===

prod-db:
	@docker compose --profile prod up -d prod-pg
	@echo "Waiting for prod PG to be ready..."
	@until docker exec covalence-prod-pg pg_isready -U covalence -d covalence_prod 2>/dev/null; do sleep 1; done
	@docker exec covalence-prod-pg psql -U covalence -d covalence_prod \
		-c "CREATE EXTENSION IF NOT EXISTS vector; CREATE EXTENSION IF NOT EXISTS pg_trgm; CREATE EXTENSION IF NOT EXISTS ltree;" \
		2>/dev/null || true
	@echo "Prod database ready on port 5437"

prod-db-stop:
	@docker compose --profile prod stop prod-pg

# === Migrations ===

migrate:
	cd engine && DATABASE_URL=postgres://covalence:covalence@localhost:5435/covalence_dev cargo run -p covalence-migrations

migrate-prod:
	cd engine && DATABASE_URL=postgres://covalence:covalence@localhost:5437/covalence_prod cargo run -p covalence-migrations

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
	@echo "WARNING: This will destroy all prod data!"
	@echo "Press Ctrl+C to abort, or wait 5 seconds..."
	@sleep 5
	@echo "Dropping and recreating covalence_prod..."
	@docker exec covalence-prod-pg psql -U covalence -d postgres \
		-c "DROP DATABASE IF EXISTS covalence_prod;" \
		-c "CREATE DATABASE covalence_prod OWNER covalence;"
	@docker exec covalence-prod-pg psql -U covalence -d covalence_prod \
		-c "CREATE EXTENSION IF NOT EXISTS vector;" \
		-c "CREATE EXTENSION IF NOT EXISTS pg_trgm;" \
		-c "CREATE EXTENSION IF NOT EXISTS ltree;"
	@echo "Running migrations on prod..."
	@cd engine && DATABASE_URL=postgres://covalence:covalence@localhost:5437/covalence_prod cargo run -p covalence-migrations
	@echo "Prod database reset complete."

# === Promote: test in dev, then apply to prod ===

promote: check
	@echo "=== Dev checks passed. Promoting migrations to prod... ==="
	@$(MAKE) prod-db
	@$(MAKE) migrate-prod
	@echo "=== Promotion complete. Prod is up to date. ==="

# === Run ===

run: run-dev

run-dev:
	cd engine && DATABASE_URL=postgres://covalence:covalence@localhost:5435/covalence_dev BIND_ADDR=0.0.0.0:8431 cargo run -p covalence-api

run-prod:
	cd engine && \
		DATABASE_URL=postgres://covalence:covalence@localhost:5437/covalence_prod \
		BIND_ADDR=0.0.0.0:8441 \
		cargo run -p covalence-api

watch:
	cd engine && cargo watch -x 'run -p covalence-api'

# === Ingestion ===
# These targets ingest content into an engine instance.
# Default: prod on :8441. Override with INGEST_API=http://localhost:8431 for dev.

INGEST_API ?= http://localhost:8441

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

REPROCESS_BATCH ?= 5

reprocess-statements:
	@echo "Finding document sources without statements..."
	@docker exec covalence-prod-pg psql -U covalence -d covalence_prod -t -c \
		"SELECT s.id FROM sources s WHERE s.source_type = 'document' AND s.id NOT IN (SELECT DISTINCT source_id FROM statements) ORDER BY s.ingested_at DESC" \
		| tr -d ' ' | grep -v '^$$' > /tmp/cov-reprocess-ids.txt; \
	total=$$(wc -l < /tmp/cov-reprocess-ids.txt | tr -d ' '); \
	echo "Found $$total unprocessed document sources (batch size: $(REPROCESS_BATCH))"; \
	i=0; batch=0; \
	while IFS= read -r id; do \
		i=$$((i+1)); batch=$$((batch+1)); \
		echo "  [$$i/$$total] reprocessing $$id..."; \
		curl -sf -X POST $(INGEST_API)/api/v1/sources/$$id/reprocess > /dev/null 2>&1 & \
		if [ $$batch -ge $(REPROCESS_BATCH) ]; then \
			wait; batch=0; \
			echo "  --- batch complete, synthesizing edges ---"; \
			curl -sf -X POST $(INGEST_API)/api/v1/admin/edges/synthesize \
				-H 'Content-Type: application/json' -d '{"min_cooccurrences": 1}' > /dev/null 2>&1 || true; \
		fi; \
	done < /tmp/cov-reprocess-ids.txt; \
	wait; \
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

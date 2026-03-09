.PHONY: build test fmt lint clippy check run watch \
       dev-db test-db migrate \
       spec spec-fetch \
       cli-build cli-install \
       docker-up docker-down

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

# === Database ===

dev-db:
	@docker compose up -d dev-pg
	@echo "Waiting for PG to be ready..."
	@until docker exec covalence-dev-pg pg_isready -U covalence -d covalence_dev 2>/dev/null; do sleep 1; done
	@docker exec covalence-dev-pg psql -U covalence -d covalence_dev \
		-c "CREATE EXTENSION IF NOT EXISTS vector; CREATE EXTENSION IF NOT EXISTS pg_trgm; CREATE EXTENSION IF NOT EXISTS ltree;" \
		2>/dev/null || true
	@echo "Dev database ready on port 5435"

dev-db-stop:
	@docker compose stop dev-pg

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

migrate:
	cd engine && cargo run -p covalence-migrations

# === OpenAPI ===

spec:
	@echo "Start the engine first, then run: make spec-fetch"

spec-fetch:
	curl -s http://localhost:8431/openapi.json | python3 -m json.tool > openapi.json
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
	@docker compose down

# === Run ===

run:
	cd engine && cargo run -p covalence-api

watch:
	cd engine && cargo watch -x 'run -p covalence-api'

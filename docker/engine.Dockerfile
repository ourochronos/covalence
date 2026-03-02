# =============================================================================
# Stage 1 — Builder
# =============================================================================
FROM rust:1.85-bookworm AS builder

# Install system deps needed by sqlx / openssl / etc.
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# ------ dependency-cache layer ------------------------------------------------
# Copy only the manifest files first so Docker can cache the dep-build layer
# separately from the source build layer.
COPY engine/Cargo.toml engine/Cargo.lock ./

# Create a stub main so `cargo build --release` can compile deps in isolation.
RUN mkdir -p src && echo 'fn main(){}' > src/main.rs

# Build deps only (this layer is cached as long as Cargo.toml / Cargo.lock
# haven't changed).
RUN cargo build --release
# Remove the stub artefacts so the real source gets a clean compile.
RUN rm -f target/release/deps/covalence_engine* \
         target/release/covalence-engine* \
         src/main.rs

# ------ source build ----------------------------------------------------------
COPY engine/src ./src
COPY engine/tests ./tests

RUN cargo build --release

# =============================================================================
# Stage 2 — Runtime
# =============================================================================
FROM debian:bookworm-slim AS runtime

# Runtime deps: libssl + CA certs for outbound HTTPS (LLM calls), pg_isready
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    postgresql-client \
    && rm -rf /var/lib/apt/lists/*

# Non-root user for better container hygiene
RUN useradd --system --no-create-home --shell /usr/sbin/nologin covalence

WORKDIR /app

# Copy engine binary
COPY --from=builder /build/target/release/covalence-engine /app/covalence-engine

# Copy the migration runner script
COPY docker/run-migrations.sh /app/run-migrations.sh
RUN chmod +x /app/run-migrations.sh

# sql/ migrations are mounted at runtime via a volume declared in compose;
# we pre-create the mount point so ownership is correct.
RUN mkdir -p /app/sql && chown -R covalence:covalence /app

USER covalence

EXPOSE 8430

ENTRYPOINT ["/app/run-migrations.sh"]

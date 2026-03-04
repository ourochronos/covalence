//! Shared test infrastructure for the slow-path worker integration tests.
//!
//! # Responsibilities
//! * [`setup_pool`] — obtain a connection pool to the test database, creating
//!   `covalence_test` and its schema on first use, then truncating all tables
//!   so every test starts with a clean slate.
//! * [`MockLlmClient`] — a configurable fake that returns pre-canned JSON
//!   responses keyed on prompt keywords and produces deterministic embeddings.
//! * [`TestFixture`] — a RAII guard that tracks every UUID created during a
//!   test and deletes them (along with cascading FK rows) at the end via
//!   [`TestFixture::cleanup`].
//! * Convenience helpers (`insert_source`, `insert_article`, …) that record
//!   the returned IDs directly into the fixture.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::anyhow;
use async_trait::async_trait;
use serde_json::{Value, json};
use sqlx::{Connection, PgPool};
use uuid::Uuid;

use covalence_engine::worker::{QueueTask, llm::LlmClient};

// ─── constants ────────────────────────────────────────────────────────────────

/// Default test database URL.  Points at `covalence_test`, a disposable
/// database that is created and schema-initialised automatically by
/// [`setup_pool`].  Override with the `DATABASE_URL` environment variable.
const DEFAULT_TEST_DB_URL: &str = "postgres://covalence:7acd460e27e782f1992a7aac892d76d19a0a30ee71b4b0d48d4542472e272bd6@localhost:5434/covalence_test";

/// Full DDL for the `covalence_test` schema, embedded at compile time.
/// Idempotent: every statement uses `IF NOT EXISTS`.
const SCHEMA_SQL: &str = include_str!("../test_db_schema.sql");

// ─── database setup ───────────────────────────────────────────────────────────

/// Derive the maintenance-database URL from a covalence test URL by replacing
/// the trailing database name with `postgres`.
///
/// e.g. `postgres://covalence:covalence@localhost:5434/covalence_test`
///    → `postgres://covalence:covalence@localhost:5434/postgres`
fn maintenance_url(test_url: &str) -> String {
    match test_url.rfind('/') {
        Some(pos) => format!("{}/postgres", &test_url[..pos]),
        None => test_url.to_string(),
    }
}

/// Create `covalence_test` (if absent), apply the full schema, then truncate
/// every table so the test run starts from a known-clean state.
///
/// Called once per test via [`setup_pool`].  Because tests run serially
/// (`--test-threads=1`) the truncation effectively replaces per-test cleanup
/// while also handling the "panic before cleanup" leak scenario.
async fn setup_test_database(test_url: &str) -> PgPool {
    // ── Step 1: ensure the test database exists ───────────────────────────
    let maint_url = maintenance_url(test_url);
    if let Ok(mut admin_conn) = sqlx::PgConnection::connect(&maint_url).await {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = 'covalence_test')",
        )
        .fetch_one(&mut admin_conn)
        .await
        .unwrap_or(false);

        if !exists {
            // CREATE DATABASE cannot run inside a transaction; PgConnection
            // uses the simple-query protocol (autocommit) so this is safe.
            sqlx::query("CREATE DATABASE covalence_test")
                .execute(&mut admin_conn)
                .await
                .ok();
        }
        admin_conn.close().await.ok();
    }
    // If we can't reach the maintenance DB we carry on — the test DB may
    // already exist (e.g. created by the CI `createdb` step).

    // ── Step 2: connect to the test database ─────────────────────────────
    let pool = PgPool::connect(test_url).await.unwrap_or_else(|e| {
        panic!(
            "Could not connect to test DB at {test_url}: {e}\n\
             Make sure `covalence-pg` is running on port 5434 \
             and that `covalence_test` exists."
        )
    });

    // ── Step 3: apply schema (idempotent CREATE … IF NOT EXISTS) ─────────
    sqlx::raw_sql(SCHEMA_SQL)
        .execute(&pool)
        .await
        .unwrap_or_else(|e| panic!("Failed to apply test schema to {test_url}: {e}"));

    // ── Step 4: truncate all tables for a clean slate ─────────────────────
    // Cascade handles FK ordering automatically.
    sqlx::raw_sql(
        "TRUNCATE \
            covalence.session_messages, \
            covalence.session_nodes, \
            covalence.usage_traces, \
            covalence.contentions, \
            covalence.slow_path_queue, \
            covalence.inference_log, \
            covalence.node_sections, \
            covalence.node_embeddings, \
            covalence.edges, \
            covalence.sessions, \
            covalence.nodes, \
            covalence.standing_concerns \
         RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .unwrap_or_else(|e| panic!("Failed to truncate test tables: {e}"));

    pool
}

/// Connect to the test Postgres instance, bootstrapping `covalence_test` if
/// needed.
///
/// Reads `DATABASE_URL` from the environment (matching `main.rs` behaviour)
/// and falls back to the hard-coded default used in CI.
pub async fn setup_pool() -> PgPool {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_TEST_DB_URL.to_string());
    assert!(
        url.contains("_test") || url.contains("5435"),
        "Refusing to run integration tests against non-test database: '{}'. \
         Use DATABASE_URL=...covalence_test or leave DATABASE_URL unset.",
        url
    );
    setup_test_database(&url).await
}

// ─── MockLlmClient ────────────────────────────────────────────────────────────

/// Configurable mock LLM client.
///
/// * `complete()` dispatches on prompt keywords and returns canned JSON
///   appropriate to each handler.
/// * `embed()` returns a deterministic 1 536-dim vector derived from a
///   simple FNV hash of the input text.
pub struct MockLlmClient {
    /// How many times `complete()` was called.
    pub complete_calls: AtomicU32,
    /// How many times `embed()` was called.
    pub embed_calls: AtomicU32,
    /// When `true`, every `complete()` call returns an error.
    pub always_error: bool,
    /// When set, `complete()` always returns this string regardless of prompt.
    pub fixed_response: Option<String>,
}

impl MockLlmClient {
    pub fn new() -> Self {
        Self {
            complete_calls: AtomicU32::new(0),
            embed_calls: AtomicU32::new(0),
            always_error: false,
            fixed_response: None,
        }
    }

    pub fn always_fail() -> Self {
        Self {
            always_error: true,
            ..Self::new()
        }
    }

    pub fn with_fixed_response(response: impl Into<String>) -> Self {
        Self {
            fixed_response: Some(response.into()),
            ..Self::new()
        }
    }

    /// Deterministic 1 536-dim unit-normalised vector seeded from FNV-1a(text).
    pub fn deterministic_embedding(text: &str) -> Vec<f32> {
        let mut hash: u64 = 14_695_981_039_346_656_037;
        for byte in text.bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(1_099_511_628_211);
        }
        let mut state = hash;
        (0..1536)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let bits = (state >> 32) as u32;
                (bits as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(&self, prompt: &str, _max_tokens: u32) -> anyhow::Result<String> {
        self.complete_calls.fetch_add(1, Ordering::SeqCst);

        if self.always_error {
            return Err(anyhow!("MockLlmClient: configured to always fail"));
        }

        if let Some(ref r) = self.fixed_response {
            return Ok(r.clone());
        }

        let resp = if prompt.contains("knowledge synthesizer")
            || prompt.contains("synthesizes their information")
            || prompt.contains("Preserve with HIGH FIDELITY")
        {
            // Extract only the source documents section so that keywords in the
            // instruction boilerplate (e.g. "chose" in the CRITICAL example) don't
            // influence the content selection branch.
            let sources_section = prompt.split("SOURCE DOCUMENTS:").nth(1).unwrap_or(prompt);
            let content = if sources_section.contains("chose")
                || sources_section.contains("rejected")
            {
                "We chose X over Y because Z provides better performance characteristics. \
                 Y was considered but rejected because it would introduce additional latency. \
                 ## Key Decisions\nSelected X for its throughput advantages.\n\
                 ## Rejected Approaches\nY was evaluated and set aside due to latency concerns."
            } else {
                "This article synthesizes the provided source documents into a coherent knowledge unit. \
                 It covers the key facts and relationships described across the source material."
            };
            json!({
                "title": "Synthesized Test Article",
                "content": content,
                "epistemic_type": "semantic",
                "source_relationships": []
            })
        } else if prompt.contains("split this article into two")
            || prompt.contains("best point to split")
        {
            json!({
                "split_index": 50,
                "part_a_title": "Test Article (Part 1)",
                "part_b_title": "Test Article (Part 2)",
                "reasoning": "Splitting at the natural section boundary."
            })
        } else if prompt.contains("Merge the two articles") {
            json!({
                "title": "Merged Test Article",
                "content": "This article merges both source articles into a unified knowledge unit.",
                "reasoning": "Both articles covered related aspects of the same topic."
            })
        } else if prompt.contains("knowledge indexing assistant") {
            json!({
                "keywords": ["federated", "knowledge", "systems", "distributed", "graph"],
                "tags": ["knowledge-management", "distributed-systems"]
            })
        } else if prompt.contains("knowledge-graph edge classifier") {
            json!({
                "relationship": "RELATES_TO",
                "confidence": 0.85,
                "reasoning": "Both items cover overlapping topics."
            })
        } else if prompt.contains("content conflicts") || prompt.contains("CONTRADICT or CONTEND") {
            json!({
                "is_contention": true,
                "relationship": "contradicts",
                "materiality": "high",
                "explanation": "The source directly contradicts the article's key claim."
            })
        } else if prompt.contains("resolving a content contention")
            || prompt.contains("choose the most appropriate resolution")
        {
            json!({
                "resolution": "supersede_b",
                "materiality": "high",
                "reasoning": "The source contains more recent and accurate information.",
                "updated_content": "Updated article content incorporating the source's corrections."
            })
        } else if prompt.contains("tree decomposition")
            || prompt.contains("tree index")
            || prompt.contains("decompose")
        {
            json!({
                "nodes": [{
                    "title": "Introduction",
                    "summary": "Introduction section.",
                    "start_char": 0,
                    "end_char": 1000
                }]
            })
        } else {
            json!({
                "note": "unrecognized prompt",
                "title": "Generic Response",
                "content": "Generic content for unrecognized prompt."
            })
        };

        Ok(resp.to_string())
    }

    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        self.embed_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Self::deterministic_embedding(text))
    }
}

// ─── TestFixture ─────────────────────────────────────────────────────────────

/// RAII test fixture.
///
/// Tracks every `Uuid` created during a test and deletes them — along with all
/// FK-dependent rows — when [`TestFixture::cleanup`] is called at the end.
///
/// Usage:
/// ```rust,ignore
/// let mut fix = TestFixture::new(pool).await;
/// let src = fix.insert_source("title", "content").await;
/// // … run handler …
/// fix.cleanup().await;
/// ```
pub struct TestFixture {
    pub pool: PgPool,
    /// Node IDs to delete at cleanup.
    node_ids: Vec<Uuid>,
    /// `(task_type, node_id)` pairs whose queue rows should be purged.
    queue_task_types: Vec<String>,
    /// Extra inference_log cleanup predicates `(operation, node_ids)`.
    infer_log_ops: Vec<(String, Vec<Uuid>)>,
}

impl TestFixture {
    pub async fn new() -> Self {
        Self {
            pool: setup_pool().await,
            node_ids: Vec::new(),
            queue_task_types: Vec::new(),
            infer_log_ops: Vec::new(),
        }
    }

    // ── tracking helpers ──────────────────────────────────────────────────────

    /// Register a node UUID for deletion at cleanup.
    pub fn track(&mut self, id: Uuid) -> Uuid {
        self.node_ids.push(id);
        id
    }

    /// Register a `task_type` whose recent queue rows should be purged.
    pub fn track_task_type(&mut self, tt: impl Into<String>) {
        self.queue_task_types.push(tt.into());
    }

    /// Register an `(operation, input_node_ids)` pair for inference_log cleanup.
    pub fn track_inference_log(&mut self, op: impl Into<String>, ids: Vec<Uuid>) {
        self.infer_log_ops.push((op.into(), ids));
    }

    // ── insert helpers ────────────────────────────────────────────────────────

    /// Insert a `source` node and track its id.
    pub async fn insert_source(&mut self, title: &str, content: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO covalence.nodes \
                 (id, node_type, status, title, content, metadata) \
             VALUES ($1, 'source', 'active', $2, $3, '{}'::jsonb)",
        )
        .bind(id)
        .bind(title)
        .bind(content)
        .execute(&self.pool)
        .await
        .unwrap_or_else(|e| panic!("insert_source({title}) failed: {e}"));
        self.track(id)
    }

    /// Insert an `article` node and track its id.
    pub async fn insert_article(&mut self, title: &str, content: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO covalence.nodes \
                 (id, node_type, status, title, content, metadata) \
             VALUES ($1, 'article', 'active', $2, $3, '{}'::jsonb)",
        )
        .bind(id)
        .bind(title)
        .bind(content)
        .execute(&self.pool)
        .await
        .unwrap_or_else(|e| panic!("insert_article({title}) failed: {e}"));
        self.track(id)
    }

    /// Insert an `article` node with explicit JSONB metadata and track its id.
    pub async fn insert_article_with_meta(
        &mut self,
        title: &str,
        content: &str,
        meta: &Value,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO covalence.nodes \
                 (id, node_type, status, title, content, metadata) \
             VALUES ($1, 'article', 'active', $2, $3, $4)",
        )
        .bind(id)
        .bind(title)
        .bind(content)
        .bind(meta)
        .execute(&self.pool)
        .await
        .unwrap_or_else(|e| panic!("insert_article_with_meta({title}) failed: {e}"));
        self.track(id)
    }

    /// Upsert a deterministic 1 536-dim halfvec embedding for `node_id`.
    pub async fn insert_embedding(&mut self, node_id: Uuid) {
        let dims = 1536usize;
        let val = 1.0_f32 / (dims as f32).sqrt();
        let literal = format!(
            "[{}]",
            std::iter::repeat_n(val.to_string(), dims)
                .collect::<Vec<_>>()
                .join(",")
        );
        sqlx::query(&format!(
            "INSERT INTO covalence.node_embeddings (node_id, embedding, model) \
             VALUES ($1, '{literal}'::halfvec({dims}), 'test-mock') \
             ON CONFLICT (node_id) DO NOTHING"
        ))
        .bind(node_id)
        .execute(&self.pool)
        .await
        .unwrap_or_else(|e| panic!("insert_embedding({node_id}) failed: {e}"));
    }

    /// Insert a `ORIGINATES` edge from `src` → `dst` (used to seed provenance).
    pub async fn insert_originates_edge(&self, src: Uuid, dst: Uuid) {
        sqlx::query(
            "INSERT INTO covalence.edges \
                 (source_node_id, target_node_id, edge_type) \
             VALUES ($1, $2, 'ORIGINATES')",
        )
        .bind(src)
        .bind(dst)
        .execute(&self.pool)
        .await
        .unwrap_or_else(|e| panic!("insert_originates_edge failed: {e}"));
    }

    /// Insert a contention row and return its id.  Defaults to `contention_type = 'rebuttal'`.
    pub async fn insert_contention(
        &self,
        node_id: Uuid,
        source_node_id: Uuid,
        severity: &str,
        materiality: f64,
    ) -> Uuid {
        self.insert_contention_typed(node_id, source_node_id, severity, materiality, "rebuttal")
            .await
    }

    /// Insert a contention row with an explicit contention_type and return its id.
    pub async fn insert_contention_typed(
        &self,
        node_id: Uuid,
        source_node_id: Uuid,
        severity: &str,
        materiality: f64,
        contention_type: &str,
    ) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO covalence.contentions \
                 (node_id, source_node_id, type, description, severity, status, materiality, contention_type) \
             VALUES ($1, $2, 'contradiction', 'Test contention', $3, 'detected', $4, $5) \
             RETURNING id",
        )
        .bind(node_id)
        .bind(source_node_id)
        .bind(severity)
        .bind(materiality)
        .bind(contention_type)
        .fetch_one(&self.pool)
        .await
        .expect("insert_contention_typed failed")
    }

    // ── QueueTask factory ─────────────────────────────────────────────────────

    /// Build an in-memory [`QueueTask`] (no DB insert needed for handlers that
    /// only read task fields).
    pub fn make_task(task_type: &str, node_id: Option<Uuid>, payload: Value) -> QueueTask {
        QueueTask {
            id: Uuid::new_v4(),
            task_type: task_type.to_string(),
            node_id,
            payload,
            result: None,
        }
    }

    // ── assertion helpers ─────────────────────────────────────────────────────

    /// Fetch the `status` column for the given node.
    pub async fn node_status(&self, id: Uuid) -> String {
        sqlx::query_scalar("SELECT status FROM covalence.nodes WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap_or_else(|_| panic!("node {id} not found"))
    }

    /// Fetch the `content` column for the given node.
    pub async fn node_content(&self, id: Uuid) -> String {
        sqlx::query_scalar("SELECT content FROM covalence.nodes WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap_or_else(|_| panic!("node {id} not found"))
    }

    /// Fetch the `metadata` JSONB for the given node.
    pub async fn node_metadata(&self, id: Uuid) -> Value {
        sqlx::query_scalar::<_, Value>("SELECT metadata FROM covalence.nodes WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap_or_else(|_| panic!("node metadata for {id} not found"))
    }

    /// Count edges of a specific `edge_type` outbound from `source_node_id`.
    pub async fn edge_count_from(&self, source: Uuid, edge_type: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM covalence.edges \
             WHERE source_node_id = $1 AND edge_type = $2",
        )
        .bind(source)
        .bind(edge_type)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0)
    }

    /// Count edges of a specific `edge_type` inbound to `target_node_id`.
    pub async fn edge_count_to(&self, target: Uuid, edge_type: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM covalence.edges \
             WHERE target_node_id = $1 AND edge_type = $2",
        )
        .bind(target)
        .bind(edge_type)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0)
    }

    /// Count pending queue tasks of a given type for a specific node.
    pub async fn pending_task_count(&self, task_type: &str, node_id: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM covalence.slow_path_queue \
             WHERE task_type = $1 AND node_id = $2 AND status = 'pending'",
        )
        .bind(task_type)
        .bind(node_id)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0)
    }

    /// Return true iff an embedding row exists for `node_id`.
    pub async fn embedding_exists(&self, node_id: Uuid) -> bool {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM covalence.node_embeddings WHERE node_id = $1)",
        )
        .bind(node_id)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false)
    }

    // ── cleanup ───────────────────────────────────────────────────────────────

    /// Delete all tracked nodes and related rows.
    ///
    /// Call this at the end of every test — even if assertions fail — by
    /// storing the fixture in a variable that stays in scope to the end of
    /// the test function body.
    ///
    /// Note: because [`setup_pool`] truncates all tables at the *start* of
    /// each test, any rows leaked by a panic are cleaned up automatically
    /// before the next test runs.  This method is retained for symmetry and
    /// to keep the DB tidy within a single test run.
    pub async fn cleanup(self) {
        let pool = &self.pool;

        // 1. Recent queue tasks by task_type (created in last 5 minutes).
        for tt in &self.queue_task_types {
            sqlx::query(
                "DELETE FROM covalence.slow_path_queue \
                 WHERE task_type = $1 AND created_at > now() - interval '5 minutes'",
            )
            .bind(tt)
            .execute(pool)
            .await
            .ok();
        }

        // 2. Inference log rows.
        for (op, ids) in &self.infer_log_ops {
            sqlx::query(
                "DELETE FROM covalence.inference_log \
                 WHERE operation = $1 AND input_node_ids @> $2",
            )
            .bind(op)
            .bind(ids)
            .execute(pool)
            .await
            .ok();
        }

        // 3. Per-node cleanup in FK dependency order:
        //    session_nodes → usage_traces → queue → contentions → edges
        //    → embeddings → sections → node
        for &id in &self.node_ids {
            // FK tables that reference nodes (must precede node deletion)
            sqlx::query("DELETE FROM covalence.session_nodes WHERE node_id = $1")
                .bind(id)
                .execute(pool)
                .await
                .ok();

            sqlx::query("DELETE FROM covalence.usage_traces WHERE node_id = $1")
                .bind(id)
                .execute(pool)
                .await
                .ok();

            sqlx::query("DELETE FROM covalence.slow_path_queue WHERE node_id = $1")
                .bind(id)
                .execute(pool)
                .await
                .ok();

            sqlx::query(
                "DELETE FROM covalence.contentions \
                 WHERE node_id = $1 OR source_node_id = $1",
            )
            .bind(id)
            .execute(pool)
            .await
            .ok();

            sqlx::query(
                "DELETE FROM covalence.edges \
                 WHERE source_node_id = $1 OR target_node_id = $1",
            )
            .bind(id)
            .execute(pool)
            .await
            .ok();

            // ON DELETE CASCADE handles node_embeddings and node_sections,
            // but we delete explicitly for clarity.
            sqlx::query("DELETE FROM covalence.node_embeddings WHERE node_id = $1")
                .bind(id)
                .execute(pool)
                .await
                .ok();

            sqlx::query("DELETE FROM covalence.nodes WHERE id = $1")
                .bind(id)
                .execute(pool)
                .await
                .ok();
        }
    }
}

// ─── Test AppState factory ─────────────────────────────────────────────────────

/// Build a test [`covalence_engine::api::AppState`] with an empty in-memory
/// graph (graph is re-loaded from DB on each edge mutation in production, but
/// for tests that don't exercise edge routes a fresh empty graph is fine).
pub async fn make_test_state(pool: sqlx::PgPool) -> covalence_engine::api::AppState {
    use std::sync::Arc;
    let llm: Arc<dyn covalence_engine::worker::llm::LlmClient> = Arc::new(MockLlmClient::new());
    let graph = Arc::new(tokio::sync::RwLock::new(
        covalence_engine::graph::CovalenceGraph::new(),
    ));
    covalence_engine::api::AppState {
        pool,
        llm,
        graph,
        api_key: None,
    }
}

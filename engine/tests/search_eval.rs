//! Search Precision Eval Suite — covalence#56.
//!
//! Evaluates the four-dimensional search pipeline by verifying:
//!   1. Each strategy produces a *distinct* weight distribution.
//!   2. Each strategy is dominated by the dimension it is named after.
//!   3. Explicit caller weights override strategy presets.
//!   4. The `search_debug` endpoint returns all required fields.
//!   5. Vector dimension fires (non-empty) when an embedding is provided.
//!   6. Lexical dimension fires for plain-text keyword queries.
//!   7. Graph dimension fires when the candidate set has neighbouring edges.
//!   8. Structural dimension fires when `COVALENCE_STRUCTURAL_SEARCH=true`
//!      and graph embeddings exist in the DB.
//!   9. A node whose content closely matches the query scores above a noise
//!      node whose content is unrelated.
//!
//! Tests 4-9 require a live database and are skipped gracefully when
//! `DATABASE_URL` is not set or the connection fails.

use covalence_engine::services::search_service::{
    resolve_weights, SearchDebugResponse, SearchRequest, SearchService, SearchStrategy,
    WeightsInput,
};
use uuid::Uuid;

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Connect to the database or return `None` (causes the calling test to skip).
async fn try_pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    sqlx::PgPool::connect(&url).await.ok()
}

/// Build a minimal `SearchRequest` for testing.
fn make_req(query: &str) -> SearchRequest {
    SearchRequest {
        query: query.to_string(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: None,
        limit: 10,
        weights: None,
        mode: None,
        recency_bias: None,
        domain_path: None,
        strategy: None,
        max_hops: None,
        after: None,
        before: None,
        min_score: None,
    }
}

// ─── 1. Strategy weight distinctness ─────────────────────────────────────────

/// Every strategy must produce a unique (vector, lexical, graph, structural)
/// weight tuple.  If any two strategies collapse to the same weights the fusion
/// layer cannot differentiate their intent.
#[test]
fn test_all_strategies_produce_distinct_weights() {
    let strategies = [
        SearchStrategy::Balanced,
        SearchStrategy::Precise,
        SearchStrategy::Exploratory,
        SearchStrategy::Graph,
        SearchStrategy::Structural,
    ];

    let weights: Vec<(f32, f32, f32, f32)> = strategies
        .iter()
        .map(|s| resolve_weights(&None, &Some(s.clone())))
        .collect();

    for (i, wa) in weights.iter().enumerate() {
        for (j, wb) in weights.iter().enumerate() {
            if i == j {
                continue;
            }
            assert_ne!(
                wa, wb,
                "strategies {:?} and {:?} resolve to identical weights {:?}",
                strategies[i], strategies[j], wa
            );
        }
    }
}

// ─── 2. Dominant-dimension assertions per strategy ────────────────────────────

#[test]
fn test_precise_strategy_is_lexical_heavy() {
    let (w_vec, w_lex, w_graph, w_struct) =
        resolve_weights(&None, &Some(SearchStrategy::Precise));
    assert!(
        w_lex > w_vec,
        "Precise: lexical ({w_lex:.3}) should exceed vector ({w_vec:.3})"
    );
    assert!(
        w_lex > w_graph,
        "Precise: lexical ({w_lex:.3}) should exceed graph ({w_graph:.3})"
    );
    assert!(
        w_lex > w_struct,
        "Precise: lexical ({w_lex:.3}) should exceed structural ({w_struct:.3})"
    );
}

#[test]
fn test_exploratory_strategy_is_vector_heavy() {
    let (w_vec, w_lex, w_graph, w_struct) =
        resolve_weights(&None, &Some(SearchStrategy::Exploratory));
    assert!(
        w_vec > w_lex,
        "Exploratory: vector ({w_vec:.3}) should exceed lexical ({w_lex:.3})"
    );
    assert!(
        w_vec > w_graph,
        "Exploratory: vector ({w_vec:.3}) should exceed graph ({w_graph:.3})"
    );
    assert!(
        w_vec > w_struct,
        "Exploratory: vector ({w_vec:.3}) should exceed structural ({w_struct:.3})"
    );
}

#[test]
fn test_graph_strategy_is_graph_heavy() {
    let (w_vec, w_lex, w_graph, w_struct) = resolve_weights(&None, &Some(SearchStrategy::Graph));
    assert!(
        w_graph > w_vec,
        "Graph: graph ({w_graph:.3}) should exceed vector ({w_vec:.3})"
    );
    assert!(
        w_graph > w_lex,
        "Graph: graph ({w_graph:.3}) should exceed lexical ({w_lex:.3})"
    );
    assert!(
        w_graph > w_struct,
        "Graph: graph ({w_graph:.3}) should exceed structural ({w_struct:.3})"
    );
}

#[test]
fn test_structural_strategy_is_structural_heavy() {
    let (w_vec, w_lex, w_graph, w_struct) =
        resolve_weights(&None, &Some(SearchStrategy::Structural));
    assert!(
        w_struct > w_vec,
        "Structural: structural ({w_struct:.3}) should exceed vector ({w_vec:.3})"
    );
    assert!(
        w_struct > w_lex,
        "Structural: structural ({w_struct:.3}) should exceed lexical ({w_lex:.3})"
    );
    assert!(
        w_struct > w_graph,
        "Structural: structural ({w_struct:.3}) should exceed graph ({w_graph:.3})"
    );
}

#[test]
fn test_balanced_strategy_weights_sum_to_one() {
    let (w_vec, w_lex, w_graph, w_struct) =
        resolve_weights(&None, &Some(SearchStrategy::Balanced));
    let sum = w_vec + w_lex + w_graph + w_struct;
    assert!(
        (sum - 1.0_f32).abs() < 1e-5,
        "Balanced weights should sum to 1.0, got {sum}"
    );
}

/// All five strategy weight sets must sum to 1.0 (within float tolerance).
#[test]
fn test_all_strategy_weights_sum_to_one() {
    let strategies = [
        SearchStrategy::Balanced,
        SearchStrategy::Precise,
        SearchStrategy::Exploratory,
        SearchStrategy::Graph,
        SearchStrategy::Structural,
    ];
    for s in &strategies {
        let (v, l, g, st) = resolve_weights(&None, &Some(s.clone()));
        let sum = v + l + g + st;
        assert!(
            (sum - 1.0_f32).abs() < 1e-5,
            "{:?}: weights should sum to 1.0, got {sum}",
            s
        );
    }
}

// ─── 3. Explicit caller weights override strategy ─────────────────────────────

#[test]
fn test_explicit_weights_override_strategy() {
    // Caller passes explicit weights; strategy should be ignored.
    let caller_weights = WeightsInput {
        vector: Some(0.10),
        lexical: Some(0.60),
        graph: Some(0.20),
        structural: Some(0.10),
    };
    // With explicit weights, strategy is irrelevant.
    let (w_vec, w_lex, w_graph, w_struct) =
        resolve_weights(&Some(caller_weights), &Some(SearchStrategy::Exploratory));

    // After normalization (sum == 1.0 already), values are preserved.
    assert!(
        (w_lex - 0.60_f32).abs() < 1e-4,
        "lexical weight should be ~0.60, got {w_lex}"
    );
    assert!(
        w_vec < w_lex,
        "caller-specified: lexical should dominate vector"
    );
    let _ = (w_graph, w_struct);
}

#[test]
fn test_explicit_weights_without_structural_default_zero() {
    // When structural is omitted from explicit weights it defaults to 0.0
    // (backward-compatible with pre-structural callers).
    let caller_weights = WeightsInput {
        vector: Some(0.55),
        lexical: Some(0.25),
        graph: Some(0.20),
        structural: None,
    };
    let (_, _, _, w_struct) = resolve_weights(&Some(caller_weights), &None);
    // After normalization the structural share should be 0.0 / total ≈ 0.0.
    assert!(
        w_struct < 1e-6,
        "structural should be ~0 when omitted from explicit weights, got {w_struct}"
    );
}

// ─── 4. Debug endpoint structure (DB-optional) ───────────────────────────────

/// Verifies that `search_debug` returns a well-formed `SearchDebugResponse`
/// with all required fields populated.  Skips when no DB is available.
#[tokio::test]
async fn test_debug_response_has_all_required_fields() {
    let pool = match try_pool().await {
        Some(p) => p,
        None => {
            eprintln!("DATABASE_URL not set — skipping test_debug_response_has_all_required_fields");
            return;
        }
    };

    let service = SearchService::new(pool);
    service.init().await;

    let req = make_req("knowledge graph embedding");
    let debug: SearchDebugResponse = service
        .search_debug(req)
        .await
        .expect("search_debug should not error");

    // Top-level fields
    assert!(!debug.query.is_empty(), "query should be non-empty");
    assert!(
        !debug.strategy_selected.is_empty(),
        "strategy_selected should be non-empty"
    );
    assert!(
        debug.elapsed_ms < 30_000,
        "elapsed_ms should be a plausible value"
    );

    // Fusion weights must sum to 1.0.
    let fw = &debug.fusion_weights;
    let sum = fw.vector + fw.lexical + fw.graph + fw.structural;
    assert!(
        (sum - 1.0_f32).abs() < 1e-4,
        "fusion_weights should sum to 1.0, got {sum}"
    );

    // Dimensions struct must exist with results_count ≥ 0.
    let dims = &debug.dimensions;
    let _ = dims.vector.results_count;
    let _ = dims.lexical.results_count;
    let _ = dims.graph.results_count;
    let _ = dims.structural.results_count;

    // Each raw_scores entry must have a non-zero-by-construction node_id.
    for entry in &dims.vector.raw_scores {
        assert_ne!(entry.node_id, Uuid::nil(), "vector raw_score node_id must not be nil");
    }
    for entry in &dims.lexical.raw_scores {
        assert_ne!(entry.node_id, Uuid::nil(), "lexical raw_score node_id must not be nil");
    }
}

/// The default strategy label for a bare request should be `"balanced"`.
#[tokio::test]
async fn test_debug_default_strategy_is_balanced() {
    let pool = match try_pool().await {
        Some(p) => p,
        None => {
            eprintln!(
                "DATABASE_URL not set — skipping test_debug_default_strategy_is_balanced"
            );
            return;
        }
    };

    let service = SearchService::new(pool);
    let req = make_req("anything");
    let debug = service
        .search_debug(req)
        .await
        .expect("search_debug should not error");

    assert_eq!(
        debug.strategy_selected, "balanced",
        "default strategy label should be 'balanced', got '{}'",
        debug.strategy_selected
    );
}

// ─── 5. Vector dimension fires for semantic queries (DB-optional) ─────────────

/// When the request includes a non-zero embedding *and* the DB has node
/// embeddings, the vector dimension should return at least one result.
#[tokio::test]
async fn test_vector_dimension_fires_for_semantic_queries() {
    let pool = match try_pool().await {
        Some(p) => p,
        None => {
            eprintln!(
                "DATABASE_URL not set — skipping test_vector_dimension_fires_for_semantic_queries"
            );
            return;
        }
    };

    // Check whether any node embeddings exist.
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM covalence.node_embeddings")
            .fetch_one(&pool)
            .await
            .unwrap_or(0);

    if count == 0 {
        eprintln!("no node_embeddings in DB — skipping vector dimension test");
        return;
    }

    // Fetch one real embedding to use as the query vector.
    let (emb_str,): (String,) = sqlx::query_as(
        "SELECT embedding::text FROM covalence.node_embeddings LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("fetch embedding");

    // Parse the pgvector text format "[a,b,c,...]" into Vec<f32>.
    let trimmed = emb_str.trim_start_matches('[').trim_end_matches(']');
    let embedding: Vec<f32> = trimmed
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    if embedding.is_empty() {
        eprintln!("could not parse embedding — skipping");
        return;
    }

    let service = SearchService::new(pool);
    let mut req = make_req("semantic similarity query");
    req.embedding = Some(embedding);

    let debug = service
        .search_debug(req)
        .await
        .expect("search_debug should not error");

    assert!(
        debug.dimensions.vector.results_count > 0,
        "vector dimension should fire (results_count > 0) when a query embedding is provided \
         and node_embeddings exist; got {}",
        debug.dimensions.vector.results_count
    );
    assert!(
        debug.dimensions.vector.available,
        "vector dimension should report available=true when pgvector extension is present"
    );
}

// ─── 6. Lexical dimension fires for keyword queries (DB-optional) ─────────────

/// For any non-empty text query, the lexical dimension should return results
/// as long as there is at least one 'active' node in the database.
#[tokio::test]
async fn test_lexical_dimension_fires_for_keyword_queries() {
    let pool = match try_pool().await {
        Some(p) => p,
        None => {
            eprintln!(
                "DATABASE_URL not set — skipping test_lexical_dimension_fires_for_keyword_queries"
            );
            return;
        }
    };

    // Fetch the title of an existing active node to use as a guaranteed keyword hit.
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT COALESCE(title, LEFT(content, 30)) \
         FROM covalence.nodes \
         WHERE status = 'active' AND (title IS NOT NULL OR content IS NOT NULL) \
         LIMIT 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap_or(None);

    let keyword = match row {
        Some((text,)) => {
            // Use the first significant word (>3 chars) from the title/content.
            text.split_whitespace()
                .find(|w| w.len() > 3)
                .unwrap_or("knowledge")
                .to_string()
        }
        None => {
            eprintln!("no active nodes in DB — skipping lexical dimension test");
            return;
        }
    };

    let service = SearchService::new(pool);
    let req = make_req(&keyword);

    let debug = service
        .search_debug(req)
        .await
        .expect("search_debug should not error");

    assert!(
        debug.dimensions.lexical.results_count > 0,
        "lexical dimension should fire for keyword query {:?}; got {} results",
        keyword,
        debug.dimensions.lexical.results_count
    );
    assert!(
        debug.dimensions.lexical.available,
        "lexical dimension should always report available=true"
    );
}

// ─── 7. Graph dimension fires when anchors have edges (DB-optional) ───────────

/// The graph dimension fires (results_count > 0) when the candidate anchor set
/// contains nodes that have at least one active edge to another active node.
#[tokio::test]
async fn test_graph_dimension_fires_when_anchors_have_edges() {
    let pool = match try_pool().await {
        Some(p) => p,
        None => {
            eprintln!(
                "DATABASE_URL not set — skipping test_graph_dimension_fires_when_anchors_have_edges"
            );
            return;
        }
    };

    // Find a node that has at least one active edge.
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT DISTINCT e.source_node_id
         FROM covalence.edges e
         JOIN covalence.nodes n ON n.id = e.source_node_id
         JOIN covalence.nodes t ON t.id = e.target_node_id
         WHERE n.status = 'active' AND t.status = 'active'
         LIMIT 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap_or(None);

    let anchor_id = match row {
        Some((id,)) => id,
        None => {
            eprintln!("no nodes with edges in DB — skipping graph dimension test");
            return;
        }
    };

    // Look up a keyword from the anchor node's content/title.
    let row2: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT title, LEFT(content, 80) FROM covalence.nodes WHERE id = $1")
            .bind(anchor_id)
            .fetch_optional(&pool)
            .await
            .unwrap_or(None);

    let keyword = match row2 {
        Some((Some(title), _)) => title
            .split_whitespace()
            .find(|w| w.len() > 3)
            .unwrap_or("knowledge")
            .to_string(),
        Some((None, Some(content))) => content
            .split_whitespace()
            .find(|w| w.len() > 3)
            .unwrap_or("knowledge")
            .to_string(),
        _ => "knowledge".to_string(),
    };

    // Use the Graph strategy so the graph dimension has maximum weight.
    let service = SearchService::new(pool);
    let mut req = make_req(&keyword);
    req.strategy = Some(SearchStrategy::Graph);
    req.max_hops = Some(1);
    // Raise limit so the graph dimension has room to return neighbours.
    req.limit = 20;

    let debug = service
        .search_debug(req)
        .await
        .expect("search_debug should not error");

    // The graph dimension requires at least one anchor from the lexical/vector
    // results.  If the keyword matched the anchor node, graph should fire.
    if debug.dimensions.lexical.results_count > 0 || debug.dimensions.vector.results_count > 0 {
        assert!(
            debug.dimensions.graph.results_count > 0,
            "graph dimension should fire when anchors have edges; got {} graph results \
             (lexical={}, vector={})",
            debug.dimensions.graph.results_count,
            debug.dimensions.lexical.results_count,
            debug.dimensions.vector.results_count,
        );
    } else {
        eprintln!("no lexical/vector anchors found for keyword {keyword:?} — graph test inconclusive");
    }
}

// ─── 8. Structural dimension fires when graph embeddings exist (DB-optional) ──

/// When `COVALENCE_STRUCTURAL_SEARCH=true` and graph_embeddings rows exist,
/// the structural dimension should produce results for a non-empty anchor set.
#[tokio::test]
async fn test_structural_dimension_fires_when_graph_embeddings_exist() {
    let pool = match try_pool().await {
        Some(p) => p,
        None => {
            eprintln!(
                "DATABASE_URL not set — skipping test_structural_dimension_fires"
            );
            return;
        }
    };

    // Count graph_embeddings rows.
    let emb_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM covalence.graph_embeddings")
            .fetch_one(&pool)
            .await
            .unwrap_or(0);

    if emb_count == 0 {
        eprintln!("no graph_embeddings in DB — skipping structural dimension test");
        return;
    }

    // Enable the feature flag for this test.
    // SAFETY: single-threaded test runner at this point.
    unsafe {
        std::env::set_var("COVALENCE_STRUCTURAL_SEARCH", "true");
    }

    let service = SearchService::new(pool);
    let mut req = make_req("structural similarity");
    req.strategy = Some(SearchStrategy::Structural);
    req.limit = 20;

    let debug = service
        .search_debug(req)
        .await
        .expect("search_debug should not error");

    assert!(
        debug.dimensions.structural.available,
        "structural dimension should report available=true when graph_embeddings rows exist"
    );

    // Only assert firing if prior dimensions produced anchors.
    let has_anchors = debug.dimensions.vector.results_count > 0
        || debug.dimensions.lexical.results_count > 0;

    if has_anchors {
        assert!(
            debug.dimensions.structural.results_count > 0,
            "structural dimension should fire when COVALENCE_STRUCTURAL_SEARCH=true, \
             graph_embeddings exist, and anchors are present; got {}",
            debug.dimensions.structural.results_count
        );
    } else {
        eprintln!("no anchors produced — structural dimension test inconclusive");
    }

    // Restore env for other tests.
    unsafe {
        std::env::remove_var("COVALENCE_STRUCTURAL_SEARCH");
    }
}

// ─── 9. Relevant document scores above noise (DB-optional) ────────────────────

/// Insert two nodes: one whose content is an exact phrase match for the query
/// (relevant) and one with entirely unrelated content (noise).  After search,
/// the relevant node's final score must exceed the noise node's score.
///
/// Cleanup: both nodes are deleted in a `finally`-style block.
#[tokio::test]
async fn test_relevant_document_scores_above_noise() {
    let pool = match try_pool().await {
        Some(p) => p,
        None => {
            eprintln!(
                "DATABASE_URL not set — skipping test_relevant_document_scores_above_noise"
            );
            return;
        }
    };

    let tag = format!("eval-test-{}", Uuid::new_v4().simple());
    let query = format!("covalence search eval relevance test {tag}");

    // Insert a *relevant* node whose content closely matches the query.
    let relevant_id: Uuid = sqlx::query_scalar(
        "INSERT INTO covalence.nodes
            (node_type, title, content, status, confidence, reliability,
             content_tsv, created_at, modified_at)
         VALUES
            ('source',
             $1,
             $2,
             'active',
             0.9,
             0.9,
             to_tsvector('english', $2),
             now(), now())
         RETURNING id",
    )
    .bind(format!("Relevant: {tag}"))
    .bind(&query)
    .fetch_one(&pool)
    .await
    .expect("insert relevant node");

    // Insert a *noise* node whose content shares no terms with the query.
    let noise_id: Uuid = sqlx::query_scalar(
        "INSERT INTO covalence.nodes
            (node_type, title, content, status, confidence, reliability,
             content_tsv, created_at, modified_at)
         VALUES
            ('source',
             $1,
             $2,
             'active',
             0.1,
             0.1,
             to_tsvector('english', $2),
             now(), now())
         RETURNING id",
    )
    .bind(format!("Noise: {tag}"))
    .bind("xyzzy plugh twisty passage unreachable zzz")
    .fetch_one(&pool)
    .await
    .expect("insert noise node");

    // Run a search for the query.
    let service = SearchService::new(pool.clone());
    let req = SearchRequest {
        query: query.clone(),
        embedding: None,
        intent: None,
        session_id: None,
        node_types: None,
        limit: 50,
        weights: None,
        mode: None,
        recency_bias: None,
        domain_path: None,
        strategy: Some(SearchStrategy::Precise), // lexical-heavy for keyword precision
        max_hops: None,
        after: None,
        before: None,
        min_score: None,
    };

    let debug = service
        .search_debug(req)
        .await
        .expect("search_debug should not error");

    // Cleanup — always runs even if assertions below fail.
    let _ = sqlx::query("DELETE FROM covalence.nodes WHERE id = ANY($1)")
        .bind(&[relevant_id, noise_id])
        .execute(&pool)
        .await;

    // Find the scores for each node in the final results.
    let relevant_score = debug
        .final_results
        .iter()
        .find(|r| r.node_id == relevant_id)
        .map(|r| r.score);

    let noise_score = debug
        .final_results
        .iter()
        .find(|r| r.node_id == noise_id)
        .map(|r| r.score);

    match (relevant_score, noise_score) {
        (Some(rel), Some(noi)) => {
            assert!(
                rel > noi,
                "relevant node (score={rel:.4}) should score above noise node (score={noi:.4})"
            );
        }
        (Some(_), None) => {
            // Relevant node found, noise not — that's ideal.
        }
        (None, _) => {
            panic!(
                "relevant node ({relevant_id}) not found in search results for query {query:?}; \
                 debug had {} final results",
                debug.final_results.len()
            );
        }
    }
}

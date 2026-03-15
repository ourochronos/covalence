//! Live search regression harness.
//!
//! Runs queries from the search precision baseline fixture against
//! a live Covalence search API, comparing result counts and
//! flagging regressions.

use serde::{Deserialize, Serialize};

/// A query entry from the search precision baseline fixture.
#[derive(Debug, Clone, Deserialize)]
pub struct BaselineQuery {
    /// The search query text.
    pub query: String,
    /// Expected precision@5 from the baseline evaluation.
    pub precision_at_5: f64,
    /// Number of relevant results in the baseline.
    pub relevant_count: usize,
    /// Number of results returned in the baseline.
    pub result_count: usize,
    /// Notes about the baseline evaluation.
    pub notes: String,
}

/// The full baseline fixture.
#[derive(Debug, Clone, Deserialize)]
pub struct BaselineFixture {
    /// Description of the baseline.
    pub description: String,
    /// Date the baseline was established.
    pub date: String,
    /// Baseline overall P@5 score.
    pub baseline_score: f64,
    /// Quality gate threshold.
    pub quality_gate: f64,
    /// Individual query baselines.
    pub queries: Vec<BaselineQuery>,
}

/// A search result from the live API.
#[derive(Debug, Clone, Deserialize)]
pub struct LiveSearchResult {
    /// Result UUID.
    pub id: uuid::Uuid,
    /// Fused relevance score.
    pub fused_score: f64,
    /// Entity type.
    pub entity_type: Option<String>,
    /// Entity or chunk name.
    pub name: Option<String>,
    /// Source title.
    pub source_title: Option<String>,
    /// Snippet.
    pub snippet: Option<String>,
}

/// Search API request.
#[derive(Debug, Serialize)]
struct SearchRequest {
    query: String,
    limit: usize,
    mode: String,
}

/// Per-query regression result.
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// The query text.
    pub query: String,
    /// Baseline P@5.
    pub baseline_p5: f64,
    /// Baseline result count.
    pub baseline_count: usize,
    /// Live result count.
    pub live_count: usize,
    /// Top result names from live.
    pub top_results: Vec<String>,
    /// Whether this looks like a regression.
    pub regressed: bool,
    /// Reason for regression flag.
    pub regression_reason: Option<String>,
}

/// Overall regression report.
#[derive(Debug, Clone)]
pub struct RegressionReport {
    /// API URL used.
    pub api_url: String,
    /// Per-query results.
    pub queries: Vec<QueryResult>,
    /// Number of queries that regressed.
    pub regressions: usize,
    /// Number of queries with stable or improved results.
    pub stable: usize,
}

/// Load the baseline fixture from a JSON file.
pub fn load_baseline(path: &str) -> anyhow::Result<BaselineFixture> {
    let content = std::fs::read_to_string(path)?;
    let fixture: BaselineFixture = serde_json::from_str(&content)?;
    Ok(fixture)
}

/// Run all baseline queries against the live search API.
pub async fn run_regression(
    api_url: &str,
    baseline: &BaselineFixture,
) -> anyhow::Result<RegressionReport> {
    let client = reqwest::Client::new();
    let search_url = format!("{api_url}/api/v1/search");
    let mut queries = Vec::new();
    let mut regressions = 0;

    for bq in &baseline.queries {
        let req = SearchRequest {
            query: bq.query.clone(),
            limit: 5,
            mode: "results".to_string(),
        };

        let resp = client.post(&search_url).json(&req).send().await?;

        let status = resp.status();
        if !status.is_success() {
            queries.push(QueryResult {
                query: bq.query.clone(),
                baseline_p5: bq.precision_at_5,
                baseline_count: bq.result_count,
                live_count: 0,
                top_results: vec![],
                regressed: true,
                regression_reason: Some(format!("HTTP {status}")),
            });
            regressions += 1;
            continue;
        }

        // The API returns either Results([...]) or the array directly.
        let body: serde_json::Value = resp.json().await?;
        let results: Vec<LiveSearchResult> = if let Some(arr) = body.as_array() {
            serde_json::from_value(serde_json::Value::Array(arr.clone()))?
        } else if let Some(obj) = body.as_object() {
            if let Some(arr) = obj.get("Results") {
                serde_json::from_value(arr.clone())?
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        let live_count = results.len();
        let top_results: Vec<String> = results
            .iter()
            .take(5)
            .map(|r| {
                let etype = r.entity_type.as_deref().unwrap_or("?");
                let name = r.name.as_deref().unwrap_or("(unnamed)");
                format!("[{etype}] {name}")
            })
            .collect();

        // Flag regression: fewer results than baseline, or
        // baseline had >=4 results and live has 0.
        let (regressed, reason) = if live_count == 0 && bq.result_count > 0 {
            (true, Some("zero results (was non-zero)".to_string()))
        } else if live_count < bq.result_count.saturating_sub(1) {
            (
                true,
                Some(format!(
                    "result count dropped: {} -> {}",
                    bq.result_count, live_count
                )),
            )
        } else {
            (false, None)
        };

        if regressed {
            regressions += 1;
        }

        queries.push(QueryResult {
            query: bq.query.clone(),
            baseline_p5: bq.precision_at_5,
            baseline_count: bq.result_count,
            live_count,
            top_results,
            regressed,
            regression_reason: reason,
        });
    }

    let stable = queries.len() - regressions;
    Ok(RegressionReport {
        api_url: api_url.to_string(),
        queries,
        regressions,
        stable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_baseline_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fixtures/search_precision_baseline.json"
        );
        let baseline = load_baseline(path).unwrap();
        assert_eq!(baseline.queries.len(), 20);
        assert!(baseline.baseline_score > 0.8);
        assert!(baseline.quality_gate > 0.0);
    }

    #[test]
    fn baseline_queries_have_required_fields() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fixtures/search_precision_baseline.json"
        );
        let baseline = load_baseline(path).unwrap();
        for q in &baseline.queries {
            assert!(!q.query.is_empty());
            assert!(q.precision_at_5 >= 0.0 && q.precision_at_5 <= 1.0);
            assert!(q.result_count > 0 || q.precision_at_5 == 0.0);
        }
    }
}

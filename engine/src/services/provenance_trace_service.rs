//! Provenance trace — rank linked sources by TF-IDF cosine similarity to a claim.

use sqlx::{PgPool, Row};
use std::collections::HashMap;
use uuid::Uuid;

use crate::errors::{AppError, AppResult};

#[derive(Debug, serde::Deserialize)]
pub struct TraceRequest {
    pub claim_text: String,
}

#[derive(Debug, serde::Serialize)]
pub struct TraceResult {
    pub source_id: Uuid,
    pub title: Option<String>,
    pub score: f64,
    pub snippet: String,
}

pub struct ProvenanceTraceService {
    pool: PgPool,
}

impl ProvenanceTraceService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn trace(&self, article_id: Uuid, req: TraceRequest) -> AppResult<Vec<TraceResult>> {
        // 1. Verify article exists and is active
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM covalence.nodes WHERE id = $1 AND node_type = 'article' AND status = 'active')",
        )
        .bind(article_id)
        .fetch_one(&self.pool)
        .await?;

        if !exists {
            return Err(AppError::NotFound(format!(
                "article {article_id} not found or not active"
            )));
        }

        // 2. Fetch linked sources via edges (source_node_id points to source, target_node_id = article)
        let rows = sqlx::query(
            "SELECT n.id, n.title, n.content
             FROM covalence.edges e
             JOIN covalence.nodes n ON n.id = e.source_node_id
             WHERE e.target_node_id = $1
               AND e.edge_type = ANY(ARRAY['originates','confirms','supersedes'])
               AND n.node_type = 'source'
               AND n.status = 'active'",
        )
        .bind(article_id)
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Ok(vec![]);
        }

        // 3. Collect source data
        let sources: Vec<(Uuid, Option<String>, String)> = rows
            .into_iter()
            .map(|r| {
                let id: Uuid = r.get("id");
                let title: Option<String> = r.get("title");
                let content: Option<String> = r.get("content");
                (id, title, content.unwrap_or_default())
            })
            .collect();

        // 4. Compute TF-IDF cosine similarity and sort
        let mut results = tfidf_rank(&req.claim_text, &sources);
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(results)
    }
}

// ── TF-IDF helpers ───────────────────────────────────────────────────────────

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

fn term_freq(tokens: &[String]) -> HashMap<String, f64> {
    let mut tf: HashMap<String, f64> = HashMap::new();
    for tok in tokens {
        *tf.entry(tok.clone()).or_default() += 1.0;
    }
    let total = tokens.len() as f64;
    if total > 0.0 {
        for v in tf.values_mut() {
            *v /= total;
        }
    }
    tf
}

fn tfidf_rank(claim: &str, sources: &[(Uuid, Option<String>, String)]) -> Vec<TraceResult> {
    let claim_tokens = tokenize(claim);
    let claim_tf = term_freq(&claim_tokens);

    // Per-source TF maps
    let source_tfs: Vec<HashMap<String, f64>> = sources
        .iter()
        .map(|(_, _, content)| term_freq(&tokenize(content)))
        .collect();

    // IDF: document frequency across the source corpus
    let n = sources.len() as f64;
    let mut df: HashMap<String, f64> = HashMap::new();
    for stf in &source_tfs {
        for term in stf.keys() {
            *df.entry(term.clone()).or_default() += 1.0;
        }
    }
    let idf = |term: &str| -> f64 {
        let d = df.get(term).copied().unwrap_or(0.0);
        if d == 0.0 { 0.0 } else { (n / d).ln() + 1.0 }
    };

    // TF-IDF vector for the claim (using source corpus IDF)
    let claim_tfidf: HashMap<String, f64> = claim_tf
        .iter()
        .map(|(t, &tf)| (t.clone(), tf * idf(t)))
        .collect();

    sources
        .iter()
        .zip(source_tfs.iter())
        .map(|((id, title, content), stf)| {
            let src_tfidf: HashMap<String, f64> = stf
                .iter()
                .map(|(t, &tf)| (t.clone(), tf * idf(t)))
                .collect();

            let score = cosine_sim(&claim_tfidf, &src_tfidf);
            let snippet = make_snippet(content, claim, 200);

            TraceResult {
                source_id: *id,
                title: title.clone(),
                score,
                snippet,
            }
        })
        .collect()
}

fn cosine_sim(a: &HashMap<String, f64>, b: &HashMap<String, f64>) -> f64 {
    let dot: f64 = a
        .iter()
        .map(|(t, va)| va * b.get(t).copied().unwrap_or(0.0))
        .sum();
    let mag_a: f64 = a.values().map(|v| v * v).sum::<f64>().sqrt();
    let mag_b: f64 = b.values().map(|v| v * v).sum::<f64>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        0.0
    } else {
        dot / (mag_a * mag_b)
    }
}

/// Extract a snippet from content around the first term overlapping with the claim.
fn make_snippet(content: &str, claim: &str, max_chars: usize) -> String {
    let claim_tokens: std::collections::HashSet<String> = tokenize(claim).into_iter().collect();
    // Find approximate char offset of first matching word
    let start = {
        let mut offset = 0usize;
        let mut found = 0usize;
        for word in content.split_whitespace() {
            let tok = word
                .split(|c: char| !c.is_alphanumeric())
                .find(|t| !t.is_empty())
                .map(|t| t.to_lowercase())
                .unwrap_or_default();
            if claim_tokens.contains(&tok) {
                found = offset.saturating_sub(30);
                break;
            }
            offset += word.len() + 1;
        }
        found.min(content.len())
    };

    let slice = &content[start..];
    let end = slice
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(slice.len());
    let snippet = &slice[..end];
    if end < slice.len() {
        format!("{snippet}…")
    } else {
        snippet.to_string()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_basic() {
        let tokens = tokenize("Hello, World! foo-bar");
        assert_eq!(tokens, vec!["hello", "world", "foo", "bar"]);
    }

    #[test]
    fn cosine_sim_identical() {
        let mut a = HashMap::new();
        a.insert("rust".to_string(), 0.5);
        a.insert("code".to_string(), 0.5);
        let score = cosine_sim(&a, &a);
        assert!((score - 1.0).abs() < 1e-9);
    }

    #[test]
    fn cosine_sim_orthogonal() {
        let mut a = HashMap::new();
        a.insert("rust".to_string(), 1.0);
        let mut b = HashMap::new();
        b.insert("python".to_string(), 1.0);
        assert_eq!(cosine_sim(&a, &b), 0.0);
    }

    #[test]
    fn tfidf_rank_orders_correctly() {
        let sources = vec![
            (
                Uuid::new_v4(),
                Some("Relevant".to_string()),
                "rust programming language systems".to_string(),
            ),
            (
                Uuid::new_v4(),
                Some("Unrelated".to_string()),
                "baking bread flour yeast water".to_string(),
            ),
        ];
        let mut results = tfidf_rank("rust systems programming", &sources);
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title.as_deref(), Some("Relevant"));
        assert!(results[0].score > results[1].score);
    }
}

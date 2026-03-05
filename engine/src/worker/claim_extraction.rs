use sqlx::{PgPool, Row};
use uuid::Uuid;
use serde_json::{json, Value};
use std::sync::Arc;
use anyhow::{Context, Result};
use std::time::Instant;

use crate::worker::{QueueTask, llm::LlmClient, parse_llm_json, log_inference};
use crate::graph::GraphRepository as _;

/// Extract atomic claims from a source node and store them as claim nodes.
///
/// This handler implements Phase 2 of the claims layer migration (covalence#169):
///
/// 1. Fetch source content
/// 2. LLM extracts atomic, falsifiable claims (structured JSON output)
/// 3. For each claim:
///    a. Check for near-duplicate via embedding similarity (cosine > 0.95)
///    b. If duplicate found: create SAME_AS edge
///    c. If novel: create new claim node (node_type='claim', generation=2)
///    d. Create SUPPORTS edge from source → claim
///
/// Claims are nodes with:
/// - node_type = 'claim'
/// - generation = 2 (claims-layer content)
/// - Optional valid_until for temporal claims
/// - content = single atomic claim (1-2 sentences)
///
/// Deduplication uses embedding cosine similarity > 0.95 threshold to identify
/// semantically identical claims across different sources.
pub async fn handle_extract_claims(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    task: &QueueTask,
) -> Result<Value> {
    let source_id = task.node_id.context("extract_claims requires node_id")?;

    // Fetch source content
    let row = sqlx::query(
        "SELECT content, title, node_type FROM covalence.nodes WHERE id = $1 AND status = 'active'"
    )
    .bind(source_id)
    .fetch_optional(pool)
    .await?
    .with_context(|| format!("extract_claims: source {} not found or inactive", source_id))?;

    let content: String = row.get::<Option<String>, _>("content").unwrap_or_default();
    let title: String = row.get::<Option<String>, _>("title").unwrap_or_default();
    let node_type: String = row.get("node_type");

    if content.trim().is_empty() {
        return Ok(json!({
            "source_id": source_id,
            "claims_extracted": 0,
            "reason": "empty_content"
        }));
    }

    // Build extraction prompt
    let prompt = format!(
        r#"Extract atomic, falsifiable claims from the following source document.

SOURCE: {title}
TYPE: {node_type}

CONTENT:
{content}

INSTRUCTIONS:
- Extract only factual claims that can be verified or falsified
- Each claim should be a single, atomic statement (1-2 sentences max)
- Include decisions, findings, and rejected alternatives as claims
- Preserve reasoning and rationale where present
- For temporal claims (e.g., "X was CEO until 2023"), include the validity period
- Skip opinions, questions, and procedural text

Respond ONLY with valid JSON (no markdown fences):
{{
  "claims": [
    {{
      "text": "The atomic claim text",
      "confidence": 0.85,
      "temporal": false,
      "valid_until": null,
      "context": "Brief context if needed"
    }}
  ]
}}

Think step by step. Extract all meaningful claims."#
    );

    // LLM extraction with timing
    let chat_model = std::env::var("COVALENCE_CHAT_MODEL")
        .unwrap_or_else(|_| "gpt-4o-mini".into());
    
    let t0 = Instant::now();
    let raw_response = llm.complete(&prompt, 4096).await?;
    let llm_latency_ms = t0.elapsed().as_millis() as i32;

    let llm_json = parse_llm_json(&raw_response)
        .with_context(|| format!("extract_claims: failed to parse LLM response as JSON"))?;

    let claims_array = llm_json
        .get("claims")
        .and_then(|v| v.as_array())
        .context("extract_claims: response missing 'claims' array")?;

    if claims_array.is_empty() {
        // Log inference for empty extraction
        let _ = log_inference(
            pool,
            "extract_claims",
            &[source_id],
            &format!("source={}, content_len={}", source_id, content.len()),
            "no_claims",
            Some(1.0),
            "No claims extracted from source",
            &chat_model,
            llm_latency_ms,
        ).await;

        return Ok(json!({
            "source_id": source_id,
            "claims_extracted": 0,
            "reason": "no_claims_found"
        }));
    }

    let mut claims_created = 0;
    let mut claims_deduped = 0;
    let mut claim_ids: Vec<Uuid> = Vec::new();

    // Process each extracted claim
    for claim_obj in claims_array {
        let claim_text = claim_obj
            .get("text")
            .and_then(|v| v.as_str())
            .context("claim missing 'text' field")?;

        let confidence = claim_obj
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.8);

        let temporal = claim_obj
            .get("temporal")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let valid_until: Option<chrono::DateTime<chrono::Utc>> = claim_obj
            .get("valid_until")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));

        let context = claim_obj
            .get("context")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Generate embedding for dedup check
        let claim_embedding = llm.embed(claim_text).await?;
        let dims = claim_embedding.len();
        let vec_literal = format!(
            "[{}]",
            claim_embedding
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        // Check for near-duplicate claims (cosine > 0.95)
        let existing_claim_id: Option<Uuid> = sqlx::query(&format!(
            "SELECT ne.node_id, n.content,
                    (ne.embedding::halfvec({dims}) <=> '{vec_literal}'::halfvec({dims})) AS distance
             FROM covalence.node_embeddings ne
             JOIN covalence.nodes n ON n.id = ne.node_id
             WHERE n.node_type = 'claim'
               AND n.status = 'active'
               AND (ne.embedding::halfvec({dims}) <=> '{vec_literal}'::halfvec({dims})) < 0.05
             ORDER BY distance ASC
             LIMIT 1"
        ))
        .fetch_optional(pool)
        .await?
        .map(|r| r.get::<Uuid, _>("node_id"));

        let claim_id = if let Some(existing_id) = existing_claim_id {
            // Duplicate found - create SAME_AS edge
            tracing::debug!(
                source_id = %source_id,
                existing_claim_id = %existing_id,
                "extract_claims: duplicate claim found, creating SAME_AS edge"
            );
            
            claims_deduped += 1;
            existing_id
        } else {
            // Novel claim - create new claim node
            let new_claim_id = Uuid::new_v4();
            
            let metadata = json!({
                "context": context,
                "extracted_from": source_id,
                "extraction_confidence": confidence,
                "temporal": temporal,
            });

            sqlx::query(
                "INSERT INTO covalence.nodes
                    (id, node_type, status, content, metadata, generation, valid_until, created_at, modified_at)
                 VALUES ($1, 'claim', 'active', $2, $3, 2, $4, now(), now())"
            )
            .bind(new_claim_id)
            .bind(claim_text)
            .bind(&metadata)
            .bind(valid_until)
            .execute(pool)
            .await
            .with_context(|| format!("extract_claims: failed to create claim node"))?;

            // Store embedding
            let embed_model = std::env::var("COVALENCE_EMBED_MODEL")
                .unwrap_or_else(|_| "text-embedding-3-small".into());

            sqlx::query(&format!(
                "INSERT INTO covalence.node_embeddings (node_id, embedding, model)
                 VALUES ($1, '{vec_literal}'::halfvec({dims}), $2)"
            ))
            .bind(new_claim_id)
            .bind(&embed_model)
            .execute(pool)
            .await
            .context("extract_claims: failed to store claim embedding")?;

            tracing::info!(
                source_id = %source_id,
                claim_id = %new_claim_id,
                temporal,
                "extract_claims: created new claim node"
            );

            claims_created += 1;
            new_claim_id
        };

        claim_ids.push(claim_id);

        // Create SUPPORTS edge from source → claim
        // Uses graph repository pattern for consistency
        let graph_repo = crate::graph::SqlGraphRepository::new(pool.clone());
        
        if let Err(e) = graph_repo.create_edge(
            source_id,
            claim_id,
            crate::models::EdgeType::SupportsClaim,
            confidence as f32,
            "claim_extraction",
            json!({
                "extraction_confidence": confidence,
                "temporal": temporal,
            }),
        ).await {
            tracing::warn!(
                source_id = %source_id,
                claim_id = %claim_id,
                "extract_claims: failed to create SUPPORTS edge: {}", e
            );
        }
    }

    // Log successful extraction
    let _ = log_inference(
        pool,
        "extract_claims",
        &[source_id],
        &format!("source={}, content_len={}, claims_extracted={}", 
                 source_id, content.len(), claims_array.len()),
        "success",
        Some(1.0),
        &format!("Extracted {} claims ({} new, {} deduped)", 
                 claims_array.len(), claims_created, claims_deduped),
        &chat_model,
        llm_latency_ms,
    ).await;

    tracing::info!(
        source_id = %source_id,
        claims_created,
        claims_deduped,
        total_claims = claims_array.len(),
        "extract_claims: completed"
    );

    Ok(json!({
        "source_id": source_id,
        "claims_extracted": claims_array.len(),
        "claims_created": claims_created,
        "claims_deduped": claims_deduped,
        "claim_ids": claim_ids,
    }))
}

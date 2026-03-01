//! Tree index builder and section embedder for large sources.
//!
//! Ports Valence's tree_index + section_embeddings pipeline to Rust.
//! No truncation — uses sliding windows with configurable overlap.
//!
//! Pipeline:
//!   1. Build tree index via LLM (single-window or multi-window + merge)
//!   2. Flatten tree to sections with tree_path
//!   3. Embed each section's content slice
//!   4. Compose (mean) all section embeddings → store as node-level embedding
//!
//! Tree index is stored in nodes.metadata->'tree_index'.
//! Section embeddings are stored in node_sections.

use anyhow::Context;
use md5;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::llm::LlmClient;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Sources below this character count get a trivial single-node tree (no LLM).
pub const TRIVIAL_THRESHOLD_CHARS: usize = 700;

/// Sources below this character count use a single LLM call.
/// Above this, we use sliding windows.
pub const SINGLE_WINDOW_MAX_CHARS: usize = 280_000; // ~80K tokens

/// Default window size in characters for multi-window indexing.
pub const DEFAULT_WINDOW_CHARS: usize = 280_000; // ~80K tokens

/// Default overlap fraction between windows.
pub const DEFAULT_OVERLAP_FRACTION: f64 = 0.20;

/// Maximum characters per section for embedding.
/// text-embedding-3-small accepts 8191 tokens (~28K chars).
/// We use 24K to leave margin.
pub const MAX_SECTION_EMBED_CHARS: usize = 24_000;

/// Minimum section size — don't create tiny fragments.
pub const MIN_SECTION_CHARS: usize = 50;

// ---------------------------------------------------------------------------
// Tree node schema
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    pub title: String,
    #[serde(default)]
    pub summary: String,
    pub start_char: usize,
    pub end_char: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TreeNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeIndex {
    pub nodes: Vec<TreeNode>,
}

/// Flattened section for embedding.
#[derive(Debug, Clone)]
pub struct FlatSection {
    pub tree_path: String,
    pub depth: i32,
    pub title: String,
    pub summary: String,
    pub start_char: usize,
    pub end_char: usize,
}

// ---------------------------------------------------------------------------
// Prompts
// ---------------------------------------------------------------------------

const SINGLE_WINDOW_PROMPT: &str = r#"You are building a tree index (table of contents) for a source document.

Identify the natural topic structure and return a tree where each node references a region of the original text by character offset.

Rules:
- Identify major topics (depth 1) and subtopics (depth 2+) where they naturally exist
- Each leaf node MUST have start_char and end_char pointing to exact character positions
- Parent nodes should span their children's full range
- Nodes must cover the entire document without gaps
- Do NOT split mid-sentence
- Keep titles concise (5-10 words)
- Add a one-sentence summary for each node

Return ONLY valid JSON:
{
  "nodes": [
    {
      "title": "Topic Title",
      "summary": "One sentence summary",
      "start_char": 0,
      "end_char": 500,
      "children": [
        {
          "title": "Subtopic",
          "summary": "One sentence summary",
          "start_char": 0,
          "end_char": 250
        }
      ]
    }
  ]
}

Omit "children" for leaf nodes. The source is SOURCE_CHARS characters long.

--- SOURCE TEXT ---
SOURCE_TEXT
--- END SOURCE TEXT ---"#;

const WINDOW_PROMPT: &str = r#"You are building a tree index for a SECTION of a larger document.

This section starts at character offset GLOBAL_OFFSET in the full document.
CONTEXT_NOTE

All start_char/end_char values must be GLOBAL offsets (relative to the full document).
This section covers global characters GLOBAL_OFFSET to GLOBAL_END.

Rules:
- All offsets must be within [GLOBAL_OFFSET, GLOBAL_END]
- Do NOT split mid-sentence
- Keep titles concise (5-10 words)
- Add a one-sentence summary for each node

Return ONLY valid JSON:
{
  "nodes": [
    {
      "title": "Topic Title",
      "summary": "One sentence summary",
      "start_char": GLOBAL_OFFSET,
      "end_char": <offset>,
      "children": [...]
    }
  ]
}

Omit "children" for leaf nodes.

--- SECTION TEXT ---
WINDOW_TEXT
--- END SECTION TEXT ---"#;

const MERGE_PROMPT: &str = r#"You are merging local tree indexes into a coherent top-level tree structure.

Below are tree indexes built from sequential sections of a large document.
Merge them into a single coherent hierarchy:
- Combine nodes that clearly belong to the same topic across section boundaries
- Create parent nodes to group related sections
- Preserve all leaf-level start_char/end_char offsets exactly as given
- Keep titles concise (5-10 words)
- Add a one-sentence summary for each new parent node

Return ONLY valid JSON:
{
  "nodes": [...]
}

--- LOCAL TREES ---
LOCAL_TREES_JSON
--- END LOCAL TREES ---"#;

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Extract JSON from LLM response, handling markdown code blocks.
fn extract_json(text: &str) -> anyhow::Result<TreeIndex> {
    let text = text.trim();
    let json_str = if text.starts_with("```") {
        // Strip markdown fences
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() >= 3 {
            lines[1..lines.len() - 1].join("\n")
        } else {
            text.to_string()
        }
    } else {
        text.to_string()
    };
    serde_json::from_str(&json_str).context("failed to parse tree index JSON from LLM response")
}

/// Validate tree node offsets. Clamps end_char to source_len.
fn validate_tree(tree: &mut TreeIndex, source_len: usize) {
    fn walk(nodes: &mut [TreeNode], source_len: usize) {
        for node in nodes.iter_mut() {
            if node.end_char > source_len {
                node.end_char = source_len;
            }
            if node.start_char > source_len {
                node.start_char = source_len.saturating_sub(1);
            }
            if node.start_char >= node.end_char && source_len > 0 {
                node.end_char = node.start_char + 1;
            }
            walk(&mut node.children, source_len);
        }
    }
    walk(&mut tree.nodes, source_len);
}

/// Flatten tree into sections with tree_path.
pub fn flatten_tree(nodes: &[TreeNode], prefix: &str, depth: i32) -> Vec<FlatSection> {
    let mut flat = Vec::new();
    for (i, node) in nodes.iter().enumerate() {
        let path = if prefix.is_empty() {
            format!("{}", i)
        } else {
            format!("{}.{}", prefix, i)
        };
        flat.push(FlatSection {
            tree_path: path.clone(),
            depth,
            title: node.title.clone(),
            summary: node.summary.clone(),
            start_char: node.start_char,
            end_char: node.end_char,
        });
        if !node.children.is_empty() {
            flat.extend(flatten_tree(&node.children, &path, depth + 1));
        }
    }
    flat
}

/// Build tree index for small sources (single LLM call).
async fn build_tree_single(content: &str, llm: &Arc<dyn LlmClient>) -> anyhow::Result<TreeIndex> {
    let prompt = SINGLE_WINDOW_PROMPT
        .replace("SOURCE_CHARS", &content.len().to_string())
        .replace("SOURCE_TEXT", content);
    let response = llm.complete(&prompt, 8000).await?;
    extract_json(&response)
}

/// Build tree index using sliding windows with overlap.
async fn build_tree_windowed(
    content: &str,
    llm: &Arc<dyn LlmClient>,
    window_chars: usize,
    overlap_fraction: f64,
) -> anyhow::Result<TreeIndex> {
    let overlap_chars = (window_chars as f64 * overlap_fraction) as usize;
    let step = window_chars - overlap_chars;

    let mut local_trees: Vec<TreeIndex> = Vec::new();
    let mut offset = 0usize;

    while offset < content.len() {
        let end = (offset + window_chars).min(content.len());
        let window_text = &content[offset..end];

        let context_note = if let Some(prev) = local_trees.last() {
            if let Some(last_node) = prev.nodes.last() {
                format!(
                    "The previous section ended with topic: '{}'",
                    last_node.title
                )
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let prompt = WINDOW_PROMPT
            .replace("GLOBAL_OFFSET", &offset.to_string())
            .replace("GLOBAL_END", &end.to_string())
            .replace("CONTEXT_NOTE", &context_note)
            .replace("WINDOW_TEXT", window_text);

        let response = llm.complete(&prompt, 8000).await?;
        let local_tree = extract_json(&response)?;

        tracing::info!(
            window = local_trees.len() + 1,
            chars_start = offset,
            chars_end = end,
            nodes = local_tree.nodes.len(),
            "tree_index: window processed"
        );

        local_trees.push(local_tree);

        if end >= content.len() {
            break;
        }
        offset += step;
    }

    if local_trees.len() == 1 {
        return Ok(local_trees.into_iter().next().unwrap());
    }

    // Merge pass
    let local_trees_json = serde_json::to_string_pretty(&local_trees)?;
    let estimated_tokens = local_trees_json.len() / 4;

    if estimated_tokens < 80_000 {
        // Single merge call
        let prompt = MERGE_PROMPT.replace("LOCAL_TREES_JSON", &local_trees_json);
        let response = llm.complete(&prompt, 8000).await?;
        extract_json(&response)
    } else {
        // Recursive merge in batches
        let batch_size = 4.max(80_000 / (estimated_tokens / local_trees.len() + 1));
        let mut merged_groups: Vec<TreeIndex> = Vec::new();

        for chunk in local_trees.chunks(batch_size) {
            let group_json = serde_json::to_string_pretty(chunk)?;
            let prompt = MERGE_PROMPT.replace("LOCAL_TREES_JSON", &group_json);
            let response = llm.complete(&prompt, 8000).await?;
            merged_groups.push(extract_json(&response)?);
        }

        // Final merge
        let final_json = serde_json::to_string_pretty(&merged_groups)?;
        let prompt = MERGE_PROMPT.replace("LOCAL_TREES_JSON", &final_json);
        let response = llm.complete(&prompt, 8000).await?;
        extract_json(&response)
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Build a tree index for a node and store it in metadata.
///
/// Returns (TreeIndex, method, node_count).
pub async fn build_tree_index(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    node_id: Uuid,
    overlap_fraction: f64,
    force: bool,
) -> anyhow::Result<Value> {
    // Fetch node
    let row = sqlx::query_as::<_, (Option<String>, Option<String>, Value)>(
        "SELECT title, content, metadata FROM covalence.nodes WHERE id = $1",
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await?
    .context("node not found")?;

    let (title, content, metadata) = row;
    let content = content.unwrap_or_default();
    if content.is_empty() {
        anyhow::bail!("node has no content");
    }

    // Check existing tree index
    if !force {
        if let Some(tree) = metadata.get("tree_index") {
            if !tree.is_null() {
                anyhow::bail!("node already has tree_index. Use force=true to rebuild.");
            }
        }
    }

    let (tree, method) = if content.len() < TRIVIAL_THRESHOLD_CHARS {
        // Trivial: single node covering entire content
        let tree = TreeIndex {
            nodes: vec![TreeNode {
                title: title.unwrap_or_else(|| "Full content".into()),
                summary: content
                    .chars()
                    .take(120)
                    .collect::<String>()
                    .replace('\n', " "),
                start_char: 0,
                end_char: content.len(),
                children: vec![],
            }],
        };
        (tree, "trivial")
    } else if content.len() <= SINGLE_WINDOW_MAX_CHARS {
        let tree = build_tree_single(&content, llm).await?;
        (tree, "single")
    } else {
        let tree =
            build_tree_windowed(&content, llm, DEFAULT_WINDOW_CHARS, overlap_fraction).await?;
        (tree, "windowed")
    };

    // Validate and clamp
    let mut tree = tree;
    validate_tree(&mut tree, content.len());

    // Count nodes
    fn count_nodes(nodes: &[TreeNode]) -> usize {
        nodes.iter().map(|n| 1 + count_nodes(&n.children)).sum()
    }
    let node_count = count_nodes(&tree.nodes);

    // Store in metadata
    let tree_json = serde_json::to_value(&tree)?;
    let now = chrono::Utc::now().to_rfc3339();

    sqlx::query("UPDATE covalence.nodes SET metadata = metadata || $1::jsonb WHERE id = $2")
        .bind(json!({
            "tree_index": tree_json,
            "tree_indexed_at": now,
            "tree_method": method,
            "tree_node_count": node_count,
            "tree_overlap": overlap_fraction,
        }))
        .bind(node_id)
        .execute(pool)
        .await
        .context("failed to store tree index in metadata")?;

    tracing::info!(
        node_id = %node_id,
        method,
        node_count,
        content_len = content.len(),
        "tree_index: built and stored"
    );

    Ok(json!({
        "node_id": node_id,
        "method": method,
        "node_count": node_count,
        "content_len": content.len(),
    }))
}

/// Embed all sections of a tree-indexed node.
/// Also composes a node-level embedding (mean of leaf sections).
///
/// Returns count of sections embedded.
pub async fn embed_sections(
    pool: &PgPool,
    llm: &Arc<dyn LlmClient>,
    node_id: Uuid,
) -> anyhow::Result<Value> {
    // Fetch node content and tree index
    let row = sqlx::query_as::<_, (Option<String>, Value)>(
        "SELECT content, metadata FROM covalence.nodes WHERE id = $1",
    )
    .bind(node_id)
    .fetch_optional(pool)
    .await?
    .context("node not found")?;

    let (content, metadata) = row;
    let content = content.unwrap_or_default();

    // Get tree index from metadata
    let tree_value = metadata
        .get("tree_index")
        .context("node has no tree_index in metadata")?;

    let tree: TreeIndex =
        serde_json::from_value(tree_value.clone()).context("failed to deserialize tree_index")?;

    // Flatten
    let sections = flatten_tree(&tree.nodes, "", 0);
    if sections.is_empty() {
        return Ok(json!({ "node_id": node_id, "sections_embedded": 0 }));
    }

    let model =
        std::env::var("COVALENCE_EMBED_MODEL").unwrap_or_else(|_| "text-embedding-3-small".into());

    let mut embedded_count = 0;
    let mut leaf_embeddings: Vec<Vec<f32>> = Vec::new();

    for section in &sections {
        let start = section.start_char;
        let end = section.end_char.min(content.len());
        if start >= end {
            continue;
        }
        let slice = &content[start..end];
        let slice = slice.trim();
        if slice.len() < MIN_SECTION_CHARS {
            continue;
        }

        // Content hash for change detection
        let hash = format!("{:x}", md5::compute(slice.as_bytes()));

        // Check if section exists and is current
        let existing = sqlx::query_as::<_, (Option<String>,)>(
            "SELECT content_hash FROM covalence.node_sections WHERE node_id = $1 AND tree_path = $2",
        )
        .bind(node_id)
        .bind(&section.tree_path)
        .fetch_optional(pool)
        .await?;

        if let Some((Some(existing_hash),)) = existing {
            if existing_hash == hash {
                // Unchanged — fetch existing embedding for composition
                let emb_row = sqlx::query_as::<_, (String,)>(
                    "SELECT embedding::text FROM covalence.node_sections WHERE node_id = $1 AND tree_path = $2",
                )
                .bind(node_id)
                .bind(&section.tree_path)
                .fetch_optional(pool)
                .await?;

                if let Some((emb_str,)) = emb_row {
                    if section.children_count(&sections) == 0 {
                        if let Some(vec) = parse_pgvector(&emb_str) {
                            leaf_embeddings.push(vec);
                        }
                    }
                }
                embedded_count += 1;
                continue;
            }
        }

        // Embed — for sections larger than MAX_SECTION_EMBED_CHARS,
        // we use a sliding window within the section and average
        let embedding = if slice.len() > MAX_SECTION_EMBED_CHARS {
            embed_long_section(
                slice,
                llm,
                MAX_SECTION_EMBED_CHARS,
                DEFAULT_OVERLAP_FRACTION,
            )
            .await?
        } else {
            llm.embed(slice).await?
        };

        let dims = embedding.len() as i32;
        let vec_literal = format!(
            "[{}]",
            embedding
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        // Track leaf embeddings for composition
        if section.children_count(&sections) == 0 {
            leaf_embeddings.push(embedding);
        }

        // Upsert section
        sqlx::query(&format!(
            "INSERT INTO covalence.node_sections
                (node_id, tree_path, depth, title, summary, start_char, end_char, content_hash, embedding, model)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, '{vec_literal}'::halfvec({dims}), $9)
             ON CONFLICT (node_id, tree_path) DO UPDATE SET
                depth = EXCLUDED.depth,
                title = EXCLUDED.title,
                summary = EXCLUDED.summary,
                start_char = EXCLUDED.start_char,
                end_char = EXCLUDED.end_char,
                content_hash = EXCLUDED.content_hash,
                embedding = EXCLUDED.embedding,
                model = EXCLUDED.model"
        ))
        .bind(node_id)
        .bind(&section.tree_path)
        .bind(section.depth)
        .bind(&section.title)
        .bind(&section.summary)
        .bind(section.start_char as i32)
        .bind(section.end_char as i32)
        .bind(&hash)
        .bind(&model)
        .execute(pool)
        .await
        .context("failed to upsert section")?;

        embedded_count += 1;
    }

    // Compose node-level embedding from leaf section embeddings
    if !leaf_embeddings.is_empty() {
        let composed = compose_embeddings(&leaf_embeddings);
        let dims = composed.len() as i32;
        let vec_literal = format!(
            "[{}]",
            composed
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        sqlx::query(&format!(
            "INSERT INTO covalence.node_embeddings (node_id, embedding, model)
             VALUES ($1, '{vec_literal}'::halfvec({dims}), $2)
             ON CONFLICT (node_id) DO UPDATE
               SET embedding = EXCLUDED.embedding,
                   model = EXCLUDED.model"
        ))
        .bind(node_id)
        .bind(&format!("{model}:composed"))
        .execute(pool)
        .await
        .context("failed to upsert composed embedding")?;

        tracing::info!(
            node_id = %node_id,
            leaf_count = leaf_embeddings.len(),
            "tree_index: composed embedding stored"
        );
    }

    tracing::info!(
        node_id = %node_id,
        sections = embedded_count,
        leaves = leaf_embeddings.len(),
        "tree_index: section embeddings complete"
    );

    Ok(json!({
        "node_id": node_id,
        "sections_embedded": embedded_count,
        "leaf_embeddings_composed": leaf_embeddings.len(),
    }))
}

/// Embed a long section using sliding windows and averaging.
/// No truncation — every part of the content contributes to the embedding.
async fn embed_long_section(
    text: &str,
    llm: &Arc<dyn LlmClient>,
    window_chars: usize,
    overlap_fraction: f64,
) -> anyhow::Result<Vec<f32>> {
    let overlap = (window_chars as f64 * overlap_fraction) as usize;
    let step = window_chars - overlap;

    let mut embeddings: Vec<Vec<f32>> = Vec::new();
    let mut offset = 0usize;

    while offset < text.len() {
        let end = (offset + window_chars).min(text.len());
        let window = &text[offset..end];

        let embedding = llm.embed(window).await?;
        embeddings.push(embedding);

        if end >= text.len() {
            break;
        }
        offset += step;
    }

    if embeddings.is_empty() {
        anyhow::bail!("no windows produced embeddings");
    }

    Ok(compose_embeddings(&embeddings))
}

/// Compute element-wise mean of embedding vectors.
fn compose_embeddings(vecs: &[Vec<f32>]) -> Vec<f32> {
    if vecs.is_empty() {
        return vec![];
    }
    let n = vecs.len() as f32;
    let dims = vecs[0].len();
    let mut result = vec![0.0f32; dims];
    for vec in vecs {
        for (i, v) in vec.iter().enumerate() {
            if i < dims {
                result[i] += v;
            }
        }
    }
    for v in result.iter_mut() {
        *v /= n;
    }
    result
}

/// Parse pgvector string "[0.1,0.2,...]" into Vec<f32>.
fn parse_pgvector(s: &str) -> Option<Vec<f32>> {
    let s = s.trim();
    if !s.starts_with('[') || !s.ends_with(']') {
        return None;
    }
    let inner = &s[1..s.len() - 1];
    inner
        .split(',')
        .map(|v| v.trim().parse::<f32>().ok())
        .collect()
}

// ---------------------------------------------------------------------------
// Helper trait extension for FlatSection
// ---------------------------------------------------------------------------

impl FlatSection {
    /// Count how many sections are children of this one.
    fn children_count(&self, all: &[FlatSection]) -> usize {
        let prefix = format!("{}.", self.tree_path);
        all.iter()
            .filter(|s| s.tree_path.starts_with(&prefix))
            .count()
    }
}

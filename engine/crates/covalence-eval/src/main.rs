//! CLI entry point for the Covalence evaluation harness.
//!
//! Runs layer-specific evaluations against fixture files
//! and prints metrics to stdout.

use anyhow::{Context, Result};
use clap::Parser;

use covalence_eval::LayerEvaluator;
use covalence_eval::chunker_eval::{ChunkerEval, ChunkerInput};
use covalence_eval::extractor_eval::{EvalEntity, ExtractorEval, ExtractorOutput};
use covalence_eval::fixtures::load_fixture;
use covalence_eval::search_eval::{RankedResult, SearchEval, SearchOutput};

/// Layer-by-layer evaluation harness for the Covalence pipeline.
#[derive(Parser, Debug)]
#[command(name = "covalence-eval")]
#[command(about = "Evaluate individual Covalence pipeline layers")]
struct Cli {
    /// Which layer to evaluate: chunker, extractor, or search.
    #[arg(long)]
    layer: String,

    /// Path to the fixture JSON file.
    #[arg(long)]
    input: String,

    /// Maximum chunk size for the chunker layer (bytes).
    #[arg(long, default_value = "500")]
    max_chunk_size: usize,

    /// K value for Precision@K in the search layer.
    #[arg(long, default_value = "10")]
    k: usize,

    /// Output as JSON instead of human-readable text.
    #[arg(long)]
    json: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let fixture = load_fixture(&cli.input).context("failed to load fixture")?;

    match cli.layer.as_str() {
        "chunker" => run_chunker_eval(&cli, &fixture),
        "extractor" => run_extractor_eval(&cli, &fixture),
        "search" => run_search_eval(&cli, &fixture),
        other => {
            anyhow::bail!(
                "unknown layer '{other}': \
                 expected chunker, extractor, or search"
            );
        }
    }
}

fn run_chunker_eval(cli: &Cli, fixture: &covalence_eval::fixtures::EvalFixture) -> Result<()> {
    let eval = ChunkerEval::new(cli.max_chunk_size);
    let input = ChunkerInput {
        text: fixture.document.clone(),
    };
    let output = eval.evaluate(&input);
    let metrics = eval.score(&output, &output);

    if cli.json {
        let json = serde_json::to_string_pretty(&metrics).context("failed to serialize metrics")?;
        println!("{json}");
    } else {
        println!("=== Chunker Evaluation ===");
        println!("Coverage:          {:.2}%", metrics.coverage * 100.0);
        println!("Chunk count:       {}", metrics.chunk_count);
        println!("Avg chunk size:    {:.1} bytes", metrics.avg_chunk_size);
        println!("Min chunk size:    {} bytes", metrics.min_chunk_size);
        println!("Max chunk size:    {} bytes", metrics.max_chunk_size);
        println!("  Document chunks: {}", metrics.document_chunks);
        println!("  Section chunks:  {}", metrics.section_chunks);
        println!("  Paragraph chunks: {}", metrics.paragraph_chunks);
    }

    Ok(())
}

fn run_extractor_eval(cli: &Cli, fixture: &covalence_eval::fixtures::EvalFixture) -> Result<()> {
    let eval = ExtractorEval::default();

    // In offline evaluation mode, both the "predicted" and
    // "expected" come from the fixture. This demonstrates the
    // scoring pipeline; in a real run the predicted entities
    // would come from the actual extractor.
    let expected = ExtractorOutput {
        entities: fixture
            .expected_entities
            .iter()
            .map(|e| EvalEntity {
                name: e.name.clone(),
                entity_type: e.entity_type.clone(),
            })
            .collect(),
    };

    // For demonstration, score the gold set against itself
    // (perfect extraction).
    let metrics = eval.score(&expected, &expected);

    if cli.json {
        let json = serde_json::to_string_pretty(&metrics).context("failed to serialize metrics")?;
        println!("{json}");
    } else {
        println!("=== Extractor Evaluation ===");
        println!("Precision:         {:.4}", metrics.precision);
        println!("Recall:            {:.4}", metrics.recall);
        println!("F1:                {:.4}", metrics.f1);
        println!("Predicted:         {}", metrics.predicted_count);
        println!("Gold:              {}", metrics.gold_count);
        println!("True positives:    {}", metrics.true_positives);
    }

    Ok(())
}

fn run_search_eval(cli: &Cli, fixture: &covalence_eval::fixtures::EvalFixture) -> Result<()> {
    let eval = SearchEval::new(cli.k);

    if fixture.queries.is_empty() {
        println!("No queries in fixture.");
        return Ok(());
    }

    for query_fixture in &fixture.queries {
        // Build expected output from fixture ground truth.
        let expected = SearchOutput {
            results: Vec::new(),
            relevance: query_fixture
                .expected_ids
                .iter()
                .zip(query_fixture.relevance_grades.iter())
                .map(|(id, &grade)| (id.clone(), grade))
                .collect(),
        };

        // Simulate a perfect ranking: results in the same
        // order as the expected IDs with descending scores.
        let perfect_results: Vec<RankedResult> = query_fixture
            .expected_ids
            .iter()
            .enumerate()
            .map(|(i, id)| RankedResult {
                id: id.clone(),
                score: if query_fixture.expected_ids.len() > 1 {
                    1.0 - (i as f64 / (query_fixture.expected_ids.len() - 1) as f64)
                } else {
                    1.0
                },
            })
            .collect();

        let actual = SearchOutput {
            results: perfect_results,
            relevance: expected.relevance.clone(),
        };

        let metrics = eval.score(&actual, &expected);

        if cli.json {
            let json =
                serde_json::to_string_pretty(&metrics).context("failed to serialize metrics")?;
            println!("{json}");
        } else {
            println!("=== Search Evaluation: \"{}\" ===", query_fixture.query);
            println!(
                "P@{}:              {:.4}",
                metrics.k, metrics.precision_at_k
            );
            println!("nDCG:              {:.4}", metrics.ndcg);
            println!("MRR:               {:.4}", metrics.mrr);
            println!("Result count:      {}", metrics.result_count);
            println!();
        }
    }

    Ok(())
}

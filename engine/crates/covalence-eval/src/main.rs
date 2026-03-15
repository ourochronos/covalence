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
use covalence_eval::search_regression;

/// Layer-by-layer evaluation harness for the Covalence pipeline.
#[derive(Parser, Debug)]
#[command(name = "covalence-eval")]
#[command(about = "Evaluate individual Covalence pipeline layers")]
struct Cli {
    /// Which layer to evaluate: chunker, extractor, search,
    /// or search-regression.
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

    /// API URL for live evaluation (search-regression layer).
    #[arg(long, default_value = "http://localhost:8441")]
    api_url: String,

    /// Output as JSON instead of human-readable text.
    #[arg(long)]
    json: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.layer.as_str() {
        "search-regression" => {
            let rt = tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;
            rt.block_on(run_search_regression(&cli))
        }
        _ => {
            let fixture = load_fixture(&cli.input).context("failed to load fixture")?;
            match cli.layer.as_str() {
                "chunker" => run_chunker_eval(&cli, &fixture),
                "extractor" => run_extractor_eval(&cli, &fixture),
                "search" => run_search_eval(&cli, &fixture),
                other => {
                    anyhow::bail!(
                        "unknown layer '{other}': expected chunker, \
                         extractor, search, or search-regression"
                    );
                }
            }
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

async fn run_search_regression(cli: &Cli) -> Result<()> {
    let baseline =
        search_regression::load_baseline(&cli.input).context("failed to load search baseline")?;

    println!(
        "Running {} queries against {} ...",
        baseline.queries.len(),
        cli.api_url
    );
    println!(
        "Baseline: P@5 = {:.2}, gate = {:.2}",
        baseline.baseline_score, baseline.quality_gate
    );
    println!();

    let report = search_regression::run_regression(&cli.api_url, &baseline).await?;

    for qr in &report.queries {
        let status = if qr.regressed { "REGR" } else { " OK " };
        let count_delta = qr.live_count as i32 - qr.baseline_count as i32;
        let delta_str = if count_delta >= 0 {
            format!("+{count_delta}")
        } else {
            format!("{count_delta}")
        };

        println!(
            "[{status}] \"{query}\"  baseline_p5={bp5:.1}  \
             results={live}/{base} ({delta})",
            query = qr.query,
            bp5 = qr.baseline_p5,
            live = qr.live_count,
            base = qr.baseline_count,
            delta = delta_str,
        );

        if !qr.top_results.is_empty() && !cli.json {
            for (i, r) in qr.top_results.iter().enumerate() {
                println!("       {}: {r}", i + 1);
            }
        }

        if let Some(ref reason) = qr.regression_reason {
            println!("       ** {reason}");
        }

        println!();
    }

    println!("---");
    println!(
        "Stable: {}/{}, Regressions: {}",
        report.stable,
        report.queries.len(),
        report.regressions,
    );

    if report.regressions > 0 {
        std::process::exit(1);
    }

    Ok(())
}

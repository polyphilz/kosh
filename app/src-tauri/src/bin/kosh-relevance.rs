use std::path::{Path, PathBuf};

use kosh_lib::relevance::{
    benchmark_scale_lexical, enforce_quality_gate, generate_hybrid_vector_fixture,
    generate_scale_corpus, measure_scale_generation, read_json, run_relevance_suite,
    write_pretty_json, write_text, CitationAudit, EmptyRetriever, HybridFixtureRetriever,
    HybridVectorFixture, LexicalFixtureRetriever, RelevanceFixture, ScaleGenerationOptions,
};
use kosh_lib::EmbeddingRuntime;

const DEFAULT_FIXTURE: &str = "fixtures/relevance/v1.json";
const DEFAULT_EMPTY_PREFIX: &str = ".data/relevance/reports/empty-v1";
const DEFAULT_LEXICAL_PREFIX: &str = ".data/relevance/reports/lexical-v1";
const DEFAULT_HYBRID_VECTOR_FIXTURE: &str = "fixtures/relevance/jina-v1-vectors.json";
const DEFAULT_HYBRID_PREFIX: &str = ".data/relevance/reports/hybrid-v1";
const DEFAULT_CITATION_AUDIT: &str = "fixtures/relevance/citation-audit-v1.json";
const DEFAULT_QUALITY_GATE_OUTPUT: &str = ".data/relevance/reports/quality-gate-v1.json";
const DEFAULT_SEMANTIC_DATA_ROOT: &str = ".data/relevance/semantic-runtime";
const DEFAULT_RESOURCE_DIR: &str = "src-tauri/resources";
const DEFAULT_SCALE_OUTPUT: &str = ".data/relevance/scale-v1.json";
const DEFAULT_LEXICAL_SCALE_OUTPUT: &str =
    ".data/relevance/reports/lexical-scale-v1.performance.json";

fn main() {
    if let Err(error) = run(std::env::args().skip(1).collect()) {
        eprintln!("kosh-relevance: {error}");
        std::process::exit(1);
    }
}

fn run(arguments: Vec<String>) -> Result<(), String> {
    let Some(command) = arguments.first().map(String::as_str) else {
        return Err(usage());
    };
    match command {
        "validate" => {
            expect_argument_count(&arguments, 1, 2)?;
            let path = PathBuf::from(arguments.get(1).map_or(DEFAULT_FIXTURE, String::as_str));
            let fixture: RelevanceFixture = read_json(&path).map_err(|error| error.to_string())?;
            fixture.validate().map_err(|error| error.to_string())?;
            println!(
                "valid fixture {}: {} passages, {} queries, digest {}",
                fixture.fixture_id,
                fixture.corpus.len(),
                fixture.queries.len(),
                fixture.digest().map_err(|error| error.to_string())?,
            );
        }
        "empty-report" => {
            expect_argument_count(&arguments, 1, 3)?;
            let fixture_path =
                PathBuf::from(arguments.get(1).map_or(DEFAULT_FIXTURE, String::as_str));
            let output_prefix = PathBuf::from(
                arguments
                    .get(2)
                    .map_or(DEFAULT_EMPTY_PREFIX, String::as_str),
            );
            let fixture: RelevanceFixture =
                read_json(&fixture_path).map_err(|error| error.to_string())?;
            let report = run_relevance_suite(&fixture, &mut EmptyRetriever)
                .map_err(|error| error.to_string())?;
            let json_path = with_suffix(&output_prefix, "json");
            let text_path = with_suffix(&output_prefix, "txt");
            write_pretty_json(&json_path, &report).map_err(|error| error.to_string())?;
            write_text(&text_path, &report.to_text()).map_err(|error| error.to_string())?;
            print!("{}", report.to_text());
            println!(
                "\nwrote {} and {}",
                json_path.display(),
                text_path.display()
            );
        }
        "lexical-report" => {
            expect_argument_count(&arguments, 1, 3)?;
            let fixture_path =
                PathBuf::from(arguments.get(1).map_or(DEFAULT_FIXTURE, String::as_str));
            let output_prefix = PathBuf::from(
                arguments
                    .get(2)
                    .map_or(DEFAULT_LEXICAL_PREFIX, String::as_str),
            );
            let fixture: RelevanceFixture =
                read_json(&fixture_path).map_err(|error| error.to_string())?;
            let report = run_relevance_suite(&fixture, &mut LexicalFixtureRetriever)
                .map_err(|error| error.to_string())?;
            let json_path = with_suffix(&output_prefix, "json");
            let text_path = with_suffix(&output_prefix, "txt");
            write_pretty_json(&json_path, &report).map_err(|error| error.to_string())?;
            write_text(&text_path, &report.to_text()).map_err(|error| error.to_string())?;
            print!("{}", report.to_text());
            println!(
                "\nwrote {} and {}",
                json_path.display(),
                text_path.display()
            );
        }
        "hybrid-vectors" => {
            expect_argument_count(&arguments, 1, 5)?;
            let fixture_path =
                PathBuf::from(arguments.get(1).map_or(DEFAULT_FIXTURE, String::as_str));
            let output = PathBuf::from(
                arguments
                    .get(2)
                    .map_or(DEFAULT_HYBRID_VECTOR_FIXTURE, String::as_str),
            );
            let data_root = PathBuf::from(
                arguments
                    .get(3)
                    .map_or(DEFAULT_SEMANTIC_DATA_ROOT, String::as_str),
            );
            let resource_dir = PathBuf::from(
                arguments
                    .get(4)
                    .map_or(DEFAULT_RESOURCE_DIR, String::as_str),
            );
            let fixture: RelevanceFixture =
                read_json(&fixture_path).map_err(|error| error.to_string())?;
            let runtime = EmbeddingRuntime::new(&data_root, Some(&resource_dir));
            let vectors = (|| {
                runtime.prepare().map_err(|error| error.to_string())?;
                generate_hybrid_vector_fixture(&fixture, &runtime)
                    .map_err(|error| error.to_string())
            })();
            runtime.shutdown();
            let vectors = vectors?;
            write_pretty_json(&output, &vectors).map_err(|error| error.to_string())?;
            println!(
                "wrote {} passage and {} query vectors to {}",
                vectors.passage_embeddings.len(),
                vectors.query_embeddings.len(),
                output.display()
            );
        }
        "hybrid-report" => {
            expect_argument_count(&arguments, 1, 4)?;
            let fixture_path =
                PathBuf::from(arguments.get(1).map_or(DEFAULT_FIXTURE, String::as_str));
            let vector_path = PathBuf::from(
                arguments
                    .get(2)
                    .map_or(DEFAULT_HYBRID_VECTOR_FIXTURE, String::as_str),
            );
            let output_prefix = PathBuf::from(
                arguments
                    .get(3)
                    .map_or(DEFAULT_HYBRID_PREFIX, String::as_str),
            );
            let fixture: RelevanceFixture =
                read_json(&fixture_path).map_err(|error| error.to_string())?;
            let vectors: HybridVectorFixture =
                read_json(&vector_path).map_err(|error| error.to_string())?;
            let mut retriever = HybridFixtureRetriever::new(&fixture, vectors)
                .map_err(|error| error.to_string())?;
            let report =
                run_relevance_suite(&fixture, &mut retriever).map_err(|error| error.to_string())?;
            let json_path = with_suffix(&output_prefix, "json");
            let text_path = with_suffix(&output_prefix, "txt");
            write_pretty_json(&json_path, &report).map_err(|error| error.to_string())?;
            write_text(&text_path, &report.to_text()).map_err(|error| error.to_string())?;
            print!("{}", report.to_text());
            println!(
                "\nwrote {} and {}",
                json_path.display(),
                text_path.display()
            );
        }
        "quality-gate" => {
            expect_argument_count(&arguments, 1, 5)?;
            let fixture_path =
                PathBuf::from(arguments.get(1).map_or(DEFAULT_FIXTURE, String::as_str));
            let vector_path = PathBuf::from(
                arguments
                    .get(2)
                    .map_or(DEFAULT_HYBRID_VECTOR_FIXTURE, String::as_str),
            );
            let audit_path = PathBuf::from(
                arguments
                    .get(3)
                    .map_or(DEFAULT_CITATION_AUDIT, String::as_str),
            );
            let output = PathBuf::from(
                arguments
                    .get(4)
                    .map_or(DEFAULT_QUALITY_GATE_OUTPUT, String::as_str),
            );
            let fixture: RelevanceFixture =
                read_json(&fixture_path).map_err(|error| error.to_string())?;
            let vectors: HybridVectorFixture =
                read_json(&vector_path).map_err(|error| error.to_string())?;
            let audit: CitationAudit = read_json(&audit_path).map_err(|error| error.to_string())?;
            let lexical = run_relevance_suite(&fixture, &mut LexicalFixtureRetriever)
                .map_err(|error| error.to_string())?;
            let mut hybrid_retriever = HybridFixtureRetriever::new(&fixture, vectors)
                .map_err(|error| error.to_string())?;
            let hybrid = run_relevance_suite(&fixture, &mut hybrid_retriever)
                .map_err(|error| error.to_string())?;
            let report = enforce_quality_gate(&fixture, &lexical, &hybrid, &audit)
                .map_err(|error| error.to_string())?;
            write_pretty_json(&output, &report).map_err(|error| error.to_string())?;
            println!(
                "search quality gate passed: {} queries, {} audited citations, hybrid Recall@10 {:.4}, MRR {:.4}, nDCG@10 {:.4}",
                report.hybrid.query_count,
                report.citation_audit_count,
                report.hybrid.recall_at_10,
                report.hybrid.mean_reciprocal_rank,
                report.hybrid.ndcg_at_10,
            );
            println!("wrote {}", output.display());
        }
        "generate-scale" => {
            expect_argument_count(&arguments, 1, 4)?;
            let output = PathBuf::from(
                arguments
                    .get(1)
                    .map_or(DEFAULT_SCALE_OUTPUT, String::as_str),
            );
            let count = arguments
                .get(2)
                .map(|value| {
                    value
                        .parse::<usize>()
                        .map_err(|_| format!("invalid tidbit count {value}"))
                })
                .transpose()?
                .unwrap_or_else(|| ScaleGenerationOptions::default().count);
            let seed = arguments
                .get(3)
                .map(|value| parse_seed(value))
                .transpose()?
                .unwrap_or_else(|| ScaleGenerationOptions::default().seed);
            let (corpus, report) = measure_scale_generation(ScaleGenerationOptions { seed, count })
                .map_err(|error| error.to_string())?;
            let performance_output = output.with_extension("performance.json");
            write_pretty_json(&output, &corpus).map_err(|error| error.to_string())?;
            write_pretty_json(&performance_output, &report).map_err(|error| error.to_string())?;
            println!(
                "generated {} deterministic tidbits at {} (seed {}); performance metadata at {}",
                corpus.tidbits.len(),
                output.display(),
                seed,
                performance_output.display(),
            );
        }
        "benchmark-lexical" => {
            expect_argument_count(&arguments, 1, 5)?;
            let output = PathBuf::from(
                arguments
                    .get(1)
                    .map_or(DEFAULT_LEXICAL_SCALE_OUTPUT, String::as_str),
            );
            let count = parse_optional_usize(
                arguments.get(2),
                ScaleGenerationOptions::default().count,
                "tidbit count",
            )?;
            let query_count = parse_optional_usize(arguments.get(3), 200, "query count")?;
            let seed = arguments
                .get(4)
                .map(|value| parse_seed(value))
                .transpose()?
                .unwrap_or_else(|| ScaleGenerationOptions::default().seed);
            let corpus = generate_scale_corpus(ScaleGenerationOptions { seed, count })
                .map_err(|error| error.to_string())?;
            let report =
                benchmark_scale_lexical(&corpus, query_count).map_err(|error| error.to_string())?;
            write_pretty_json(&output, &report).map_err(|error| error.to_string())?;
            println!(
                "indexed {} passages from {} tidbits in {:.2} ms; {} queries p50 {:.3} ms, p95 {:.3} ms, max {:.3} ms",
                report.passage_count,
                report.tidbit_count,
                report.indexing_duration_ms,
                report.query_count,
                report.query_p50_ms,
                report.query_p95_ms,
                report.query_max_ms,
            );
            println!(
                "interactive p95 budget: {:.1} ms ({})",
                report.interactive_p95_budget_ms,
                if report.interactive_budget_met {
                    "PASS"
                } else {
                    "FAIL"
                }
            );
            println!("wrote {}", output.display());
            if !report.interactive_budget_met {
                return Err("lexical query p95 exceeded the interactive budget".into());
            }
        }
        _ => return Err(usage()),
    }
    Ok(())
}

fn with_suffix(prefix: &Path, suffix: &str) -> PathBuf {
    let mut value = prefix.as_os_str().to_owned();
    value.push(".");
    value.push(suffix);
    PathBuf::from(value)
}

fn parse_seed(value: &str) -> Result<u64, String> {
    value
        .strip_prefix("0x")
        .map_or_else(|| value.parse::<u64>(), |hex| u64::from_str_radix(hex, 16))
        .map_err(|_| format!("invalid seed {value}"))
}

fn parse_optional_usize(
    value: Option<&String>,
    default: usize,
    label: &str,
) -> Result<usize, String> {
    value
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| format!("invalid {label} {value}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn expect_argument_count(
    arguments: &[String],
    minimum: usize,
    maximum: usize,
) -> Result<(), String> {
    if (minimum..=maximum).contains(&arguments.len()) {
        Ok(())
    } else {
        Err(usage())
    }
}

fn usage() -> String {
    "usage:\n  \
     kosh-relevance validate [fixture]\n  \
     kosh-relevance empty-report [fixture] [output-prefix]\n  \
     kosh-relevance lexical-report [fixture] [output-prefix]\n  \
     kosh-relevance hybrid-vectors [fixture] [output] [data-root] [resource-dir]\n  \
     kosh-relevance hybrid-report [fixture] [vectors] [output-prefix]\n  \
     kosh-relevance quality-gate [fixture] [vectors] [citation-audit] [output]\n  \
     kosh-relevance generate-scale [output] [count] [seed]\n  \
     kosh-relevance benchmark-lexical [output] [count] [queries] [seed]"
        .into()
}

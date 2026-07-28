use std::path::{Path, PathBuf};

use kosh_lib::relevance::{
    benchmark_scale_lexical, generate_scale_corpus, measure_scale_generation, read_json,
    run_relevance_suite, write_pretty_json, write_text, EmptyRetriever, LexicalFixtureRetriever,
    RelevanceFixture, ScaleGenerationOptions,
};

const DEFAULT_FIXTURE: &str = "fixtures/relevance/v1.json";
const DEFAULT_EMPTY_PREFIX: &str = ".data/relevance/reports/empty-v1";
const DEFAULT_LEXICAL_PREFIX: &str = ".data/relevance/reports/lexical-v1";
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
                "indexed {} tidbits in {:.2} ms; {} queries p50 {:.3} ms, p95 {:.3} ms, max {:.3} ms",
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
     kosh-relevance generate-scale [output] [count] [seed]\n  \
     kosh-relevance benchmark-lexical [output] [count] [queries] [seed]"
        .into()
}

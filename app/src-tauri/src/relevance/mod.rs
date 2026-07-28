mod fixture;
mod lexical;
mod report;
mod scale;

use std::path::Path;

pub use fixture::{
    EvaluationAttachment, EvaluationLocator, EvaluationPassage, EvaluationQuery, EvaluationRegion,
    EvaluationSource, ExpectedCitation, QueryCategory, RelevanceFixture, RelevanceJudgment,
    RetrievalNeed, SearchMode, FIXTURE_SCHEMA_VERSION,
};
pub use lexical::{
    benchmark_scale_lexical, LexicalFixtureRetriever, LexicalPerformanceReport,
    INTERACTIVE_LEXICAL_P95_BUDGET_MS, LEXICAL_PERFORMANCE_SCHEMA_VERSION,
};
pub use report::{
    run_relevance_suite, EmptyRetriever, QueryMetrics, QueryReport, RelevanceReport, ReportSummary,
    RetrievalHit, RetrievalRequest, Retriever, REPORT_SCHEMA_VERSION,
};
pub use scale::{
    generate_scale_corpus, measure_scale_generation, AttachmentExtractionPlaceholder,
    RuntimeMetadata, ScaleAttachment, ScaleCorpus, ScaleGenerationOptions, ScaleLengthClass,
    ScalePerformanceReport, ScaleSource, ScaleStats, ScaleTidbit, SCALE_SCHEMA_VERSION,
};

#[derive(Debug, thiserror::Error)]
pub enum RelevanceError {
    #[error("could not read relevance data from {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not write relevance data to {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid relevance JSON in {path}: {source}")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid relevance fixture: {0}")]
    InvalidFixture(String),
    #[error("retriever failed for query {query_id}: {message}")]
    Retrieval { query_id: String, message: String },
    #[error("lexical benchmark failed: {0}")]
    LexicalBenchmark(String),
}

pub type Result<T> = std::result::Result<T, RelevanceError>;

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = std::fs::read(path).map_err(|source| RelevanceError::Read {
        path: path.display().to_string(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| RelevanceError::Json {
        path: path.display().to_string(),
        source,
    })
}

pub fn write_pretty_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|source| RelevanceError::Json {
        path: path.display().to_string(),
        source,
    })?;
    bytes.push(b'\n');
    write_bytes(path, &bytes)
}

pub fn write_text(path: &Path, value: &str) -> Result<()> {
    write_bytes(path, value.as_bytes())
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| RelevanceError::Write {
            path: parent.display().to_string(),
            source,
        })?;
    }
    std::fs::write(path, bytes).map_err(|source| RelevanceError::Write {
        path: path.display().to_string(),
        source,
    })
}

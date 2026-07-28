use std::{collections::BTreeMap, time::Instant};

use crate::database::search::{
    candidate_limit, parse_lexical_query, rank_lexical_documents, LexicalDocument,
    LexicalSearchMode, SearchField,
};
use rusqlite::{params, Connection, Statement};
use serde::{Deserialize, Serialize};

use super::{
    EvaluationLocator, EvaluationPassage, RelevanceError, RetrievalHit, RetrievalRequest,
    Retriever, RuntimeMetadata, ScaleCorpus, SearchMode,
};

pub const LEXICAL_PERFORMANCE_SCHEMA_VERSION: u32 = 1;
pub const INTERACTIVE_LEXICAL_P95_BUDGET_MS: f64 = 100.0;
const BENCHMARK_RESULT_LIMIT: u32 = 20;

pub struct LexicalFixtureRetriever;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LexicalPerformanceReport {
    pub schema_version: u32,
    pub workload: String,
    pub generator_version: String,
    pub seed: String,
    pub tidbit_count: u32,
    pub query_count: u32,
    pub result_limit: u32,
    pub indexing_duration_ms: f64,
    pub query_p50_ms: f64,
    pub query_p95_ms: f64,
    pub query_max_ms: f64,
    pub interactive_p95_budget_ms: f64,
    pub interactive_budget_met: bool,
    pub runtime: RuntimeMetadata,
}

impl Retriever for LexicalFixtureRetriever {
    fn name(&self) -> &str {
        "kosh-lexical-v1"
    }

    fn retrieve(
        &mut self,
        request: &RetrievalRequest,
        corpus: &[EvaluationPassage],
        limit: usize,
    ) -> std::result::Result<Vec<RetrievalHit>, String> {
        let mode = match request.search_mode {
            SearchMode::Default => LexicalSearchMode::Default,
            SearchMode::Exact => LexicalSearchMode::Exact,
        };
        let Some(query) =
            parse_lexical_query(&request.text, mode).map_err(|error| error.to_string())?
        else {
            return Ok(Vec::new());
        };
        let documents = corpus
            .iter()
            .map(|passage| LexicalDocument {
                passage_id: passage.id.clone(),
                updated_at_ms: 0,
                fields: fixture_fields(passage),
                word_rank: None,
                trigram_rank: None,
            })
            .collect();
        let passages = corpus
            .iter()
            .map(|passage| (passage.id.as_str(), passage))
            .collect::<BTreeMap<_, _>>();

        rank_lexical_documents(&query, documents, limit)
            .into_iter()
            .map(|ranked| {
                let passage = passages
                    .get(ranked.passage_id.as_str())
                    .ok_or_else(|| format!("ranked unknown passage {}", ranked.passage_id))?;
                Ok(RetrievalHit {
                    passage_id: ranked.passage_id,
                    score: ranked.score,
                    locator: passage.locator.clone(),
                    matched_fields: ranked
                        .matched_fields
                        .into_iter()
                        .map(|field| field.label().into())
                        .collect(),
                })
            })
            .collect()
    }
}

fn fixture_fields(passage: &EvaluationPassage) -> BTreeMap<SearchField, String> {
    let authored = matches!(passage.locator, EvaluationLocator::MarkdownBlocks { .. });
    [
        (
            SearchField::Title,
            passage.title.clone().unwrap_or_default(),
        ),
        (
            SearchField::HeadingContext,
            passage.heading_context.join("\n"),
        ),
        (
            SearchField::Body,
            if authored {
                passage.content.clone()
            } else {
                String::new()
            },
        ),
        (
            SearchField::SourceLabel,
            passage
                .sources
                .iter()
                .map(|source| source.label.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        (
            SearchField::SourceDomain,
            passage
                .sources
                .iter()
                .map(|source| source.domain.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        (
            SearchField::AttachmentName,
            passage
                .attachments
                .iter()
                .map(|attachment| attachment.filename.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        (
            SearchField::ExtractedText,
            if authored {
                String::new()
            } else {
                passage.content.clone()
            },
        ),
    ]
    .into_iter()
    .collect()
}

pub fn benchmark_scale_lexical(
    corpus: &ScaleCorpus,
    query_count: usize,
) -> super::Result<LexicalPerformanceReport> {
    if corpus.tidbits.is_empty() {
        return Err(RelevanceError::LexicalBenchmark(
            "scale corpus must contain tidbits".into(),
        ));
    }
    if query_count == 0 || query_count > u32::MAX as usize {
        return Err(RelevanceError::LexicalBenchmark(
            "query count must fit in a positive u32".into(),
        ));
    }

    let mut connection =
        Connection::open_in_memory().map_err(|error| benchmark_error("open index", error))?;
    connection
        .execute_batch(
            "CREATE TABLE benchmark_document(
                rowid INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                heading_context TEXT NOT NULL,
                body TEXT NOT NULL,
                source_labels TEXT NOT NULL,
                source_domains TEXT NOT NULL,
                attachment_names TEXT NOT NULL,
                extracted_text TEXT NOT NULL
             ) STRICT;
             CREATE VIRTUAL TABLE benchmark_word USING fts5(
                title,
                heading_context,
                body,
                source_labels,
                source_domains,
                attachment_names,
                extracted_text,
                tokenize = 'unicode61 remove_diacritics 2 tokenchars ''_'''
             );
             CREATE VIRTUAL TABLE benchmark_trigram USING fts5(
                title,
                heading_context,
                body,
                source_labels,
                source_domains,
                attachment_names,
                extracted_text,
                tokenize = 'trigram'
             );",
        )
        .map_err(|error| benchmark_error("create indexes", error))?;

    let indexing_started = Instant::now();
    let transaction = connection
        .transaction()
        .map_err(|error| benchmark_error("start index transaction", error))?;
    {
        let mut insert_document = transaction
            .prepare(
                "INSERT INTO benchmark_document(
                    rowid, title, heading_context, body, source_labels,
                    source_domains, attachment_names, extracted_text
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .map_err(|error| benchmark_error("prepare document insert", error))?;
        let mut insert_word = transaction
            .prepare(
                "INSERT INTO benchmark_word(
                    rowid, title, heading_context, body, source_labels,
                    source_domains, attachment_names, extracted_text
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .map_err(|error| benchmark_error("prepare word insert", error))?;
        let mut insert_trigram = transaction
            .prepare(
                "INSERT INTO benchmark_trigram(
                    rowid, title, heading_context, body, source_labels,
                    source_domains, attachment_names, extracted_text
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .map_err(|error| benchmark_error("prepare trigram insert", error))?;
        for (index, tidbit) in corpus.tidbits.iter().enumerate() {
            let rowid = i64::try_from(index + 1)
                .map_err(|_| RelevanceError::LexicalBenchmark("rowid overflow".into()))?;
            let source_labels = tidbit
                .sources
                .iter()
                .map(|source| source.label.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let source_domains = tidbit
                .sources
                .iter()
                .map(|source| source.url.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let attachment_names = tidbit
                .attachments
                .iter()
                .map(|attachment| attachment.filename.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            insert_document
                .execute(params![
                    rowid,
                    tidbit.title.as_deref().unwrap_or_default(),
                    "",
                    tidbit.body_markdown,
                    source_labels,
                    source_domains,
                    attachment_names,
                    ""
                ])
                .map_err(|error| benchmark_error("insert source document", error))?;
            insert_word
                .execute(params![
                    rowid,
                    tidbit.title.as_deref().unwrap_or_default(),
                    "",
                    tidbit.body_markdown,
                    source_labels,
                    source_domains,
                    attachment_names,
                    ""
                ])
                .map_err(|error| benchmark_error("insert word document", error))?;
            insert_trigram
                .execute(params![
                    rowid,
                    tidbit.title.as_deref().unwrap_or_default(),
                    "",
                    tidbit.body_markdown,
                    source_labels,
                    source_domains,
                    attachment_names,
                    ""
                ])
                .map_err(|error| benchmark_error("insert trigram document", error))?;
        }
    }
    transaction
        .commit()
        .map_err(|error| benchmark_error("commit indexes", error))?;
    let indexing_duration_ms = indexing_started.elapsed().as_secs_f64() * 1_000.0;

    const BROAD_QUERIES: [&str; 8] = [
        "retrieval",
        "gardening",
        "distributed systems",
        "thermodynamics",
        "music",
        "language",
        "databases",
        "cooking",
    ];
    let queries = (0..query_count)
        .map(|ordinal| {
            let index = ordinal.wrapping_mul(9_973) % corpus.tidbits.len();
            match ordinal % 4 {
                0 => format!("kosh_{:05}", (index * 31) % 100_000),
                1 => BROAD_QUERIES[(ordinal / 4) % BROAD_QUERIES.len()].into(),
                2 => "Observation".into(),
                _ => "Retrieval note".into(),
            }
        })
        .collect::<Vec<_>>();
    let mut word_statement = connection
        .prepare(
            "SELECT rowid
             FROM benchmark_word
             WHERE benchmark_word MATCH ?1
             ORDER BY bm25(benchmark_word, 8.0, 6.0, 3.0, 4.5, 5.0, 5.0, 2.5)
             LIMIT ?2",
        )
        .map_err(|error| benchmark_error("prepare word query", error))?;
    let mut trigram_statement = connection
        .prepare(
            "SELECT rowid
             FROM benchmark_trigram
             WHERE benchmark_trigram MATCH ?1
             ORDER BY bm25(benchmark_trigram, 8.0, 6.0, 3.0, 4.5, 5.0, 5.0, 2.5)
             LIMIT ?2",
        )
        .map_err(|error| benchmark_error("prepare trigram query", error))?;
    let mut document_statement = connection
        .prepare(
            "SELECT
                title, heading_context, body, source_labels, source_domains,
                attachment_names, extracted_text
             FROM benchmark_document
             WHERE rowid = ?1",
        )
        .map_err(|error| benchmark_error("prepare document hydration", error))?;
    let mut citation_statement = connection
        .prepare(
            "SELECT rowid, title, body
             FROM benchmark_document
             WHERE rowid = ?1",
        )
        .map_err(|error| benchmark_error("prepare citation resolution", error))?;
    for query in queries.iter().take(10) {
        run_benchmark_query(
            query,
            &mut word_statement,
            &mut trigram_statement,
            &mut document_statement,
            &mut citation_statement,
        )?;
    }
    let mut durations_ms = Vec::with_capacity(queries.len());
    for query in &queries {
        let started = Instant::now();
        run_benchmark_query(
            query,
            &mut word_statement,
            &mut trigram_statement,
            &mut document_statement,
            &mut citation_statement,
        )?;
        durations_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    durations_ms.sort_by(|left, right| left.total_cmp(right));
    let query_p50_ms = percentile(&durations_ms, 0.50);
    let query_p95_ms = percentile(&durations_ms, 0.95);
    let query_max_ms = durations_ms.last().copied().unwrap_or_default();

    Ok(LexicalPerformanceReport {
        schema_version: LEXICAL_PERFORMANCE_SCHEMA_VERSION,
        workload: "lexical-search-10k".into(),
        generator_version: corpus.generator_version.clone(),
        seed: corpus.seed.clone(),
        tidbit_count: corpus.stats.tidbit_count,
        query_count: u32::try_from(query_count).expect("validated query count"),
        result_limit: BENCHMARK_RESULT_LIMIT,
        indexing_duration_ms,
        query_p50_ms,
        query_p95_ms,
        query_max_ms,
        interactive_p95_budget_ms: INTERACTIVE_LEXICAL_P95_BUDGET_MS,
        interactive_budget_met: query_p95_ms <= INTERACTIVE_LEXICAL_P95_BUDGET_MS,
        runtime: RuntimeMetadata::capture(),
    })
}

fn run_benchmark_query(
    query: &str,
    word_statement: &mut Statement<'_>,
    trigram_statement: &mut Statement<'_>,
    document_statement: &mut Statement<'_>,
    citation_statement: &mut Statement<'_>,
) -> super::Result<()> {
    let parsed = parse_lexical_query(query, LexicalSearchMode::Default)
        .map_err(|error| RelevanceError::LexicalBenchmark(error.to_string()))?
        .ok_or_else(|| RelevanceError::LexicalBenchmark("generated empty query".into()))?;
    let mut candidates = BTreeMap::<i64, (Option<usize>, Option<usize>)>::new();
    let candidate_limit = candidate_limit(BENCHMARK_RESULT_LIMIT);
    if let Some(word_query) = parsed.word_match_query() {
        for (position, rowid) in query_benchmark_rows(word_statement, &word_query, candidate_limit)?
            .into_iter()
            .enumerate()
        {
            candidates.entry(rowid).or_default().0 = Some(position + 1);
        }
    }
    if let Some(trigram_query) = parsed.trigram_match_query() {
        for (position, rowid) in
            query_benchmark_rows(trigram_statement, &trigram_query, candidate_limit)?
                .into_iter()
                .enumerate()
        {
            candidates.entry(rowid).or_default().1 = Some(position + 1);
        }
    }
    let documents = candidates
        .into_iter()
        .map(|(rowid, (word_rank, trigram_rank))| {
            document_statement
                .query_row(params![rowid], |row| {
                    Ok(LexicalDocument {
                        passage_id: rowid.to_string(),
                        updated_at_ms: 0,
                        fields: [
                            (SearchField::Title, row.get::<_, String>(0)?),
                            (SearchField::HeadingContext, row.get::<_, String>(1)?),
                            (SearchField::Body, row.get::<_, String>(2)?),
                            (SearchField::SourceLabel, row.get::<_, String>(3)?),
                            (SearchField::SourceDomain, row.get::<_, String>(4)?),
                            (SearchField::AttachmentName, row.get::<_, String>(5)?),
                            (SearchField::ExtractedText, row.get::<_, String>(6)?),
                        ]
                        .into_iter()
                        .collect(),
                        word_rank,
                        trigram_rank,
                    })
                })
                .map_err(|error| benchmark_error("hydrate query candidate", error))
        })
        .collect::<super::Result<Vec<_>>>()?;
    let ranked = rank_lexical_documents(&parsed, documents, BENCHMARK_RESULT_LIMIT as usize);
    for result in ranked {
        let rowid = result
            .passage_id
            .parse::<i64>()
            .map_err(|error| benchmark_error("parse citation rowid", error))?;
        citation_statement
            .query_row(params![rowid], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|error| benchmark_error("resolve citation candidate", error))?;
    }
    Ok(())
}

fn query_benchmark_rows(
    statement: &mut Statement<'_>,
    query: &str,
    limit: u32,
) -> super::Result<Vec<i64>> {
    let rows = statement
        .query_map(params![query, limit], |row| row.get::<_, i64>(0))
        .map_err(|error| benchmark_error("run query", error))?;
    rows.map(|row| row.map_err(|error| benchmark_error("read query result", error)))
        .collect()
}

fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    let index = ((sorted.len() as f64 * percentile).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len().saturating_sub(1));
    sorted.get(index).copied().unwrap_or_default()
}

fn benchmark_error(context: &str, error: impl std::fmt::Display) -> RelevanceError {
    RelevanceError::LexicalBenchmark(format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{benchmark_scale_lexical, LexicalFixtureRetriever};
    use crate::relevance::{
        generate_scale_corpus, run_relevance_suite, RelevanceFixture, ScaleGenerationOptions,
    };

    #[test]
    fn lexical_fixture_baseline_has_trusted_citations_and_core_query_coverage() {
        let fixture: RelevanceFixture =
            serde_json::from_str(include_str!("../../../fixtures/relevance/v1.json"))
                .expect("checked-in fixture");
        let report = run_relevance_suite(&fixture, &mut LexicalFixtureRetriever)
            .expect("lexical fixture report");

        assert_eq!(
            report.summary.citation_locator_accuracy,
            report.summary.recall_at_10
        );
        assert_eq!(report.summary.exact_phrase_success, Some(1.0));
        assert_eq!(report.summary.forbidden_hits_at_10, 0);
        assert!(report.summary.recall_at_10 >= 0.9);
        let mut generated_json =
            serde_json::to_string_pretty(&report).expect("serialize lexical report");
        generated_json.push('\n');
        assert_eq!(
            generated_json,
            include_str!("../../../fixtures/relevance/reports/lexical-v1.json")
        );
        assert_eq!(
            report.to_text(),
            include_str!("../../../fixtures/relevance/reports/lexical-v1.txt")
        );
    }

    #[test]
    fn scale_benchmark_records_observational_latency_without_a_brittle_assertion() {
        let corpus = generate_scale_corpus(ScaleGenerationOptions {
            seed: 42,
            count: 256,
        })
        .expect("scale corpus");
        let report = benchmark_scale_lexical(&corpus, 20).expect("lexical benchmark");

        assert_eq!(report.tidbit_count, 256);
        assert_eq!(report.query_count, 20);
        assert!(report.indexing_duration_ms >= 0.0);
        assert!(report.query_p50_ms >= 0.0);
        assert!(report.query_p95_ms >= report.query_p50_ms);
        assert!(report.query_max_ms >= report.query_p95_ms);
        assert!(report.interactive_p95_budget_ms > 0.0);
    }
}

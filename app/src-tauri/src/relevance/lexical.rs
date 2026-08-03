use std::{collections::BTreeMap, path::PathBuf, time::Instant};

use crate::database::search::{
    candidate_limit, diversify_ranked, normalize_for_search, parse_lexical_query,
    rank_lexical_documents, short_grams_for_search, trigram_candidate_limit, LexicalDocument,
    LexicalSearchMode, RankedLexicalDocument, SearchDiversityKey, SearchEvidenceKind, SearchField,
    FTS_BM25_WEIGHTS,
};
use crate::database::{
    Database, DatabaseClient, DatabasePaths, LexicalBenchmarkAttachmentWrite, SearchPassagesInput,
    SourceDraft, TidbitDraft,
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    EvaluationLocator, EvaluationOwnerKind, EvaluationPassage, RelevanceError, RetrievalHit,
    RetrievalRequest, Retriever, RuntimeMetadata, ScaleCorpus, SearchMode,
};

pub const LEXICAL_PERFORMANCE_SCHEMA_VERSION: u32 = 4;
pub const INTERACTIVE_LEXICAL_P95_BUDGET_MS: f64 = 100.0;
const BENCHMARK_RESULT_LIMIT: u32 = 20;

pub struct LexicalFixtureRetriever;

type FixtureRanks = (Option<usize>, Option<usize>, Option<usize>);
type FixtureCandidates = BTreeMap<usize, FixtureRanks>;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LexicalPerformanceReport {
    pub schema_version: u32,
    pub source_revision: String,
    pub workload: String,
    pub generator_version: String,
    pub seed: String,
    pub tidbit_count: u32,
    pub passage_count: u32,
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
        let lexical_candidate_limit =
            candidate_limit(u32::try_from(limit).unwrap_or(u32::MAX)) as usize;
        let candidates = fixture_candidate_ranks(corpus, &query, limit)?;
        let documents = candidates
            .into_iter()
            .map(|(index, (word_rank, trigram_rank, short_rank))| {
                let passage = &corpus[index];
                LexicalDocument {
                    passage_id: passage.id.clone(),
                    updated_at_ms: 0,
                    evidence_kind: fixture_evidence_kind(passage),
                    fields: fixture_fields(passage),
                    word_rank,
                    trigram_rank,
                    short_rank,
                }
            })
            .collect();
        hydrate_fixture_hits(
            rank_lexical_documents(&query, documents, lexical_candidate_limit),
            corpus,
            limit,
            false,
        )
    }
}

pub(crate) fn fixture_candidate_ranks(
    corpus: &[EvaluationPassage],
    query: &crate::database::search::ParsedLexicalQuery,
    result_limit: usize,
) -> std::result::Result<FixtureCandidates, String> {
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "CREATE VIRTUAL TABLE fixture_word USING fts5(
                heading_context, body, source_labels, source_domains,
                attachment_names, extracted_text,
                tokenize = 'unicode61 remove_diacritics 2 tokenchars ''_'''
             );
             CREATE VIRTUAL TABLE fixture_trigram USING fts5(
                heading_context, body, source_labels, source_domains,
                attachment_names, extracted_text,
                tokenize = 'trigram'
             );
             CREATE VIRTUAL TABLE fixture_short USING fts5(
                heading_context, body, source_labels, source_domains,
                attachment_names, extracted_text,
                tokenize = 'unicode61'
             );",
        )
        .map_err(|error| error.to_string())?;
    for (index, passage) in corpus.iter().enumerate() {
        let rowid = i64::try_from(index + 1).map_err(|error| error.to_string())?;
        let fields = fixture_fields(passage);
        let values = params![
            rowid,
            normalize_for_search(&fields[&SearchField::HeadingContext]),
            normalize_for_search(&fields[&SearchField::Body]),
            normalize_for_search(&fields[&SearchField::SourceLabel]),
            normalize_for_search(&fields[&SearchField::SourceDomain]),
            normalize_for_search(&fields[&SearchField::AttachmentName]),
            normalize_for_search(&fields[&SearchField::ExtractedText]),
        ];
        connection
            .execute(
                "INSERT INTO fixture_word(
                    rowid, heading_context, body, source_labels,
                    source_domains, attachment_names, extracted_text
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                values,
            )
            .map_err(|error| error.to_string())?;
        let short_values = params![
            rowid,
            short_grams_for_search(&fields[&SearchField::HeadingContext]),
            short_grams_for_search(&fields[&SearchField::Body]),
            short_grams_for_search(&fields[&SearchField::SourceLabel]),
            short_grams_for_search(&fields[&SearchField::SourceDomain]),
            short_grams_for_search(&fields[&SearchField::AttachmentName]),
            short_grams_for_search(&fields[&SearchField::ExtractedText]),
        ];
        connection
            .execute(
                "INSERT INTO fixture_short(
                    rowid, heading_context, body, source_labels,
                    source_domains, attachment_names, extracted_text
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                short_values,
            )
            .map_err(|error| error.to_string())?;
        connection
            .execute(
                "INSERT INTO fixture_trigram(
                    rowid, heading_context, body, source_labels,
                    source_domains, attachment_names, extracted_text
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                values,
            )
            .map_err(|error| error.to_string())?;
    }

    let result_limit = u32::try_from(result_limit).unwrap_or(u32::MAX);
    let limit = candidate_limit(result_limit);
    let mut candidates = BTreeMap::new();
    if let Some(word_query) = query.word_match_query() {
        install_fixture_ranks(
            &connection,
            "fixture_word",
            &word_query,
            limit,
            &mut candidates,
            0,
        )?;
    }
    if let Some(trigram_query) = query.trigram_match_query() {
        install_fixture_ranks(
            &connection,
            "fixture_trigram",
            &trigram_query,
            trigram_candidate_limit(result_limit),
            &mut candidates,
            1,
        )?;
    }
    if let Some(short_query) = query.short_match_query() {
        install_fixture_ranks(
            &connection,
            "fixture_short",
            &short_query,
            limit,
            &mut candidates,
            2,
        )?;
    }
    Ok(candidates)
}

fn install_fixture_ranks(
    connection: &Connection,
    index: &'static str,
    query: &str,
    limit: u32,
    candidates: &mut FixtureCandidates,
    rank_slot: usize,
) -> std::result::Result<(), String> {
    let sql = format!(
        "SELECT rowid
         FROM {index}
         WHERE {index} MATCH ?1
           AND {index}.rank MATCH 'bm25({FTS_BM25_WEIGHTS})'
         ORDER BY {index}.rank
         LIMIT ?2"
    );
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![query, limit], |row| row.get::<_, i64>(0))
        .map_err(|error| error.to_string())?;
    for (position, rowid) in rows.enumerate() {
        let index = usize::try_from(rowid.map_err(|error| error.to_string())? - 1)
            .map_err(|error| error.to_string())?;
        let ranks = candidates.entry(index).or_default();
        match rank_slot {
            0 => ranks.0 = Some(position + 1),
            1 => ranks.1 = Some(position + 1),
            2 => ranks.2 = Some(position + 1),
            _ => return Err(format!("unknown fixture rank slot {rank_slot}")),
        }
    }
    Ok(())
}

pub(crate) fn fixture_fields(passage: &EvaluationPassage) -> BTreeMap<SearchField, String> {
    let authored = passage.owner_kind == EvaluationOwnerKind::Author;
    [
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

pub(crate) fn fixture_evidence_kind(passage: &EvaluationPassage) -> SearchEvidenceKind {
    match passage.locator {
        EvaluationLocator::MarkdownBlocks { .. } => SearchEvidenceKind::Author,
        EvaluationLocator::OcrRegion { .. } => SearchEvidenceKind::Ocr,
        EvaluationLocator::PdfPage { .. } => SearchEvidenceKind::Pdf,
        EvaluationLocator::TextLines { .. } => SearchEvidenceKind::Text,
    }
}

pub(crate) fn hydrate_fixture_hits(
    ranked: Vec<RankedLexicalDocument>,
    corpus: &[EvaluationPassage],
    limit: usize,
    collapse_tidbits: bool,
) -> std::result::Result<Vec<RetrievalHit>, String> {
    struct FixtureCandidate<'a> {
        ranked: RankedLexicalDocument,
        passage: &'a EvaluationPassage,
    }

    let passages = corpus
        .iter()
        .map(|passage| (passage.id.as_str(), passage))
        .collect::<BTreeMap<_, _>>();
    let mut seen_tidbit_locators = BTreeMap::<&str, Vec<&EvaluationLocator>>::new();
    let mut candidates = Vec::with_capacity(ranked.len().min(limit.saturating_mul(4)));
    for ranked in ranked {
        let passage = passages
            .get(ranked.passage_id.as_str())
            .copied()
            .ok_or_else(|| format!("ranked unknown passage {}", ranked.passage_id))?;
        if collapse_tidbits {
            if let Some(tidbit_id) = passage.tidbit_id.as_deref() {
                let locators = seen_tidbit_locators.entry(tidbit_id).or_default();
                let overlaps = locators
                    .iter()
                    .any(|locator| evaluation_locators_overlap(locator, &passage.locator));
                locators.push(&passage.locator);
                if overlaps {
                    continue;
                }
            }
        }
        candidates.push(FixtureCandidate { ranked, passage });
    }

    Ok(
        diversify_ranked(candidates, limit, |candidate| SearchDiversityKey {
            attachment_id: candidate.passage.evidence_attachment_id.clone(),
            page: match &candidate.passage.locator {
                EvaluationLocator::PdfPage { page } => Some(*page),
                EvaluationLocator::OcrRegion { page, .. } => *page,
                EvaluationLocator::MarkdownBlocks { .. } | EvaluationLocator::TextLines { .. } => {
                    None
                }
            },
        })
        .into_iter()
        .map(|candidate| RetrievalHit {
            passage_id: candidate.ranked.passage_id,
            score: candidate.ranked.score,
            locator: candidate.passage.locator.clone(),
            matched_fields: candidate
                .ranked
                .matched_fields
                .into_iter()
                .map(|field| field.label().into())
                .collect(),
        })
        .collect(),
    )
}

fn evaluation_locators_overlap(left: &EvaluationLocator, right: &EvaluationLocator) -> bool {
    let (
        EvaluationLocator::MarkdownBlocks {
            start_block: left_start_block,
            end_block: left_end_block,
            source_start_byte: left_start_byte,
            source_end_byte: left_end_byte,
            start_char: left_start_char,
            end_char: left_end_char,
            start_line: left_start_line,
            end_line: left_end_line,
        },
        EvaluationLocator::MarkdownBlocks {
            start_block: right_start_block,
            end_block: right_end_block,
            source_start_byte: right_start_byte,
            source_end_byte: right_end_byte,
            start_char: right_start_char,
            end_char: right_end_char,
            start_line: right_start_line,
            end_line: right_end_line,
        },
    ) = (left, right)
    else {
        return false;
    };
    if let (Some(left_start), Some(left_end), Some(right_start), Some(right_end)) = (
        left_start_byte,
        left_end_byte,
        right_start_byte,
        right_end_byte,
    ) {
        return half_open_ranges_overlap(*left_start, *left_end, *right_start, *right_end);
    }
    if let (Some(left_start), Some(left_end), Some(right_start), Some(right_end)) = (
        left_start_char,
        left_end_char,
        right_start_char,
        right_end_char,
    ) {
        return half_open_ranges_overlap(*left_start, *left_end, *right_start, *right_end);
    }
    if let (Some(left_start), Some(left_end), Some(right_start), Some(right_end)) = (
        left_start_line,
        left_end_line,
        right_start_line,
        right_end_line,
    ) {
        return ranges_overlap(*left_start, *left_end, *right_start, *right_end);
    }
    ranges_overlap(
        *left_start_block,
        *left_end_block,
        *right_start_block,
        *right_end_block,
    )
}

fn ranges_overlap<T: Ord>(left_start: T, left_end: T, right_start: T, right_end: T) -> bool {
    left_start <= right_end && right_start <= left_end
}

fn half_open_ranges_overlap<T: Ord>(
    left_start: T,
    left_end: T,
    right_start: T,
    right_end: T,
) -> bool {
    left_start < right_end && right_start < left_end
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

    let library = TemporaryBenchmarkLibrary::create()?;
    let database = Database::initialize(DatabasePaths::new(&library.root))
        .map_err(|error| benchmark_error("initialize production database", error))?;
    let client = database.client();
    let indexing_started = Instant::now();
    for tidbit in &corpus.tidbits {
        let now_ms = i64::try_from(tidbit.created_at_ms)
            .map_err(|error| benchmark_error("convert tidbit timestamp", error))?;
        let input = TidbitDraft {
            body_markdown: tidbit.body_markdown.clone(),
            sources: tidbit
                .sources
                .iter()
                .map(|source| SourceDraft {
                    label: Some(source.label.clone()),
                    url: Some(source.url.clone()),
                })
                .collect(),
        };
        let source_ids = input
            .sources
            .iter()
            .map(|_| Uuid::now_v7().to_string())
            .collect();
        client
            .create_tidbit_with_ids(
                input,
                now_ms,
                tidbit.id.clone(),
                tidbit.revision_id.clone(),
                source_ids,
            )
            .map_err(|error| benchmark_error("index production tidbit", error))?;
    }
    let attachment_writes = corpus
        .tidbits
        .iter()
        .flat_map(|tidbit| {
            tidbit.attachments.iter().map(|attachment| {
                Ok(LexicalBenchmarkAttachmentWrite {
                    revision_id: tidbit.revision_id.clone(),
                    attachment_id: attachment.id.clone(),
                    created_at_ms: i64::try_from(tidbit.created_at_ms)
                        .map_err(|error| benchmark_error("convert attachment timestamp", error))?,
                    display_filename: attachment.filename.clone(),
                    media_type: attachment.media_type.clone(),
                    byte_length: i64::try_from(attachment.byte_length)
                        .map_err(|error| benchmark_error("convert attachment length", error))?,
                })
            })
        })
        .collect::<super::Result<Vec<_>>>()?;
    client
        .install_lexical_benchmark_attachments(attachment_writes)
        .map_err(|error| benchmark_error("index production attachment metadata", error))?;
    let indexing_duration_ms = indexing_started.elapsed().as_secs_f64() * 1_000.0;
    let passage_count = u32::try_from(
        database
            .open_main_read_only()
            .and_then(|connection| {
                connection
                    .query_row(
                        "SELECT count(*)
                         FROM passage_search_document
                         WHERE tidbit_id IS NOT NULL",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(Into::into)
            })
            .map_err(|error| benchmark_error("count production passages", error))?,
    )
    .map_err(|error| benchmark_error("convert production passage count", error))?;

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
    for query in queries.iter().take(10) {
        run_benchmark_query(&client, query)?;
    }
    let mut durations_ms = Vec::with_capacity(queries.len());
    for query in &queries {
        let started = Instant::now();
        run_benchmark_query(&client, query)?;
        durations_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    durations_ms.sort_by(|left, right| left.total_cmp(right));
    let query_p50_ms = percentile(&durations_ms, 0.50);
    let query_p95_ms = percentile(&durations_ms, 0.95);
    let query_max_ms = durations_ms.last().copied().unwrap_or_default();
    database
        .shutdown()
        .map_err(|error| benchmark_error("shut down production database", error))?;

    Ok(LexicalPerformanceReport {
        schema_version: LEXICAL_PERFORMANCE_SCHEMA_VERSION,
        source_revision: env!("KOSH_BUILD_GIT_SHA").into(),
        workload: "lexical-search-10k".into(),
        generator_version: corpus.generator_version.clone(),
        seed: corpus.seed.clone(),
        tidbit_count: corpus.stats.tidbit_count,
        passage_count,
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

fn run_benchmark_query(client: &DatabaseClient, query: &str) -> super::Result<()> {
    client
        .search_passages(SearchPassagesInput {
            query: query.into(),
            mode: LexicalSearchMode::Default,
            limit: BENCHMARK_RESULT_LIMIT,
        })
        .map(|_| ())
        .map_err(|error| benchmark_error("run production search", error))
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

struct TemporaryBenchmarkLibrary {
    root: PathBuf,
}

impl TemporaryBenchmarkLibrary {
    fn create() -> super::Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "kosh-lexical-benchmark-{}",
            Uuid::now_v7().as_simple()
        ));
        std::fs::create_dir(&root)
            .map_err(|error| benchmark_error("create temporary production library", error))?;
        Ok(Self { root })
    }
}

impl Drop for TemporaryBenchmarkLibrary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::{benchmark_scale_lexical, LexicalFixtureRetriever};
    use crate::relevance::{
        generate_scale_corpus, run_relevance_suite, RelevanceFixture, RetrievalRequest, Retriever,
        ScaleGenerationOptions, SearchMode,
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
    fn lexical_fixture_diversifies_beyond_the_display_limit() {
        let fixture: RelevanceFixture =
            serde_json::from_str(include_str!("../../../fixtures/relevance/v1.json"))
                .expect("checked-in fixture");
        let hits = LexicalFixtureRetriever
            .retrieve(
                &RetrievalRequest {
                    text: "vector clock reconciliation causal order".into(),
                    search_mode: SearchMode::Default,
                },
                &fixture.corpus,
                4,
            )
            .expect("diverse lexical hits");

        assert_eq!(hits[0].passage_id, "passage-authored-vector-clock");
        assert!(hits
            .iter()
            .any(|hit| hit.passage_id == "passage-pdf-conference-clock"));
        assert!(
            hits.iter()
                .filter(|hit| hit.passage_id.starts_with("passage-pdf-distributed-"))
                .count()
                <= 2
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
        assert_eq!(report.source_revision, env!("KOSH_BUILD_GIT_SHA"));
        assert!(report.passage_count >= report.tidbit_count);
        assert_eq!(report.query_count, 20);
        assert!(report.indexing_duration_ms >= 0.0);
        assert!(report.query_p50_ms >= 0.0);
        assert!(report.query_p95_ms >= report.query_p50_ms);
        assert!(report.query_max_ms >= report.query_p95_ms);
        assert!(report.interactive_p95_budget_ms > 0.0);
    }
}

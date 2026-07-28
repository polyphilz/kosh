use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::{
    EvaluationLocator, EvaluationPassage, EvaluationQuery, QueryCategory, RelevanceError,
    RelevanceFixture, Result,
};

pub const REPORT_SCHEMA_VERSION: u32 = 1;
const REPORT_LIMIT: usize = 10;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetrievalHit {
    pub passage_id: String,
    pub score: f64,
    pub locator: EvaluationLocator,
    #[serde(default)]
    pub matched_fields: Vec<String>,
}

pub trait Retriever {
    fn name(&self) -> &str;

    fn retrieve(
        &mut self,
        query: &EvaluationQuery,
        corpus: &[EvaluationPassage],
        limit: usize,
    ) -> std::result::Result<Vec<RetrievalHit>, String>;
}

#[derive(Default)]
pub struct EmptyRetriever;

impl Retriever for EmptyRetriever {
    fn name(&self) -> &str {
        "empty-v1"
    }

    fn retrieve(
        &mut self,
        _query: &EvaluationQuery,
        _corpus: &[EvaluationPassage],
        _limit: usize,
    ) -> std::result::Result<Vec<RetrievalHit>, String> {
        Ok(Vec::new())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelevanceReport {
    pub schema_version: u32,
    pub fixture_id: String,
    pub fixture_digest: String,
    pub retriever: String,
    pub result_limit: u32,
    pub passed: bool,
    pub summary: ReportSummary,
    pub categories: BTreeMap<QueryCategory, ReportSummary>,
    pub queries: Vec<QueryReport>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportSummary {
    pub query_count: u32,
    pub passed_query_count: u32,
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    pub mean_reciprocal_rank: f64,
    pub ndcg_at_10: f64,
    pub citation_locator_accuracy: f64,
    pub exact_phrase_success: Option<f64>,
    pub forbidden_hits_at_10: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryReport {
    pub query_id: String,
    pub text: String,
    pub category: QueryCategory,
    pub passed: bool,
    pub metrics: QueryMetrics,
    pub hits: Vec<RetrievalHit>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryMetrics {
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    pub reciprocal_rank: f64,
    pub ndcg_at_10: f64,
    pub citation_locator_correct: bool,
    pub exact_phrase_success: Option<bool>,
    pub forbidden_hits_at_10: Vec<String>,
}

pub fn run_relevance_suite(
    fixture: &RelevanceFixture,
    retriever: &mut impl Retriever,
) -> Result<RelevanceReport> {
    fixture.validate()?;
    let known_passages = fixture
        .corpus
        .iter()
        .map(|passage| (passage.id.as_str(), passage))
        .collect::<HashMap<_, _>>();
    let mut query_reports = Vec::with_capacity(fixture.queries.len());

    for query in &fixture.queries {
        let raw_hits = retriever
            .retrieve(query, &fixture.corpus, REPORT_LIMIT)
            .map_err(|message| RelevanceError::Retrieval {
                query_id: query.id.clone(),
                message,
            })?;
        validate_hits(query, &raw_hits, &known_passages)?;
        let hits = raw_hits.into_iter().take(REPORT_LIMIT).collect::<Vec<_>>();
        let metrics = calculate_query_metrics(query, &hits);
        let passed = metrics.recall_at_10 > 0.0
            && metrics.citation_locator_correct
            && metrics.forbidden_hits_at_10.is_empty();
        query_reports.push(QueryReport {
            query_id: query.id.clone(),
            text: query.text.clone(),
            category: query.category.clone(),
            passed,
            metrics,
            hits,
        });
    }

    let summary = summarize(query_reports.iter());
    let mut categories = BTreeMap::new();
    for category in query_reports
        .iter()
        .map(|query| query.category.clone())
        .collect::<std::collections::BTreeSet<_>>()
    {
        categories.insert(
            category.clone(),
            summarize(
                query_reports
                    .iter()
                    .filter(|query| query.category == category),
            ),
        );
    }

    Ok(RelevanceReport {
        schema_version: REPORT_SCHEMA_VERSION,
        fixture_id: fixture.fixture_id.clone(),
        fixture_digest: fixture.digest()?,
        retriever: retriever.name().to_owned(),
        result_limit: u32::try_from(REPORT_LIMIT).expect("report limit fits in u32"),
        passed: summary.query_count > 0 && summary.query_count == summary.passed_query_count,
        summary,
        categories,
        queries: query_reports,
    })
}

impl RelevanceReport {
    pub fn to_text(&self) -> String {
        let exact_phrase = self
            .summary
            .exact_phrase_success
            .map_or_else(|| "n/a".to_owned(), |value| format!("{value:.4}"));
        let mut output = format!(
            "Kosh relevance report v{}\n\
             fixture: {} ({})\n\
             retriever: {}\n\
             status: {}\n\
             queries: {}/{} passed\n\
             recall@5: {:.4}\n\
             recall@10: {:.4}\n\
             MRR: {:.4}\n\
             nDCG@10: {:.4}\n\
             citation locator accuracy: {:.4}\n\
             exact/phrase success: {}\n\
             forbidden hits@10: {}\n",
            self.schema_version,
            self.fixture_id,
            self.fixture_digest,
            self.retriever,
            if self.passed { "PASS" } else { "FAIL" },
            self.summary.passed_query_count,
            self.summary.query_count,
            self.summary.recall_at_5,
            self.summary.recall_at_10,
            self.summary.mean_reciprocal_rank,
            self.summary.ndcg_at_10,
            self.summary.citation_locator_accuracy,
            exact_phrase,
            self.summary.forbidden_hits_at_10,
        );
        output.push_str("\nquery results:\n");
        for query in &self.queries {
            let first_hit = query
                .hits
                .first()
                .map_or("-", |hit| hit.passage_id.as_str());
            output.push_str(&format!(
                "- [{}] {} ({:?}): recall@10={:.4}, rr={:.4}, citation={}, top={}\n",
                if query.passed { "PASS" } else { "FAIL" },
                query.query_id,
                query.category,
                query.metrics.recall_at_10,
                query.metrics.reciprocal_rank,
                if query.metrics.citation_locator_correct {
                    "valid"
                } else {
                    "missing/invalid"
                },
                first_hit,
            ));
        }
        output
    }
}

fn validate_hits(
    query: &EvaluationQuery,
    hits: &[RetrievalHit],
    known_passages: &HashMap<&str, &EvaluationPassage>,
) -> Result<()> {
    let mut seen = HashSet::with_capacity(hits.len());
    for hit in hits {
        if !hit.score.is_finite() {
            return Err(RelevanceError::Retrieval {
                query_id: query.id.clone(),
                message: format!("passage {} has a non-finite score", hit.passage_id),
            });
        }
        if !known_passages.contains_key(hit.passage_id.as_str()) {
            return Err(RelevanceError::Retrieval {
                query_id: query.id.clone(),
                message: format!("unknown passage {}", hit.passage_id),
            });
        }
        if !seen.insert(hit.passage_id.as_str()) {
            return Err(RelevanceError::Retrieval {
                query_id: query.id.clone(),
                message: format!("duplicate passage {}", hit.passage_id),
            });
        }
    }
    Ok(())
}

fn calculate_query_metrics(query: &EvaluationQuery, hits: &[RetrievalHit]) -> QueryMetrics {
    let relevance = query
        .relevance
        .iter()
        .map(|judgment| (judgment.passage_id.as_str(), judgment.grade))
        .collect::<HashMap<_, _>>();
    let recall_at_5 = recall_at(&relevance, hits, 5);
    let recall_at_10 = recall_at(&relevance, hits, 10);
    let reciprocal_rank = hits
        .iter()
        .position(|hit| relevance.contains_key(hit.passage_id.as_str()))
        .map_or(0.0, |index| 1.0 / (index + 1) as f64);
    let ndcg_at_10 = ndcg_at(&relevance, hits, 10);
    let citation_locator_correct = hits.iter().take(10).any(|hit| {
        hit.passage_id == query.expected_citation.passage_id
            && hit.locator == query.expected_citation.locator
    });
    let exact_phrase_success =
        matches!(query.category, QueryCategory::Exact | QueryCategory::Phrase).then(|| {
            hits.first()
                .is_some_and(|hit| relevance.contains_key(hit.passage_id.as_str()))
        });
    let forbidden = query
        .must_not_rank
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let forbidden_hits_at_10 = hits
        .iter()
        .take(10)
        .filter(|hit| forbidden.contains(hit.passage_id.as_str()))
        .map(|hit| hit.passage_id.clone())
        .collect();

    QueryMetrics {
        recall_at_5,
        recall_at_10,
        reciprocal_rank,
        ndcg_at_10,
        citation_locator_correct,
        exact_phrase_success,
        forbidden_hits_at_10,
    }
}

fn recall_at(relevance: &HashMap<&str, u8>, hits: &[RetrievalHit], limit: usize) -> f64 {
    hits.iter()
        .take(limit)
        .filter(|hit| relevance.contains_key(hit.passage_id.as_str()))
        .count() as f64
        / relevance.len() as f64
}

fn ndcg_at(relevance: &HashMap<&str, u8>, hits: &[RetrievalHit], limit: usize) -> f64 {
    let dcg = hits
        .iter()
        .take(limit)
        .enumerate()
        .map(|(index, hit)| {
            let grade = relevance.get(hit.passage_id.as_str()).copied().unwrap_or(0);
            discounted_gain(grade, index)
        })
        .sum::<f64>();
    let mut ideal = relevance.values().copied().collect::<Vec<_>>();
    ideal.sort_unstable_by(|left, right| right.cmp(left));
    let ideal_dcg = ideal
        .into_iter()
        .take(limit)
        .enumerate()
        .map(|(index, grade)| discounted_gain(grade, index))
        .sum::<f64>();
    if ideal_dcg == 0.0 || dcg == 0.0 {
        0.0
    } else {
        dcg / ideal_dcg
    }
}

fn discounted_gain(grade: u8, zero_based_rank: usize) -> f64 {
    let gain = if grade == 0 {
        0.0
    } else {
        2_f64.powi(i32::from(grade)) - 1.0
    };
    gain / (zero_based_rank as f64 + 2.0).log2()
}

fn summarize<'a>(queries: impl Iterator<Item = &'a QueryReport>) -> ReportSummary {
    let queries = queries.collect::<Vec<_>>();
    let count = queries.len();
    let exact_phrase = queries
        .iter()
        .filter_map(|query| query.metrics.exact_phrase_success)
        .collect::<Vec<_>>();
    ReportSummary {
        query_count: u32::try_from(count).expect("query count fits in u32"),
        passed_query_count: u32::try_from(queries.iter().filter(|query| query.passed).count())
            .expect("query count fits in u32"),
        recall_at_5: average(queries.iter().map(|query| query.metrics.recall_at_5), count),
        recall_at_10: average(
            queries.iter().map(|query| query.metrics.recall_at_10),
            count,
        ),
        mean_reciprocal_rank: average(
            queries.iter().map(|query| query.metrics.reciprocal_rank),
            count,
        ),
        ndcg_at_10: average(queries.iter().map(|query| query.metrics.ndcg_at_10), count),
        citation_locator_accuracy: average(
            queries
                .iter()
                .map(|query| f64::from(query.metrics.citation_locator_correct)),
            count,
        ),
        exact_phrase_success: (!exact_phrase.is_empty()).then(|| {
            exact_phrase.iter().filter(|success| **success).count() as f64
                / exact_phrase.len() as f64
        }),
        forbidden_hits_at_10: queries
            .iter()
            .map(|query| query.metrics.forbidden_hits_at_10.len() as u32)
            .sum(),
    }
}

fn average(values: impl Iterator<Item = f64>, count: usize) -> f64 {
    if count == 0 {
        0.0
    } else {
        values.sum::<f64>() / count as f64
    }
}

#[cfg(test)]
mod tests {
    use super::{run_relevance_suite, EmptyRetriever, RetrievalHit, Retriever};
    use crate::relevance::{
        EvaluationPassage, EvaluationQuery, RelevanceFixture, REPORT_SCHEMA_VERSION,
    };

    fn fixture() -> RelevanceFixture {
        serde_json::from_str(include_str!("../../../fixtures/relevance/v1.json"))
            .expect("checked-in fixture")
    }

    struct ExpectedCitationRetriever;

    impl Retriever for ExpectedCitationRetriever {
        fn name(&self) -> &str {
            "expected-citation-test"
        }

        fn retrieve(
            &mut self,
            query: &EvaluationQuery,
            corpus: &[EvaluationPassage],
            _limit: usize,
        ) -> std::result::Result<Vec<RetrievalHit>, String> {
            let mut judgments = query.relevance.iter().collect::<Vec<_>>();
            judgments.sort_by_key(|judgment| std::cmp::Reverse(judgment.grade));
            Ok(judgments
                .into_iter()
                .enumerate()
                .map(|(index, judgment)| {
                    let passage = corpus
                        .iter()
                        .find(|passage| passage.id == judgment.passage_id)
                        .expect("judged passage");
                    RetrievalHit {
                        passage_id: passage.id.clone(),
                        score: 1.0 / (index + 1) as f64,
                        locator: passage.locator.clone(),
                        matched_fields: vec!["test".into()],
                    }
                })
                .collect())
        }
    }

    struct FabricatedLocatorRetriever;

    impl Retriever for FabricatedLocatorRetriever {
        fn name(&self) -> &str {
            "fabricated-locator-test"
        }

        fn retrieve(
            &mut self,
            query: &EvaluationQuery,
            _corpus: &[EvaluationPassage],
            _limit: usize,
        ) -> std::result::Result<Vec<RetrievalHit>, String> {
            Ok(vec![RetrievalHit {
                passage_id: query.expected_citation.passage_id.clone(),
                score: 1.0,
                locator: crate::relevance::EvaluationLocator::PdfPage { page: 999 },
                matched_fields: vec!["fabricated".into()],
            }])
        }
    }

    #[test]
    fn empty_retrieval_produces_a_valid_failing_report() {
        let fixture = fixture();
        let report = run_relevance_suite(&fixture, &mut EmptyRetriever).expect("empty report runs");

        assert_eq!(report.schema_version, REPORT_SCHEMA_VERSION);
        assert!(!report.passed);
        assert_eq!(report.summary.passed_query_count, 0);
        assert_eq!(report.summary.recall_at_10, 0.0);
        assert_eq!(report.summary.ndcg_at_10.to_bits(), 0.0_f64.to_bits());
        assert_eq!(report.summary.citation_locator_accuracy, 0.0);
        assert!(report.to_text().contains("status: FAIL"));
        let round_trip = serde_json::from_str(
            &serde_json::to_string_pretty(&report).expect("serialize relevance report"),
        )
        .expect("deserialize relevance report");
        assert_eq!(report, round_trip);
    }

    #[test]
    fn expected_citation_results_calculate_perfect_metrics() {
        let fixture = fixture();
        let report = run_relevance_suite(&fixture, &mut ExpectedCitationRetriever)
            .expect("expected citation report");

        assert!(report.passed);
        assert_eq!(report.summary.recall_at_5, 1.0);
        assert_eq!(report.summary.recall_at_10, 1.0);
        assert_eq!(report.summary.mean_reciprocal_rank, 1.0);
        assert_eq!(report.summary.ndcg_at_10, 1.0);
        assert_eq!(report.summary.citation_locator_accuracy, 1.0);
        assert_eq!(report.summary.forbidden_hits_at_10, 0);
    }

    #[test]
    fn a_relevant_passage_with_fabricated_provenance_fails_the_report() {
        let fixture = fixture();
        let report = run_relevance_suite(&fixture, &mut FabricatedLocatorRetriever)
            .expect("fabricated locator report");

        assert!(!report.passed);
        assert!(report.summary.recall_at_10 > 0.9);
        assert_eq!(report.summary.citation_locator_accuracy, 0.0);
        assert!(report
            .queries
            .iter()
            .all(|query| !query.metrics.citation_locator_correct));
    }
}

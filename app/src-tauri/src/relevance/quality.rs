use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use super::{
    EvaluationLocator, EvaluationOwnerKind, RelevanceError, RelevanceFixture, RelevanceReport,
    ReportSummary, Result,
};

pub const QUALITY_GATE_SCHEMA_VERSION: u32 = 1;
pub const CITATION_AUDIT_SCHEMA_VERSION: u32 = 1;
pub const MINIMUM_QUERY_COUNT: usize = 25;
pub const MINIMUM_CITATION_AUDIT_COUNT: usize = 10;
pub const MINIMUM_LEXICAL_RECALL_AT_10: f64 = 0.95;
pub const MINIMUM_LEXICAL_CITATION_ACCURACY: f64 = 0.95;
pub const MINIMUM_HYBRID_RECALL_AT_10: f64 = 1.0;
pub const MINIMUM_HYBRID_MRR: f64 = 0.95;
pub const MINIMUM_HYBRID_NDCG_AT_10: f64 = 0.95;
pub const MINIMUM_HYBRID_CITATION_ACCURACY: f64 = 1.0;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CitationAudit {
    pub schema_version: u32,
    pub fixture_digest: String,
    pub reviewed_at: String,
    pub reviewer: String,
    pub entries: Vec<CitationAuditEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CitationAuditEntry {
    pub query_id: String,
    pub passage_id: String,
    pub evidence_excerpt: String,
    pub locator: EvaluationLocator,
    pub attachment_filename: Option<String>,
    pub verified: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityGateReport {
    pub schema_version: u32,
    pub fixture_id: String,
    pub fixture_digest: String,
    pub result: String,
    pub thresholds: QualityThresholds,
    pub lexical: ReportSummary,
    pub hybrid: ReportSummary,
    pub citation_audit_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityThresholds {
    pub minimum_query_count: usize,
    pub minimum_citation_audit_count: usize,
    pub lexical_recall_at_10: f64,
    pub lexical_citation_accuracy: f64,
    pub hybrid_recall_at_10: f64,
    pub hybrid_mean_reciprocal_rank: f64,
    pub hybrid_ndcg_at_10: f64,
    pub hybrid_citation_accuracy: f64,
    pub exact_phrase_success: f64,
    pub maximum_forbidden_hits_at_10: u32,
}

pub fn enforce_quality_gate(
    fixture: &RelevanceFixture,
    lexical: &RelevanceReport,
    hybrid: &RelevanceReport,
    audit: &CitationAudit,
) -> Result<QualityGateReport> {
    fixture.validate()?;
    let fixture_digest = fixture.digest()?;
    require(
        fixture.queries.len() >= MINIMUM_QUERY_COUNT,
        format!(
            "relevance corpus has {} queries; at least {MINIMUM_QUERY_COUNT} are required",
            fixture.queries.len()
        ),
    )?;
    for (label, report, expected_retriever) in [
        ("lexical", lexical, "kosh-lexical-v1"),
        ("hybrid", hybrid, "kosh-hybrid-jina-v1"),
    ] {
        require(
            report.fixture_id == fixture.fixture_id
                && report.fixture_digest == fixture_digest
                && report.summary.query_count as usize == fixture.queries.len(),
            format!("{label} report does not describe the current fixture"),
        )?;
        require(
            report.retriever == expected_retriever,
            format!(
                "{label} report used unexpected retriever {}",
                report.retriever
            ),
        )?;
    }

    require(
        lexical.summary.recall_at_10 >= MINIMUM_LEXICAL_RECALL_AT_10,
        "lexical Recall@10 is below 0.95",
    )?;
    require(
        lexical.summary.citation_locator_accuracy >= MINIMUM_LEXICAL_CITATION_ACCURACY,
        "lexical citation accuracy is below 0.95",
    )?;
    require(
        lexical.summary.exact_phrase_success == Some(1.0),
        "lexical exact/phrase success is not 1.0",
    )?;
    require(
        lexical.summary.forbidden_hits_at_10 == 0,
        "lexical retrieval returned a forbidden hit",
    )?;

    require(hybrid.passed, "hybrid report contains a failing query")?;
    require(
        hybrid.summary.recall_at_10 >= MINIMUM_HYBRID_RECALL_AT_10,
        "hybrid Recall@10 is below 1.0",
    )?;
    require(
        hybrid.summary.mean_reciprocal_rank >= MINIMUM_HYBRID_MRR,
        "hybrid MRR is below 0.95",
    )?;
    require(
        hybrid.summary.ndcg_at_10 >= MINIMUM_HYBRID_NDCG_AT_10,
        "hybrid nDCG@10 is below 0.95",
    )?;
    require(
        hybrid.summary.citation_locator_accuracy >= MINIMUM_HYBRID_CITATION_ACCURACY,
        "hybrid citation accuracy is below 1.0",
    )?;
    require(
        hybrid.summary.exact_phrase_success == Some(1.0),
        "hybrid exact/phrase success is not 1.0",
    )?;
    require(
        hybrid.summary.forbidden_hits_at_10 == 0,
        "hybrid retrieval returned a forbidden hit",
    )?;
    require(
        hybrid.summary.recall_at_10 >= lexical.summary.recall_at_10
            && hybrid.summary.mean_reciprocal_rank >= lexical.summary.mean_reciprocal_rank
            && hybrid.summary.ndcg_at_10 >= lexical.summary.ndcg_at_10,
        "hybrid retrieval regressed against the lexical baseline",
    )?;

    let citation_audit_count = validate_citation_audit(fixture, &fixture_digest, audit)?;
    Ok(QualityGateReport {
        schema_version: QUALITY_GATE_SCHEMA_VERSION,
        fixture_id: fixture.fixture_id.clone(),
        fixture_digest,
        result: "pass".into(),
        thresholds: QualityThresholds {
            minimum_query_count: MINIMUM_QUERY_COUNT,
            minimum_citation_audit_count: MINIMUM_CITATION_AUDIT_COUNT,
            lexical_recall_at_10: MINIMUM_LEXICAL_RECALL_AT_10,
            lexical_citation_accuracy: MINIMUM_LEXICAL_CITATION_ACCURACY,
            hybrid_recall_at_10: MINIMUM_HYBRID_RECALL_AT_10,
            hybrid_mean_reciprocal_rank: MINIMUM_HYBRID_MRR,
            hybrid_ndcg_at_10: MINIMUM_HYBRID_NDCG_AT_10,
            hybrid_citation_accuracy: MINIMUM_HYBRID_CITATION_ACCURACY,
            exact_phrase_success: 1.0,
            maximum_forbidden_hits_at_10: 0,
        },
        lexical: lexical.summary.clone(),
        hybrid: hybrid.summary.clone(),
        citation_audit_count,
    })
}

fn validate_citation_audit(
    fixture: &RelevanceFixture,
    fixture_digest: &str,
    audit: &CitationAudit,
) -> Result<usize> {
    require(
        audit.schema_version == CITATION_AUDIT_SCHEMA_VERSION,
        "citation audit schema is unsupported",
    )?;
    require(
        audit.fixture_digest == fixture_digest,
        "citation audit does not describe the current fixture",
    )?;
    require(
        !audit.reviewed_at.trim().is_empty() && !audit.reviewer.trim().is_empty(),
        "citation audit has no review identity",
    )?;
    require(
        audit.entries.len() >= MINIMUM_CITATION_AUDIT_COUNT,
        format!(
            "citation audit has {} entries; at least {MINIMUM_CITATION_AUDIT_COUNT} are required",
            audit.entries.len()
        ),
    )?;

    let queries = fixture
        .queries
        .iter()
        .map(|query| (query.id.as_str(), query))
        .collect::<HashMap<_, _>>();
    let passages = fixture
        .corpus
        .iter()
        .map(|passage| (passage.id.as_str(), passage))
        .collect::<HashMap<_, _>>();
    let mut query_ids = BTreeSet::new();
    let mut locator_kinds = BTreeSet::new();
    let mut owner_kinds = BTreeSet::new();
    for entry in &audit.entries {
        require(
            entry.verified,
            format!("{} is not verified", entry.query_id),
        )?;
        require(
            query_ids.insert(entry.query_id.as_str()),
            format!("duplicate citation audit query {}", entry.query_id),
        )?;
        let query = queries
            .get(entry.query_id.as_str())
            .ok_or_else(|| quality_error(format!("unknown audit query {}", entry.query_id)))?;
        let passage = passages
            .get(entry.passage_id.as_str())
            .ok_or_else(|| quality_error(format!("unknown audit passage {}", entry.passage_id)))?;
        require(
            query.expected_citation.passage_id == entry.passage_id
                && query.expected_citation.locator == entry.locator
                && passage.locator == entry.locator,
            format!(
                "{} does not match expected citation provenance",
                entry.query_id
            ),
        )?;
        require(
            entry.evidence_excerpt.trim().len() >= 8
                && passage.content.contains(&entry.evidence_excerpt),
            format!(
                "{} excerpt does not occur in the cited passage",
                entry.query_id
            ),
        )?;
        if let Some(filename) = &entry.attachment_filename {
            require(
                passage
                    .attachments
                    .iter()
                    .any(|attachment| &attachment.filename == filename),
                format!(
                    "{} attachment filename is not cited by the passage",
                    entry.query_id
                ),
            )?;
        }
        locator_kinds.insert(locator_kind(&entry.locator));
        owner_kinds.insert(match passage.owner_kind {
            EvaluationOwnerKind::Author => "AUTHOR",
            EvaluationOwnerKind::Attachment => "ATTACHMENT",
        });
    }
    require(
        locator_kinds
            == BTreeSet::from(["MARKDOWN_BLOCKS", "OCR_REGION", "PDF_PAGE", "TEXT_LINES"]),
        "citation audit must cover every locator kind",
    )?;
    require(
        owner_kinds == BTreeSet::from(["ATTACHMENT", "AUTHOR"]),
        "citation audit must cover authored and attachment evidence",
    )?;
    Ok(audit.entries.len())
}

fn locator_kind(locator: &EvaluationLocator) -> &'static str {
    match locator {
        EvaluationLocator::MarkdownBlocks { .. } => "MARKDOWN_BLOCKS",
        EvaluationLocator::PdfPage { .. } => "PDF_PAGE",
        EvaluationLocator::OcrRegion { .. } => "OCR_REGION",
        EvaluationLocator::TextLines { .. } => "TEXT_LINES",
    }
}

fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(quality_error(message))
    }
}

fn quality_error(message: impl Into<String>) -> RelevanceError {
    RelevanceError::QualityGate(message.into())
}

#[cfg(test)]
mod tests {
    use super::{enforce_quality_gate, CitationAudit};
    use crate::relevance::{
        run_relevance_suite, HybridFixtureRetriever, HybridVectorFixture, LexicalFixtureRetriever,
        RelevanceFixture,
    };

    fn inputs() -> (
        RelevanceFixture,
        crate::relevance::RelevanceReport,
        crate::relevance::RelevanceReport,
        CitationAudit,
    ) {
        let fixture: RelevanceFixture =
            serde_json::from_str(include_str!("../../../fixtures/relevance/v1.json"))
                .expect("fixture");
        let vectors: HybridVectorFixture = serde_json::from_str(include_str!(
            "../../../fixtures/relevance/jina-v1-vectors.json"
        ))
        .expect("vectors");
        let lexical =
            run_relevance_suite(&fixture, &mut LexicalFixtureRetriever).expect("lexical report");
        let mut hybrid_retriever =
            HybridFixtureRetriever::new(&fixture, vectors).expect("hybrid retriever");
        let hybrid = run_relevance_suite(&fixture, &mut hybrid_retriever).expect("hybrid report");
        let audit = serde_json::from_str(include_str!(
            "../../../fixtures/relevance/citation-audit-v1.json"
        ))
        .expect("citation audit");
        (fixture, lexical, hybrid, audit)
    }

    #[test]
    fn checked_quality_evidence_meets_every_release_threshold() {
        let (fixture, lexical, hybrid, audit) = inputs();
        let report =
            enforce_quality_gate(&fixture, &lexical, &hybrid, &audit).expect("quality gate");
        assert_eq!(report.result, "pass");
        assert_eq!(report.citation_audit_count, 10);
    }

    #[test]
    fn metric_and_manual_audit_regressions_are_rejected() {
        let (fixture, lexical, mut hybrid, mut audit) = inputs();
        hybrid.summary.mean_reciprocal_rank = 0.94;
        assert!(enforce_quality_gate(&fixture, &lexical, &hybrid, &audit).is_err());

        let (_, _, hybrid, _) = inputs();
        audit.entries[0].verified = false;
        assert!(enforce_quality_gate(&fixture, &lexical, &hybrid, &audit).is_err());
    }
}

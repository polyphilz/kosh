use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use super::{RelevanceError, RelevanceFixture, RelevanceReport, ReportSummary, Result};

pub const QUALITY_GATE_SCHEMA_VERSION: u32 = 1;
pub const BLOCK_AUDIT_SCHEMA_VERSION: u32 = 1;
pub const MINIMUM_QUERY_COUNT: usize = 25;
pub const MINIMUM_BLOCK_AUDIT_COUNT: usize = 10;
pub const MINIMUM_LEXICAL_RECALL_AT_10: f64 = 0.95;
pub const MINIMUM_LEXICAL_EXPECTED_BLOCK_ACCURACY: f64 = 0.95;
pub const MINIMUM_HYBRID_RECALL_AT_10: f64 = 1.0;
pub const MINIMUM_HYBRID_MRR: f64 = 0.95;
pub const MINIMUM_HYBRID_NDCG_AT_10: f64 = 0.95;
pub const MINIMUM_HYBRID_EXPECTED_BLOCK_ACCURACY: f64 = 1.0;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlockAudit {
    pub schema_version: u32,
    pub fixture_digest: String,
    pub reviewed_at: String,
    pub reviewer: String,
    pub entries: Vec<BlockAuditEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlockAuditEntry {
    pub query_id: String,
    pub block_id: String,
    pub evidence_excerpt: String,
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
    pub block_audit_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityThresholds {
    pub minimum_query_count: usize,
    pub minimum_block_audit_count: usize,
    pub lexical_recall_at_10: f64,
    pub lexical_expected_block_accuracy: f64,
    pub hybrid_recall_at_10: f64,
    pub hybrid_mean_reciprocal_rank: f64,
    pub hybrid_ndcg_at_10: f64,
    pub hybrid_expected_block_accuracy: f64,
    pub exact_phrase_success: f64,
    pub maximum_forbidden_hits_at_10: u32,
}

pub fn enforce_quality_gate(
    fixture: &RelevanceFixture,
    lexical: &RelevanceReport,
    hybrid: &RelevanceReport,
    audit: &BlockAudit,
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
        lexical.summary.expected_block_accuracy >= MINIMUM_LEXICAL_EXPECTED_BLOCK_ACCURACY,
        "lexical expected block accuracy is below 0.95",
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
        hybrid.summary.expected_block_accuracy >= MINIMUM_HYBRID_EXPECTED_BLOCK_ACCURACY,
        "hybrid expected block accuracy is below 1.0",
    )?;
    require(
        hybrid.summary.exact_phrase_success == Some(1.0),
        "hybrid exact/phrase success is not 1.0",
    )?;
    require(
        hybrid.summary.forbidden_hits_at_10 == 0,
        "hybrid retrieval returned a forbidden hit",
    )?;
    let block_audit_count = validate_block_audit(fixture, &fixture_digest, audit)?;
    Ok(QualityGateReport {
        schema_version: QUALITY_GATE_SCHEMA_VERSION,
        fixture_id: fixture.fixture_id.clone(),
        fixture_digest,
        result: "pass".into(),
        thresholds: QualityThresholds {
            minimum_query_count: MINIMUM_QUERY_COUNT,
            minimum_block_audit_count: MINIMUM_BLOCK_AUDIT_COUNT,
            lexical_recall_at_10: MINIMUM_LEXICAL_RECALL_AT_10,
            lexical_expected_block_accuracy: MINIMUM_LEXICAL_EXPECTED_BLOCK_ACCURACY,
            hybrid_recall_at_10: MINIMUM_HYBRID_RECALL_AT_10,
            hybrid_mean_reciprocal_rank: MINIMUM_HYBRID_MRR,
            hybrid_ndcg_at_10: MINIMUM_HYBRID_NDCG_AT_10,
            hybrid_expected_block_accuracy: MINIMUM_HYBRID_EXPECTED_BLOCK_ACCURACY,
            exact_phrase_success: 1.0,
            maximum_forbidden_hits_at_10: 0,
        },
        lexical: lexical.summary.clone(),
        hybrid: hybrid.summary.clone(),
        block_audit_count,
    })
}

fn validate_block_audit(
    fixture: &RelevanceFixture,
    fixture_digest: &str,
    audit: &BlockAudit,
) -> Result<usize> {
    require(
        audit.schema_version == BLOCK_AUDIT_SCHEMA_VERSION,
        "block audit schema is unsupported",
    )?;
    require(
        audit.fixture_digest == fixture_digest,
        "block audit does not describe the current fixture",
    )?;
    require(
        !audit.reviewed_at.trim().is_empty() && !audit.reviewer.trim().is_empty(),
        "block audit has no review identity",
    )?;
    require(
        audit.entries.len() >= MINIMUM_BLOCK_AUDIT_COUNT,
        format!(
            "block audit has {} entries; at least {MINIMUM_BLOCK_AUDIT_COUNT} are required",
            audit.entries.len()
        ),
    )?;

    let queries = fixture
        .queries
        .iter()
        .map(|query| (query.id.as_str(), query))
        .collect::<HashMap<_, _>>();
    let blocks = fixture
        .corpus
        .iter()
        .map(|block| (block.id.as_str(), block))
        .collect::<HashMap<_, _>>();
    let mut query_ids = BTreeSet::new();
    for entry in &audit.entries {
        require(
            entry.verified,
            format!("{} is not verified", entry.query_id),
        )?;
        require(
            query_ids.insert(entry.query_id.as_str()),
            format!("duplicate block audit query {}", entry.query_id),
        )?;
        let query = queries
            .get(entry.query_id.as_str())
            .ok_or_else(|| quality_error(format!("unknown audit query {}", entry.query_id)))?;
        let block = blocks
            .get(entry.block_id.as_str())
            .ok_or_else(|| quality_error(format!("unknown audit block {}", entry.block_id)))?;
        require(
            query.expected_block_id == entry.block_id,
            format!("{} does not match its expected block", entry.query_id),
        )?;
        let searchable_content = [
            block.body.as_str(),
            block.extracted_text.as_str(),
            &block.attachment_names.join("\n"),
        ]
        .join("\n");
        require(
            entry.evidence_excerpt.trim().len() >= 8
                && searchable_content.contains(&entry.evidence_excerpt),
            format!("{} excerpt does not occur in the block", entry.query_id),
        )?;
    }
    Ok(audit.entries.len())
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
    use super::{enforce_quality_gate, BlockAudit};
    use crate::relevance::{
        run_relevance_suite, HybridFixtureRetriever, HybridVectorFixture, LexicalFixtureRetriever,
        RelevanceFixture,
    };

    fn inputs() -> (
        RelevanceFixture,
        crate::relevance::RelevanceReport,
        crate::relevance::RelevanceReport,
        BlockAudit,
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
            "../../../fixtures/relevance/block-audit-v1.json"
        ))
        .expect("block audit");
        (fixture, lexical, hybrid, audit)
    }

    #[test]
    fn checked_quality_evidence_meets_every_release_threshold() {
        let (fixture, lexical, hybrid, audit) = inputs();
        let report =
            enforce_quality_gate(&fixture, &lexical, &hybrid, &audit).expect("quality gate");
        assert_eq!(report.result, "pass");
        assert_eq!(report.block_audit_count, 10);
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

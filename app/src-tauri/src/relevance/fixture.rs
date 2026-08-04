use std::collections::{BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{RelevanceError, Result};

pub const FIXTURE_SCHEMA_VERSION: u32 = 3;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelevanceFixture {
    pub schema_version: u32,
    pub fixture_id: String,
    pub description: String,
    pub corpus: Vec<EvaluationPassage>,
    pub queries: Vec<EvaluationQuery>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvaluationPassage {
    pub id: String,
    pub owner_kind: EvaluationOwnerKind,
    pub tidbit_id: Option<String>,
    pub evidence_attachment_id: Option<String>,
    #[serde(default)]
    pub heading_context: Vec<String>,
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<EvaluationAttachment>,
    pub locator: EvaluationLocator,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EvaluationOwnerKind {
    Author,
    Attachment,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvaluationAttachment {
    pub filename: String,
    pub media_type: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum EvaluationLocator {
    MarkdownBlocks {
        start_block: u32,
        end_block: u32,
        source_start_byte: Option<u64>,
        source_end_byte: Option<u64>,
        start_char: Option<u32>,
        end_char: Option<u32>,
        start_line: Option<u32>,
        end_line: Option<u32>,
    },
    PdfPage {
        page: u32,
    },
    OcrRegion {
        page: Option<u32>,
        region: EvaluationRegion,
    },
    TextLines {
        start_line: u32,
        end_line: u32,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvaluationRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum QueryCategory {
    CodeIdentifier,
    Exact,
    Formula,
    Misspelling,
    NearDuplicate,
    Ocr,
    Pdf,
    MediaVolume,
    Phrase,
    Prose,
    Synonym,
    TextAttachment,
    Unicode,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RetrievalNeed {
    Both,
    Lexical,
    Semantic,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SearchMode {
    Default,
    Exact,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvaluationQuery {
    pub id: String,
    pub text: String,
    pub search_mode: SearchMode,
    pub category: QueryCategory,
    pub retrieval_need: RetrievalNeed,
    pub relevance: Vec<RelevanceJudgment>,
    #[serde(default)]
    pub must_not_rank: Vec<String>,
    pub expected_citation: ExpectedCitation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelevanceJudgment {
    pub passage_id: String,
    pub grade: u8,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExpectedCitation {
    pub passage_id: String,
    pub locator: EvaluationLocator,
}

impl RelevanceFixture {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != FIXTURE_SCHEMA_VERSION {
            return invalid(format!(
                "unsupported schemaVersion {}; expected {FIXTURE_SCHEMA_VERSION}",
                self.schema_version
            ));
        }
        require_nonempty("fixtureId", &self.fixture_id)?;
        require_nonempty("description", &self.description)?;
        if self.corpus.is_empty() {
            return invalid("corpus must not be empty");
        }
        if self.queries.is_empty() {
            return invalid("queries must not be empty");
        }

        let mut passage_ids = HashSet::with_capacity(self.corpus.len());
        let mut passages = HashMap::with_capacity(self.corpus.len());
        for passage in &self.corpus {
            require_nonempty("passage.id", &passage.id)?;
            require_nonempty("passage.content", &passage.content)?;
            if !passage_ids.insert(passage.id.as_str()) {
                return invalid(format!("duplicate passage id {}", passage.id));
            }
            passage.locator.validate(&passage.id)?;
            match passage.owner_kind {
                EvaluationOwnerKind::Author => {
                    let Some(tidbit_id) = passage.tidbit_id.as_deref() else {
                        return invalid(format!(
                            "authored passage {} must name a tidbitId",
                            passage.id
                        ));
                    };
                    require_nonempty("passage.tidbitId", tidbit_id)?;
                    if passage.evidence_attachment_id.is_some()
                        || !matches!(passage.locator, EvaluationLocator::MarkdownBlocks { .. })
                    {
                        return invalid(format!(
                            "authored passage {} has attachment provenance",
                            passage.id
                        ));
                    }
                }
                EvaluationOwnerKind::Attachment => {
                    if passage.tidbit_id.is_some()
                        || matches!(passage.locator, EvaluationLocator::MarkdownBlocks { .. })
                    {
                        return invalid(format!(
                            "attachment passage {} has authored provenance",
                            passage.id
                        ));
                    }
                    let Some(attachment_id) = passage.evidence_attachment_id.as_deref() else {
                        return invalid(format!(
                            "attachment passage {} must name an evidenceAttachmentId",
                            passage.id
                        ));
                    };
                    require_nonempty("passage.evidenceAttachmentId", attachment_id)?;
                    if passage.attachments.len() != 1 {
                        return invalid(format!(
                            "attachment passage {} must describe exactly one attachment",
                            passage.id
                        ));
                    }
                }
            }
            for attachment in &passage.attachments {
                require_nonempty("attachment.filename", &attachment.filename)?;
                require_nonempty("attachment.mediaType", &attachment.media_type)?;
            }
            passages.insert(passage.id.as_str(), passage);
        }

        let mut query_ids = HashSet::with_capacity(self.queries.len());
        for query in &self.queries {
            require_nonempty("query.id", &query.id)?;
            require_nonempty("query.text", &query.text)?;
            if !query_ids.insert(query.id.as_str()) {
                return invalid(format!("duplicate query id {}", query.id));
            }
            if query.relevance.is_empty() {
                return invalid(format!("query {} has no relevance judgments", query.id));
            }
            let mut judged = BTreeSet::new();
            for judgment in &query.relevance {
                if !passages.contains_key(judgment.passage_id.as_str()) {
                    return invalid(format!(
                        "query {} references unknown passage {}",
                        query.id, judgment.passage_id
                    ));
                }
                if !(1..=3).contains(&judgment.grade) {
                    return invalid(format!(
                        "query {} assigns grade {} outside 1..=3",
                        query.id, judgment.grade
                    ));
                }
                if !judged.insert(judgment.passage_id.as_str()) {
                    return invalid(format!(
                        "query {} judges passage {} more than once",
                        query.id, judgment.passage_id
                    ));
                }
            }
            let mut forbidden = BTreeSet::new();
            for passage_id in &query.must_not_rank {
                if !passages.contains_key(passage_id.as_str()) {
                    return invalid(format!(
                        "query {} forbids unknown passage {}",
                        query.id, passage_id
                    ));
                }
                if judged.contains(passage_id.as_str()) {
                    return invalid(format!(
                        "query {} both judges and forbids passage {}",
                        query.id, passage_id
                    ));
                }
                if !forbidden.insert(passage_id.as_str()) {
                    return invalid(format!(
                        "query {} forbids passage {} more than once",
                        query.id, passage_id
                    ));
                }
            }
            if !judged.contains(query.expected_citation.passage_id.as_str()) {
                return invalid(format!(
                    "query {} expects a citation to an unjudged passage",
                    query.id
                ));
            }
            let expected = passages
                .get(query.expected_citation.passage_id.as_str())
                .expect("expected citation passage validated above");
            if expected.locator != query.expected_citation.locator {
                return invalid(format!(
                    "query {} expected citation locator differs from corpus provenance",
                    query.id
                ));
            }
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        let canonical = serde_json::to_vec(self).map_err(|source| RelevanceError::Json {
            path: format!("fixture {}", self.fixture_id),
            source,
        })?;
        let digest = Sha256::digest(canonical);
        Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
    }
}

impl EvaluationLocator {
    fn validate(&self, passage_id: &str) -> Result<()> {
        match self {
            Self::MarkdownBlocks {
                start_block,
                end_block,
                source_start_byte,
                source_end_byte,
                start_char,
                end_char,
                start_line,
                end_line,
            } => {
                if end_block < start_block {
                    return invalid(format!(
                        "passage {passage_id} has a reversed Markdown block range"
                    ));
                }
                validate_optional_range(
                    passage_id,
                    "source byte",
                    *source_start_byte,
                    *source_end_byte,
                    false,
                )?;
                validate_optional_range(
                    passage_id,
                    "character",
                    start_char.map(u64::from),
                    end_char.map(u64::from),
                    false,
                )?;
                validate_optional_range(
                    passage_id,
                    "line",
                    start_line.map(u64::from),
                    end_line.map(u64::from),
                    true,
                )?;
            }
            Self::PdfPage { page } if *page == 0 => {
                return invalid(format!("passage {passage_id} has PDF page zero"));
            }
            Self::OcrRegion { page, region } => {
                if page.is_some_and(|value| value == 0) {
                    return invalid(format!("passage {passage_id} has OCR page zero"));
                }
                if region.width == 0 || region.height == 0 {
                    return invalid(format!("passage {passage_id} has an empty OCR region"));
                }
            }
            Self::TextLines {
                start_line,
                end_line,
            } if *start_line == 0 || end_line < start_line => {
                return invalid(format!(
                    "passage {passage_id} has an invalid text line range"
                ));
            }
            _ => {}
        }
        Ok(())
    }
}

fn validate_optional_range(
    passage_id: &str,
    label: &str,
    start: Option<u64>,
    end: Option<u64>,
    one_based: bool,
) -> Result<()> {
    match (start, end) {
        (None, None) => Ok(()),
        (Some(start), Some(end))
            if end >= start && (!one_based || start > 0) && (one_based || end > start) =>
        {
            Ok(())
        }
        _ => invalid(format!(
            "passage {passage_id} has an invalid optional {label} range"
        )),
    }
}

fn require_nonempty(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return invalid(format!("{label} must not be empty"));
    }
    Ok(())
}

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(RelevanceError::InvalidFixture(message.into()))
}

#[cfg(test)]
mod tests {
    use super::{QueryCategory, RelevanceFixture, RetrievalNeed, SearchMode};

    fn checked_in_fixture() -> RelevanceFixture {
        serde_json::from_str(include_str!("../../../fixtures/relevance/v1.json"))
            .expect("checked-in relevance fixture")
    }

    #[test]
    fn checked_in_fixture_is_valid_and_covers_search_categories() {
        let fixture = checked_in_fixture();
        fixture.validate().expect("valid relevance fixture");

        assert!(fixture.queries.len() >= 12);
        assert!(fixture
            .queries
            .iter()
            .any(|query| query.retrieval_need == RetrievalNeed::Lexical));
        assert!(fixture
            .queries
            .iter()
            .any(|query| query.retrieval_need == RetrievalNeed::Semantic));
        assert!(fixture
            .queries
            .iter()
            .any(|query| query.retrieval_need == RetrievalNeed::Both));
        assert!(fixture.queries.iter().any(|query| {
            query.category == QueryCategory::Exact && query.search_mode == SearchMode::Exact
        }));
        for category in [
            QueryCategory::CodeIdentifier,
            QueryCategory::Formula,
            QueryCategory::Misspelling,
            QueryCategory::NearDuplicate,
            QueryCategory::Ocr,
            QueryCategory::Pdf,
            QueryCategory::MediaVolume,
            QueryCategory::Synonym,
            QueryCategory::TextAttachment,
        ] {
            assert!(
                fixture
                    .queries
                    .iter()
                    .any(|query| query.category == category),
                "missing category {category:?}"
            );
        }
    }

    #[test]
    fn fixture_rejects_a_citation_locator_that_does_not_match_provenance() {
        let mut fixture = checked_in_fixture();
        fixture.queries[0].expected_citation.locator = fixture.corpus[1].locator.clone();

        assert!(fixture
            .validate()
            .expect_err("mismatched citation must fail")
            .to_string()
            .contains("differs from corpus provenance"));
    }

    #[test]
    fn fixture_rejects_owner_and_locator_provenance_mismatches() {
        let mut fixture = checked_in_fixture();
        let attachment = fixture
            .corpus
            .iter_mut()
            .find(|passage| passage.evidence_attachment_id.is_some())
            .expect("attachment evidence");
        attachment.tidbit_id = Some("fabricated-owner".into());

        assert!(fixture
            .validate()
            .expect_err("attachment evidence cannot masquerade as authored")
            .to_string()
            .contains("has authored provenance"));
    }
}

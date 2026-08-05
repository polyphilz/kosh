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
    pub corpus: Vec<EvaluationBlock>,
    pub queries: Vec<EvaluationQuery>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvaluationBlock {
    pub id: String,
    pub note_id: String,
    pub block_type: String,
    #[serde(default)]
    pub heading_context: Vec<String>,
    pub body: String,
    #[serde(default)]
    pub attachment_names: Vec<String>,
    #[serde(default)]
    pub extracted_text: String,
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
    AttachmentFilename,
    Ranking,
    Phrase,
    Prose,
    Link,
    Synonym,
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
    pub expected_block_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelevanceJudgment {
    pub block_id: String,
    pub grade: u8,
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

        let mut block_ids = HashSet::with_capacity(self.corpus.len());
        let mut blocks = HashMap::with_capacity(self.corpus.len());
        for block in &self.corpus {
            require_nonempty("block.id", &block.id)?;
            require_nonempty("block.noteId", &block.note_id)?;
            require_nonempty("block.blockType", &block.block_type)?;
            if block.body.trim().is_empty()
                && block
                    .attachment_names
                    .iter()
                    .all(|name| name.trim().is_empty())
                && block.extracted_text.trim().is_empty()
            {
                return invalid(format!("block {} has no searchable content", block.id));
            }
            if !block_ids.insert(block.id.as_str()) {
                return invalid(format!("duplicate block id {}", block.id));
            }
            for filename in &block.attachment_names {
                require_nonempty("block.attachmentNames", filename)?;
            }
            blocks.insert(block.id.as_str(), block);
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
                if !blocks.contains_key(judgment.block_id.as_str()) {
                    return invalid(format!(
                        "query {} references unknown block {}",
                        query.id, judgment.block_id
                    ));
                }
                if !(1..=3).contains(&judgment.grade) {
                    return invalid(format!(
                        "query {} assigns grade {} outside 1..=3",
                        query.id, judgment.grade
                    ));
                }
                if !judged.insert(judgment.block_id.as_str()) {
                    return invalid(format!(
                        "query {} judges block {} more than once",
                        query.id, judgment.block_id
                    ));
                }
            }
            let mut forbidden = BTreeSet::new();
            for block_id in &query.must_not_rank {
                if !blocks.contains_key(block_id.as_str()) {
                    return invalid(format!(
                        "query {} forbids unknown block {}",
                        query.id, block_id
                    ));
                }
                if judged.contains(block_id.as_str()) {
                    return invalid(format!(
                        "query {} both judges and forbids block {}",
                        query.id, block_id
                    ));
                }
                if !forbidden.insert(block_id.as_str()) {
                    return invalid(format!(
                        "query {} forbids block {} more than once",
                        query.id, block_id
                    ));
                }
            }
            if !judged.contains(query.expected_block_id.as_str()) {
                return invalid(format!("query {} expects an unjudged block", query.id));
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
            QueryCategory::AttachmentFilename,
            QueryCategory::Ranking,
            QueryCategory::Link,
            QueryCategory::Synonym,
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
    fn fixture_rejects_an_expected_block_that_is_not_relevant() {
        let mut fixture = checked_in_fixture();
        fixture.queries[0].expected_block_id = fixture.corpus[1].id.clone();

        assert!(fixture
            .validate()
            .expect_err("unjudged expected block must fail")
            .to_string()
            .contains("expects an unjudged block"));
    }

    #[test]
    fn fixture_rejects_a_block_without_searchable_content() {
        let mut fixture = checked_in_fixture();
        let block = &mut fixture.corpus[0];
        block.body.clear();
        block.attachment_names.clear();
        block.extracted_text.clear();

        assert!(fixture
            .validate()
            .expect_err("empty block must fail")
            .to_string()
            .contains("has no searchable content"));
    }
}

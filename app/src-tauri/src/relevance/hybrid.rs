use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    database::{
        embedding_index,
        search::{
            candidate_limit, fuse_ranked_passages, parse_lexical_query, rank_lexical_documents,
            LexicalSearchMode, RankedLexicalDocument,
        },
    },
    EmbeddingRuntime,
};

use super::{
    lexical::{fixture_candidate_ranks, fixture_fields},
    EvaluationLocator, EvaluationPassage, RelevanceError, RelevanceFixture, RetrievalHit,
    RetrievalRequest, Retriever, SearchMode,
};

pub const HYBRID_VECTOR_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HybridVectorFixture {
    pub schema_version: u32,
    pub fixture_digest: String,
    pub embedding_index_id: String,
    pub model_file_sha256: String,
    pub passage_embeddings: BTreeMap<String, Vec<f32>>,
    pub query_embeddings: BTreeMap<String, Vec<f32>>,
}

pub fn generate_hybrid_vector_fixture(
    fixture: &RelevanceFixture,
    runtime: &EmbeddingRuntime,
) -> super::Result<HybridVectorFixture> {
    fixture.validate()?;
    let manifest = embedding_index::manifest();
    let mut passage_embeddings = BTreeMap::new();
    for passage in &fixture.corpus {
        let embedding = runtime
            .embed_document(&passage.content)
            .map_err(|error| RelevanceError::HybridVectors(error.public_message()))?;
        if passage_embeddings
            .insert(passage.id.clone(), embedding)
            .is_some()
        {
            return hybrid_error(format!("duplicate passage ID {}", passage.id));
        }
    }
    let mut query_embeddings = BTreeMap::new();
    for query in &fixture.queries {
        let embedding = runtime
            .embed_query(&query.text)
            .map_err(|error| RelevanceError::HybridVectors(error.public_message()))?;
        if query_embeddings
            .insert(query.text.clone(), embedding)
            .is_some()
        {
            return hybrid_error(format!(
                "duplicate query text cannot key vector fixture: {}",
                query.text
            ));
        }
    }
    let vectors = HybridVectorFixture {
        schema_version: HYBRID_VECTOR_SCHEMA_VERSION,
        fixture_digest: fixture.digest()?,
        embedding_index_id: manifest.id,
        model_file_sha256: manifest.model_file_sha256,
        passage_embeddings,
        query_embeddings,
    };
    validate_hybrid_vector_fixture(fixture, &vectors)?;
    Ok(vectors)
}

pub fn validate_hybrid_vector_fixture(
    fixture: &RelevanceFixture,
    vectors: &HybridVectorFixture,
) -> super::Result<()> {
    if vectors.schema_version != HYBRID_VECTOR_SCHEMA_VERSION {
        return hybrid_error(format!(
            "unsupported hybrid vector schema {}; expected {HYBRID_VECTOR_SCHEMA_VERSION}",
            vectors.schema_version
        ));
    }
    let manifest = embedding_index::manifest();
    if vectors.fixture_digest != fixture.digest()? {
        return hybrid_error("hybrid vectors do not match the relevance fixture".into());
    }
    if vectors.embedding_index_id != manifest.id
        || vectors.model_file_sha256 != manifest.model_file_sha256
    {
        return hybrid_error("hybrid vectors do not match the shipped embedding index".into());
    }
    let expected_passages = fixture
        .corpus
        .iter()
        .map(|passage| passage.id.as_str())
        .collect::<BTreeSet<_>>();
    let actual_passages = vectors
        .passage_embeddings
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if actual_passages != expected_passages {
        return hybrid_error("hybrid vectors do not cover the exact passage corpus".into());
    }
    let expected_queries = fixture
        .queries
        .iter()
        .map(|query| query.text.as_str())
        .collect::<BTreeSet<_>>();
    if expected_queries.len() != fixture.queries.len() {
        return hybrid_error("relevance query text must be unique for vector lookup".into());
    }
    let actual_queries = vectors
        .query_embeddings
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if actual_queries != expected_queries {
        return hybrid_error("hybrid vectors do not cover the exact query corpus".into());
    }
    for (key, vector) in vectors
        .passage_embeddings
        .iter()
        .chain(vectors.query_embeddings.iter())
    {
        embedding_index::validate_embedding(vector, manifest.dimension as usize).map_err(
            |error| RelevanceError::HybridVectors(format!("invalid vector for {key}: {error}")),
        )?;
    }
    Ok(())
}

pub struct HybridFixtureRetriever {
    vectors: HybridVectorFixture,
}

impl HybridFixtureRetriever {
    pub fn new(fixture: &RelevanceFixture, vectors: HybridVectorFixture) -> super::Result<Self> {
        validate_hybrid_vector_fixture(fixture, &vectors)?;
        Ok(Self { vectors })
    }
}

impl Retriever for HybridFixtureRetriever {
    fn name(&self) -> &str {
        "kosh-hybrid-jina-v1"
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
        let lexical_documents = fixture_candidate_ranks(corpus, &query, limit)?
            .into_iter()
            .map(|(index, (word_rank, trigram_rank, short_rank))| {
                let passage = &corpus[index];
                crate::database::search::LexicalDocument {
                    passage_id: passage.id.clone(),
                    updated_at_ms: 0,
                    fields: fixture_fields(passage),
                    word_rank,
                    trigram_rank,
                    short_rank,
                }
            })
            .collect();
        let lexical = rank_lexical_documents(&query, lexical_documents, lexical_candidate_limit);
        let ranked = if mode == LexicalSearchMode::Exact {
            lexical
        } else {
            let query_embedding = self
                .vectors
                .query_embeddings
                .get(&request.text)
                .ok_or_else(|| format!("missing query embedding for {}", request.text))?;
            let mut semantic = corpus
                .iter()
                .map(|passage| {
                    let passage_embedding = self
                        .vectors
                        .passage_embeddings
                        .get(&passage.id)
                        .ok_or_else(|| format!("missing passage embedding for {}", passage.id))?;
                    let similarity = cosine_similarity(query_embedding, passage_embedding)?;
                    Ok((passage.id.clone(), similarity))
                })
                .collect::<std::result::Result<Vec<_>, String>>()?;
            semantic.sort_by(|left, right| {
                right
                    .1
                    .total_cmp(&left.1)
                    .then_with(|| left.0.cmp(&right.0))
            });
            let semantic = semantic
                .into_iter()
                .take(lexical_candidate_limit)
                .map(|(passage_id, _)| passage_id)
                .collect();
            fuse_ranked_passages(&query, lexical, semantic)
        };
        hydrate_fixture_hits(ranked, corpus, limit, mode == LexicalSearchMode::Default)
    }
}

fn hydrate_fixture_hits(
    ranked: Vec<RankedLexicalDocument>,
    corpus: &[EvaluationPassage],
    limit: usize,
    collapse_tidbits: bool,
) -> std::result::Result<Vec<RetrievalHit>, String> {
    let passages = corpus
        .iter()
        .map(|passage| (passage.id.as_str(), passage))
        .collect::<BTreeMap<_, _>>();
    let mut seen_tidbit_locators = BTreeMap::<&str, Vec<&EvaluationLocator>>::new();
    let mut hits = Vec::with_capacity(limit);
    for ranked in ranked {
        let passage = passages
            .get(ranked.passage_id.as_str())
            .ok_or_else(|| format!("ranked unknown passage {}", ranked.passage_id))?;
        if collapse_tidbits {
            let locators = seen_tidbit_locators
                .entry(passage.tidbit_id.as_str())
                .or_default();
            let overlaps = locators
                .iter()
                .any(|locator| evaluation_locators_overlap(locator, &passage.locator));
            locators.push(&passage.locator);
            if overlaps {
                continue;
            }
        }
        hits.push(RetrievalHit {
            passage_id: ranked.passage_id,
            score: ranked.score,
            locator: passage.locator.clone(),
            matched_fields: ranked
                .matched_fields
                .into_iter()
                .map(|field| field.label().into())
                .collect(),
        });
        if hits.len() == limit {
            break;
        }
    }
    Ok(hits)
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

fn cosine_similarity(left: &[f32], right: &[f32]) -> std::result::Result<f64, String> {
    if left.len() != right.len() {
        return Err(format!(
            "embedding dimensions differ: {} and {}",
            left.len(),
            right.len()
        ));
    }
    Ok(left
        .iter()
        .zip(right)
        .map(|(left, right)| f64::from(*left) * f64::from(*right))
        .sum())
}

fn hybrid_error<T>(message: String) -> super::Result<T> {
    Err(RelevanceError::HybridVectors(message))
}

#[cfg(test)]
mod tests {
    use super::{HybridFixtureRetriever, HybridVectorFixture};
    use crate::relevance::{
        run_relevance_suite, LexicalFixtureRetriever, QueryCategory, RelevanceFixture,
    };

    #[test]
    fn pinned_hybrid_vectors_improve_the_lexical_baseline_without_precision_regressions() {
        let fixture: RelevanceFixture =
            serde_json::from_str(include_str!("../../../fixtures/relevance/v1.json"))
                .expect("checked-in relevance fixture");
        let vectors: HybridVectorFixture = serde_json::from_str(include_str!(
            "../../../fixtures/relevance/jina-v1-vectors.json"
        ))
        .expect("checked-in hybrid vectors");
        let mut retriever =
            HybridFixtureRetriever::new(&fixture, vectors).expect("validated hybrid retriever");
        let hybrid =
            run_relevance_suite(&fixture, &mut retriever).expect("hybrid relevance report");
        let lexical = run_relevance_suite(&fixture, &mut LexicalFixtureRetriever)
            .expect("lexical relevance report");

        assert!(hybrid.passed);
        assert!(hybrid.summary.recall_at_10 >= lexical.summary.recall_at_10);
        assert!(hybrid.summary.mean_reciprocal_rank >= lexical.summary.mean_reciprocal_rank);
        assert!(hybrid.summary.ndcg_at_10 >= lexical.summary.ndcg_at_10);
        assert_eq!(hybrid.summary.forbidden_hits_at_10, 0);
        for category in [QueryCategory::Exact, QueryCategory::CodeIdentifier] {
            assert_eq!(
                hybrid.categories.get(&category),
                lexical.categories.get(&category)
            );
        }

        let mut generated_json =
            serde_json::to_string_pretty(&hybrid).expect("serialize hybrid report");
        generated_json.push('\n');
        assert_eq!(
            generated_json,
            include_str!("../../../fixtures/relevance/reports/hybrid-v1.json")
        );
        assert_eq!(
            hybrid.to_text(),
            include_str!("../../../fixtures/relevance/reports/hybrid-v1.txt")
        );
    }
}

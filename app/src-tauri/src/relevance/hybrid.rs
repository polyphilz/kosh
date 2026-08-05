use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    database::{
        block_embedding_index,
        relevance_search::{
            candidate_limit, fuse_ranked_blocks, parse_lexical_query, rank_lexical_documents,
            LexicalSearchMode, RankedSemanticBlock,
        },
    },
    embedding, EmbeddingRuntime,
};

use super::{
    lexical::{
        fixture_candidate_ranks, fixture_evidence_kind, fixture_fields, hydrate_fixture_hits,
    },
    EvaluationBlock, RelevanceError, RelevanceFixture, RetrievalHit, RetrievalRequest, Retriever,
    SearchMode,
};

pub const HYBRID_VECTOR_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HybridVectorFixture {
    pub schema_version: u32,
    pub fixture_digest: String,
    pub embedding_index_id: String,
    pub model_file_sha256: String,
    pub block_embeddings: BTreeMap<String, Vec<f32>>,
    pub query_embeddings: BTreeMap<String, Vec<f32>>,
}

pub fn generate_hybrid_vector_fixture(
    fixture: &RelevanceFixture,
    runtime: &EmbeddingRuntime,
) -> super::Result<HybridVectorFixture> {
    fixture.validate()?;
    let manifest = embedding::jina_v1_manifest();
    let mut block_embeddings = BTreeMap::new();
    for block in &fixture.corpus {
        let embedding = runtime
            .embed_document(&fixture_embedding_input(block))
            .map_err(|error| RelevanceError::HybridVectors(error.public_message()))?;
        if block_embeddings
            .insert(block.id.clone(), embedding)
            .is_some()
        {
            return hybrid_error(format!("duplicate block ID {}", block.id));
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
        block_embeddings,
        query_embeddings,
    };
    validate_hybrid_vector_fixture(fixture, &vectors)?;
    Ok(vectors)
}

fn fixture_embedding_input(block: &EvaluationBlock) -> String {
    block
        .heading_context
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(block.body.as_str()))
        .chain(block.attachment_names.iter().map(String::as_str))
        .chain(std::iter::once(block.extracted_text.as_str()))
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
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
    let manifest = embedding::jina_v1_manifest();
    if vectors.fixture_digest != fixture.digest()? {
        return hybrid_error("hybrid vectors do not match the relevance fixture".into());
    }
    if vectors.embedding_index_id != manifest.id
        || vectors.model_file_sha256 != manifest.model_file_sha256
    {
        return hybrid_error("hybrid vectors do not match the shipped embedding index".into());
    }
    let expected_blocks = fixture
        .corpus
        .iter()
        .map(|block| block.id.as_str())
        .collect::<BTreeSet<_>>();
    let actual_blocks = vectors
        .block_embeddings
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if actual_blocks != expected_blocks {
        return hybrid_error("hybrid vectors do not cover the exact block corpus".into());
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
        .block_embeddings
        .iter()
        .chain(vectors.query_embeddings.iter())
    {
        block_embedding_index::validate_embedding(vector, manifest.dimension as usize).map_err(
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
        corpus: &[EvaluationBlock],
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
                let block = &corpus[index];
                crate::database::relevance_search::LexicalDocument {
                    block_id: block.id.clone(),
                    updated_at_ms: 0,
                    evidence_kind: fixture_evidence_kind(block),
                    fields: fixture_fields(block),
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
                .map(|block| {
                    let block_embedding = self
                        .vectors
                        .block_embeddings
                        .get(&block.id)
                        .ok_or_else(|| format!("missing block embedding for {}", block.id))?;
                    let similarity = cosine_similarity(query_embedding, block_embedding)?;
                    let evidence_kind = fixture_evidence_kind(block);
                    Ok((
                        block.id.clone(),
                        evidence_kind.adjusted_semantic_similarity(similarity),
                        evidence_kind,
                    ))
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
                .map(|(block_id, _, evidence_kind)| RankedSemanticBlock {
                    block_id,
                    evidence_kind,
                })
                .collect();
            fuse_ranked_blocks(&query, lexical, semantic)
        };
        hydrate_fixture_hits(ranked, corpus, limit)
    }
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
    fn pinned_hybrid_vectors_meet_block_retrieval_contracts() {
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
        assert_eq!(hybrid.summary.recall_at_10, 1.0);
        assert_eq!(hybrid.summary.expected_block_accuracy, 1.0);
        assert!(hybrid.summary.mean_reciprocal_rank >= 0.95);
        assert!(hybrid.summary.ndcg_at_10 >= 0.95);
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

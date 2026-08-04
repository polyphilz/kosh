use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap},
};

use rusqlite::{params, types::Type, Connection, Transaction};
use serde::{Deserialize, Serialize};
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

use super::{
    embedding_index, passages, CitationLocator, CitationResolution, CitationTidbit, DatabaseError,
    Result,
};

const MAX_QUERY_CHARACTERS: usize = 512;
const MAX_SEARCH_LIMIT: u32 = 100;
const MAX_HIGHLIGHTS_PER_RESULT: usize = 32;
const LEXICAL_CANDIDATE_EXPANSION: u32 = 16;
const MIN_CANDIDATE_LIMIT: u32 = 64;
const MAX_CANDIDATE_LIMIT: u32 = 512;
const RRF_RANK_CONSTANT: f64 = 60.0;
const LEXICAL_RRF_WEIGHT: f64 = 1.0;
const SEMANTIC_RRF_WEIGHT: f64 = 0.85;
const AGREEMENT_RRF_WEIGHT: f64 = 0.20;
const PHRASE_RRF_WEIGHT: f64 = 0.15;
const HEADING_RRF_WEIGHT: f64 = 0.10;
const SEMANTIC_EXPANSION_RANK_LIMIT: usize = 1;
const SEMANTIC_RERANK_EXPANSION: u32 = 2;
const MAX_SEMANTIC_RERANK_CANDIDATES: u32 = 1_024;
const SEMANTIC_EVIDENCE_PENALTY: f64 = 0.1;
const INITIAL_RESULTS_PER_ATTACHMENT: usize = 2;
pub(crate) const FTS_BM25_WEIGHTS: &str = "6.0, 3.5, 5.0, 2.25";
pub(super) const FTS_VERSION: &str = "lexical-v5";

pub(crate) fn candidate_limit(result_limit: u32) -> u32 {
    result_limit
        .saturating_mul(LEXICAL_CANDIDATE_EXPANSION)
        .clamp(MIN_CANDIDATE_LIMIT, MAX_CANDIDATE_LIMIT)
}

pub(crate) fn trigram_candidate_limit(result_limit: u32) -> u32 {
    result_limit
        .saturating_mul(4)
        .clamp(MIN_CANDIDATE_LIMIT, MAX_CANDIDATE_LIMIT)
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LexicalSearchMode {
    Default,
    Exact,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchPassagesInput {
    pub query: String,
    pub mode: LexicalSearchMode,
    pub limit: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SearchField {
    HeadingContext,
    Body,
    AttachmentName,
    ExtractedText,
}

impl SearchField {
    const fn weight(self, evidence_kind: SearchEvidenceKind) -> f64 {
        match self {
            Self::HeadingContext => 6.0,
            Self::Body => 3.5,
            Self::AttachmentName => 5.0,
            Self::ExtractedText => evidence_kind.extracted_text_weight(),
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::HeadingContext => "headingContext",
            Self::Body => "body",
            Self::AttachmentName => "attachmentName",
            Self::ExtractedText => "extractedText",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SearchEvidenceKind {
    Author,
    Ocr,
    Pdf,
    Text,
}

impl SearchEvidenceKind {
    fn from_locator_kind(locator_kind: &str) -> Result<Self> {
        match locator_kind {
            "MARKDOWN_BLOCKS" => Ok(Self::Author),
            "OCR_REGION" => Ok(Self::Ocr),
            "PDF_PAGE" => Ok(Self::Pdf),
            "TEXT_LINES" => Ok(Self::Text),
            kind => Err(DatabaseError::Validation {
                kind: "main",
                reason: format!("search passage has unknown locator kind {kind}"),
            }),
        }
    }

    const fn extracted_text_weight(self) -> f64 {
        match self {
            Self::Author => 0.0,
            Self::Ocr => 1.75,
            Self::Pdf => 2.25,
            Self::Text => 2.5,
        }
    }

    pub(crate) const fn semantic_weight(self) -> f64 {
        match self {
            Self::Author => 1.0,
            Self::Text => 0.95,
            Self::Pdf => 0.9,
            Self::Ocr => 0.8,
        }
    }

    pub(crate) fn adjusted_semantic_similarity(self, similarity: f64) -> f64 {
        similarity - (1.0 - self.semantic_weight()) * SEMANTIC_EVIDENCE_PENALTY
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchHighlight {
    pub field: SearchField,
    pub start_char: u32,
    pub end_char: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PassageSearchResult {
    pub passage_id: String,
    pub score: f64,
    pub matched_fields: Vec<SearchField>,
    pub highlights: Vec<SearchHighlight>,
    pub note: CitationTidbit,
    pub citation: CitationResolution,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SearchExecutionMode {
    Exact,
    Hybrid,
    LexicalOnly,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SemanticSearchReadiness {
    Ready,
    Indexing,
    WaitingForRuntime,
    Failed,
    NotRequested,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchPassagesResponse {
    pub results: Vec<PassageSearchResult>,
    pub execution_mode: SearchExecutionMode,
    pub semantic_readiness: SemanticSearchReadiness,
}

#[derive(Clone, Debug)]
pub(crate) struct LexicalDocument {
    pub passage_id: String,
    pub updated_at_ms: i64,
    pub evidence_kind: SearchEvidenceKind,
    pub fields: BTreeMap<SearchField, String>,
    pub word_rank: Option<usize>,
    pub trigram_rank: Option<usize>,
    pub short_rank: Option<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct RankedLexicalDocument {
    pub passage_id: String,
    pub score: f64,
    pub evidence_kind: SearchEvidenceKind,
    pub matched_fields: Vec<SearchField>,
    pub highlights: Vec<SearchHighlight>,
}

#[derive(Clone, Debug)]
pub(crate) struct RankedSemanticPassage {
    pub passage_id: String,
    pub evidence_kind: SearchEvidenceKind,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SearchDiversityKey {
    pub attachment_id: Option<String>,
    pub page: Option<u32>,
}

#[derive(Clone, Debug)]
struct QueryAtom {
    text: String,
    quoted: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedLexicalQuery {
    atoms: Vec<QueryAtom>,
    mode: LexicalSearchMode,
}

impl ParsedLexicalQuery {
    fn parse(query: &str, mode: LexicalSearchMode) -> Result<Option<Self>> {
        if query.chars().count() > MAX_QUERY_CHARACTERS {
            return Err(DatabaseError::InvalidInput(format!(
                "query must contain at most {MAX_QUERY_CHARACTERS} characters"
            )));
        }
        let atoms = parse_atoms(query);
        Ok((!atoms.is_empty()).then_some(Self { atoms, mode }))
    }

    pub(crate) fn word_match_query(&self) -> Option<String> {
        let mut clauses = Vec::new();
        for atom in &self.atoms {
            let tokens = word_tokens(&atom.text);
            if tokens.is_empty() {
                continue;
            }
            if atom.quoted && tokens.len() > 1 {
                clauses.push(format!("\"{}\"", tokens.join(" ")));
            } else {
                clauses.extend(tokens.into_iter().map(|token| format!("\"{token}\"")));
            }
        }
        join_fts_clauses(clauses, self.mode)
    }

    pub(crate) fn trigram_match_query(&self) -> Option<String> {
        let mut clauses = BTreeSet::new();
        for atom in &self.atoms {
            let text = normalize_for_search(atom.text.trim());
            let characters = text.chars().collect::<Vec<_>>();
            if characters.len() < 3 {
                continue;
            }
            clauses.insert(format!("\"{}\"", text.replace('"', "\"\"")));
            if self.mode == LexicalSearchMode::Default && self.atoms.len() <= 4 && !atom.quoted {
                clauses.extend(
                    characters
                        .windows(3)
                        .map(|window| format!("\"{}\"", window.iter().collect::<String>())),
                );
            }
        }
        join_fts_clauses(clauses, self.mode)
    }

    pub(crate) fn short_match_query(&self) -> Option<String> {
        if self
            .atoms
            .iter()
            .any(|atom| normalize_for_search(atom.text.trim()).chars().count() > 2)
        {
            return None;
        }
        let clauses = self
            .atoms
            .iter()
            .filter_map(|atom| {
                let characters = normalize_for_search(atom.text.trim())
                    .chars()
                    .collect::<Vec<_>>();
                ((1..=2).contains(&characters.len())
                    && characters
                        .iter()
                        .any(|character| character.is_alphanumeric() || *character == '_'))
                .then(|| format!("\"{}\"", short_gram_token(&characters)))
            })
            .collect::<Vec<_>>();
        join_fts_clauses(clauses, self.mode)
    }

    fn has_quoted_phrase(&self) -> bool {
        self.atoms
            .iter()
            .any(|atom| atom.quoted && word_tokens(&atom.text).len() > 1)
    }

    fn has_searchable_content(&self) -> bool {
        self.atoms.iter().any(|atom| {
            atom.text
                .chars()
                .any(|character| character.is_alphanumeric() || character == '_')
        })
    }
}

#[cfg(test)]
pub(super) fn search_passages(
    connection: &Connection,
    input: SearchPassagesInput,
) -> Result<Vec<PassageSearchResult>> {
    Ok(search_passages_with_semantics(
        connection,
        input,
        None,
        SemanticSearchReadiness::WaitingForRuntime,
    )?
    .results)
}

pub(crate) fn search_passages_with_semantics(
    connection: &Connection,
    input: SearchPassagesInput,
    query_embedding: Option<&[f32]>,
    fallback_readiness: SemanticSearchReadiness,
) -> Result<SearchPassagesResponse> {
    if input.limit == 0 || input.limit > MAX_SEARCH_LIMIT {
        return Err(DatabaseError::InvalidInput(format!(
            "limit must be between 1 and {MAX_SEARCH_LIMIT}"
        )));
    }
    let Some(query) = ParsedLexicalQuery::parse(&input.query, input.mode)?
        .filter(ParsedLexicalQuery::has_searchable_content)
    else {
        return Ok(SearchPassagesResponse {
            results: Vec::new(),
            execution_mode: match input.mode {
                LexicalSearchMode::Default => SearchExecutionMode::LexicalOnly,
                LexicalSearchMode::Exact => SearchExecutionMode::Exact,
            },
            semantic_readiness: match input.mode {
                LexicalSearchMode::Default => fallback_readiness,
                LexicalSearchMode::Exact => SemanticSearchReadiness::NotRequested,
            },
        });
    };
    let candidate_limit = candidate_limit(input.limit);
    let lexical = lexical_ranked_candidates(
        connection,
        &query,
        candidate_limit,
        trigram_candidate_limit(input.limit),
        input.limit as usize,
    )?;
    if input.mode == LexicalSearchMode::Exact {
        return Ok(SearchPassagesResponse {
            results: hydrate_ranked_passages(connection, lexical, input.limit as usize, false)?,
            execution_mode: SearchExecutionMode::Exact,
            semantic_readiness: SemanticSearchReadiness::NotRequested,
        });
    }

    let index_readiness = semantic_index_readiness(connection)?;
    let semantic_ready =
        query_embedding.is_some() && index_readiness == SemanticSearchReadiness::Ready;
    if !semantic_ready {
        return Ok(SearchPassagesResponse {
            results: hydrate_ranked_passages(connection, lexical, input.limit as usize, false)?,
            execution_mode: SearchExecutionMode::LexicalOnly,
            semantic_readiness: if query_embedding.is_some() {
                index_readiness
            } else {
                fallback_readiness
            },
        });
    }

    let query_embedding = query_embedding.expect("semantic readiness requires an embedding");
    let manifest = embedding_index::manifest();
    embedding_index::validate_embedding(query_embedding, manifest.dimension as usize)?;
    let semantic = match semantic_ranked_passages(connection, query_embedding, candidate_limit) {
        Ok(semantic) => semantic,
        Err(error) => {
            log::warn!("semantic passage retrieval failed; using lexical search: {error}");
            let _ = embedding_index::quarantine(
                connection,
                "semantic passage search is unavailable; repair is required",
                0,
            );
            return Ok(SearchPassagesResponse {
                results: hydrate_ranked_passages(connection, lexical, input.limit as usize, false)?,
                execution_mode: SearchExecutionMode::LexicalOnly,
                semantic_readiness: SemanticSearchReadiness::Failed,
            });
        }
    };
    let fused = fuse_ranked_passages(&query, lexical, semantic);
    Ok(SearchPassagesResponse {
        results: hydrate_ranked_passages(connection, fused, input.limit as usize, true)?,
        execution_mode: SearchExecutionMode::Hybrid,
        semantic_readiness: SemanticSearchReadiness::Ready,
    })
}

pub(crate) fn validate_search_input(input: &SearchPassagesInput) -> Result<bool> {
    if input.limit == 0 || input.limit > MAX_SEARCH_LIMIT {
        return Err(DatabaseError::InvalidInput(format!(
            "limit must be between 1 and {MAX_SEARCH_LIMIT}"
        )));
    }
    Ok(ParsedLexicalQuery::parse(&input.query, input.mode)?
        .is_some_and(|query| query.has_searchable_content()))
}

fn lexical_ranked_candidates(
    connection: &Connection,
    query: &ParsedLexicalQuery,
    limit: u32,
    trigram_limit: u32,
    result_limit: usize,
) -> Result<Vec<RankedLexicalDocument>> {
    let mut ranks = HashMap::<String, CandidateRanks>::new();
    let word_query = query.word_match_query();
    let mut word_saturated = false;
    if let Some(word_query) = word_query.as_deref() {
        let word_candidates = query_fts_index(connection, "passage_fts_word", word_query, limit)?;
        word_saturated = word_candidates.len() == limit as usize;
        install_ranks(&mut ranks, word_candidates, CandidateIndex::Word);
    }
    let short_query = query.short_match_query();
    let mut short_saturated = false;
    if let Some(short_query) = short_query.as_deref() {
        let short_candidates =
            query_fts_index(connection, "passage_fts_short", short_query, limit)?;
        short_saturated = short_candidates.len() == limit as usize;
        install_ranks(&mut ranks, short_candidates, CandidateIndex::Short);
    }

    // Exact-token evidence is preferable to substrings. Avoid scanning the
    // trigram index across the whole corpus when a saturated word pool already
    // produces a complete page after authoritative qualification. Rare,
    // tampered, and underfilled word pools still continue through trigrams.
    if word_saturated && !ranks.is_empty() {
        let word_ranked = rank_candidate_documents(connection, query, ranks.clone(), limit)?;
        if word_ranked.len() >= result_limit {
            return Ok(word_ranked);
        }
    }

    let trigram_query = query.trigram_match_query();
    let mut trigram_saturated = false;
    if let Some(trigram_query) = trigram_query.as_deref() {
        let trigram_candidates = query_fts_index(
            connection,
            "passage_fts_trigram",
            trigram_query,
            trigram_limit,
        )?;
        trigram_saturated = trigram_candidates.len() == trigram_limit as usize;
        install_ranks(&mut ranks, trigram_candidates, CandidateIndex::Trigram);
    }
    if ranks.is_empty() {
        return Ok(Vec::new());
    }

    let ranked = rank_candidate_documents(connection, query, ranks.clone(), limit)?;
    let saturated_pools_are_exhausted = (!word_saturated || limit >= MAX_CANDIDATE_LIMIT)
        && (!trigram_saturated || trigram_limit >= MAX_CANDIDATE_LIMIT)
        && (!short_saturated || limit >= MAX_CANDIDATE_LIMIT);
    if ranked.len() >= result_limit || saturated_pools_are_exhausted {
        return Ok(ranked);
    }

    // FTS rows nominate candidates, while immutable authored/extracted evidence
    // decides whether they qualify. Widen only saturated pools when the
    // authoritative pass underfills so stale or tampered derived rows cannot
    // hide a valid result without charging every healthy query for 512 rows.
    if word_saturated {
        install_ranks(
            &mut ranks,
            query_fts_index(
                connection,
                "passage_fts_word",
                word_query
                    .as_deref()
                    .expect("word saturation requires a word query"),
                MAX_CANDIDATE_LIMIT,
            )?,
            CandidateIndex::Word,
        );
    }
    if trigram_saturated {
        install_ranks(
            &mut ranks,
            query_fts_index(
                connection,
                "passage_fts_trigram",
                trigram_query
                    .as_deref()
                    .expect("trigram saturation requires a trigram query"),
                MAX_CANDIDATE_LIMIT,
            )?,
            CandidateIndex::Trigram,
        );
    }
    if short_saturated {
        install_ranks(
            &mut ranks,
            query_fts_index(
                connection,
                "passage_fts_short",
                short_query
                    .as_deref()
                    .expect("short saturation requires a short query"),
                MAX_CANDIDATE_LIMIT,
            )?,
            CandidateIndex::Short,
        );
    }
    rank_candidate_documents(connection, query, ranks, MAX_CANDIDATE_LIMIT)
}

fn rank_candidate_documents(
    connection: &Connection,
    query: &ParsedLexicalQuery,
    ranks: HashMap<String, CandidateRanks>,
    limit: u32,
) -> Result<Vec<RankedLexicalDocument>> {
    let documents = load_search_documents(connection, ranks)?;
    Ok(rank_lexical_documents(query, documents, limit as usize))
}

pub(crate) fn semantic_index_readiness(connection: &Connection) -> Result<SemanticSearchReadiness> {
    let manifest = embedding_index::manifest();
    let (version, status, active_index_id, has_reap_work) = connection.query_row(
        "SELECT
            state.version,
            state.status,
            settings.active_embedding_index_id,
            EXISTS(SELECT 1 FROM passage_embedding_reap_queue)
         FROM index_state AS state
         JOIN passage_embedding_settings AS settings
           ON settings.singleton_id = 1
         WHERE state.name = 'PASSAGE_EMBEDDING'",
        [],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, bool>(3)?,
            ))
        },
    )?;
    if version != manifest.index_key || status == "FAILED" {
        return Ok(SemanticSearchReadiness::Failed);
    }
    if !matches!(status.as_str(), "IDLE" | "DIRTY" | "RUNNING") {
        return Err(DatabaseError::Validation {
            kind: "main",
            reason: format!("PASSAGE_EMBEDDING has unknown status {status}"),
        });
    }
    Ok(
        if status == "IDLE"
            && active_index_id.as_deref() == Some(manifest.id.as_str())
            && !has_reap_work
        {
            SemanticSearchReadiness::Ready
        } else {
            SemanticSearchReadiness::Indexing
        },
    )
}

fn semantic_ranked_passages(
    connection: &Connection,
    embedding: &[f32],
    limit: u32,
) -> Result<Vec<RankedSemanticPassage>> {
    let manifest = embedding_index::manifest();
    let vector_json = serde_json::to_string(embedding)?;
    let rerank_limit = limit
        .saturating_mul(SEMANTIC_RERANK_EXPANSION)
        .min(MAX_SEMANTIC_RERANK_CANDIDATES);
    let mut statement = connection.prepare(
        "SELECT
            document.passage_id,
            passage.locator_kind,
            nearest.distance,
            document.updated_at
         FROM (
             SELECT rowid, distance
             FROM passage_embedding_vec_jina_v1
             WHERE embedding MATCH ?1 AND k = ?2
             ORDER BY distance, rowid
         ) AS nearest
         JOIN passage_search_document AS document
           ON document.rowid = nearest.rowid
         JOIN passage
           ON passage.rowid = document.rowid
          AND passage.id = document.passage_id
         JOIN passage_embedding AS metadata
           ON metadata.passage_id = passage.id
          AND metadata.embedding_index_id = ?3
          AND metadata.passage_content_hash = passage.content_hash
         JOIN passage_embedding_settings AS settings
           ON settings.singleton_id = 1
          AND settings.active_embedding_index_id = metadata.embedding_index_id
         JOIN index_state AS state
           ON state.name = 'PASSAGE_EMBEDDING'
          AND state.version = ?4
          AND state.status = 'IDLE'
         WHERE NOT EXISTS (
             SELECT 1 FROM passage_embedding_reap_queue
         )
         ORDER BY nearest.distance, document.updated_at DESC, document.passage_id",
    )?;
    let rows = statement
        .query_map(
            params![vector_json, rerank_limit, manifest.id, manifest.index_key],
            |row| {
                let locator_kind = row.get::<_, String>(1)?;
                Ok((
                    row.get::<_, String>(0)?,
                    locator_kind,
                    row.get::<_, f64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut ranked = rows
        .into_iter()
        .map(
            |(passage_id, locator_kind, distance, updated_at)| -> Result<_> {
                if !distance.is_finite() {
                    return Err(DatabaseError::Validation {
                        kind: "main",
                        reason: format!("semantic distance for passage {passage_id} is not finite"),
                    });
                }
                let evidence_kind = SearchEvidenceKind::from_locator_kind(&locator_kind)?;
                let adjusted_similarity =
                    evidence_kind.adjusted_semantic_similarity(1.0 - distance);
                Ok((
                    RankedSemanticPassage {
                        passage_id,
                        evidence_kind,
                    },
                    adjusted_similarity,
                    updated_at,
                ))
            },
        )
        .collect::<Result<Vec<_>>>()?;
    ranked.sort_by(
        |(left, left_score, left_updated_at), (right, right_score, right_updated_at)| {
            right_score
                .total_cmp(left_score)
                .then_with(|| right_updated_at.cmp(left_updated_at))
                .then_with(|| left.passage_id.cmp(&right.passage_id))
        },
    );
    Ok(ranked
        .into_iter()
        .take(limit as usize)
        .map(|(candidate, _, _)| candidate)
        .collect())
}

pub(crate) fn fuse_ranked_passages(
    query: &ParsedLexicalQuery,
    lexical: Vec<RankedLexicalDocument>,
    semantic: Vec<RankedSemanticPassage>,
) -> Vec<RankedLexicalDocument> {
    struct FusedCandidate {
        passage_id: String,
        score: f64,
        best_rank: usize,
        lexical_rank: Option<usize>,
        semantic_rank: Option<usize>,
        evidence_kind: SearchEvidenceKind,
        matched_fields: Vec<SearchField>,
        highlights: Vec<SearchHighlight>,
    }

    let mut candidates = HashMap::<String, FusedCandidate>::new();
    for (index, candidate) in lexical.into_iter().enumerate() {
        let rank = index + 1;
        let heading = candidate
            .matched_fields
            .iter()
            .any(|field| matches!(field, SearchField::HeadingContext));
        let mut score = LEXICAL_RRF_WEIGHT / (RRF_RANK_CONSTANT + rank as f64);
        if query.has_quoted_phrase() {
            score += PHRASE_RRF_WEIGHT / (RRF_RANK_CONSTANT + rank as f64);
        }
        if heading {
            score += HEADING_RRF_WEIGHT / (RRF_RANK_CONSTANT + rank as f64);
        }
        candidates.insert(
            candidate.passage_id.clone(),
            FusedCandidate {
                passage_id: candidate.passage_id,
                score,
                best_rank: rank,
                lexical_rank: Some(rank),
                semantic_rank: None,
                evidence_kind: candidate.evidence_kind,
                matched_fields: candidate.matched_fields,
                highlights: candidate.highlights,
            },
        );
    }
    for (index, semantic) in semantic.into_iter().enumerate() {
        let rank = index + 1;
        if !candidates.contains_key(&semantic.passage_id) && rank > SEMANTIC_EXPANSION_RANK_LIMIT {
            continue;
        }
        let candidate = candidates
            .entry(semantic.passage_id.clone())
            .or_insert_with(|| FusedCandidate {
                passage_id: semantic.passage_id,
                score: 0.0,
                best_rank: rank,
                lexical_rank: None,
                semantic_rank: None,
                evidence_kind: semantic.evidence_kind,
                matched_fields: Vec::new(),
                highlights: Vec::new(),
            });
        candidate.score += SEMANTIC_RRF_WEIGHT * candidate.evidence_kind.semantic_weight()
            / (RRF_RANK_CONSTANT + rank as f64);
        candidate.best_rank = candidate.best_rank.min(rank);
        candidate.semantic_rank = Some(rank);
        if let Some(lexical_rank) = candidate.lexical_rank {
            candidate.score +=
                AGREEMENT_RRF_WEIGHT / (RRF_RANK_CONSTANT + lexical_rank.min(rank) as f64);
        }
    }
    let mut candidates = candidates.into_values().collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.best_rank.cmp(&right.best_rank))
            .then_with(|| {
                left.lexical_rank
                    .is_none()
                    .cmp(&right.lexical_rank.is_none())
            })
            .then_with(|| left.lexical_rank.cmp(&right.lexical_rank))
            .then_with(|| left.semantic_rank.cmp(&right.semantic_rank))
            .then_with(|| left.passage_id.cmp(&right.passage_id))
    });
    candidates
        .into_iter()
        .map(|candidate| RankedLexicalDocument {
            passage_id: candidate.passage_id,
            score: candidate.score,
            evidence_kind: candidate.evidence_kind,
            matched_fields: candidate.matched_fields,
            highlights: candidate.highlights,
        })
        .collect()
}

fn hydrate_ranked_passages(
    connection: &Connection,
    ranked: Vec<RankedLexicalDocument>,
    limit: usize,
    collapse_tidbits: bool,
) -> Result<Vec<PassageSearchResult>> {
    struct HydratedCandidate {
        ranked: RankedLexicalDocument,
        note: CitationTidbit,
        citation: CitationResolution,
    }

    let mut diversified = DiversitySelector::new(limit);
    let mut seen_tidbit_locators = BTreeMap::<String, Vec<CitationLocator>>::new();
    for ranked in ranked {
        let citation = passages::resolve_citation(connection, &ranked.passage_id)?;
        if citation.state != super::CitationState::Current {
            return Err(DatabaseError::Validation {
                kind: "main",
                reason: format!("search returned non-current passage {}", ranked.passage_id),
            });
        }
        let Some(note) = search_result_note(connection, &ranked.passage_id, &citation)? else {
            continue;
        };
        if collapse_tidbits {
            if let Some(tidbit) = citation.tidbit.as_ref() {
                let locators = seen_tidbit_locators.entry(tidbit.id.clone()).or_default();
                let overlaps = locators
                    .iter()
                    .any(|locator| citation_locators_overlap(locator, &citation.locator));
                locators.push(citation.locator.clone());
                if overlaps {
                    continue;
                }
            }
        }
        let diversity_key = citation_diversity_key(&citation);
        if diversified.push(
            HydratedCandidate {
                ranked,
                note,
                citation,
            },
            diversity_key,
        ) {
            break;
        }
    }
    Ok(diversified
        .finish()
        .into_iter()
        .map(|candidate| PassageSearchResult {
            passage_id: candidate.ranked.passage_id,
            score: candidate.ranked.score,
            matched_fields: candidate.ranked.matched_fields,
            highlights: candidate.ranked.highlights,
            note: candidate.note,
            citation: candidate.citation,
        })
        .collect())
}

fn search_result_note(
    connection: &Connection,
    passage_id: &str,
    citation: &CitationResolution,
) -> Result<Option<CitationTidbit>> {
    if let Some(note) = citation.tidbit.as_ref() {
        return Ok(Some(note.clone()));
    }
    let attachment_id = citation
        .attachment
        .as_ref()
        .map(|attachment| attachment.id.as_str())
        .ok_or_else(|| DatabaseError::Validation {
            kind: "main",
            reason: format!("search passage {passage_id} has no note or attachment owner"),
        })?;
    match connection.query_row(
        "SELECT
                tidbit.id,
                revision.id,
                revision.revision_number,
                revision.body_markdown
             FROM tidbit_revision_attachment AS membership
             JOIN tidbit_revision AS revision
               ON revision.id = membership.tidbit_revision_id
             JOIN tidbit
               ON tidbit.id = revision.tidbit_id
              AND tidbit.current_revision_id = revision.id
              AND tidbit.deleted_at IS NULL
             WHERE membership.attachment_id = ?1
             ORDER BY tidbit.updated_at DESC, tidbit.id
             LIMIT 1",
        params![attachment_id],
        |row| {
            let body_markdown = row.get::<_, String>(3)?;
            Ok(CitationTidbit {
                id: row.get(0)?,
                revision_id: row.get(1)?,
                revision_number: row.get(2)?,
                display_title: super::tidbits::derive_display_title(&body_markdown),
                deleted: false,
            })
        },
    ) {
        Ok(note) => Ok(Some(note)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn citation_diversity_key(citation: &CitationResolution) -> SearchDiversityKey {
    SearchDiversityKey {
        attachment_id: citation
            .attachment
            .as_ref()
            .map(|attachment| attachment.id.clone()),
        page: match &citation.locator {
            CitationLocator::PdfPage { page } => Some(*page),
            CitationLocator::OcrRegion { page, .. } => *page,
            CitationLocator::MarkdownBlocks { .. } | CitationLocator::TextLines { .. } => None,
        },
    }
}

pub(crate) fn diversify_ranked<T>(
    ranked: Vec<T>,
    limit: usize,
    diversity_key: impl Fn(&T) -> SearchDiversityKey,
) -> Vec<T> {
    let mut diversified = DiversitySelector::new(limit);
    for candidate in ranked {
        let key = diversity_key(&candidate);
        if diversified.push(candidate, key) {
            break;
        }
    }
    diversified.finish()
}

struct DiversitySelector<T> {
    limit: usize,
    selected: Vec<T>,
    deferred: Vec<T>,
    attachment_counts: HashMap<String, usize>,
    attachment_pages: BTreeSet<(String, u32)>,
}

impl<T> DiversitySelector<T> {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            selected: Vec::with_capacity(limit),
            deferred: Vec::new(),
            attachment_counts: HashMap::new(),
            attachment_pages: BTreeSet::new(),
        }
    }

    fn push(&mut self, candidate: T, key: SearchDiversityKey) -> bool {
        if self.limit == 0 || self.selected.len() == self.limit {
            return true;
        }
        let defer = key.attachment_id.as_ref().is_some_and(|attachment_id| {
            let attachment_full = self
                .attachment_counts
                .get(attachment_id)
                .copied()
                .unwrap_or_default()
                >= INITIAL_RESULTS_PER_ATTACHMENT;
            let page_seen = key.page.is_some_and(|page| {
                self.attachment_pages
                    .contains(&(attachment_id.clone(), page))
            });
            attachment_full || page_seen
        });
        if defer {
            self.deferred.push(candidate);
            return false;
        }
        if let Some(attachment_id) = key.attachment_id {
            *self
                .attachment_counts
                .entry(attachment_id.clone())
                .or_default() += 1;
            if let Some(page) = key.page {
                self.attachment_pages.insert((attachment_id, page));
            }
        }
        self.selected.push(candidate);
        self.selected.len() == self.limit
    }

    fn finish(mut self) -> Vec<T> {
        self.selected.extend(
            self.deferred
                .into_iter()
                .take(self.limit.saturating_sub(self.selected.len())),
        );
        self.selected
    }
}

fn citation_locators_overlap(left: &CitationLocator, right: &CitationLocator) -> bool {
    let (
        CitationLocator::MarkdownBlocks {
            start_block: left_start_block,
            end_block: left_end_block,
            source_start_byte: left_start_byte,
            source_end_byte: left_end_byte,
            start_char: left_start_char,
            end_char: left_end_char,
            start_line: left_start_line,
            end_line: left_end_line,
        },
        CitationLocator::MarkdownBlocks {
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

pub(super) fn replace_tidbit_documents(
    transaction: &Transaction<'_>,
    tidbit_id: &str,
) -> Result<()> {
    transaction.execute(
        "DELETE FROM passage_search_document WHERE tidbit_id = ?1",
        params![tidbit_id],
    )?;
    transaction.execute(
        "INSERT INTO passage_search_document(
            rowid,
            passage_id,
            tidbit_id,
            heading_context,
            body,
            attachment_names,
            extracted_text,
            owner_content_hash,
            updated_at
         )
         SELECT
            passage.rowid,
            passage.id,
            active.tidbit_id,
            coalesce(
                (
                    SELECT group_concat(value, char(10))
                    FROM json_each(passage.heading_context_json)
                ),
                ''
            ),
            passage.content,
            coalesce(
                (
                    SELECT group_concat(
                        attachment.display_filename || char(10) || attachment.media_type,
                        char(10)
                    )
                    FROM tidbit_revision_attachment AS membership
                    JOIN attachment ON attachment.id = membership.attachment_id
                    WHERE membership.tidbit_revision_id = revision.id
                      AND attachment.deleted_at IS NULL
                    ORDER BY membership.sort_order
                ),
                ''
            ),
            '',
            revision.content_hash,
            tidbit.updated_at
         FROM active_passage AS active
         JOIN passage ON passage.id = active.passage_id
         JOIN tidbit ON tidbit.id = active.tidbit_id
         JOIN tidbit_revision AS revision
           ON revision.id = passage.tidbit_revision_id
          AND revision.id = tidbit.current_revision_id
          AND revision.tidbit_id = tidbit.id
         WHERE active.tidbit_id = ?1
           AND tidbit.deleted_at IS NULL
           AND passage.owner_kind = 'AUTHOR'
         ORDER BY passage.ordinal",
        params![tidbit_id],
    )?;
    Ok(())
}

pub(super) fn replace_attachment_documents(
    connection: &mut Connection,
    attachment_id: &str,
) -> Result<()> {
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    replace_attachment_documents_in_transaction(&transaction, attachment_id)?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn replace_attachment_documents_in_transaction(
    transaction: &Transaction<'_>,
    attachment_id: &str,
) -> Result<()> {
    transaction.execute(
        "DELETE FROM passage_search_document
         WHERE passage_id IN (
            SELECT passage.id
            FROM passage
            JOIN attachment_segment AS segment
              ON segment.id = passage.attachment_segment_id
            JOIN attachment_extraction AS extraction
              ON extraction.id = segment.extraction_id
            WHERE passage.owner_kind = 'ATTACHMENT'
              AND extraction.attachment_id = ?1
         )",
        params![attachment_id],
    )?;
    transaction.execute(
        "INSERT INTO passage_search_document(
            rowid,
            passage_id,
            tidbit_id,
            heading_context,
            body,
            attachment_names,
            extracted_text,
            owner_content_hash,
            updated_at
         )
         SELECT
            passage.rowid,
            passage.id,
            NULL,
            coalesce(
                (
                    SELECT group_concat(value, char(10))
                    FROM json_each(passage.heading_context_json)
                ),
                ''
            ),
            '',
            attachment.display_filename || char(10) || attachment.media_type,
            passage.content,
            passage.content_hash,
            attachment.updated_at
         FROM current_attachment_passage AS current
         JOIN passage ON passage.id = current.passage_id
         JOIN attachment ON attachment.id = current.attachment_id
         WHERE current.attachment_id = ?1
         ORDER BY passage.ordinal",
        params![attachment_id],
    )?;
    Ok(())
}

pub(super) fn rebuild_documents(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute("DELETE FROM passage_search_document", [])?;
    transaction.execute(
        "INSERT INTO passage_search_document(
            rowid,
            passage_id,
            tidbit_id,
            heading_context,
            body,
            attachment_names,
            extracted_text,
            owner_content_hash,
            updated_at
         )
         SELECT
            passage.rowid,
            passage.id,
            active.tidbit_id,
            coalesce(
                (
                    SELECT group_concat(value, char(10))
                    FROM json_each(passage.heading_context_json)
                ),
                ''
            ),
            passage.content,
            coalesce(
                (
                    SELECT group_concat(
                        attachment.display_filename || char(10) || attachment.media_type,
                        char(10)
                    )
                    FROM tidbit_revision_attachment AS membership
                    JOIN attachment ON attachment.id = membership.attachment_id
                    WHERE membership.tidbit_revision_id = revision.id
                      AND attachment.deleted_at IS NULL
                    ORDER BY membership.sort_order
                ),
                ''
            ),
            '',
            revision.content_hash,
            tidbit.updated_at
         FROM active_passage AS active
         JOIN passage ON passage.id = active.passage_id
         JOIN tidbit ON tidbit.id = active.tidbit_id
         JOIN tidbit_revision AS revision
           ON revision.id = passage.tidbit_revision_id
          AND revision.id = tidbit.current_revision_id
          AND revision.tidbit_id = tidbit.id
         WHERE tidbit.deleted_at IS NULL
           AND passage.owner_kind = 'AUTHOR'
         ORDER BY active.tidbit_id, passage.ordinal",
        [],
    )?;
    transaction.execute(
        "INSERT INTO passage_search_document(
            rowid,
            passage_id,
            tidbit_id,
            heading_context,
            body,
            attachment_names,
            extracted_text,
            owner_content_hash,
            updated_at
         )
         SELECT
            passage.rowid,
            passage.id,
            NULL,
            coalesce(
                (
                    SELECT group_concat(value, char(10))
                    FROM json_each(passage.heading_context_json)
                ),
                ''
            ),
            '',
            attachment.display_filename || char(10) || attachment.media_type,
            passage.content,
            passage.content_hash,
            attachment.updated_at
         FROM current_attachment_passage AS current
         JOIN passage ON passage.id = current.passage_id
         JOIN attachment ON attachment.id = current.attachment_id
         ORDER BY attachment.id, passage.ordinal",
        [],
    )?;
    mark_fts_current(transaction)
}

fn mark_fts_current(transaction: &Transaction<'_>) -> Result<()> {
    let changed = transaction.execute(
        "UPDATE index_state
         SET version = ?1,
             status = 'IDLE',
             cursor = NULL,
             error = NULL
         WHERE name = 'PASSAGE_FTS'",
        params![FTS_VERSION],
    )?;
    if changed != 1 {
        return Err(DatabaseError::Validation {
            kind: "main",
            reason: "PASSAGE_FTS index state is missing".into(),
        });
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default)]
struct CandidateRanks {
    word: Option<usize>,
    trigram: Option<usize>,
    short: Option<usize>,
}

enum CandidateIndex {
    Word,
    Trigram,
    Short,
}

fn install_ranks(
    ranks: &mut HashMap<String, CandidateRanks>,
    passage_ids: Vec<String>,
    index: CandidateIndex,
) {
    for (position, passage_id) in passage_ids.into_iter().enumerate() {
        let rank = position + 1;
        let candidate = ranks.entry(passage_id).or_default();
        match index {
            CandidateIndex::Word => candidate.word = Some(rank),
            CandidateIndex::Trigram => candidate.trigram = Some(rank),
            CandidateIndex::Short => candidate.short = Some(rank),
        }
    }
}

fn query_fts_index(
    connection: &Connection,
    index: &'static str,
    query: &str,
    limit: u32,
) -> Result<Vec<String>> {
    let sql = format!(
        "SELECT document.passage_id
         FROM {index}
         JOIN passage_search_document AS document
           ON document.rowid = {index}.rowid
         WHERE {index} MATCH ?1
           AND {index}.rank MATCH 'bm25({FTS_BM25_WEIGHTS})'
         ORDER BY {index}.rank,
                  document.updated_at DESC,
                  document.passage_id
         LIMIT ?2"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(params![query, limit], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn load_search_documents(
    connection: &Connection,
    ranks: HashMap<String, CandidateRanks>,
) -> Result<Vec<LexicalDocument>> {
    let candidates = ranks
        .into_iter()
        .map(|(passage_id, ranks)| {
            serde_json::json!({
                "passageId": passage_id,
                "wordRank": ranks.word,
                "trigramRank": ranks.trigram,
                "shortRank": ranks.short,
            })
        })
        .collect::<Vec<_>>();
    let candidates_json =
        serde_json::to_string(&candidates).map_err(|error| DatabaseError::Validation {
            kind: "main",
            reason: format!("could not serialize lexical candidates: {error}"),
        })?;
    let mut statement = connection.prepare(
        "WITH candidate AS (
            SELECT
                json_extract(value, '$.passageId') AS passage_id,
                json_extract(value, '$.wordRank') AS word_rank,
                json_extract(value, '$.trigramRank') AS trigram_rank,
                json_extract(value, '$.shortRank') AS short_rank
            FROM json_each(?1)
         )
         SELECT
            candidate.passage_id,
            tidbit.updated_at,
            coalesce(
                (
                    SELECT group_concat(value, char(10))
                    FROM json_each(passage.heading_context_json)
                ),
                ''
            ),
            passage.content,
            coalesce(
                (
                    SELECT group_concat(
                        attachment.display_filename || char(10) || attachment.media_type,
                        char(10)
                    )
                    FROM tidbit_revision_attachment AS membership
                    JOIN attachment ON attachment.id = membership.attachment_id
                    WHERE membership.tidbit_revision_id = revision.id
                      AND attachment.deleted_at IS NULL
                    ORDER BY membership.sort_order
                ),
                ''
            ),
            '',
            passage.locator_kind,
            candidate.word_rank,
            candidate.trigram_rank,
            candidate.short_rank
         FROM candidate
         JOIN passage_search_document AS document
           ON document.passage_id = candidate.passage_id
         JOIN passage
           ON passage.id = document.passage_id
          AND passage.owner_kind = 'AUTHOR'
         JOIN active_passage AS active
           ON active.passage_id = passage.id
          AND active.tidbit_id = document.tidbit_id
         JOIN tidbit
           ON tidbit.id = active.tidbit_id
          AND tidbit.deleted_at IS NULL
          AND tidbit.current_revision_id = passage.tidbit_revision_id
         JOIN tidbit_revision AS revision
           ON revision.id = tidbit.current_revision_id
          AND revision.tidbit_id = tidbit.id
          AND revision.content_hash = document.owner_content_hash
         UNION ALL
         SELECT
            candidate.passage_id,
            attachment.updated_at,
            coalesce(
                (
                    SELECT group_concat(value, char(10))
                    FROM json_each(passage.heading_context_json)
                ),
                ''
            ),
            '',
            attachment.display_filename || char(10) || attachment.media_type,
            passage.content,
            passage.locator_kind,
            candidate.word_rank,
            candidate.trigram_rank,
            candidate.short_rank
         FROM candidate
         JOIN passage_search_document AS document
           ON document.passage_id = candidate.passage_id
          AND document.tidbit_id IS NULL
         JOIN passage
           ON passage.id = document.passage_id
          AND passage.owner_kind = 'ATTACHMENT'
          AND passage.content_hash = document.owner_content_hash
         JOIN current_attachment_passage AS current
           ON current.passage_id = passage.id
         JOIN attachment ON attachment.id = current.attachment_id",
    )?;
    let rows = statement.query_map(params![candidates_json], |row| {
        let word_rank = row
            .get::<_, Option<i64>>(7)?
            .and_then(|rank| usize::try_from(rank).ok());
        let trigram_rank = row
            .get::<_, Option<i64>>(8)?
            .and_then(|rank| usize::try_from(rank).ok());
        let short_rank = row
            .get::<_, Option<i64>>(9)?
            .and_then(|rank| usize::try_from(rank).ok());
        let locator_kind = row.get::<_, String>(6)?;
        let evidence_kind =
            SearchEvidenceKind::from_locator_kind(&locator_kind).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    6,
                    Type::Text,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        error.to_string(),
                    )),
                )
            })?;
        Ok(LexicalDocument {
            passage_id: row.get(0)?,
            updated_at_ms: row.get(1)?,
            evidence_kind,
            fields: [
                (SearchField::HeadingContext, row.get::<_, String>(2)?),
                (SearchField::Body, row.get::<_, String>(3)?),
                (SearchField::AttachmentName, row.get::<_, String>(4)?),
                (SearchField::ExtractedText, row.get::<_, String>(5)?),
            ]
            .into_iter()
            .collect(),
            word_rank,
            trigram_rank,
            short_rank,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub(crate) fn parse_lexical_query(
    query: &str,
    mode: LexicalSearchMode,
) -> Result<Option<ParsedLexicalQuery>> {
    ParsedLexicalQuery::parse(query, mode)
}

pub(crate) fn rank_lexical_documents(
    query: &ParsedLexicalQuery,
    documents: Vec<LexicalDocument>,
    limit: usize,
) -> Vec<RankedLexicalDocument> {
    let mut ranked = documents
        .into_iter()
        .filter_map(|document| score_document(query, document))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| right.updated_at_ms.cmp(&left.updated_at_ms))
            .then_with(|| left.passage_id.cmp(&right.passage_id))
    });
    ranked
        .into_iter()
        .take(limit)
        .map(|scored| RankedLexicalDocument {
            passage_id: scored.passage_id,
            score: scored.score,
            evidence_kind: scored.evidence_kind,
            matched_fields: scored.matched_fields,
            highlights: scored.highlights,
        })
        .collect()
}

struct ScoredDocument {
    passage_id: String,
    updated_at_ms: i64,
    score: f64,
    evidence_kind: SearchEvidenceKind,
    matched_fields: Vec<SearchField>,
    highlights: Vec<SearchHighlight>,
}

fn score_document(query: &ParsedLexicalQuery, document: LexicalDocument) -> Option<ScoredDocument> {
    let mut matched_atoms = vec![false; query.atoms.len()];
    let mut matched_fields = BTreeSet::new();
    let mut highlight_spans = BTreeSet::new();
    let mut field_score = 0.0;

    for (field, value) in &document.fields {
        if value.is_empty() {
            continue;
        }
        for (atom_index, atom) in query.atoms.iter().enumerate() {
            let fuzzy_enabled = query.mode == LexicalSearchMode::Default && query.atoms.len() <= 4;
            let (spans, match_quality) = find_atom_spans(value, atom, fuzzy_enabled);
            if spans.is_empty() {
                continue;
            }
            matched_atoms[atom_index] = true;
            matched_fields.insert(*field);
            let phrase_multiplier = if atom.quoted { 2.0 } else { 1.0 };
            field_score += field.weight(document.evidence_kind) * phrase_multiplier * match_quality;
            for (start_char, end_char) in spans {
                highlight_spans.insert((*field, start_char, end_char));
            }
        }
    }

    let matched_atom_count = matched_atoms.iter().filter(|matched| **matched).count();
    let qualifies = match query.mode {
        LexicalSearchMode::Default => {
            let required = query.atoms.len().min(2);
            matched_atom_count >= required
        }
        LexicalSearchMode::Exact => matched_atoms.iter().all(|matched| *matched),
    };
    if !qualifies {
        return None;
    }

    let coverage = matched_atom_count as f64 / query.atoms.len() as f64;
    let rank_score = document
        .word_rank
        .into_iter()
        .chain(document.trigram_rank)
        .chain(document.short_rank)
        .map(|rank| 1.0 / (RRF_RANK_CONSTANT + rank as f64))
        .sum::<f64>();
    let exact_mode_bonus = if query.mode == LexicalSearchMode::Exact {
        12.0
    } else {
        0.0
    };
    let score = field_score + coverage * 10.0 + rank_score + exact_mode_bonus;
    let highlights = highlight_spans
        .into_iter()
        .take(MAX_HIGHLIGHTS_PER_RESULT)
        .map(|(field, start_char, end_char)| SearchHighlight {
            field,
            start_char,
            end_char,
        })
        .collect();

    Some(ScoredDocument {
        passage_id: document.passage_id,
        updated_at_ms: document.updated_at_ms,
        score,
        evidence_kind: document.evidence_kind,
        matched_fields: matched_fields.into_iter().collect(),
        highlights,
    })
}

fn parse_atoms(query: &str) -> Vec<QueryAtom> {
    let mut atoms = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut current_quoted = false;
    for character in query.trim().chars() {
        match character {
            '"' => {
                if quoted {
                    push_atom(&mut atoms, &mut current, true);
                    quoted = false;
                    current_quoted = false;
                } else {
                    push_atom(&mut atoms, &mut current, current_quoted);
                    quoted = true;
                    current_quoted = true;
                }
            }
            character if character.is_whitespace() && !quoted => {
                push_atom(&mut atoms, &mut current, current_quoted);
                current_quoted = false;
            }
            _ => current.push(character),
        }
    }
    push_atom(&mut atoms, &mut current, current_quoted || quoted);
    atoms
}

fn push_atom(atoms: &mut Vec<QueryAtom>, current: &mut String, quoted: bool) {
    let text = current.trim();
    if !text.is_empty() {
        atoms.push(QueryAtom {
            text: text.to_owned(),
            quoted,
        });
    }
    current.clear();
}

fn word_tokens(value: &str) -> Vec<String> {
    normalize_for_search(value)
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

fn join_fts_clauses(
    clauses: impl IntoIterator<Item = String>,
    mode: LexicalSearchMode,
) -> Option<String> {
    let clauses = clauses.into_iter().collect::<Vec<_>>();
    (!clauses.is_empty()).then(|| {
        clauses.join(match mode {
            LexicalSearchMode::Default => " OR ",
            LexicalSearchMode::Exact => " AND ",
        })
    })
}

pub(crate) fn normalize_for_search(value: &str) -> String {
    normalize_with_mapping(value).0.into_iter().collect()
}

pub(crate) fn short_grams_for_search(value: &str) -> String {
    let characters = normalize_for_search(value).chars().collect::<Vec<_>>();
    let mut tokens = BTreeSet::new();
    for start in 0..characters.len() {
        tokens.insert(short_gram_token(&characters[start..=start]));
        if let Some(end) = start.checked_add(2).filter(|end| *end <= characters.len()) {
            tokens.insert(short_gram_token(&characters[start..end]));
        }
    }
    tokens.into_iter().collect::<Vec<_>>().join(" ")
}

fn short_gram_token(characters: &[char]) -> String {
    let prefix = if characters.len() == 1 { 'a' } else { 'b' };
    let encoded = characters
        .iter()
        .map(|character| format!("{:06x}", u32::from(*character)))
        .collect::<String>();
    format!("{prefix}{encoded}")
}

fn normalize_with_mapping(value: &str) -> (Vec<char>, Vec<usize>) {
    let mut normalized = Vec::new();
    let mut original_indices = Vec::new();
    for (original_index, character) in value.chars().enumerate() {
        for decomposed in std::iter::once(character).nfkd() {
            if is_combining_mark(decomposed) {
                continue;
            }
            for lowercase in decomposed.to_lowercase() {
                normalized.push(lowercase);
                original_indices.push(original_index);
            }
        }
    }
    (normalized, original_indices)
}

fn find_normalized_spans(value: &str, needle: &str) -> Vec<(u32, u32)> {
    let (haystack, original_indices) = normalize_with_mapping(value);
    let needle = normalize_for_search(needle).chars().collect::<Vec<_>>();
    if needle.is_empty() || needle.len() > haystack.len() {
        return Vec::new();
    }
    haystack
        .windows(needle.len())
        .enumerate()
        .filter(|(_, window)| *window == needle.as_slice())
        .map(|(start, _)| {
            let original_start = original_indices[start];
            let original_end = original_indices[start + needle.len() - 1] + 1;
            (
                u32::try_from(original_start).unwrap_or(u32::MAX),
                u32::try_from(original_end).unwrap_or(u32::MAX),
            )
        })
        .collect()
}

fn find_atom_spans(value: &str, atom: &QueryAtom, fuzzy_enabled: bool) -> (Vec<(u32, u32)>, f64) {
    let exact = find_normalized_spans(value, &atom.text);
    if !exact.is_empty() || !fuzzy_enabled || atom.quoted {
        return (exact, 1.0);
    }
    (find_fuzzy_word_spans(value, &atom.text), 0.7)
}

fn find_fuzzy_word_spans(value: &str, needle: &str) -> Vec<(u32, u32)> {
    let needle_tokens = word_tokens(needle);
    if needle_tokens.len() != 1 {
        return Vec::new();
    }
    let needle = needle_tokens.into_iter().next().expect("one token");
    let maximum_distance = match needle.chars().count() {
        0..=3 => return Vec::new(),
        4..=7 => 1,
        _ => 2,
    };
    let (normalized, original_indices) = normalize_with_mapping(value);
    let mut spans = Vec::new();
    let mut start = 0;
    while start < normalized.len() {
        while start < normalized.len()
            && !normalized[start].is_alphanumeric()
            && normalized[start] != '_'
        {
            start += 1;
        }
        if start == normalized.len() {
            break;
        }
        let mut end = start + 1;
        while end < normalized.len()
            && (normalized[end].is_alphanumeric() || normalized[end] == '_')
        {
            end += 1;
        }
        let candidate = normalized[start..end].iter().collect::<String>();
        if bounded_edit_distance(&needle, &candidate, maximum_distance).is_some() {
            spans.push((
                u32::try_from(original_indices[start]).unwrap_or(u32::MAX),
                u32::try_from(original_indices[end - 1] + 1).unwrap_or(u32::MAX),
            ));
        }
        start = end;
    }
    spans
}

fn bounded_edit_distance(left: &str, right: &str, maximum: usize) -> Option<usize> {
    let left = left.chars().collect::<Vec<_>>();
    let right = right.chars().collect::<Vec<_>>();
    if left.len().abs_diff(right.len()) > maximum {
        return None;
    }
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for (left_index, left_character) in left.iter().enumerate() {
        let mut current = vec![left_index + 1];
        for (right_index, right_character) in right.iter().enumerate() {
            current.push(
                (previous[right_index + 1] + 1)
                    .min(current[right_index] + 1)
                    .min(previous[right_index] + usize::from(left_character != right_character)),
            );
        }
        if current.iter().copied().min().unwrap_or(maximum + 1) > maximum {
            return None;
        }
        previous = current;
    }
    (previous[right.len()] <= maximum).then_some(previous[right.len()])
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rusqlite::params;

    use super::{
        diversify_ranked, parse_lexical_query, rank_lexical_documents, LexicalDocument,
        LexicalSearchMode, SearchDiversityKey, SearchEvidenceKind, SearchField,
        SearchPassagesInput,
    };
    use crate::database::{
        connection::{self, DatabaseKind, FileState},
        tidbits::CreateTidbitWrite,
        CitationLocator, Database, DatabasePaths, DeleteTidbitInput, RestoreTidbitInput,
        TidbitDraft,
    };

    fn document(
        id: &str,
        fields: impl IntoIterator<Item = (SearchField, &'static str)>,
    ) -> LexicalDocument {
        LexicalDocument {
            passage_id: id.into(),
            updated_at_ms: 0,
            evidence_kind: SearchEvidenceKind::Author,
            fields: fields
                .into_iter()
                .map(|(field, value)| (field, value.into()))
                .collect::<BTreeMap<_, _>>(),
            word_rank: None,
            trigram_rank: None,
            short_rank: None,
        }
    }

    #[test]
    fn parser_never_forwards_raw_fts_syntax() {
        let parsed = parse_lexical_query(
            "title:secret OR \"unfinished phrase * NEAR(foo)",
            LexicalSearchMode::Default,
        )
        .expect("safe query")
        .expect("nonempty query");

        let word = parsed.word_match_query().expect("word query");
        assert!(!word.contains("title:"));
        assert!(!word.contains(" NEAR("));
        assert!(word.contains("\"title\""));
        assert!(word.contains("\"secret\""));
        assert!(word.contains("\"or\""));
    }

    #[test]
    fn diacritic_highlights_map_back_to_original_character_offsets() {
        let parsed = parse_lexical_query("naive cafe resume", LexicalSearchMode::Exact)
            .expect("valid query")
            .expect("nonempty query");
        let ranked = rank_lexical_documents(
            &parsed,
            vec![document(
                "unicode",
                [(
                    SearchField::Body,
                    "A naïve résumé draft mentioned the café project.",
                )],
            )],
            10,
        );

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].matched_fields, vec![SearchField::Body]);
        assert_eq!(
            ranked[0]
                .highlights
                .iter()
                .map(|highlight| (highlight.start_char, highlight.end_char))
                .collect::<Vec<_>>(),
            vec![(2, 7), (8, 14), (35, 39)]
        );
    }

    #[test]
    fn exact_mode_requires_every_literal_atom_across_weighted_fields() {
        let parsed = parse_lexical_query(
            "jina-embeddings-v5-small.onnx tokenizer digest",
            LexicalSearchMode::Exact,
        )
        .expect("valid query")
        .expect("nonempty query");
        let ranked = rank_lexical_documents(
            &parsed,
            vec![
                document(
                    "complete",
                    [
                        (SearchField::AttachmentName, "jina-embeddings-v5-small.onnx"),
                        (SearchField::Body, "Tokenizer revision and SHA digest."),
                    ],
                ),
                document(
                    "partial",
                    [(SearchField::Body, "Tokenizer revision and SHA digest.")],
                ),
            ],
            10,
        );

        assert_eq!(
            ranked
                .iter()
                .map(|result| result.passage_id.as_str())
                .collect::<Vec<_>>(),
            vec!["complete"]
        );
    }

    #[test]
    fn default_mode_recovers_bounded_misspellings_without_weakening_exact_mode() {
        let passage = || {
            document(
                "concurrency",
                [(
                    SearchField::Body,
                    "A semaphore bounds concurrency while each worker holds a permit.",
                )],
            )
        };
        let default = parse_lexical_query(
            "concurency semafore worker permits",
            LexicalSearchMode::Default,
        )
        .expect("valid query")
        .expect("nonempty query");
        let exact = parse_lexical_query(
            "concurency semafore worker permits",
            LexicalSearchMode::Exact,
        )
        .expect("valid query")
        .expect("nonempty query");

        assert_eq!(
            rank_lexical_documents(&default, vec![passage()], 10)[0].passage_id,
            "concurrency"
        );
        assert!(rank_lexical_documents(&exact, vec![passage()], 10).is_empty());
        assert!(default
            .trigram_match_query()
            .expect("fuzzy candidate query")
            .contains("\"con\""));
        assert!(!exact
            .trigram_match_query()
            .expect("literal candidate query")
            .contains(" OR "));
    }

    #[test]
    fn authored_and_extracted_fields_have_deliberate_evidence_weights() {
        let parsed = parse_lexical_query("calibration", LexicalSearchMode::Default)
            .expect("valid query")
            .expect("nonempty query");
        let author = document("author", [(SearchField::Body, "calibration")]);
        let mut text = document("text", [(SearchField::ExtractedText, "calibration")]);
        text.evidence_kind = SearchEvidenceKind::Text;
        let mut pdf = document("pdf", [(SearchField::ExtractedText, "calibration")]);
        pdf.evidence_kind = SearchEvidenceKind::Pdf;
        let mut ocr = document("ocr", [(SearchField::ExtractedText, "calibration")]);
        ocr.evidence_kind = SearchEvidenceKind::Ocr;

        assert_eq!(
            rank_lexical_documents(&parsed, vec![ocr, pdf, text, author], 10)
                .into_iter()
                .map(|ranked| ranked.passage_id)
                .collect::<Vec<_>>(),
            ["author", "text", "pdf", "ocr"]
        );
    }

    #[test]
    fn attachment_diversity_defers_repeated_pages_but_backfills_when_needed() {
        #[derive(Debug, Eq, PartialEq)]
        struct Candidate {
            id: &'static str,
            attachment: Option<&'static str>,
            page: Option<u32>,
        }
        let candidates = || {
            vec![
                Candidate {
                    id: "pdf-a-page-1",
                    attachment: Some("pdf-a"),
                    page: Some(1),
                },
                Candidate {
                    id: "pdf-a-page-1-region-2",
                    attachment: Some("pdf-a"),
                    page: Some(1),
                },
                Candidate {
                    id: "pdf-a-page-2",
                    attachment: Some("pdf-a"),
                    page: Some(2),
                },
                Candidate {
                    id: "pdf-a-page-3",
                    attachment: Some("pdf-a"),
                    page: Some(3),
                },
                Candidate {
                    id: "pdf-b-page-1",
                    attachment: Some("pdf-b"),
                    page: Some(1),
                },
                Candidate {
                    id: "authored",
                    attachment: None,
                    page: None,
                },
            ]
        };
        let key = |candidate: &Candidate| SearchDiversityKey {
            attachment_id: candidate.attachment.map(str::to_owned),
            page: candidate.page,
        };

        assert_eq!(
            diversify_ranked(candidates(), 4, key)
                .into_iter()
                .map(|candidate| candidate.id)
                .collect::<Vec<_>>(),
            ["pdf-a-page-1", "pdf-a-page-2", "pdf-b-page-1", "authored"]
        );
        assert_eq!(
            diversify_ranked(candidates().into_iter().take(4).collect(), 4, key)
                .into_iter()
                .map(|candidate| candidate.id)
                .collect::<Vec<_>>(),
            [
                "pdf-a-page-1",
                "pdf-a-page-2",
                "pdf-a-page-1-region-2",
                "pdf-a-page-3"
            ]
        );
    }

    #[test]
    fn quoted_phrase_outranks_documents_with_only_separate_words() {
        let parsed = parse_lexical_query("\"impatient motion\"", LexicalSearchMode::Default)
            .expect("valid query")
            .expect("nonempty query");
        let ranked = rank_lexical_documents(
            &parsed,
            vec![
                document(
                    "separate",
                    [(SearchField::Body, "Motion can make a learner impatient.")],
                ),
                document(
                    "phrase",
                    [(
                        SearchField::Body,
                        "Heat is the ceaseless and impatient motion of particles.",
                    )],
                ),
            ],
            10,
        );

        assert_eq!(ranked[0].passage_id, "phrase");
        assert_eq!(ranked.len(), 1);
    }

    #[test]
    fn heading_relevance_outranks_body_recency() {
        let parsed = parse_lexical_query("sentinel", LexicalSearchMode::Default)
            .expect("valid query")
            .expect("nonempty query");
        let mut heading_match = document(
            "older-heading",
            [(SearchField::HeadingContext, "Sentinel architecture")],
        );
        heading_match.updated_at_ms = 1;
        let mut body_match = document(
            "newer-body",
            [(SearchField::Body, "A sentinel appears in recent prose.")],
        );
        body_match.updated_at_ms = 1_000_000;

        let ranked = rank_lexical_documents(&parsed, vec![body_match, heading_match], 10);

        assert_eq!(ranked[0].passage_id, "older-heading");
    }

    #[test]
    fn empty_and_punctuation_only_queries_have_no_executable_plan() {
        assert!(parse_lexical_query("  ", LexicalSearchMode::Default)
            .expect("empty query")
            .is_none());
        let punctuation = parse_lexical_query("*", LexicalSearchMode::Default)
            .expect("punctuation query")
            .expect("literal punctuation atom");
        assert!(punctuation.word_match_query().is_none());
        assert!(punctuation.trigram_match_query().is_none());
        assert!(punctuation.short_match_query().is_none());
    }

    #[test]
    fn database_search_tracks_active_revisions_and_trusted_citations() {
        let root = tempfile::tempdir().expect("temporary search library");
        let database =
            Database::initialize(DatabasePaths::new(root.path())).expect("search database");
        let client = database.client();
        let created = client
            .create_tidbit(CreateTidbitWrite {
                input: TidbitDraft {
                    body_markdown: "# SQLite\n\nThe first lexical sentinel uses `resolveCitationTarget`.\n\nhttps://sqlite.org/fts5.html".into(),
                },
                now_ms: 10,
                tidbit_id: "019f547b-6200-7000-8000-000000009001".into(),
                revision_id: "019f547b-6200-7000-8000-000000009002".into(),
            })
            .expect("create searchable tidbit");

        let initial = client
            .search_passages(SearchPassagesInput {
                query: "sqlite.org FTS5 resolveCitationTarget".into(),
                mode: LexicalSearchMode::Exact,
                limit: 10,
            })
            .expect("initial exact search");
        assert_eq!(initial.len(), 1);
        assert_eq!(
            initial[0]
                .citation
                .tidbit
                .as_ref()
                .map(|tidbit| tidbit.revision_id.as_str()),
            Some(created.current_revision_id.as_str())
        );
        assert!(initial[0].matched_fields.contains(&SearchField::Body));
        assert!(initial[0]
            .highlights
            .iter()
            .all(|highlight| highlight.start_char < highlight.end_char));
        assert_eq!(
            client
                .search_passages(SearchPassagesInput {
                    query: "resolveCitationTar".into(),
                    mode: LexicalSearchMode::Default,
                    limit: 10,
                })
                .expect("trigram substring search")
                .len(),
            1
        );

        client
            .save_working_copy_for_test(
                created.id.clone(),
                Some(created.current_revision_id.clone()),
                1,
                "# Search\n\nThe replacement carries café, a ﬁle, OpenAI, C, R2, and $$E=mc^2$$."
                    .into(),
                20,
            )
            .expect("save searchable edit");
        let edited = client
            .checkpoint_working_copy_for_test(
                created.id,
                1,
                21,
                "019f547b-6200-7000-8000-000000009004".into(),
            )
            .expect("checkpoint searchable edit")
            .note
            .expect("edited note");
        assert!(client
            .search_passages(SearchPassagesInput {
                query: "first lexical sentinel".into(),
                mode: LexicalSearchMode::Exact,
                limit: 10,
            })
            .expect("old revision search")
            .is_empty());
        for query in ["cafe", "file", "AI", "R2", "E=mc^2"] {
            let results = client
                .search_passages(SearchPassagesInput {
                    query: query.into(),
                    mode: LexicalSearchMode::Default,
                    limit: 10,
                })
                .expect("literal search");
            assert_eq!(results.len(), 1, "query {query}");
            assert_eq!(
                results[0]
                    .citation
                    .tidbit
                    .as_ref()
                    .map(|tidbit| tidbit.revision_id.as_str()),
                Some(edited.current_revision_id.as_str())
            );
        }
        let single_character = client
            .search_passages(SearchPassagesInput {
                query: "C".into(),
                mode: LexicalSearchMode::Default,
                limit: 10,
            })
            .expect("single-character substring search");
        assert!(!single_character.is_empty());
        assert!(single_character.iter().all(|result| {
            result
                .citation
                .tidbit
                .as_ref()
                .is_some_and(|tidbit| tidbit.revision_id == edited.current_revision_id)
        }));

        client
            .delete_tidbit(
                DeleteTidbitInput {
                    id: edited.id.clone(),
                    expected_revision_id: edited.current_revision_id.clone(),
                },
                30,
            )
            .expect("delete indexed tidbit");
        assert!(client
            .search_passages(SearchPassagesInput {
                query: "replacement".into(),
                mode: LexicalSearchMode::Default,
                limit: 10,
            })
            .expect("deleted search")
            .is_empty());
        assert_eq!(
            client
                .resolve_citation(initial[0].passage_id.clone())
                .expect("historical citation remains")
                .state,
            crate::database::CitationState::Historical
        );

        client
            .restore_tidbit(
                RestoreTidbitInput {
                    id: edited.id,
                    expected_revision_id: edited.current_revision_id,
                },
                40,
            )
            .expect("restore indexed tidbit");
        assert_eq!(
            client
                .search_passages(SearchPassagesInput {
                    query: "\"replacement carries\"".into(),
                    mode: LexicalSearchMode::Default,
                    limit: 10,
                })
                .expect("restored search")
                .len(),
            1
        );
    }

    #[test]
    fn independently_saturated_candidate_pools_cannot_hide_a_qualifying_substring_result() {
        let root = tempfile::tempdir().expect("temporary saturated search library");
        let paths = DatabasePaths::new(root.path());
        let database = Database::initialize(paths.clone()).expect("search database");
        let client = database.client();

        for index in 0..544_u64 {
            let atom = if index < 272 { "apple" } else { "orange" };
            client
                .create_tidbit(CreateTidbitWrite {
                    input: TidbitDraft {
                        body_markdown: format!(
                            "# {atom}\n\n{atom} {atom} {atom} single-atom decoy."
                        ),
                    },
                    now_ms: i64::try_from(index).expect("bounded timestamp"),
                    tidbit_id: format!("019f547b-6200-7000-8000-{:012x}", index * 2 + 1),
                    revision_id: format!("019f547b-6200-7000-8000-{:012x}", index * 2 + 2),
                })
                .expect("create word-only decoy");
        }
        let target = client
            .create_tidbit(CreateTidbitWrite {
                input: TidbitDraft {
                    body_markdown: "The pineapple and bloodorange pairing is the answer.".into(),
                },
                now_ms: 100,
                tidbit_id: "019f547b-6200-7000-8000-000000001001".into(),
                revision_id: "019f547b-6200-7000-8000-000000001002".into(),
            })
            .expect("create substring target");

        drop(client);
        database
            .shutdown()
            .expect("close saturated search database");
        drop(database);
        let connection =
            connection::open_writer(&paths.main, DatabaseKind::Main, FileState::Existing)
                .expect("derived-index maintenance writer");
        connection
            .execute(
                "UPDATE passage_search_document
                 SET heading_context = 'apple orange apple orange apple orange'
                 WHERE rowid IN (
                    SELECT rowid
                    FROM passage_search_document
                    WHERE tidbit_id <> ?1
                    ORDER BY rowid
                    LIMIT 256
                 )",
                params![target.id],
            )
            .expect("simulate stale high-rank derived candidates");

        let results = super::search_passages(
            &connection,
            SearchPassagesInput {
                query: "apple orange".into(),
                mode: LexicalSearchMode::Default,
                limit: 32,
            },
        )
        .expect("search saturated word and trigram candidates");

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0]
                .citation
                .tidbit
                .as_ref()
                .map(|citation| citation.id.as_str()),
            Some(target.id.as_str())
        );
        assert_eq!(
            results[0].matched_fields,
            vec![SearchField::Body],
            "both atoms must be proven from immutable authored text"
        );
    }

    #[test]
    fn database_search_indexes_ready_attachment_passages_and_tracks_deletion() {
        let root = tempfile::tempdir().expect("temporary attachment search library");
        let paths = DatabasePaths::new(root.path());
        let database = Database::initialize(paths.clone()).expect("attachment search database");
        database
            .client()
            .create_tidbit(CreateTidbitWrite {
                input: TidbitDraft {
                    body_markdown: "Authored passage linked to a file.".into(),
                },
                now_ms: 5,
                tidbit_id: "019f547b-6200-7000-8000-000000009511".into(),
                revision_id: "019f547b-6200-7000-8000-000000009512".into(),
            })
            .expect("create authored attachment host");
        database
            .shutdown()
            .expect("close attachment search database");
        drop(database);
        let mut connection =
            connection::open_writer(&paths.main, DatabaseKind::Main, FileState::Existing)
                .expect("attachment fixture writer");
        connection
            .execute_batch(
                "INSERT INTO attachment(
                    id, created_at, updated_at, sha256, display_filename,
                    media_type, byte_length, kind, extraction_state
                 ) VALUES(
                    '019f547b-6200-7000-8000-000000009501',
                    10, 10, zeroblob(32), 'calibration-plate.pdf',
                    'application/pdf', 128, 'PDF', 'READY'
                 );
                 INSERT INTO tidbit_revision_attachment(
                    tidbit_revision_id, attachment_id, sort_order, display_role
                 ) VALUES(
                    '019f547b-6200-7000-8000-000000009512',
                    '019f547b-6200-7000-8000-000000009501',
                    0, 'ATTACHMENT'
                 );",
            )
            .expect("associate attachment with current revision");
        let membership_results = super::search_passages(
            &connection,
            SearchPassagesInput {
                query: "calibration-plate.pdf".into(),
                mode: LexicalSearchMode::Default,
                limit: 10,
            },
        )
        .expect("search newly associated attachment filename");
        assert_eq!(membership_results.len(), 1);
        assert!(membership_results[0].citation.tidbit.is_some());
        assert!(membership_results[0].citation.attachment.is_none());
        assert!(membership_results[0]
            .matched_fields
            .contains(&SearchField::AttachmentName));

        connection
            .execute_batch(
                "INSERT INTO attachment_extraction(
                    id, attachment_id, extractor, extractor_version, content_hash,
                    status, created_at, started_at, completed_at
                 ) VALUES(
                    '019f547b-6200-7000-8000-000000009502',
                    '019f547b-6200-7000-8000-000000009501',
                    'pdf-text', '1', zeroblob(32), 'READY', 10, 10, 10
                 );
                 INSERT INTO attachment_segment(
                    id, extraction_id, ordinal, locator_kind, page_number,
                    content, content_hash
                 ) VALUES(
                    '019f547b-6200-7000-8000-000000009503',
                    '019f547b-6200-7000-8000-000000009502',
                    0, 'PDF_PAGE', 7, 'The quasar_needle calibration is authoritative.',
                    zeroblob(32)
                 ), (
                    '019f547b-6200-7000-8000-000000009516',
                    '019f547b-6200-7000-8000-000000009502',
                    1, 'PDF_PAGE', 8, 'A second page is installed in the same batch.',
                    zeroblob(32)
                 );
                 INSERT INTO passage(
                    id, attachment_segment_id, owner_kind, ordinal, content,
                    content_hash, locator_kind, locator_json, created_at,
                    construction_version, heading_context_json
                 ) VALUES(
                    '019f547b-6200-7000-8000-000000009504',
                    '019f547b-6200-7000-8000-000000009503',
                    'ATTACHMENT', 0,
                    'The quasar_needle calibration is authoritative.', zeroblob(32),
                    'PDF_PAGE', '{\"page\":7}', 10, 'pdf-page-v1', '[]'
                 ), (
                    '019f547b-6200-7000-8000-000000009517',
                    '019f547b-6200-7000-8000-000000009516',
                    'ATTACHMENT', 0,
                    'A second page is installed in the same batch.', zeroblob(32),
                    'PDF_PAGE', '{\"page\":8}', 10, 'pdf-page-v1', '[]'
                 );",
            )
            .expect("ready attachment passage");
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM passage_search_document
                     WHERE passage_id IN (
                        '019f547b-6200-7000-8000-000000009504',
                        '019f547b-6200-7000-8000-000000009517'
                     )",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("unrefreshed attachment document count"),
            0
        );
        super::replace_attachment_documents(
            &mut connection,
            "019f547b-6200-7000-8000-000000009501",
        )
        .expect("refresh completed attachment passage batch");
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM passage_search_document
                     WHERE passage_id IN (
                        '019f547b-6200-7000-8000-000000009504',
                        '019f547b-6200-7000-8000-000000009517'
                     )",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("batched attachment document count"),
            2
        );
        let results = super::search_passages(
            &connection,
            SearchPassagesInput {
                query: "quasar_needle calibration-plate.pdf".into(),
                mode: LexicalSearchMode::Exact,
                limit: 10,
            },
        )
        .expect("search extracted attachment text");
        assert_eq!(results.len(), 1);
        assert!(results[0]
            .matched_fields
            .contains(&SearchField::AttachmentName));
        assert!(results[0]
            .matched_fields
            .contains(&SearchField::ExtractedText));
        assert!(results[0].citation.tidbit.is_none());
        assert_eq!(
            (
                results[0].note.id.as_str(),
                results[0].note.revision_id.as_str(),
                results[0].note.display_title.as_str(),
            ),
            (
                "019f547b-6200-7000-8000-000000009511",
                "019f547b-6200-7000-8000-000000009512",
                "Authored passage linked to a file.",
            ),
            "attachment results retain the current note and revision needed for exact navigation"
        );
        assert_eq!(
            results[0].citation.locator,
            CitationLocator::PdfPage { page: 7 }
        );
        assert_eq!(
            results[0]
                .citation
                .attachment
                .as_ref()
                .map(|attachment| attachment.display_filename.as_str()),
            Some("calibration-plate.pdf")
        );
        connection
            .execute(
                "UPDATE attachment
                 SET updated_at = 15, display_filename = 'renamed-calibration.pdf'
                 WHERE id = ?1",
                params!["019f547b-6200-7000-8000-000000009501"],
            )
            .expect("rename attachment");
        let renamed = super::search_passages(
            &connection,
            SearchPassagesInput {
                query: "renamed-calibration.pdf".into(),
                mode: LexicalSearchMode::Default,
                limit: 10,
            },
        )
        .expect("search renamed attachment metadata");
        assert!(renamed
            .iter()
            .any(|result| result.citation.tidbit.is_some()));
        assert!(renamed
            .iter()
            .any(|result| result.citation.attachment.is_some()));
        connection
            .execute(
                "UPDATE attachment SET updated_at = 20, deleted_at = 20 WHERE id = ?1",
                params!["019f547b-6200-7000-8000-000000009501"],
            )
            .expect("soft delete attachment");
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM passage_search_document WHERE passage_id = ?1",
                    params!["019f547b-6200-7000-8000-000000009504"],
                    |row| row.get::<_, i64>(0),
                )
                .expect("deleted document count"),
            0
        );
        connection
            .execute(
                "UPDATE attachment SET updated_at = 30, deleted_at = NULL WHERE id = ?1",
                params!["019f547b-6200-7000-8000-000000009501"],
            )
            .expect("restore attachment");
        assert_eq!(
            super::search_passages(
                &connection,
                SearchPassagesInput {
                    query: "quasar_needle".into(),
                    mode: LexicalSearchMode::Default,
                    limit: 10,
                },
            )
            .expect("search restored attachment")
            .len(),
            1
        );
        let mismatched = connection
            .execute(
                "INSERT INTO passage(
                    id, attachment_segment_id, owner_kind, ordinal, content,
                    content_hash, locator_kind, locator_json, created_at,
                    construction_version, heading_context_json
                 ) VALUES(
                    '019f547b-6200-7000-8000-000000009508',
                    '019f547b-6200-7000-8000-000000009503',
                    'ATTACHMENT', 0, 'fabricated attachment text', randomblob(32),
                    'PDF_PAGE', '{\"page\":7}', 35, 'pdf-page-bad', '[]'
                 )",
                [],
            )
            .expect_err("mismatched attachment passage must be rejected");
        assert!(mismatched
            .to_string()
            .contains("does not match its immutable segment"));

        connection
            .execute_batch(
                "UPDATE attachment_extractor_config
                 SET version = '2',
                     passage_construction_version = 'pdf-page-v2',
                     updated_at = 40
                 WHERE extractor = 'pdf-text';
                 INSERT INTO attachment_extraction(
                    id, attachment_id, extractor, extractor_version, content_hash,
                    status, created_at, started_at, completed_at
                 ) VALUES(
                    '019f547b-6200-7000-8000-000000009505',
                    '019f547b-6200-7000-8000-000000009501',
                    'pdf-text', '2', zeroblob(32), 'READY', 40, 40, 40
                 );
                 INSERT INTO attachment_segment(
                    id, extraction_id, ordinal, locator_kind, page_number,
                    content, content_hash
                 ) VALUES(
                    '019f547b-6200-7000-8000-000000009506',
                    '019f547b-6200-7000-8000-000000009505',
                    0, 'PDF_PAGE', 8,
                    'The nova_needle supersedes the old extraction.', zeroblob(32)
                 );
                 INSERT INTO passage(
                    id, attachment_segment_id, owner_kind, ordinal, content,
                    content_hash, locator_kind, locator_json, created_at,
                    construction_version, heading_context_json
                 ) VALUES(
                    '019f547b-6200-7000-8000-000000009507',
                    '019f547b-6200-7000-8000-000000009506',
                    'ATTACHMENT', 0,
                    'The nova_needle supersedes the old extraction.', zeroblob(32),
                    'PDF_PAGE', '{\"page\":8}', 40, 'pdf-page-v2', '[]'
                 );",
            )
            .expect("new extraction version");
        super::replace_attachment_documents(
            &mut connection,
            "019f547b-6200-7000-8000-000000009501",
        )
        .expect("refresh versioned extraction passage batch");
        assert!(super::search_passages(
            &connection,
            SearchPassagesInput {
                query: "quasar_needle".into(),
                mode: LexicalSearchMode::Default,
                limit: 10,
            },
        )
        .expect("search stale extraction")
        .is_empty());
        assert_eq!(
            super::search_passages(
                &connection,
                SearchPassagesInput {
                    query: "nova_needle".into(),
                    mode: LexicalSearchMode::Default,
                    limit: 10,
                },
            )
            .expect("search current extraction")
            .len(),
            1
        );
        connection
            .execute_batch(
                "UPDATE attachment_extractor_config
                 SET passage_construction_version = 'pdf-page-v3',
                     updated_at = 50
                 WHERE extractor = 'pdf-text';
                 INSERT INTO passage(
                    id, attachment_segment_id, owner_kind, ordinal, content,
                    content_hash, locator_kind, locator_json, created_at,
                    construction_version, heading_context_json
                 ) VALUES(
                    '019f547b-6200-7000-8000-000000009509',
                    '019f547b-6200-7000-8000-000000009506',
                    'ATTACHMENT', 0,
                    'The nova_needle supersedes the old extraction.', zeroblob(32),
                    'PDF_PAGE', '{\"page\":8}', 50, 'pdf-page-v3', '[]'
                 )",
            )
            .expect("new attachment passage construction");
        super::replace_attachment_documents(
            &mut connection,
            "019f547b-6200-7000-8000-000000009501",
        )
        .expect("refresh versioned passage construction batch");
        let rebuilt = super::search_passages(
            &connection,
            SearchPassagesInput {
                query: "nova_needle".into(),
                mode: LexicalSearchMode::Default,
                limit: 10,
            },
        )
        .expect("search current attachment passage construction");
        assert_eq!(
            rebuilt
                .iter()
                .map(|result| result.passage_id.as_str())
                .collect::<Vec<_>>(),
            vec!["019f547b-6200-7000-8000-000000009509"]
        );
        assert_eq!(
            crate::database::passages::resolve_citation(
                &connection,
                "019f547b-6200-7000-8000-000000009507",
            )
            .expect("resolve stale construction citation")
            .state,
            crate::database::CitationState::Historical
        );
        assert_eq!(
            crate::database::passages::resolve_citation(
                &connection,
                "019f547b-6200-7000-8000-000000009504",
            )
            .expect("resolve stale extraction citation")
            .state,
            crate::database::CitationState::Historical
        );

        connection
            .execute(
                "INSERT INTO passage(
                    id, attachment_segment_id, owner_kind, ordinal, content,
                    content_hash, locator_kind, locator_json, created_at,
                    construction_version, heading_context_json
                 ) VALUES(
                    '019f547b-6200-7000-8000-000000009515',
                    '019f547b-6200-7000-8000-000000009506',
                    'ATTACHMENT', 0,
                    'The nova_needle supersedes the old extraction.', zeroblob(32),
                    'PDF_PAGE', '{\"page\":8}', 60, 'pdf-page-v1', '[]'
                 )",
                [],
            )
            .expect("late stale passage builder retry");
        assert_eq!(
            super::search_passages(
                &connection,
                SearchPassagesInput {
                    query: "nova_needle".into(),
                    mode: LexicalSearchMode::Default,
                    limit: 10,
                },
            )
            .expect("search configured passage construction after stale retry")
            .iter()
            .map(|result| result.passage_id.as_str())
            .collect::<Vec<_>>(),
            vec!["019f547b-6200-7000-8000-000000009509"]
        );
        assert_eq!(
            crate::database::passages::resolve_citation(
                &connection,
                "019f547b-6200-7000-8000-000000009515",
            )
            .expect("resolve stale configured construction citation")
            .state,
            crate::database::CitationState::Historical
        );

        connection
            .execute_batch(
                "INSERT INTO attachment_extraction(
                    id, attachment_id, extractor, extractor_version, content_hash,
                    status, created_at, started_at, completed_at
                 ) VALUES(
                    '019f547b-6200-7000-8000-000000009510',
                    '019f547b-6200-7000-8000-000000009501',
                    'pdf-text', '0', zeroblob(32), 'READY', 70, 70, 70
                 );
                 INSERT INTO attachment_segment(
                    id, extraction_id, ordinal, locator_kind, page_number,
                    content, content_hash
                 ) VALUES(
                    '019f547b-6200-7000-8000-000000009513',
                    '019f547b-6200-7000-8000-000000009510',
                    0, 'PDF_PAGE', 9,
                    'The stale_needle retry must stay historical.', zeroblob(32)
                 );
                 INSERT INTO passage(
                    id, attachment_segment_id, owner_kind, ordinal, content,
                    content_hash, locator_kind, locator_json, created_at,
                    construction_version, heading_context_json
                 ) VALUES(
                    '019f547b-6200-7000-8000-000000009514',
                    '019f547b-6200-7000-8000-000000009513',
                    'ATTACHMENT', 0,
                    'The stale_needle retry must stay historical.', zeroblob(32),
                    'PDF_PAGE', '{\"page\":9}', 70, 'pdf-page-v0', '[]'
                 );",
            )
            .expect("late stale extractor retry");
        assert!(super::search_passages(
            &connection,
            SearchPassagesInput {
                query: "stale_needle".into(),
                mode: LexicalSearchMode::Default,
                limit: 10,
            },
        )
        .expect("search stale configured extractor version")
        .is_empty());
        assert_eq!(
            super::search_passages(
                &connection,
                SearchPassagesInput {
                    query: "nova_needle".into(),
                    mode: LexicalSearchMode::Default,
                    limit: 10,
                },
            )
            .expect("search configured extractor version after stale retry")
            .iter()
            .map(|result| result.passage_id.as_str())
            .collect::<Vec<_>>(),
            vec!["019f547b-6200-7000-8000-000000009509"]
        );
        assert_eq!(
            crate::database::passages::resolve_citation(
                &connection,
                "019f547b-6200-7000-8000-000000009514",
            )
            .expect("resolve stale configured extractor citation")
            .state,
            crate::database::CitationState::Historical
        );
        connection
            .execute(
                "UPDATE tidbit SET deleted_at = 80, updated_at = 80 WHERE id = ?1",
                params!["019f547b-6200-7000-8000-000000009511"],
            )
            .expect("delete the attachment's only current note owner");
        assert!(super::search_passages(
            &connection,
            SearchPassagesInput {
                query: "nova_needle".into(),
                mode: LexicalSearchMode::Default,
                limit: 10,
            },
        )
        .expect("skip attachment evidence without a current note owner")
        .is_empty());
    }

    #[test]
    fn database_parser_bounds_empty_and_injection_like_queries() {
        let root = tempfile::tempdir().expect("temporary safe-query library");
        let database =
            Database::initialize(DatabasePaths::new(root.path())).expect("search database");
        let client = database.client();
        client
            .create_tidbit(CreateTidbitWrite {
                input: TidbitDraft {
                    body_markdown: "Literal OR NEAR syntax stays authored text.".into(),
                },
                now_ms: 10,
                tidbit_id: "019f547b-6200-7000-8000-000000009101".into(),
                revision_id: "019f547b-6200-7000-8000-000000009102".into(),
            })
            .expect("create safe-query fixture");

        assert!(client
            .search_passages(SearchPassagesInput {
                query: String::new(),
                mode: LexicalSearchMode::Default,
                limit: 10,
            })
            .expect("empty search")
            .is_empty());
        client
            .search_passages(SearchPassagesInput {
                query: "title:secret OR \"unfinished * NEAR(foo)".into(),
                mode: LexicalSearchMode::Default,
                limit: 10,
            })
            .expect("injection-like query remains data");
        assert!(client
            .search_passages(SearchPassagesInput {
                query: "x".repeat(513),
                mode: LexicalSearchMode::Default,
                limit: 10,
            })
            .is_err());
    }

    #[test]
    fn derived_index_text_cannot_fabricate_result_provenance() {
        let root = tempfile::tempdir().expect("temporary provenance library");
        let paths = DatabasePaths::new(root.path());
        let database = Database::initialize(paths.clone()).expect("search database");
        database
            .client()
            .create_tidbit(CreateTidbitWrite {
                input: TidbitDraft {
                    body_markdown: "Immutable authored evidence.".into(),
                },
                now_ms: 10,
                tidbit_id: "019f547b-6200-7000-8000-000000009201".into(),
                revision_id: "019f547b-6200-7000-8000-000000009202".into(),
            })
            .expect("create provenance fixture");
        database.shutdown().expect("close search database");
        drop(database);

        let connection =
            connection::open_writer(&paths.main, DatabaseKind::Main, FileState::Existing)
                .expect("maintenance writer");
        connection
            .execute(
                "UPDATE passage_search_document
                 SET heading_context = 'fabricated_needle'
                 WHERE tidbit_id = '019f547b-6200-7000-8000-000000009201'",
                [],
            )
            .expect("tamper derived text");
        drop(connection);

        let reopened = Database::initialize(paths).expect("reopen provenance library");
        assert!(reopened
            .client()
            .search_passages(SearchPassagesInput {
                query: "fabricated_needle".into(),
                mode: LexicalSearchMode::Default,
                limit: 10,
            })
            .expect("search tampered candidate")
            .is_empty());
        assert_eq!(
            reopened
                .client()
                .search_passages(SearchPassagesInput {
                    query: "authored evidence".into(),
                    mode: LexicalSearchMode::Exact,
                    limit: 10,
                })
                .expect("search authoritative evidence")
                .len(),
            1
        );
    }
}

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

use crate::embedding;

use super::{block_embedding_index, tidbits, DatabaseError, Result};

const MAX_QUERY_CHARACTERS: usize = 512;
const MAX_SEARCH_LIMIT: u32 = 100;
const MAX_HIGHLIGHTS_PER_RESULT: usize = 32;
const CANDIDATE_EXPANSION: u32 = 16;
const MIN_CANDIDATES: u32 = 64;
const MAX_CANDIDATES: u32 = 512;
const RRF_CONSTANT: f64 = 60.0;
const LEXICAL_WEIGHT: f64 = 1.0;
const SEMANTIC_WEIGHT: f64 = 0.85;
const AGREEMENT_WEIGHT: f64 = 0.20;
const FTS_BM25_WEIGHTS: &str = "6.0, 3.5, 5.0, 2.25";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LexicalSearchMode {
    Default,
    Exact,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchBlocksInput {
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
    const fn weight(self) -> f64 {
        match self {
            Self::HeadingContext => 6.0,
            Self::Body => 3.5,
            Self::AttachmentName => 5.0,
            Self::ExtractedText => 2.25,
        }
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
pub struct BlockSearchResult {
    pub note_id: String,
    pub block_id: String,
    pub block_type: String,
    pub block_ordinal: u32,
    pub display_title: String,
    pub heading_context: Vec<String>,
    pub excerpt: String,
    pub attachment_names: Vec<String>,
    pub score: f64,
    pub matched_fields: Vec<SearchField>,
    pub highlights: Vec<SearchHighlight>,
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
pub struct SearchBlocksResponse {
    pub results: Vec<BlockSearchResult>,
    pub execution_mode: SearchExecutionMode,
    pub semantic_readiness: SemanticSearchReadiness,
}

#[derive(Clone, Debug)]
struct QueryAtom {
    text: String,
    quoted: bool,
}

#[derive(Clone, Debug)]
struct ParsedQuery {
    atoms: Vec<QueryAtom>,
    mode: LexicalSearchMode,
}

#[derive(Clone, Copy, Debug, Default)]
struct CandidateRanks {
    word: Option<usize>,
    trigram: Option<usize>,
    short: Option<usize>,
}

#[derive(Clone, Debug)]
struct SearchDocument {
    rowid: i64,
    note_id: String,
    block_id: String,
    block_type: String,
    block_ordinal: u32,
    display_title: String,
    heading_context: String,
    body: String,
    attachment_names: String,
    extracted_text: String,
    updated_at_ms: i64,
}

#[derive(Clone, Debug)]
struct RankedBlock {
    rowid: i64,
    score: f64,
    matched_fields: Vec<SearchField>,
    highlights: Vec<SearchHighlight>,
}

pub(crate) fn validate_search_input(input: &SearchBlocksInput) -> Result<bool> {
    validate_limit(input.limit)?;
    Ok(ParsedQuery::parse(&input.query, input.mode)?.is_some_and(|query| query.searchable()))
}

pub(crate) fn search_blocks_with_semantics(
    connection: &Connection,
    input: SearchBlocksInput,
    query_embedding: Option<&[f32]>,
    fallback_readiness: SemanticSearchReadiness,
) -> Result<SearchBlocksResponse> {
    validate_limit(input.limit)?;
    let Some(query) = ParsedQuery::parse(&input.query, input.mode)?.filter(ParsedQuery::searchable)
    else {
        return Ok(SearchBlocksResponse {
            results: Vec::new(),
            execution_mode: execution_mode(input.mode, false),
            semantic_readiness: if input.mode == LexicalSearchMode::Exact {
                SemanticSearchReadiness::NotRequested
            } else {
                fallback_readiness
            },
        });
    };
    let candidate_limit = input
        .limit
        .saturating_mul(CANDIDATE_EXPANSION)
        .clamp(MIN_CANDIDATES, MAX_CANDIDATES);
    let lexical = lexical_candidates(connection, &query, candidate_limit)?;
    if input.mode == LexicalSearchMode::Exact {
        return Ok(SearchBlocksResponse {
            results: hydrate(connection, lexical, input.limit as usize)?,
            execution_mode: SearchExecutionMode::Exact,
            semantic_readiness: SemanticSearchReadiness::NotRequested,
        });
    }

    let index_readiness = semantic_index_readiness(connection)?;
    if query_embedding.is_none() || index_readiness != SemanticSearchReadiness::Ready {
        return Ok(SearchBlocksResponse {
            results: hydrate(connection, lexical, input.limit as usize)?,
            execution_mode: SearchExecutionMode::LexicalOnly,
            semantic_readiness: if query_embedding.is_some() {
                index_readiness
            } else {
                fallback_readiness
            },
        });
    }

    let query_embedding = query_embedding.expect("semantic readiness requires an embedding");
    let manifest = embedding::jina_v1_manifest();
    block_embedding_index::validate_embedding(query_embedding, manifest.dimension as usize)?;
    let semantic = match semantic_candidates(connection, query_embedding, candidate_limit) {
        Ok(candidates) => candidates,
        Err(error) => {
            log::warn!("semantic block retrieval failed; using lexical search: {error}");
            let _ = block_embedding_index::quarantine(
                connection,
                "semantic block search is unavailable; repair is required",
                0,
            );
            return Ok(SearchBlocksResponse {
                results: hydrate(connection, lexical, input.limit as usize)?,
                execution_mode: SearchExecutionMode::LexicalOnly,
                semantic_readiness: SemanticSearchReadiness::Failed,
            });
        }
    };
    let fused = fuse(lexical, semantic);
    Ok(SearchBlocksResponse {
        results: hydrate(connection, fused, input.limit as usize)?,
        execution_mode: SearchExecutionMode::Hybrid,
        semantic_readiness: SemanticSearchReadiness::Ready,
    })
}

pub(crate) fn semantic_index_readiness(connection: &Connection) -> Result<SemanticSearchReadiness> {
    let manifest = embedding::jina_v1_manifest();
    let (version, status, active_index_id, has_reap_work) = connection.query_row(
        "SELECT state.version, state.status, settings.active_embedding_index_id,
                EXISTS(SELECT 1 FROM block_embedding_reap_queue)
         FROM index_state AS state
         CROSS JOIN block_embedding_settings AS settings
         WHERE state.name = 'BLOCK_EMBEDDING' AND settings.singleton_id = 1",
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
            reason: format!("BLOCK_EMBEDDING has unknown status {status}"),
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

fn validate_limit(limit: u32) -> Result<()> {
    if limit == 0 || limit > MAX_SEARCH_LIMIT {
        return Err(DatabaseError::InvalidInput(format!(
            "limit must be between 1 and {MAX_SEARCH_LIMIT}"
        )));
    }
    Ok(())
}

fn execution_mode(mode: LexicalSearchMode, hybrid: bool) -> SearchExecutionMode {
    match (mode, hybrid) {
        (LexicalSearchMode::Exact, _) => SearchExecutionMode::Exact,
        (_, true) => SearchExecutionMode::Hybrid,
        _ => SearchExecutionMode::LexicalOnly,
    }
}

fn lexical_candidates(
    connection: &Connection,
    query: &ParsedQuery,
    limit: u32,
) -> Result<Vec<RankedBlock>> {
    let mut ranks = HashMap::<i64, CandidateRanks>::new();
    if let Some(fts_query) = query.word_query() {
        install_ranks(
            &mut ranks,
            query_index(connection, "block_fts_word", &fts_query, limit)?,
            0,
        );
    }
    if let Some(fts_query) = query.trigram_query() {
        install_ranks(
            &mut ranks,
            query_index(connection, "block_fts_trigram", &fts_query, limit)?,
            1,
        );
    }
    if let Some(fts_query) = query.short_query() {
        install_ranks(
            &mut ranks,
            query_index(connection, "block_fts_short", &fts_query, limit)?,
            2,
        );
    }
    let mut ranked = load_documents(connection, ranks.keys().copied().collect())?
        .into_iter()
        .filter_map(|document| {
            let document_ranks = ranks.get(&document.rowid).copied().unwrap_or_default();
            score_document(query, document, document_ranks)
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.rowid.cmp(&right.rowid))
    });
    ranked.truncate(limit as usize);
    Ok(ranked)
}

fn query_index(connection: &Connection, index: &str, query: &str, limit: u32) -> Result<Vec<i64>> {
    let sql = format!(
        "SELECT document.rowid
         FROM {index}
         JOIN block_search_document AS document ON document.rowid = {index}.rowid
         WHERE {index} MATCH ?1
           AND {index}.rank MATCH 'bm25({FTS_BM25_WEIGHTS})'
         ORDER BY {index}.rank, document.updated_at DESC, document.rowid
         LIMIT ?2"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(params![query, limit], |row| row.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn install_ranks(ranks: &mut HashMap<i64, CandidateRanks>, rowids: Vec<i64>, index: u8) {
    for (position, rowid) in rowids.into_iter().enumerate() {
        let candidate = ranks.entry(rowid).or_default();
        match index {
            0 => candidate.word = Some(position + 1),
            1 => candidate.trigram = Some(position + 1),
            _ => candidate.short = Some(position + 1),
        }
    }
}

fn load_documents(connection: &Connection, rowids: Vec<i64>) -> Result<Vec<SearchDocument>> {
    if rowids.is_empty() {
        return Ok(Vec::new());
    }
    let rowids_json = serde_json::to_string(&rowids)?;
    let mut statement = connection.prepare(
        "SELECT document.rowid, document.tidbit_id, document.block_id, document.block_type,
                document.block_ordinal, revision.body_markdown, document.heading_context,
                document.body, document.attachment_names, document.extracted_text,
                document.updated_at
         FROM json_each(?1) AS candidate
         JOIN block_search_document AS document ON document.rowid = candidate.value
         JOIN tidbit ON tidbit.id = document.tidbit_id
                    AND tidbit.current_revision_id = document.tidbit_revision_id
                    AND tidbit.deleted_at IS NULL
         JOIN tidbit_revision AS revision ON revision.id = tidbit.current_revision_id
                                        AND revision.tidbit_id = tidbit.id",
    )?;
    let rows = statement.query_map(params![rowids_json], |row| {
        let ordinal = row.get::<_, i64>(4)?;
        Ok(SearchDocument {
            rowid: row.get(0)?,
            note_id: row.get(1)?,
            block_id: row.get(2)?,
            block_type: row.get(3)?,
            block_ordinal: u32::try_from(ordinal).unwrap_or(u32::MAX),
            display_title: tidbits::derive_display_title(&row.get::<_, String>(5)?),
            heading_context: row.get(6)?,
            body: row.get(7)?,
            attachment_names: row.get(8)?,
            extracted_text: row.get(9)?,
            updated_at_ms: row.get(10)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn score_document(
    query: &ParsedQuery,
    document: SearchDocument,
    ranks: CandidateRanks,
) -> Option<RankedBlock> {
    let fields = [
        (
            SearchField::HeadingContext,
            document.heading_context.as_str(),
        ),
        (SearchField::Body, document.body.as_str()),
        (
            SearchField::AttachmentName,
            document.attachment_names.as_str(),
        ),
        (SearchField::ExtractedText, document.extracted_text.as_str()),
    ];
    let mut matched_atoms = vec![false; query.atoms.len()];
    let mut matched_fields = BTreeSet::new();
    let mut spans = BTreeSet::new();
    let mut score = 0.0;
    for (field, value) in fields {
        for (atom_index, atom) in query.atoms.iter().enumerate() {
            let matches = find_spans(value, &atom.text);
            if matches.is_empty() {
                continue;
            }
            matched_atoms[atom_index] = true;
            matched_fields.insert(field);
            score += field.weight() * if atom.quoted { 2.0 } else { 1.0 };
            for (start_char, end_char) in matches {
                spans.insert((field, start_char, end_char));
            }
        }
    }
    let matched_count = matched_atoms.iter().filter(|matched| **matched).count();
    let qualifies = match query.mode {
        LexicalSearchMode::Exact => matched_atoms.iter().all(|matched| *matched),
        LexicalSearchMode::Default => matched_count >= query.atoms.len().min(2),
    };
    if !qualifies {
        return None;
    }
    for rank in [ranks.word, ranks.trigram, ranks.short]
        .into_iter()
        .flatten()
    {
        score += 8.0 / (RRF_CONSTANT + rank as f64);
    }
    // Recency is only a deterministic tie-breaker; relevance owns the score.
    score += (document.updated_at_ms.max(0) as f64) * f64::EPSILON;
    Some(RankedBlock {
        rowid: document.rowid,
        score,
        matched_fields: matched_fields.into_iter().collect(),
        highlights: spans
            .into_iter()
            .take(MAX_HIGHLIGHTS_PER_RESULT)
            .map(|(field, start_char, end_char)| SearchHighlight {
                field,
                start_char,
                end_char,
            })
            .collect(),
    })
}

fn semantic_candidates(
    connection: &Connection,
    embedding: &[f32],
    limit: u32,
) -> Result<Vec<(i64, f64)>> {
    let manifest = embedding::jina_v1_manifest();
    let vector_json = serde_json::to_string(embedding)?;
    let rerank_limit = limit.saturating_mul(2).min(1_024);
    let mut statement = connection.prepare(
        "SELECT vector.rowid, vector.distance
         FROM block_embedding_vec_jina_v1 AS vector
         JOIN block_search_document AS document ON document.rowid = vector.rowid
         JOIN block_embedding AS metadata
           ON metadata.tidbit_id = document.tidbit_id
          AND metadata.block_id = document.block_id
          AND metadata.embedding_index_id = ?3
          AND metadata.block_content_hash = document.content_hash
         CROSS JOIN block_embedding_settings AS settings
         WHERE vector.embedding MATCH ?1
           AND k = ?2
           AND settings.singleton_id = 1
           AND settings.active_embedding_index_id = metadata.embedding_index_id
           AND NOT EXISTS(SELECT 1 FROM block_embedding_reap_queue AS reap WHERE reap.block_rowid = vector.rowid)
         ORDER BY vector.distance, vector.rowid",
    )?;
    let rows = statement.query_map(
        params![vector_json, rerank_limit, manifest.id.as_str()],
        |row| {
            let distance = row.get::<_, f64>(1)?;
            Ok((row.get::<_, i64>(0)?, (1.0 - distance).clamp(-1.0, 1.0)))
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn fuse(lexical: Vec<RankedBlock>, semantic: Vec<(i64, f64)>) -> Vec<RankedBlock> {
    let mut fused = HashMap::<i64, RankedBlock>::new();
    let mut lexical_ranks = HashMap::<i64, usize>::new();
    for (index, mut candidate) in lexical.into_iter().enumerate() {
        let rank = index + 1;
        lexical_ranks.insert(candidate.rowid, rank);
        candidate.score = LEXICAL_WEIGHT / (RRF_CONSTANT + rank as f64);
        fused.insert(candidate.rowid, candidate);
    }
    for (index, (rowid, similarity)) in semantic.into_iter().enumerate() {
        let rank = index + 1;
        if !fused.contains_key(&rowid) && rank > 1 {
            continue;
        }
        let candidate = fused.entry(rowid).or_insert(RankedBlock {
            rowid,
            score: 0.0,
            matched_fields: Vec::new(),
            highlights: Vec::new(),
        });
        candidate.score += SEMANTIC_WEIGHT * similarity.max(0.0) / (RRF_CONSTANT + rank as f64);
        if let Some(lexical_rank) = lexical_ranks.get(&rowid) {
            candidate.score += AGREEMENT_WEIGHT / (RRF_CONSTANT + (*lexical_rank).min(rank) as f64);
        }
    }
    let mut candidates = fused.into_values().collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.rowid.cmp(&right.rowid))
    });
    candidates
}

fn hydrate(
    connection: &Connection,
    ranked: Vec<RankedBlock>,
    limit: usize,
) -> Result<Vec<BlockSearchResult>> {
    let by_id = ranked
        .iter()
        .map(|candidate| candidate.rowid)
        .collect::<Vec<_>>();
    let documents = load_documents(connection, by_id)?
        .into_iter()
        .map(|document| (document.rowid, document))
        .collect::<BTreeMap<_, _>>();
    Ok(ranked
        .into_iter()
        .filter_map(|candidate| {
            let document = documents.get(&candidate.rowid)?;
            let excerpt = [
                &document.body,
                &document.extracted_text,
                &document.attachment_names,
            ]
            .into_iter()
            .find(|value| !value.trim().is_empty())
            .cloned()
            .unwrap_or_default();
            Some(BlockSearchResult {
                note_id: document.note_id.clone(),
                block_id: document.block_id.clone(),
                block_type: document.block_type.clone(),
                block_ordinal: document.block_ordinal,
                display_title: document.display_title.clone(),
                heading_context: document
                    .heading_context
                    .lines()
                    .map(str::to_owned)
                    .filter(|value| !value.is_empty())
                    .collect(),
                excerpt,
                attachment_names: document
                    .attachment_names
                    .lines()
                    .map(str::to_owned)
                    .filter(|value| !value.is_empty())
                    .collect(),
                score: candidate.score,
                matched_fields: candidate.matched_fields,
                highlights: candidate.highlights,
            })
        })
        .take(limit)
        .collect())
}

impl ParsedQuery {
    fn parse(query: &str, mode: LexicalSearchMode) -> Result<Option<Self>> {
        if query.chars().count() > MAX_QUERY_CHARACTERS {
            return Err(DatabaseError::InvalidInput(format!(
                "query must contain at most {MAX_QUERY_CHARACTERS} characters"
            )));
        }
        let atoms = parse_atoms(query);
        Ok((!atoms.is_empty()).then_some(Self { atoms, mode }))
    }

    fn searchable(&self) -> bool {
        self.atoms.iter().any(|atom| {
            atom.text
                .chars()
                .any(|character| character.is_alphanumeric() || character == '_')
        })
    }

    fn word_query(&self) -> Option<String> {
        let mut clauses = Vec::new();
        for atom in &self.atoms {
            let tokens = word_tokens(&atom.text);
            if atom.quoted && tokens.len() > 1 {
                clauses.push(format!("\"{}\"", tokens.join(" ")));
            } else {
                clauses.extend(tokens.into_iter().map(|token| format!("\"{token}\"")));
            }
        }
        join_clauses(clauses, self.mode)
    }

    fn trigram_query(&self) -> Option<String> {
        let mut clauses = BTreeSet::new();
        for atom in &self.atoms {
            let text = normalize(atom.text.trim());
            let characters = text.chars().collect::<Vec<_>>();
            if characters.len() < 3 {
                continue;
            }
            clauses.insert(format!("\"{}\"", text.replace('"', "\"\"")));
            if self.mode == LexicalSearchMode::Default && !atom.quoted {
                clauses.extend(
                    characters
                        .windows(3)
                        .map(|window| format!("\"{}\"", window.iter().collect::<String>())),
                );
            }
        }
        join_clauses(clauses, self.mode)
    }

    fn short_query(&self) -> Option<String> {
        if self
            .atoms
            .iter()
            .any(|atom| normalize(&atom.text).chars().count() > 2)
        {
            return None;
        }
        join_clauses(
            self.atoms.iter().filter_map(|atom| {
                let chars = normalize(&atom.text).chars().collect::<Vec<_>>();
                ((1..=2).contains(&chars.len())).then(|| format!("\"{}\"", short_gram(&chars)))
            }),
            self.mode,
        )
    }
}

fn join_clauses<I>(clauses: I, mode: LexicalSearchMode) -> Option<String>
where
    I: IntoIterator<Item = String>,
{
    let clauses = clauses
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!clauses.is_empty()).then(|| {
        clauses.join(if mode == LexicalSearchMode::Exact {
            " AND "
        } else {
            " OR "
        })
    })
}

fn parse_atoms(query: &str) -> Vec<QueryAtom> {
    let mut atoms = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in query.chars() {
        match character {
            '"' => {
                if quoted {
                    push_atom(&mut atoms, &mut current, true);
                } else {
                    push_atom(&mut atoms, &mut current, false);
                }
                quoted = !quoted;
            }
            character if character.is_whitespace() && !quoted => {
                push_atom(&mut atoms, &mut current, false)
            }
            _ => current.push(character),
        }
    }
    push_atom(&mut atoms, &mut current, quoted);
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
    normalize(value)
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(crate) fn normalize(value: &str) -> String {
    value
        .nfkd()
        .filter(|character| !is_combining_mark(*character))
        .flat_map(char::to_lowercase)
        .collect()
}

pub(crate) fn short_grams(value: &str) -> String {
    let characters = normalize(value).chars().collect::<Vec<_>>();
    let mut tokens = BTreeSet::new();
    for start in 0..characters.len() {
        tokens.insert(short_gram(&characters[start..=start]));
        if let Some(end) = start.checked_add(2).filter(|end| *end <= characters.len()) {
            tokens.insert(short_gram(&characters[start..end]));
        }
    }
    tokens.into_iter().collect::<Vec<_>>().join(" ")
}

fn short_gram(chars: &[char]) -> String {
    let prefix = if chars.len() == 1 { 'a' } else { 'b' };
    let encoded = chars
        .iter()
        .map(|character| format!("{:06x}", u32::from(*character)))
        .collect::<String>();
    format!("{prefix}{encoded}")
}

fn find_spans(value: &str, needle: &str) -> Vec<(u32, u32)> {
    let normalized_needle = normalize(needle)
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '_' {
                character
            } else {
                ' '
            }
        })
        .collect::<String>();
    if normalized_needle.is_empty() {
        return Vec::new();
    }
    let original = value.chars().collect::<Vec<_>>();
    let mut normalized = Vec::new();
    let mut mapping = Vec::new();
    for (index, character) in original.iter().enumerate() {
        for lowered in character
            .to_string()
            .nfkd()
            .filter(|candidate| !is_combining_mark(*candidate))
            .flat_map(char::to_lowercase)
        {
            normalized.push(if lowered.is_alphanumeric() || lowered == '_' {
                lowered
            } else {
                ' '
            });
            mapping.push(index);
        }
    }
    let haystack = normalized.iter().collect::<String>();
    let mut spans = Vec::new();
    for (byte_start, _) in haystack.match_indices(&normalized_needle) {
        let char_start = haystack[..byte_start].chars().count();
        let char_end = char_start + normalized_needle.chars().count();
        if char_end == 0 || char_end > mapping.len() {
            continue;
        }
        spans.push((
            mapping[char_start] as u32,
            (mapping[char_end - 1] + 1) as u32,
        ));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_preserves_phrases_and_short_grams() {
        let query = ParsedQuery::parse("alpha \"beta gamma\"", LexicalSearchMode::Exact)
            .unwrap()
            .unwrap();
        assert_eq!(
            query.word_query().as_deref(),
            Some("\"alpha\" AND \"beta gamma\"")
        );
        assert_eq!(short_grams("AI"), "a000061 a000069 b000061000069");
    }

    #[test]
    fn highlights_map_normalized_unicode_to_original_characters() {
        assert_eq!(find_spans("Café notes", "cafe"), vec![(0, 4)]);
    }
}

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap},
};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

use super::{passages, CitationResolution, DatabaseError, Result};

const MAX_QUERY_CHARACTERS: usize = 512;
const MAX_SEARCH_LIMIT: u32 = 100;
const MAX_HIGHLIGHTS_PER_RESULT: usize = 32;
const MIN_CANDIDATE_LIMIT: u32 = 64;
const MAX_CANDIDATE_LIMIT: u32 = 512;
const RRF_RANK_CONSTANT: f64 = 60.0;
const FTS_VERSION: &str = "lexical-v1";

pub(crate) fn candidate_limit(result_limit: u32) -> u32 {
    result_limit
        .saturating_mul(16)
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
    Title,
    HeadingContext,
    Body,
    SourceLabel,
    SourceDomain,
    AttachmentName,
    ExtractedText,
}

impl SearchField {
    const fn weight(self) -> f64 {
        match self {
            Self::Title => 8.0,
            Self::HeadingContext => 6.0,
            Self::Body => 3.0,
            Self::SourceLabel => 4.5,
            Self::SourceDomain => 5.0,
            Self::AttachmentName => 5.0,
            Self::ExtractedText => 2.5,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::HeadingContext => "headingContext",
            Self::Body => "body",
            Self::SourceLabel => "sourceLabel",
            Self::SourceDomain => "sourceDomain",
            Self::AttachmentName => "attachmentName",
            Self::ExtractedText => "extractedText",
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
pub struct PassageSearchResult {
    pub passage_id: String,
    pub score: f64,
    pub matched_fields: Vec<SearchField>,
    pub highlights: Vec<SearchHighlight>,
    pub citation: CitationResolution,
}

#[derive(Clone, Debug)]
pub(crate) struct LexicalDocument {
    pub passage_id: String,
    pub updated_at_ms: i64,
    pub fields: BTreeMap<SearchField, String>,
    pub word_rank: Option<usize>,
    pub trigram_rank: Option<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct RankedLexicalDocument {
    pub passage_id: String,
    pub score: f64,
    pub matched_fields: Vec<SearchField>,
    pub highlights: Vec<SearchHighlight>,
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
        let clauses = self
            .atoms
            .iter()
            .filter_map(|atom| {
                let text = atom.text.trim();
                (normalize(text).chars().count() >= 3)
                    .then(|| format!("\"{}\"", text.replace('"', "\"\"")))
            })
            .collect::<Vec<_>>();
        join_fts_clauses(clauses, self.mode)
    }
}

pub(super) fn search_passages(
    connection: &Connection,
    input: SearchPassagesInput,
) -> Result<Vec<PassageSearchResult>> {
    if input.limit == 0 || input.limit > MAX_SEARCH_LIMIT {
        return Err(DatabaseError::InvalidInput(format!(
            "limit must be between 1 and {MAX_SEARCH_LIMIT}"
        )));
    }
    let Some(query) = ParsedLexicalQuery::parse(&input.query, input.mode)? else {
        return Ok(Vec::new());
    };
    let candidate_limit = candidate_limit(input.limit);
    let mut ranks = HashMap::<String, CandidateRanks>::new();
    if let Some(word_query) = query.word_match_query() {
        install_ranks(
            &mut ranks,
            query_fts_index(connection, "passage_fts_word", &word_query, candidate_limit)?,
            CandidateIndex::Word,
        );
    }
    if let Some(trigram_query) = query.trigram_match_query() {
        install_ranks(
            &mut ranks,
            query_fts_index(
                connection,
                "passage_fts_trigram",
                &trigram_query,
                candidate_limit,
            )?,
            CandidateIndex::Trigram,
        );
    }
    if ranks.is_empty() {
        return Ok(Vec::new());
    }

    let mut documents = Vec::with_capacity(ranks.len());
    for (passage_id, ranks) in ranks {
        if let Some(document) = load_search_document(connection, &passage_id, ranks)? {
            documents.push(document);
        }
    }
    let ranked = rank_lexical_documents(&query, documents, input.limit as usize);
    ranked
        .into_iter()
        .map(|ranked| {
            let citation = passages::resolve_citation(connection, &ranked.passage_id)?;
            if citation.state != super::CitationState::Current {
                return Err(DatabaseError::Validation {
                    kind: "main",
                    reason: format!("search returned non-current passage {}", ranked.passage_id),
                });
            }
            Ok(PassageSearchResult {
                passage_id: ranked.passage_id,
                score: ranked.score,
                matched_fields: ranked.matched_fields,
                highlights: ranked.highlights,
                citation,
            })
        })
        .collect()
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
            title,
            heading_context,
            body,
            source_labels,
            source_domains,
            attachment_names,
            extracted_text,
            owner_content_hash,
            updated_at
         )
         SELECT
            passage.rowid,
            passage.id,
            active.tidbit_id,
            coalesce(revision.title, ''),
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
                    SELECT group_concat(coalesce(source.label, ''), char(10))
                    FROM tidbit_revision_source AS membership
                    JOIN source ON source.id = membership.source_id
                    WHERE membership.tidbit_revision_id = revision.id
                    ORDER BY membership.sort_order
                ),
                ''
            ),
            coalesce(
                (
                    SELECT group_concat(coalesce(source.normalized_url, ''), char(10))
                    FROM tidbit_revision_source AS membership
                    JOIN source ON source.id = membership.source_id
                    WHERE membership.tidbit_revision_id = revision.id
                    ORDER BY membership.sort_order
                ),
                ''
            ),
            coalesce(
                (
                    SELECT group_concat(attachment.display_filename, char(10))
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

pub(super) fn rebuild_documents(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute("DELETE FROM passage_search_document", [])?;
    transaction.execute(
        "INSERT INTO passage_search_document(
            rowid,
            passage_id,
            tidbit_id,
            title,
            heading_context,
            body,
            source_labels,
            source_domains,
            attachment_names,
            extracted_text,
            owner_content_hash,
            updated_at
         )
         SELECT
            passage.rowid,
            passage.id,
            active.tidbit_id,
            coalesce(revision.title, ''),
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
                    SELECT group_concat(coalesce(source.label, ''), char(10))
                    FROM tidbit_revision_source AS membership
                    JOIN source ON source.id = membership.source_id
                    WHERE membership.tidbit_revision_id = revision.id
                    ORDER BY membership.sort_order
                ),
                ''
            ),
            coalesce(
                (
                    SELECT group_concat(coalesce(source.normalized_url, ''), char(10))
                    FROM tidbit_revision_source AS membership
                    JOIN source ON source.id = membership.source_id
                    WHERE membership.tidbit_revision_id = revision.id
                    ORDER BY membership.sort_order
                ),
                ''
            ),
            coalesce(
                (
                    SELECT group_concat(attachment.display_filename, char(10))
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
            title,
            heading_context,
            body,
            source_labels,
            source_domains,
            attachment_names,
            extracted_text,
            owner_content_hash,
            updated_at
         )
         SELECT
            passage.rowid,
            passage.id,
            NULL,
            '',
            coalesce(
                (
                    SELECT group_concat(value, char(10))
                    FROM json_each(passage.heading_context_json)
                ),
                ''
            ),
            '',
            '',
            '',
            attachment.display_filename,
            passage.content,
            passage.content_hash,
            attachment.updated_at
         FROM passage
         JOIN attachment_segment AS segment
           ON segment.id = passage.attachment_segment_id
         JOIN attachment_extraction AS extraction
           ON extraction.id = segment.extraction_id
          AND extraction.status = 'READY'
         JOIN attachment
           ON attachment.id = extraction.attachment_id
          AND attachment.sha256 = extraction.content_hash
          AND attachment.deleted_at IS NULL
         WHERE passage.owner_kind = 'ATTACHMENT'
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
}

enum CandidateIndex {
    Word,
    Trigram,
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
         JOIN passage ON passage.id = document.passage_id
         WHERE {index} MATCH ?1
           AND (
               (
                   passage.owner_kind = 'AUTHOR'
                   AND EXISTS (
                       SELECT 1
                       FROM active_passage AS active
                       JOIN tidbit
                         ON tidbit.id = active.tidbit_id
                        AND tidbit.deleted_at IS NULL
                        AND tidbit.current_revision_id = passage.tidbit_revision_id
                       JOIN tidbit_revision AS revision
                         ON revision.id = tidbit.current_revision_id
                        AND revision.tidbit_id = tidbit.id
                        AND revision.content_hash = document.owner_content_hash
                       WHERE active.passage_id = passage.id
                         AND active.tidbit_id = document.tidbit_id
                   )
               )
               OR (
                   passage.owner_kind = 'ATTACHMENT'
                   AND document.tidbit_id IS NULL
                   AND document.owner_content_hash = passage.content_hash
                   AND EXISTS (
                       SELECT 1
                       FROM attachment_segment AS segment
                       JOIN attachment_extraction AS extraction
                         ON extraction.id = segment.extraction_id
                        AND extraction.status = 'READY'
                       JOIN attachment
                         ON attachment.id = extraction.attachment_id
                        AND attachment.sha256 = extraction.content_hash
                        AND attachment.deleted_at IS NULL
                       WHERE segment.id = passage.attachment_segment_id
                   )
               )
           )
         ORDER BY bm25({index}, 8.0, 6.0, 3.0, 4.5, 5.0, 5.0, 2.5),
                  document.updated_at DESC,
                  document.passage_id
         LIMIT ?2"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(params![query, limit], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn load_search_document(
    connection: &Connection,
    passage_id: &str,
    ranks: CandidateRanks,
) -> Result<Option<LexicalDocument>> {
    let authored = connection
        .query_row(
            "SELECT
                tidbit.updated_at,
                coalesce(revision.title, ''),
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
                        SELECT group_concat(coalesce(source.label, ''), char(10))
                        FROM tidbit_revision_source AS membership
                        JOIN source ON source.id = membership.source_id
                        WHERE membership.tidbit_revision_id = revision.id
                        ORDER BY membership.sort_order
                    ),
                    ''
                ),
                coalesce(
                    (
                        SELECT group_concat(coalesce(source.normalized_url, ''), char(10))
                        FROM tidbit_revision_source AS membership
                        JOIN source ON source.id = membership.source_id
                        WHERE membership.tidbit_revision_id = revision.id
                        ORDER BY membership.sort_order
                    ),
                    ''
                ),
                coalesce(
                    (
                        SELECT group_concat(attachment.display_filename, char(10))
                        FROM tidbit_revision_attachment AS membership
                        JOIN attachment ON attachment.id = membership.attachment_id
                        WHERE membership.tidbit_revision_id = revision.id
                          AND attachment.deleted_at IS NULL
                        ORDER BY membership.sort_order
                    ),
                    ''
                ),
                ''
             FROM passage_search_document AS document
             JOIN passage ON passage.id = document.passage_id
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
             WHERE document.passage_id = ?1",
            params![passage_id],
            |row| {
                let fields = [
                    (SearchField::Title, row.get::<_, String>(1)?),
                    (SearchField::HeadingContext, row.get::<_, String>(2)?),
                    (SearchField::Body, row.get::<_, String>(3)?),
                    (SearchField::SourceLabel, row.get::<_, String>(4)?),
                    (SearchField::SourceDomain, row.get::<_, String>(5)?),
                    (SearchField::AttachmentName, row.get::<_, String>(6)?),
                    (SearchField::ExtractedText, row.get::<_, String>(7)?),
                ]
                .into_iter()
                .collect();
                Ok(LexicalDocument {
                    passage_id: passage_id.to_owned(),
                    updated_at_ms: row.get(0)?,
                    fields,
                    word_rank: ranks.word,
                    trigram_rank: ranks.trigram,
                })
            },
        )
        .optional()?;
    if authored.is_some() {
        return Ok(authored);
    }

    connection
        .query_row(
            "SELECT
                attachment.updated_at,
                '',
                coalesce(
                    (
                        SELECT group_concat(value, char(10))
                        FROM json_each(passage.heading_context_json)
                    ),
                    ''
                ),
                '',
                '',
                '',
                attachment.display_filename,
                passage.content
             FROM passage_search_document AS document
             JOIN passage
               ON passage.id = document.passage_id
              AND passage.owner_kind = 'ATTACHMENT'
              AND passage.content_hash = document.owner_content_hash
             JOIN attachment_segment AS segment
               ON segment.id = passage.attachment_segment_id
             JOIN attachment_extraction AS extraction
               ON extraction.id = segment.extraction_id
              AND extraction.status = 'READY'
             JOIN attachment
               ON attachment.id = extraction.attachment_id
              AND attachment.sha256 = extraction.content_hash
              AND attachment.deleted_at IS NULL
             WHERE document.passage_id = ?1
               AND document.tidbit_id IS NULL",
            params![passage_id],
            |row| {
                let fields = [
                    (SearchField::Title, row.get::<_, String>(1)?),
                    (SearchField::HeadingContext, row.get::<_, String>(2)?),
                    (SearchField::Body, row.get::<_, String>(3)?),
                    (SearchField::SourceLabel, row.get::<_, String>(4)?),
                    (SearchField::SourceDomain, row.get::<_, String>(5)?),
                    (SearchField::AttachmentName, row.get::<_, String>(6)?),
                    (SearchField::ExtractedText, row.get::<_, String>(7)?),
                ]
                .into_iter()
                .collect();
                Ok(LexicalDocument {
                    passage_id: passage_id.to_owned(),
                    updated_at_ms: row.get(0)?,
                    fields,
                    word_rank: ranks.word,
                    trigram_rank: ranks.trigram,
                })
            },
        )
        .optional()
        .map_err(Into::into)
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
            matched_fields: scored.matched_fields,
            highlights: scored.highlights,
        })
        .collect()
}

struct ScoredDocument {
    passage_id: String,
    updated_at_ms: i64,
    score: f64,
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
            let spans = find_normalized_spans(value, &atom.text);
            if spans.is_empty() {
                continue;
            }
            matched_atoms[atom_index] = true;
            matched_fields.insert(*field);
            let phrase_multiplier = if atom.quoted { 2.0 } else { 1.0 };
            field_score += field.weight() * phrase_multiplier;
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
    value
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn join_fts_clauses(clauses: Vec<String>, mode: LexicalSearchMode) -> Option<String> {
    (!clauses.is_empty()).then(|| {
        clauses.join(match mode {
            LexicalSearchMode::Default => " OR ",
            LexicalSearchMode::Exact => " AND ",
        })
    })
}

fn normalize(value: &str) -> String {
    normalize_with_mapping(value).0.into_iter().collect()
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
    let needle = normalize(needle).chars().collect::<Vec<_>>();
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rusqlite::params;

    use super::{
        parse_lexical_query, rank_lexical_documents, LexicalDocument, LexicalSearchMode,
        SearchField, SearchPassagesInput,
    };
    use crate::database::{
        connection::{self, DatabaseKind, FileState},
        tidbits::{CreateTidbitWrite, EditTidbitWrite},
        Database, DatabasePaths, DeleteTidbitInput, EditTidbitInput, RestoreTidbitInput,
        SourceDraft, TidbitDraft,
    };

    fn document(
        id: &str,
        fields: impl IntoIterator<Item = (SearchField, &'static str)>,
    ) -> LexicalDocument {
        LexicalDocument {
            passage_id: id.into(),
            updated_at_ms: 0,
            fields: fields
                .into_iter()
                .map(|(field, value)| (field, value.into()))
                .collect::<BTreeMap<_, _>>(),
            word_rank: None,
            trigram_rank: None,
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
    fn field_relevance_outranks_recency() {
        let parsed = parse_lexical_query("sentinel", LexicalSearchMode::Default)
            .expect("valid query")
            .expect("nonempty query");
        let mut title_match = document(
            "older-title",
            [(SearchField::Title, "Sentinel architecture")],
        );
        title_match.updated_at_ms = 1;
        let mut body_match = document(
            "newer-body",
            [(SearchField::Body, "A sentinel appears in recent prose.")],
        );
        body_match.updated_at_ms = 1_000_000;

        let ranked = rank_lexical_documents(&parsed, vec![body_match, title_match], 10);

        assert_eq!(ranked[0].passage_id, "older-title");
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
                    title: Some("FTS tokenizer reminder".into()),
                    body_markdown:
                        "# SQLite\n\nThe first lexical sentinel uses `resolveCitationTarget`."
                            .into(),
                    sources: vec![SourceDraft {
                        label: Some("SQLite FTS5 documentation".into()),
                        url: Some("https://sqlite.org/fts5.html".into()),
                    }],
                },
                now_ms: 10,
                tidbit_id: "019f547b-6200-7000-8000-000000009001".into(),
                revision_id: "019f547b-6200-7000-8000-000000009002".into(),
                source_ids: vec!["019f547b-6200-7000-8000-000000009003".into()],
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
        assert!(initial[0]
            .matched_fields
            .contains(&SearchField::SourceDomain));
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

        let edited = client
            .edit_tidbit(EditTidbitWrite {
                input: EditTidbitInput {
                    id: created.id.clone(),
                    expected_revision_id: created.current_revision_id.clone(),
                    title: Some("Updated lexical note".into()),
                    body_markdown: "# Search\n\nThe replacement carries café C R2 and $$E=mc^2$$."
                        .into(),
                    sources: Vec::new(),
                },
                now_ms: 20,
                revision_id: "019f547b-6200-7000-8000-000000009004".into(),
                source_ids: Vec::new(),
            })
            .expect("edit searchable tidbit");
        assert!(client
            .search_passages(SearchPassagesInput {
                query: "first lexical sentinel".into(),
                mode: LexicalSearchMode::Exact,
                limit: 10,
            })
            .expect("old revision search")
            .is_empty());
        for query in ["cafe", "C", "R2", "E=mc^2"] {
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
    fn database_search_indexes_ready_attachment_passages_and_tracks_deletion() {
        let root = tempfile::tempdir().expect("temporary attachment search library");
        let paths = DatabasePaths::new(root.path());
        drop(Database::initialize(paths.clone()).expect("attachment search database"));
        let connection =
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
                 INSERT INTO attachment_extraction(
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
                 );",
            )
            .expect("ready attachment passage");
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
            results[0]
                .citation
                .attachment
                .as_ref()
                .map(|attachment| attachment.display_filename.as_str()),
            Some("calibration-plate.pdf")
        );
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
                    title: None,
                    body_markdown: "Literal OR NEAR syntax stays authored text.".into(),
                    sources: Vec::new(),
                },
                now_ms: 10,
                tidbit_id: "019f547b-6200-7000-8000-000000009101".into(),
                revision_id: "019f547b-6200-7000-8000-000000009102".into(),
                source_ids: Vec::new(),
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
                    title: Some("Trusted title".into()),
                    body_markdown: "Immutable authored evidence.".into(),
                    sources: Vec::new(),
                },
                now_ms: 10,
                tidbit_id: "019f547b-6200-7000-8000-000000009201".into(),
                revision_id: "019f547b-6200-7000-8000-000000009202".into(),
                source_ids: Vec::new(),
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
                 SET title = 'fabricated_needle'
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

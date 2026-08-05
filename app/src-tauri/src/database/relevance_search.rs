use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap},
};

use serde::{Deserialize, Serialize};
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

use super::{DatabaseError, Result};

const MAX_QUERY_CHARACTERS: usize = 512;
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
const SEMANTIC_EVIDENCE_PENALTY: f64 = 0.1;
const INITIAL_RESULTS_PER_ATTACHMENT: usize = 2;
pub(crate) const FTS_BM25_WEIGHTS: &str = "6.0, 3.5, 4.5, 5.0, 5.0, 2.25";

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

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SearchField {
    HeadingContext,
    Body,
    SourceLabel,
    SourceDomain,
    AttachmentName,
    ExtractedText,
}

impl SearchField {
    const fn weight(self, evidence_kind: SearchEvidenceKind) -> f64 {
        match self {
            Self::HeadingContext => 6.0,
            Self::Body => 3.5,
            Self::SourceLabel => 4.5,
            Self::SourceDomain => 5.0,
            Self::AttachmentName => 5.0,
            Self::ExtractedText => evidence_kind.extracted_text_weight(),
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::HeadingContext => "headingContext",
            Self::Body => "body",
            Self::SourceLabel => "sourceLabel",
            Self::SourceDomain => "sourceDomain",
            Self::AttachmentName => "attachmentName",
            Self::ExtractedText => "extractedText",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SearchEvidenceKind {
    Author,
    Ocr,
}

impl SearchEvidenceKind {
    const fn extracted_text_weight(self) -> f64 {
        match self {
            Self::Author => 0.0,
            Self::Ocr => 1.75,
        }
    }

    pub(crate) const fn semantic_weight(self) -> f64 {
        match self {
            Self::Author => 1.0,
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

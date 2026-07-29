use rusqlite::{params, Connection, OptionalExtension};

use crate::database::{
    passages, search, tidbits, CitationResolution, CitationState, LexicalSearchMode,
    SearchPassagesInput, SearchPassagesResponse, SemanticSearchReadiness, Tidbit,
};

use super::{ResearchError, ResearchErrorCode, ResearchResourceSnapshot};

pub(super) struct ResearchLibrary {
    connection: Connection,
}

impl ResearchLibrary {
    pub(super) fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub(super) fn search(
        &self,
        query: String,
        exact: bool,
        limit: u32,
        query_embedding: Option<&[f32]>,
        fallback_readiness: SemanticSearchReadiness,
    ) -> Result<SearchPassagesResponse, ResearchError> {
        search::search_passages_with_semantics(
            &self.connection,
            SearchPassagesInput {
                query,
                mode: if exact {
                    LexicalSearchMode::Exact
                } else {
                    LexicalSearchMode::Default
                },
                limit,
            },
            query_embedding,
            fallback_readiness,
        )
        .map_err(ResearchError::from_database)
    }

    pub(super) fn validate_current_citation(
        &self,
        snapshot: &CitationResolution,
    ) -> Result<CitationResolution, ResearchError> {
        let current = passages::resolve_citation(&self.connection, &snapshot.passage_id)
            .map_err(ResearchError::from_database)?;
        if current.state != CitationState::Current {
            return Err(content_state_error(&current));
        }
        if !same_evidence(snapshot, &current) {
            return Err(ResearchError::new(
                ResearchErrorCode::StaleContent,
                "the evidence changed after this research handle was issued",
            ));
        }
        Ok(current)
    }

    pub(super) fn passage_context(
        &self,
        snapshot: &CitationResolution,
        before: usize,
        after: usize,
    ) -> Result<Vec<CitationResolution>, ResearchError> {
        self.validate_current_citation(snapshot)?;
        let ids = if snapshot.tidbit.is_some() {
            self.author_passage_ids(&snapshot.passage_id)?
        } else if snapshot.attachment.is_some() {
            self.attachment_passage_ids_for_target(&snapshot.passage_id)?
        } else {
            return Err(ResearchError::new(
                ResearchErrorCode::ContentUnavailable,
                "the evidence has no readable Kosh owner",
            ));
        };
        let target = ids
            .iter()
            .position(|passage_id| passage_id == &snapshot.passage_id)
            .ok_or_else(|| {
                ResearchError::new(
                    ResearchErrorCode::StaleContent,
                    "the evidence is no longer in its current passage set",
                )
            })?;
        let start = target.saturating_sub(before);
        let end = (target + after + 1).min(ids.len());
        ids[start..end]
            .iter()
            .map(|passage_id| {
                let citation = passages::resolve_citation(&self.connection, passage_id)
                    .map_err(ResearchError::from_database)?;
                if citation.state != CitationState::Current {
                    return Err(content_state_error(&citation));
                }
                Ok(citation)
            })
            .collect()
    }

    pub(super) fn current_tidbit_passage_page(
        &self,
        snapshot: &ResearchResourceSnapshot,
        offset: usize,
        limit: usize,
    ) -> Result<(Tidbit, Vec<CitationResolution>, bool), ResearchError> {
        let ResearchResourceSnapshot::Tidbit {
            id, revision_id, ..
        } = snapshot
        else {
            return Err(ResearchError::new(
                ResearchErrorCode::WrongHandleKind,
                "this tool requires a tidbit owner handle",
            ));
        };
        let tidbit =
            tidbits::load_tidbit(&self.connection, id).map_err(ResearchError::from_database)?;
        if tidbit.deleted_at_ms.is_some() {
            return Err(ResearchError::new(
                ResearchErrorCode::ContentDeleted,
                "the tidbit was deleted after this research handle was issued",
            ));
        }
        if tidbit.current_revision_id != *revision_id {
            return Err(ResearchError::new(
                ResearchErrorCode::StaleContent,
                "the tidbit changed after this research handle was issued",
            ));
        }
        let query_limit = i64::try_from(limit.saturating_add(1)).map_err(|_| {
            ResearchError::new(
                ResearchErrorCode::LimitExceeded,
                "the tidbit page limit is too large",
            )
        })?;
        let query_offset = i64::try_from(offset).map_err(|_| {
            ResearchError::new(
                ResearchErrorCode::LimitExceeded,
                "the tidbit page offset is too large",
            )
        })?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT passage.id
                 FROM passage
                 JOIN active_passage ON active_passage.passage_id = passage.id
                 WHERE passage.tidbit_revision_id = ?1
                   AND passage.owner_kind = 'AUTHOR'
                 ORDER BY passage.ordinal
                 LIMIT ?2 OFFSET ?3",
            )
            .map_err(ResearchError::from_sqlite)?;
        let rows = statement
            .query_map(params![revision_id, query_limit, query_offset], |row| {
                row.get::<_, String>(0)
            })
            .map_err(ResearchError::from_sqlite)?;
        let mut ids = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(ResearchError::from_sqlite)?;
        let has_more = ids.len() > limit;
        ids.truncate(limit);
        let passages = self.resolve_current_passages(ids)?;
        Ok((tidbit, passages, has_more))
    }

    pub(super) fn current_attachment_passage_page(
        &self,
        snapshot: &ResearchResourceSnapshot,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<CitationResolution>, bool), ResearchError> {
        let ResearchResourceSnapshot::Attachment {
            id,
            extraction_id,
            provenance_passage_id,
            ..
        } = snapshot
        else {
            return Err(ResearchError::new(
                ResearchErrorCode::WrongHandleKind,
                "this tool requires an attachment owner handle",
            ));
        };
        let deleted = self
            .connection
            .query_row(
                "SELECT deleted_at IS NOT NULL FROM attachment WHERE id = ?1",
                params![id],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map_err(ResearchError::from_sqlite)?
            .ok_or_else(|| {
                ResearchError::new(
                    ResearchErrorCode::ContentUnavailable,
                    "the attachment is no longer available",
                )
            })?;
        if deleted {
            return Err(ResearchError::new(
                ResearchErrorCode::ContentDeleted,
                "the attachment was deleted after this research handle was issued",
            ));
        }
        let provenance_is_current = self
            .connection
            .query_row(
                "SELECT EXISTS (
                    SELECT 1
                    FROM current_attachment_passage
                    WHERE passage_id = ?1
                      AND attachment_id = ?2
                      AND extraction_id = ?3
                 )",
                params![provenance_passage_id, id, extraction_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(ResearchError::from_sqlite)?;
        if !provenance_is_current {
            return Err(ResearchError::new(
                ResearchErrorCode::StaleContent,
                "the attachment extraction changed after this research handle was issued",
            ));
        }
        let query_limit = i64::try_from(limit.saturating_add(1)).map_err(|_| {
            ResearchError::new(
                ResearchErrorCode::LimitExceeded,
                "the attachment page limit is too large",
            )
        })?;
        let query_offset = i64::try_from(offset).map_err(|_| {
            ResearchError::new(
                ResearchErrorCode::LimitExceeded,
                "the attachment page offset is too large",
            )
        })?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT passage.id
                 FROM current_attachment_passage AS current
                 JOIN passage ON passage.id = current.passage_id
                 JOIN attachment_segment AS segment
                   ON segment.id = passage.attachment_segment_id
                 WHERE current.attachment_id = ?1
                   AND current.extraction_id = ?2
                 ORDER BY segment.ordinal, passage.ordinal
                 LIMIT ?3 OFFSET ?4",
            )
            .map_err(ResearchError::from_sqlite)?;
        let rows = statement
            .query_map(
                params![id, extraction_id, query_limit, query_offset],
                |row| row.get::<_, String>(0),
            )
            .map_err(ResearchError::from_sqlite)?;
        let mut ids = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(ResearchError::from_sqlite)?;
        let has_more = ids.len() > limit;
        ids.truncate(limit);
        let passages = self.resolve_current_passages(ids)?;
        Ok((passages, has_more))
    }

    fn resolve_current_passages(
        &self,
        ids: Vec<String>,
    ) -> Result<Vec<CitationResolution>, ResearchError> {
        ids.into_iter()
            .map(|passage_id| {
                let citation = passages::resolve_citation(&self.connection, &passage_id)
                    .map_err(ResearchError::from_database)?;
                if citation.state != CitationState::Current {
                    return Err(content_state_error(&citation));
                }
                Ok(citation)
            })
            .collect()
    }

    fn author_passage_ids(&self, target: &str) -> Result<Vec<String>, ResearchError> {
        let revision_id = self
            .connection
            .query_row(
                "SELECT tidbit_revision_id FROM passage
                 WHERE id = ?1 AND owner_kind = 'AUTHOR'",
                params![target],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(ResearchError::from_sqlite)?
            .ok_or_else(|| {
                ResearchError::new(
                    ResearchErrorCode::ContentUnavailable,
                    "the authored passage is no longer available",
                )
            })?;
        self.author_passage_ids_for_revision(&revision_id)
    }

    fn author_passage_ids_for_revision(
        &self,
        revision_id: &str,
    ) -> Result<Vec<String>, ResearchError> {
        collect_ids(
            &self.connection,
            "SELECT passage.id
             FROM passage
             JOIN active_passage ON active_passage.passage_id = passage.id
             WHERE passage.tidbit_revision_id = ?1
               AND passage.owner_kind = 'AUTHOR'
             ORDER BY passage.ordinal",
            revision_id,
        )
    }

    fn attachment_passage_ids_for_target(
        &self,
        target: &str,
    ) -> Result<Vec<String>, ResearchError> {
        let (attachment_id, extraction_id) = self
            .connection
            .query_row(
                "SELECT attachment.id, extraction.id
                 FROM passage
                 JOIN attachment_segment AS segment
                   ON segment.id = passage.attachment_segment_id
                 JOIN attachment_extraction AS extraction
                   ON extraction.id = segment.extraction_id
                 JOIN attachment ON attachment.id = extraction.attachment_id
                 WHERE passage.id = ?1
                   AND passage.owner_kind = 'ATTACHMENT'",
                params![target],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(ResearchError::from_sqlite)?
            .ok_or_else(|| {
                ResearchError::new(
                    ResearchErrorCode::ContentUnavailable,
                    "the attachment passage is no longer available",
                )
            })?;
        self.attachment_passage_ids(&attachment_id, &extraction_id)
    }

    fn attachment_passage_ids(
        &self,
        attachment_id: &str,
        extraction_id: &str,
    ) -> Result<Vec<String>, ResearchError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT passage.id
                 FROM current_attachment_passage AS current
                 JOIN passage ON passage.id = current.passage_id
                 JOIN attachment_segment AS segment
                   ON segment.id = passage.attachment_segment_id
                 WHERE current.attachment_id = ?1
                   AND current.extraction_id = ?2
                 ORDER BY segment.ordinal, passage.ordinal",
            )
            .map_err(ResearchError::from_sqlite)?;
        let rows = statement
            .query_map(params![attachment_id, extraction_id], |row| {
                row.get::<_, String>(0)
            })
            .map_err(ResearchError::from_sqlite)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(ResearchError::from_sqlite)
    }
}

fn collect_ids(
    connection: &Connection,
    sql: &str,
    parameter: &str,
) -> Result<Vec<String>, ResearchError> {
    let mut statement = connection
        .prepare(sql)
        .map_err(ResearchError::from_sqlite)?;
    let rows = statement
        .query_map(params![parameter], |row| row.get::<_, String>(0))
        .map_err(ResearchError::from_sqlite)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(ResearchError::from_sqlite)
}

fn content_state_error(citation: &CitationResolution) -> ResearchError {
    let deleted = citation
        .tidbit
        .as_ref()
        .is_some_and(|tidbit| tidbit.deleted)
        || citation
            .attachment
            .as_ref()
            .is_some_and(|attachment| attachment.deleted);
    if deleted {
        ResearchError::new(
            ResearchErrorCode::ContentDeleted,
            "the evidence was deleted after this research handle was issued",
        )
    } else {
        ResearchError::new(
            ResearchErrorCode::StaleContent,
            "the evidence changed after this research handle was issued",
        )
    }
}

fn same_evidence(left: &CitationResolution, right: &CitationResolution) -> bool {
    if left.passage_id != right.passage_id
        || left.excerpt != right.excerpt
        || left.locator != right.locator
    {
        return false;
    }
    match (
        left.tidbit.as_ref(),
        right.tidbit.as_ref(),
        left.attachment.as_ref(),
        right.attachment.as_ref(),
    ) {
        (Some(left), Some(right), None, None) => {
            left.id == right.id && left.revision_id == right.revision_id
        }
        (None, None, Some(left), Some(right)) => {
            left.id == right.id && left.extraction_id == right.extraction_id
        }
        _ => false,
    }
}

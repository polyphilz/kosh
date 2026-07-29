pub(crate) mod builder;
#[cfg(test)]
mod tests;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{search, tidbits, DatabaseError, Result, TidbitSource};
use builder::{build_markdown_passages, MarkdownLocator, CONSTRUCTION_VERSION};

pub(super) const BACKGROUND_RECONCILE_BATCH_SIZE: u32 = 25;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CitationState {
    Current,
    Historical,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase"
)]
pub enum CitationLocator {
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
        region: serde_json::Value,
    },
    TextLines {
        start_line: u32,
        end_line: u32,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CitationTidbit {
    pub id: String,
    pub revision_id: String,
    pub revision_number: i64,
    pub title: Option<String>,
    pub display_title: String,
    pub deleted: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CitationAttachment {
    pub id: String,
    pub extraction_id: String,
    pub display_filename: String,
    pub media_type: String,
    pub deleted: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CitationResolution {
    pub passage_id: String,
    pub excerpt: String,
    pub heading_context: Vec<String>,
    pub construction_version: String,
    pub state: CitationState,
    pub locator: CitationLocator,
    pub tidbit: Option<CitationTidbit>,
    pub attachment: Option<CitationAttachment>,
    pub sources: Vec<TidbitSource>,
}

struct StoredPassage {
    id: String,
    owner_kind: String,
    tidbit_revision_id: Option<String>,
    attachment_segment_id: Option<String>,
    content: String,
    content_hash: Vec<u8>,
    locator_kind: String,
    locator_json: String,
    construction_version: String,
    heading_context_json: String,
}

pub(super) fn insert_author_passages(
    transaction: &Transaction<'_>,
    tidbit_revision_id: &str,
    markdown: &str,
    created_at_ms: i64,
) -> Result<usize> {
    let passages = build_markdown_passages(markdown);
    if passages.is_empty() {
        return Err(DatabaseError::InvalidInput(
            "authored Markdown did not produce a citation passage".into(),
        ));
    }
    for passage in &passages {
        let passage_id = deterministic_passage_id(tidbit_revision_id, passage.ordinal)?;
        let locator_json = serde_json::to_string(&passage.locator).map_err(|error| {
            DatabaseError::InvalidInput(format!("could not serialize Markdown locator: {error}"))
        })?;
        let heading_context_json =
            serde_json::to_string(&passage.heading_context).map_err(|error| {
                DatabaseError::InvalidInput(format!(
                    "could not serialize passage heading context: {error}"
                ))
            })?;
        transaction.execute(
            "INSERT INTO passage(
                id,
                tidbit_revision_id,
                attachment_segment_id,
                owner_kind,
                ordinal,
                content,
                content_hash,
                locator_kind,
                locator_json,
                created_at,
                construction_version,
                heading_context_json
             ) VALUES(
                ?1, ?2, NULL, 'AUTHOR', ?3, ?4, ?5,
                'MARKDOWN_BLOCKS', ?6, ?7, ?8, ?9
             )",
            params![
                passage_id,
                tidbit_revision_id,
                i64::from(passage.ordinal),
                &passage.content,
                passage.content_hash.as_slice(),
                locator_json,
                created_at_ms,
                CONSTRUCTION_VERSION,
                heading_context_json,
            ],
        )?;
    }
    Ok(passages.len())
}

pub(super) fn reconcile_author_passages(connection: &mut Connection) -> Result<()> {
    while reconcile_author_passage_batch(connection, BACKGROUND_RECONCILE_BATCH_SIZE)? {}
    Ok(())
}

pub(super) fn reconcile_author_passage_batch(
    connection: &mut Connection,
    limit: u32,
) -> Result<bool> {
    if limit == 0 {
        return Err(DatabaseError::InvalidInput(
            "passage reconciliation limit must be positive".into(),
        ));
    }
    let state_updated = connection.execute(
        "UPDATE index_state
         SET version = ?1, status = 'RUNNING', cursor = NULL, error = NULL
         WHERE name = 'PASSAGE_BUILD'",
        params![CONSTRUCTION_VERSION],
    )?;
    if state_updated != 1 {
        return Err(DatabaseError::Validation {
            kind: "main",
            reason: "PASSAGE_BUILD index state is missing".into(),
        });
    }
    match reconcile_author_passage_batch_transaction(connection, limit) {
        Ok(has_more) => Ok(has_more),
        Err(error) => {
            let message = error.to_string();
            let _ = connection.execute(
                "UPDATE index_state
                 SET status = 'FAILED', cursor = NULL, error = ?1
                 WHERE name = 'PASSAGE_BUILD'",
                params![message],
            );
            Err(error)
        }
    }
}

fn reconcile_author_passage_batch_transaction(
    connection: &mut Connection,
    limit: u32,
) -> Result<bool> {
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let missing = {
        let mut statement = transaction.prepare(
            "SELECT revision.id, revision.body_markdown, revision.created_at
             FROM tidbit_revision AS revision
             WHERE NOT EXISTS (
                 SELECT 1
                 FROM passage
                 WHERE passage.tidbit_revision_id = revision.id
                   AND passage.owner_kind = 'AUTHOR'
                   AND passage.construction_version = ?1
             )
             ORDER BY revision.created_at, revision.id
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![CONSTRUCTION_VERSION, limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (revision_id, body_markdown, created_at_ms) in &missing {
        insert_author_passages(&transaction, revision_id, body_markdown, *created_at_ms)?;
        let current_tidbit_id = transaction
            .query_row(
                "SELECT id
                 FROM tidbit
                 WHERE current_revision_id = ?1
                   AND deleted_at IS NULL",
                params![revision_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(tidbit_id) = current_tidbit_id {
            replace_active_author_passages(&transaction, &tidbit_id, revision_id)?;
        }
    }

    let has_more = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM tidbit_revision AS revision
            WHERE NOT EXISTS (
                SELECT 1
                FROM passage
                WHERE passage.tidbit_revision_id = revision.id
                  AND passage.owner_kind = 'AUTHOR'
                  AND passage.construction_version = ?1
            )
         )",
        params![CONSTRUCTION_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    let active_set_differs = !has_more
        && transaction.query_row(
            "SELECT
            EXISTS(
                SELECT 1
                FROM tidbit
                JOIN passage
                  ON passage.tidbit_revision_id = tidbit.current_revision_id
                 AND passage.owner_kind = 'AUTHOR'
                 AND passage.construction_version = ?1
                WHERE tidbit.deleted_at IS NULL
                  AND NOT EXISTS(
                      SELECT 1
                      FROM active_passage
                      WHERE active_passage.passage_id = passage.id
                        AND active_passage.tidbit_id = tidbit.id
                  )
            )
            OR EXISTS(
                SELECT 1
                FROM active_passage
                WHERE NOT EXISTS(
                    SELECT 1
                    FROM tidbit
                    JOIN passage
                      ON passage.tidbit_revision_id = tidbit.current_revision_id
                     AND passage.owner_kind = 'AUTHOR'
                     AND passage.construction_version = ?1
                    WHERE tidbit.deleted_at IS NULL
                      AND tidbit.id = active_passage.tidbit_id
                      AND passage.id = active_passage.passage_id
                )
            )",
            params![CONSTRUCTION_VERSION],
            |row| row.get::<_, bool>(0),
        )?;
    let search_index_needs_rebuild = !has_more
        && transaction.query_row(
            "SELECT version != ?1 OR status != 'IDLE'
             FROM index_state
             WHERE name = 'PASSAGE_FTS'",
            params![search::FTS_VERSION],
            |row| row.get::<_, bool>(0),
        )?;
    if active_set_differs {
        transaction.execute("DELETE FROM active_passage", [])?;
        transaction.execute(
            "INSERT INTO active_passage(passage_id, tidbit_id)
             SELECT passage.id, tidbit.id
             FROM tidbit
             JOIN passage
               ON passage.tidbit_revision_id = tidbit.current_revision_id
              AND passage.owner_kind = 'AUTHOR'
              AND passage.construction_version = ?1
             WHERE tidbit.deleted_at IS NULL
             ORDER BY tidbit.id, passage.ordinal",
            params![CONSTRUCTION_VERSION],
        )?;
    }
    if active_set_differs || search_index_needs_rebuild {
        search::rebuild_documents(&transaction)?;
    }
    let status = if has_more { "DIRTY" } else { "IDLE" };
    let cursor = if has_more {
        missing
            .last()
            .map(|(revision_id, _, _)| revision_id.as_str())
    } else {
        None
    };
    let batch_updated_at = missing
        .iter()
        .map(|(_, _, created_at_ms)| *created_at_ms)
        .max()
        .unwrap_or_default();
    transaction.execute(
        "UPDATE index_state
         SET status = ?1,
             cursor = ?2,
             updated_at = max(
                 updated_at,
                 ?3
             ),
             error = NULL
         WHERE name = 'PASSAGE_BUILD'",
        params![status, cursor, batch_updated_at],
    )?;
    transaction.commit()?;
    Ok(has_more)
}

pub(super) fn replace_active_author_passages(
    transaction: &Transaction<'_>,
    tidbit_id: &str,
    tidbit_revision_id: &str,
) -> Result<()> {
    transaction.execute(
        "DELETE FROM active_passage WHERE tidbit_id = ?1",
        params![tidbit_id],
    )?;
    let inserted = transaction.execute(
        "INSERT INTO active_passage(passage_id, tidbit_id)
         SELECT id, ?1
         FROM passage
         WHERE owner_kind = 'AUTHOR'
           AND tidbit_revision_id = ?2
           AND construction_version = ?3
         ORDER BY ordinal",
        params![tidbit_id, tidbit_revision_id, CONSTRUCTION_VERSION],
    )?;
    if inserted == 0 {
        return Err(DatabaseError::InvalidInput(
            "current revision has no authored passages".into(),
        ));
    }
    search::replace_tidbit_documents(transaction, tidbit_id)?;
    Ok(())
}

pub(super) fn activate_author_passages_on_restore(
    transaction: &Transaction<'_>,
    tidbit_id: &str,
    tidbit_revision_id: &str,
) -> Result<bool> {
    let has_passages = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM passage
            WHERE owner_kind = 'AUTHOR'
              AND tidbit_revision_id = ?1
              AND construction_version = ?2
         )",
        params![tidbit_revision_id, CONSTRUCTION_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    if !has_passages {
        let (body_markdown, created_at_ms) = transaction.query_row(
            "SELECT body_markdown, created_at
             FROM tidbit_revision
             WHERE id = ?1",
            params![tidbit_revision_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )?;
        if body_markdown.trim().is_empty() {
            deactivate_tidbit(transaction, tidbit_id)?;
            return Ok(false);
        }
        insert_author_passages(
            transaction,
            tidbit_revision_id,
            &body_markdown,
            created_at_ms,
        )?;
    }
    replace_active_author_passages(transaction, tidbit_id, tidbit_revision_id)?;
    Ok(true)
}

pub(super) fn deactivate_tidbit(transaction: &Transaction<'_>, tidbit_id: &str) -> Result<()> {
    transaction.execute(
        "DELETE FROM active_passage WHERE tidbit_id = ?1",
        params![tidbit_id],
    )?;
    search::replace_tidbit_documents(transaction, tidbit_id)?;
    Ok(())
}

pub(super) fn resolve_citation(
    connection: &Connection,
    passage_id: &str,
) -> Result<CitationResolution> {
    validate_uuid_v7(passage_id, "passageId")?;
    let passage = connection
        .query_row(
            "SELECT
                id,
                owner_kind,
                tidbit_revision_id,
                attachment_segment_id,
                content,
                content_hash,
                locator_kind,
                locator_json,
                construction_version,
                heading_context_json
             FROM passage
             WHERE id = ?1",
            params![passage_id],
            |row| {
                Ok(StoredPassage {
                    id: row.get(0)?,
                    owner_kind: row.get(1)?,
                    tidbit_revision_id: row.get(2)?,
                    attachment_segment_id: row.get(3)?,
                    content: row.get(4)?,
                    content_hash: row.get(5)?,
                    locator_kind: row.get(6)?,
                    locator_json: row.get(7)?,
                    construction_version: row.get(8)?,
                    heading_context_json: row.get(9)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| DatabaseError::NotFound {
            entity: "passage",
            id: passage_id.to_owned(),
        })?;
    let heading_context = serde_json::from_str::<Vec<String>>(&passage.heading_context_json)
        .map_err(|error| DatabaseError::Validation {
            kind: "main",
            reason: format!(
                "passage {} has invalid heading context: {error}",
                passage.id
            ),
        })?;

    match passage.owner_kind.as_str() {
        "AUTHOR" => resolve_author_citation(connection, passage, heading_context),
        "ATTACHMENT" => resolve_attachment_citation(connection, passage, heading_context),
        owner => Err(DatabaseError::Validation {
            kind: "main",
            reason: format!("passage {passage_id} has unknown owner kind {owner}"),
        }),
    }
}

fn resolve_author_citation(
    connection: &Connection,
    passage: StoredPassage,
    heading_context: Vec<String>,
) -> Result<CitationResolution> {
    if passage.locator_kind != "MARKDOWN_BLOCKS" {
        return invalid_passage(
            &passage.id,
            format!("authored passage has locator kind {}", passage.locator_kind),
        );
    }
    let revision_id = passage
        .tidbit_revision_id
        .as_deref()
        .ok_or_else(|| invalid_passage_error(&passage.id, "authored passage has no revision"))?;
    let locator: MarkdownLocator =
        serde_json::from_str(&passage.locator_json).map_err(|error| DatabaseError::Validation {
            kind: "main",
            reason: format!(
                "passage {} has invalid Markdown locator: {error}",
                passage.id
            ),
        })?;
    let (tidbit_id, revision_number, title, body_markdown, deleted, is_active) = connection
        .query_row(
            "SELECT
                revision.tidbit_id,
                revision.revision_number,
                revision.title,
                revision.body_markdown,
                tidbit.deleted_at IS NOT NULL,
                EXISTS(
                    SELECT 1
                    FROM active_passage
                    WHERE active_passage.passage_id = ?1
                )
             FROM tidbit_revision AS revision
             JOIN tidbit ON tidbit.id = revision.tidbit_id
             WHERE revision.id = ?2",
            params![&passage.id, revision_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, bool>(4)?,
                    row.get::<_, bool>(5)?,
                ))
            },
        )?;
    let sources = tidbits::load_sources(connection, revision_id)?;
    Ok(CitationResolution {
        passage_id: passage.id,
        excerpt: passage.content,
        heading_context,
        construction_version: passage.construction_version,
        state: if is_active {
            CitationState::Current
        } else {
            CitationState::Historical
        },
        locator: CitationLocator::MarkdownBlocks {
            start_block: locator.start,
            end_block: locator.end,
            source_start_byte: locator.source_start_byte,
            source_end_byte: locator.source_end_byte,
            start_char: locator.start_char,
            end_char: locator.end_char,
            start_line: locator.start_line,
            end_line: locator.end_line,
        },
        tidbit: Some(CitationTidbit {
            id: tidbit_id,
            revision_id: revision_id.to_owned(),
            revision_number,
            display_title: tidbits::derive_display_title(title.as_deref(), &body_markdown),
            title,
            deleted,
        }),
        attachment: None,
        sources,
    })
}

fn resolve_attachment_citation(
    connection: &Connection,
    passage: StoredPassage,
    heading_context: Vec<String>,
) -> Result<CitationResolution> {
    let segment_id = passage
        .attachment_segment_id
        .as_deref()
        .ok_or_else(|| invalid_passage_error(&passage.id, "attachment passage has no segment"))?;
    let (
        attachment_id,
        extraction_id,
        display_filename,
        media_type,
        deleted,
        evidence_matches,
        current,
    ) = connection.query_row(
        "SELECT
                attachment.id,
                extraction.id,
                attachment.display_filename,
                attachment.media_type,
                attachment.deleted_at IS NOT NULL,
                segment.content = ?2 AND segment.content_hash = ?3,
                EXISTS(
                    SELECT 1
                    FROM current_attachment_passage AS current
                    WHERE current.passage_id = ?4
                )
             FROM attachment_segment AS segment
             JOIN attachment_extraction AS extraction
               ON extraction.id = segment.extraction_id
             JOIN attachment ON attachment.id = extraction.attachment_id
             WHERE segment.id = ?1",
        params![
            segment_id,
            passage.content,
            passage.content_hash,
            passage.id
        ],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, bool>(5)?,
                row.get::<_, bool>(6)?,
            ))
        },
    )?;
    if !evidence_matches {
        return Err(invalid_passage_error(
            &passage.id,
            "attachment passage content does not match its immutable segment",
        ));
    }
    let locator = attachment_locator(&passage)?;
    let sources = load_attachment_sources(connection, &passage.id)?;
    Ok(CitationResolution {
        passage_id: passage.id,
        excerpt: passage.content,
        heading_context,
        construction_version: passage.construction_version,
        state: if current {
            CitationState::Current
        } else {
            CitationState::Historical
        },
        locator,
        tidbit: None,
        attachment: Some(CitationAttachment {
            id: attachment_id,
            extraction_id,
            display_filename,
            media_type,
            deleted,
        }),
        sources,
    })
}

fn load_attachment_sources(connection: &Connection, passage_id: &str) -> Result<Vec<TidbitSource>> {
    let mut statement = connection.prepare(
        "SELECT source.id, source.label, source.normalized_url
         FROM attachment_passage_revision AS provenance
         JOIN tidbit_revision_source AS source_membership
           ON source_membership.tidbit_revision_id = provenance.tidbit_revision_id
         JOIN source ON source.id = source_membership.source_id
         WHERE provenance.passage_id = ?1
         ORDER BY source_membership.sort_order",
    )?;
    let sources = statement.query_map(params![passage_id], |row| {
        Ok(TidbitSource {
            id: row.get(0)?,
            label: row.get(1)?,
            url: row.get(2)?,
        })
    })?;
    Ok(sources.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn attachment_locator(passage: &StoredPassage) -> Result<CitationLocator> {
    match passage.locator_kind.as_str() {
        "PDF_PAGE" => {
            #[derive(Deserialize)]
            struct Locator {
                page: u32,
            }
            let locator: Locator = parse_locator(passage)?;
            Ok(CitationLocator::PdfPage { page: locator.page })
        }
        "OCR_REGION" => {
            #[derive(Deserialize)]
            struct Locator {
                page: Option<u32>,
                region: serde_json::Value,
            }
            let locator: Locator = parse_locator(passage)?;
            Ok(CitationLocator::OcrRegion {
                page: locator.page,
                region: locator.region,
            })
        }
        "TEXT_LINES" => {
            #[derive(Deserialize)]
            struct Locator {
                start: u32,
                end: u32,
            }
            let locator: Locator = parse_locator(passage)?;
            Ok(CitationLocator::TextLines {
                start_line: locator.start,
                end_line: locator.end,
            })
        }
        kind => invalid_passage(
            &passage.id,
            format!("attachment passage has locator kind {kind}"),
        ),
    }
}

fn parse_locator<T: for<'de> Deserialize<'de>>(passage: &StoredPassage) -> Result<T> {
    serde_json::from_str(&passage.locator_json).map_err(|error| DatabaseError::Validation {
        kind: "main",
        reason: format!("passage {} has invalid locator: {error}", passage.id),
    })
}

fn deterministic_passage_id(revision_id: &str, ordinal: u32) -> Result<String> {
    let revision = Uuid::parse_str(revision_id)
        .map_err(|_| DatabaseError::InvalidInput("revisionId must be a UUIDv7".into()))?;
    if revision.get_version_num() != 7 || revision.hyphenated().to_string() != revision_id {
        return Err(DatabaseError::InvalidInput(
            "revisionId must be a lowercase UUIDv7".into(),
        ));
    }
    let mut digest = Sha256::new();
    digest.update(revision.as_bytes());
    digest.update(CONSTRUCTION_VERSION.as_bytes());
    digest.update(ordinal.to_be_bytes());
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes[..6].copy_from_slice(&revision.as_bytes()[..6]);
    bytes[6..].copy_from_slice(&digest[..10]);
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(Uuid::from_bytes(bytes).hyphenated().to_string())
}

fn validate_uuid_v7(value: &str, field: &str) -> Result<()> {
    let parsed = Uuid::parse_str(value)
        .map_err(|_| DatabaseError::InvalidInput(format!("{field} must be a UUIDv7")))?;
    if parsed.get_version_num() != 7 || value != parsed.hyphenated().to_string() {
        return Err(DatabaseError::InvalidInput(format!(
            "{field} must be a lowercase UUIDv7"
        )));
    }
    Ok(())
}

fn invalid_passage<T>(passage_id: &str, reason: String) -> Result<T> {
    Err(DatabaseError::Validation {
        kind: "main",
        reason: format!("passage {passage_id} {reason}"),
    })
}

fn invalid_passage_error(passage_id: &str, reason: &str) -> DatabaseError {
    DatabaseError::Validation {
        kind: "main",
        reason: format!("passage {passage_id} {reason}"),
    }
}

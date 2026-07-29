use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{media, DatabaseError, Result, SourceDraft};

const CAPTURE_CONTEXT: &str = "capture";
const QUICK_ADD_CONTEXT: &str = "quick-add";
const EDIT_CONTEXT_PREFIX: &str = "edit:";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveDraftInput {
    pub context_key: String,
    pub tidbit_id: Option<String>,
    pub base_revision_id: Option<String>,
    pub title: Option<String>,
    pub body_markdown: String,
    #[serde(default)]
    pub sources: Vec<SourceDraft>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClearDraftInput {
    pub context_key: String,
    pub expected_updated_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub id: String,
    pub context_key: String,
    pub tidbit_id: Option<String>,
    pub base_revision_id: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub title: Option<String>,
    pub body_markdown: String,
    pub sources: Vec<SourceDraft>,
}

pub(crate) struct SaveDraftWrite {
    pub input: SaveDraftInput,
    pub now_ms: i64,
    pub draft_id: String,
    pub media_limits: media::MediaLimits,
}

pub(super) fn save_draft(connection: &mut Connection, write: SaveDraftWrite) -> Result<Draft> {
    validate_timestamp(write.now_ms)?;
    validate_uuid_v7(&write.draft_id, "draftId")?;
    validate_context(&write.input)?;
    let media_limits = write.media_limits.validate()?;

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    validate_edit_base(&transaction, &write.input)?;
    let existing = transaction
        .query_row(
            "SELECT d.id, d.created_at, d.updated_at
             FROM draft d
             JOIN draft_context dc ON dc.draft_id = d.id
             WHERE dc.context_key = ?1",
            params![&write.input.context_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    let (draft_id, updated_at_ms) =
        if let Some((draft_id, _created_at, previous_updated)) = existing {
            let updated_at_ms = next_timestamp(previous_updated, write.now_ms)?;
            transaction.execute(
                "UPDATE draft
             SET updated_at = ?1, title = ?2, body_markdown = ?3
             WHERE id = ?4",
                params![
                    updated_at_ms,
                    normalize_draft_title(write.input.title.as_deref()),
                    &write.input.body_markdown,
                    &draft_id
                ],
            )?;
            transaction.execute(
                "UPDATE draft_context
             SET tidbit_id = ?1, base_revision_id = ?2
             WHERE draft_id = ?3",
                params![
                    &write.input.tidbit_id,
                    &write.input.base_revision_id,
                    &draft_id
                ],
            )?;
            (draft_id, updated_at_ms)
        } else {
            transaction.execute(
                "INSERT INTO draft(id, created_at, updated_at, title, body_markdown)
             VALUES(?1, ?2, ?2, ?3, ?4)",
                params![
                    &write.draft_id,
                    write.now_ms,
                    normalize_draft_title(write.input.title.as_deref()),
                    &write.input.body_markdown
                ],
            )?;
            transaction.execute(
                "INSERT INTO draft_context(
                draft_id, context_key, tidbit_id, base_revision_id
             ) VALUES(?1, ?2, ?3, ?4)",
                params![
                    &write.draft_id,
                    &write.input.context_key,
                    &write.input.tidbit_id,
                    &write.input.base_revision_id
                ],
            )?;
            (write.draft_id, write.now_ms)
        };

    transaction.execute(
        "DELETE FROM draft_source WHERE draft_id = ?1",
        params![&draft_id],
    )?;
    for (position, source) in write.input.sources.iter().enumerate() {
        let position = i64::try_from(position)
            .map_err(|_| DatabaseError::InvalidInput("too many draft sources".into()))?;
        transaction.execute(
            "INSERT INTO draft_source(draft_id, position, label, url)
             VALUES(?1, ?2, ?3, ?4)",
            params![&draft_id, position, &source.label, &source.url],
        )?;
    }
    media::sync_draft_media_leases(
        &transaction,
        &draft_id,
        &write.input.body_markdown,
        updated_at_ms,
        media_limits.max_attachments_per_draft,
        media_limits.draft_lease_duration_ms,
    )?;
    transaction.commit()?;

    load_draft(connection, &write.input.context_key)?.ok_or_else(|| {
        DatabaseError::InvalidInput(format!(
            "draft {} disappeared after save at {updated_at_ms}",
            write.input.context_key
        ))
    })
}

pub(super) fn load_draft(connection: &Connection, context_key: &str) -> Result<Option<Draft>> {
    validate_context_key(context_key)?;
    let row = connection
        .query_row(
            "SELECT
                d.id,
                dc.context_key,
                dc.tidbit_id,
                dc.base_revision_id,
                d.created_at,
                d.updated_at,
                d.title,
                d.body_markdown
             FROM draft d
             JOIN draft_context dc ON dc.draft_id = d.id
             WHERE dc.context_key = ?1",
            params![context_key],
            |row| {
                Ok(Draft {
                    id: row.get(0)?,
                    context_key: row.get(1)?,
                    tidbit_id: row.get(2)?,
                    base_revision_id: row.get(3)?,
                    created_at_ms: row.get(4)?,
                    updated_at_ms: row.get(5)?,
                    title: row.get(6)?,
                    body_markdown: row.get(7)?,
                    sources: Vec::new(),
                })
            },
        )
        .optional()?;
    row.map(|mut draft| {
        let mut statement = connection.prepare(
            "SELECT label, url
             FROM draft_source
             WHERE draft_id = ?1
             ORDER BY position",
        )?;
        draft.sources = statement
            .query_map(params![&draft.id], |row| {
                Ok(SourceDraft {
                    label: row.get(0)?,
                    url: row.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(draft)
    })
    .transpose()
}

pub(super) fn clear_draft(
    connection: &mut Connection,
    input: ClearDraftInput,
    now_ms: i64,
) -> Result<bool> {
    validate_context_key(&input.context_key)?;
    validate_timestamp(input.expected_updated_at_ms)?;
    validate_timestamp(now_ms)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let draft_id = transaction
        .query_row(
            "SELECT d.id
             FROM draft d
             JOIN draft_context dc ON dc.draft_id = d.id
             WHERE dc.context_key = ?1
               AND d.updated_at = ?2",
            params![&input.context_key, input.expected_updated_at_ms],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(draft_id) = draft_id else {
        transaction.rollback()?;
        return Ok(false);
    };
    media::abandon_draft_media_leases(&transaction, &input.context_key, now_ms)?;
    let deleted = transaction.execute("DELETE FROM draft WHERE id = ?1", params![draft_id])?;
    transaction.commit()?;
    Ok(deleted == 1)
}

fn validate_context(input: &SaveDraftInput) -> Result<()> {
    validate_context_key(&input.context_key)?;
    match (
        input.context_key.as_str(),
        input.tidbit_id.as_deref(),
        input.base_revision_id.as_deref(),
    ) {
        (CAPTURE_CONTEXT | QUICK_ADD_CONTEXT, None, None) => Ok(()),
        (context, Some(tidbit_id), Some(base_revision_id))
            if context == format!("{EDIT_CONTEXT_PREFIX}{tidbit_id}") =>
        {
            validate_uuid_v7(tidbit_id, "tidbitId")?;
            validate_uuid_v7(base_revision_id, "baseRevisionId")
        }
        _ => Err(DatabaseError::InvalidInput(
            "draft context must be capture, quick-add, or edit:<tidbitId> with matching edit metadata"
                .into(),
        )),
    }
}

fn validate_context_key(context_key: &str) -> Result<()> {
    if matches!(context_key, CAPTURE_CONTEXT | QUICK_ADD_CONTEXT) {
        return Ok(());
    }
    let Some(tidbit_id) = context_key.strip_prefix(EDIT_CONTEXT_PREFIX) else {
        return Err(DatabaseError::InvalidInput(
            "draft context must be capture or edit:<tidbitId>".into(),
        ));
    };
    validate_uuid_v7(tidbit_id, "draft context tidbit ID")
}

fn validate_edit_base(transaction: &Transaction<'_>, input: &SaveDraftInput) -> Result<()> {
    let (Some(tidbit_id), Some(base_revision_id)) = (
        input.tidbit_id.as_deref(),
        input.base_revision_id.as_deref(),
    ) else {
        return Ok(());
    };
    let belongs = transaction
        .query_row(
            "SELECT 1
             FROM tidbit_revision
             WHERE id = ?1 AND tidbit_id = ?2",
            params![base_revision_id, tidbit_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if belongs {
        Ok(())
    } else {
        Err(DatabaseError::InvalidInput(
            "draft base revision must belong to its tidbit".into(),
        ))
    }
}

fn normalize_draft_title(title: Option<&str>) -> Option<&str> {
    title.filter(|value| !value.is_empty())
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

fn validate_timestamp(value: i64) -> Result<()> {
    if (0..=9_007_199_254_740_991).contains(&value) {
        Ok(())
    } else {
        Err(DatabaseError::InvalidInput(
            "draft timestamp must be a non-negative JavaScript-safe integer".into(),
        ))
    }
}

fn next_timestamp(previous: i64, now: i64) -> Result<i64> {
    previous
        .checked_add(1)
        .map(|next| next.max(now))
        .filter(|value| *value <= 9_007_199_254_740_991)
        .ok_or_else(|| DatabaseError::InvalidInput("draft timestamp overflow".into()))
}

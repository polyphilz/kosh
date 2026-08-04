use pulldown_cmark::{Event, Options, Parser};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};

use super::{
    document, media, passages,
    tidbits::{self, PreparedRevision},
    DatabaseError, Result, SourceDraft, Tidbit,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveWorkingCopyInput {
    pub note_id: String,
    pub base_revision_id: Option<String>,
    pub edit_generation: i64,
    pub document_json: String,
    pub body_markdown: String,
    #[serde(default)]
    pub sources: Vec<SourceDraft>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckpointWorkingCopyInput {
    pub note_id: String,
    pub expected_edit_generation: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscardWorkingCopyInput {
    pub note_id: String,
    pub expected_edit_generation: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingCopy {
    pub id: String,
    pub note_id: String,
    pub base_revision_id: Option<String>,
    pub edit_generation: i64,
    pub media_reservation: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub document_json: String,
    pub body_markdown: String,
    pub sources: Vec<SourceDraft>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorkingCopySaveStatus {
    Saved,
    Cleared,
    Stale,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingCopySaveResult {
    pub status: WorkingCopySaveStatus,
    pub accepted_edit_generation: i64,
    pub working_copy: Option<WorkingCopy>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorkingCopyCheckpointStatus {
    Checkpointed,
    Stale,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingCopyCheckpointResult {
    pub status: WorkingCopyCheckpointStatus,
    pub consumed_edit_generation: Option<i64>,
    pub note: Option<Tidbit>,
    pub working_copy: Option<WorkingCopy>,
}

pub(crate) struct SaveWorkingCopyWrite {
    pub input: SaveWorkingCopyInput,
    pub now_ms: i64,
    pub media_limits: media::MediaLimits,
    pub allow_empty_ephemeral: bool,
}

pub(crate) struct CheckpointWorkingCopyWrite {
    pub input: CheckpointWorkingCopyInput,
    pub now_ms: i64,
    pub revision_id: String,
    pub source_ids: Vec<String>,
}

pub(super) fn save(
    connection: &mut Connection,
    write: SaveWorkingCopyWrite,
) -> Result<WorkingCopySaveResult> {
    validate_save_input(&write.input)?;
    tidbits::validate_timestamp(write.now_ms, "nowMs")?;
    let limits = write.media_limits.validate()?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    validate_base_revision(&transaction, &write.input)?;
    let existing = load_in_transaction(&transaction, &write.input.note_id)?;

    if let Some(existing) = existing.as_ref() {
        if write.input.edit_generation < existing.edit_generation {
            transaction.rollback()?;
            return Ok(save_result(
                WorkingCopySaveStatus::Stale,
                Some(existing.clone()),
            ));
        }
        if write.input.edit_generation == existing.edit_generation {
            if same_authored_state(existing, &write.input) {
                if existing.media_reservation && !write.allow_empty_ephemeral {
                    transaction.execute(
                        "UPDATE draft SET media_reservation = 0 WHERE id = ?1",
                        params![&existing.id],
                    )?;
                    let working_copy = load_in_transaction(&transaction, &write.input.note_id)?
                        .ok_or_else(|| {
                            DatabaseError::InvalidInput(
                                "working copy disappeared after reservation classification".into(),
                            )
                        })?;
                    transaction.commit()?;
                    return Ok(save_result(
                        WorkingCopySaveStatus::Saved,
                        Some(working_copy),
                    ));
                }
                transaction.rollback()?;
                return Ok(save_result(
                    WorkingCopySaveStatus::Saved,
                    Some(existing.clone()),
                ));
            }
            return Err(DatabaseError::InvalidInput(
                "an edit generation cannot be reused for different working-copy content".into(),
            ));
        }
    }

    if !write.allow_empty_ephemeral
        && write.input.base_revision_id.is_none()
        && !has_meaningful_authored_content(&write.input.body_markdown)
    {
        if existing.is_some() {
            media::abandon_draft_media_leases(&transaction, &write.input.note_id, write.now_ms)?;
            transaction.execute(
                "DELETE FROM draft WHERE id = ?1",
                params![&write.input.note_id],
            )?;
        }
        transaction.commit()?;
        return Ok(WorkingCopySaveResult {
            status: WorkingCopySaveStatus::Cleared,
            accepted_edit_generation: write.input.edit_generation,
            working_copy: None,
        });
    }

    let draft_id = if let Some(existing) = existing {
        let updated_at_ms = tidbits::next_timestamp(existing.updated_at_ms, write.now_ms)?;
        transaction.execute(
            "UPDATE draft
             SET updated_at = ?1,
                 document_json = ?2,
                 body_markdown = ?3,
                 edit_generation = ?4,
                 media_reservation = ?5
             WHERE id = ?6",
            params![
                updated_at_ms,
                &write.input.document_json,
                &write.input.body_markdown,
                write.input.edit_generation,
                write.allow_empty_ephemeral,
                &existing.id,
            ],
        )?;
        existing.id
    } else {
        transaction.execute(
            "INSERT INTO draft(
                id,
                base_revision_id,
                edit_generation,
                media_reservation,
                created_at,
                updated_at,
                document_json,
                body_markdown
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7)",
            params![
                &write.input.note_id,
                &write.input.base_revision_id,
                write.input.edit_generation,
                write.allow_empty_ephemeral,
                write.now_ms,
                &write.input.document_json,
                &write.input.body_markdown,
            ],
        )?;
        write.input.note_id.clone()
    };

    transaction.execute(
        "DELETE FROM draft_source WHERE draft_id = ?1",
        params![&draft_id],
    )?;
    for (position, source) in write.input.sources.iter().enumerate() {
        transaction.execute(
            "INSERT INTO draft_source(draft_id, position, label, url)
             VALUES(?1, ?2, ?3, ?4)",
            params![
                &draft_id,
                i64::try_from(position).map_err(|_| DatabaseError::InvalidInput(
                    "too many working-copy sources".into()
                ))?,
                &source.label,
                &source.url,
            ],
        )?;
    }
    let saved_at_ms = transaction.query_row(
        "SELECT updated_at FROM draft WHERE id = ?1",
        params![&draft_id],
        |row| row.get::<_, i64>(0),
    )?;
    media::sync_draft_media_leases(
        &transaction,
        &draft_id,
        &write.input.document_json,
        &write.input.body_markdown,
        saved_at_ms,
        limits.max_attachments_per_draft,
        limits.draft_lease_duration_ms,
    )?;
    let working_copy = load_in_transaction(&transaction, &write.input.note_id)?
        .ok_or_else(|| DatabaseError::InvalidInput("working copy disappeared after save".into()))?;
    transaction.commit()?;
    Ok(save_result(
        WorkingCopySaveStatus::Saved,
        Some(working_copy),
    ))
}

pub(super) fn discard(
    connection: &mut Connection,
    input: DiscardWorkingCopyInput,
    now_ms: i64,
) -> Result<bool> {
    tidbits::validate_uuid_v7(&input.note_id, "noteId")?;
    validate_generation(input.expected_edit_generation, "expectedEditGeneration")?;
    tidbits::validate_timestamp(now_ms, "nowMs")?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(working_copy) = load_in_transaction(&transaction, &input.note_id)? else {
        transaction.rollback()?;
        return Ok(false);
    };
    if working_copy.edit_generation != input.expected_edit_generation {
        transaction.rollback()?;
        return Ok(false);
    }
    media::abandon_draft_media_leases(&transaction, &input.note_id, now_ms)?;
    let deleted =
        transaction.execute("DELETE FROM draft WHERE id = ?1", params![&working_copy.id])?;
    transaction.commit()?;
    Ok(deleted == 1)
}

pub(super) fn load(connection: &Connection, note_id: &str) -> Result<Option<WorkingCopy>> {
    tidbits::validate_uuid_v7(note_id, "noteId")?;
    load_from_connection(connection, note_id)
}

pub(super) fn list(connection: &Connection) -> Result<Vec<WorkingCopy>> {
    let note_ids = {
        let mut statement =
            connection.prepare("SELECT id FROM draft ORDER BY updated_at DESC, id")?;
        let note_ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        note_ids
    };
    note_ids
        .iter()
        .map(|note_id| {
            load_from_connection(connection, note_id)?.ok_or_else(|| {
                DatabaseError::InvalidInput(format!(
                    "working copy {note_id} disappeared while listing"
                ))
            })
        })
        .collect()
}

pub(super) fn checkpoint(
    connection: &mut Connection,
    write: CheckpointWorkingCopyWrite,
) -> Result<WorkingCopyCheckpointResult> {
    tidbits::validate_uuid_v7(&write.input.note_id, "noteId")?;
    validate_generation(
        write.input.expected_edit_generation,
        "expectedEditGeneration",
    )?;
    tidbits::validate_timestamp(write.now_ms, "nowMs")?;
    tidbits::validate_uuid_v7(&write.revision_id, "revisionId")?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(working_copy) = load_in_transaction(&transaction, &write.input.note_id)? else {
        return Err(DatabaseError::NotFound {
            entity: "working copy",
            id: write.input.note_id,
        });
    };
    if working_copy.edit_generation != write.input.expected_edit_generation {
        transaction.rollback()?;
        return Ok(WorkingCopyCheckpointResult {
            status: WorkingCopyCheckpointStatus::Stale,
            consumed_edit_generation: None,
            note: None,
            working_copy: Some(working_copy),
        });
    }

    let allow_empty_body = working_copy.base_revision_id.is_some();
    if !allow_empty_body && !has_meaningful_authored_content(&working_copy.body_markdown) {
        return Err(DatabaseError::InvalidInput(
            "an ephemeral note must contain authored text or media before checkpoint".into(),
        ));
    }
    let prepared = tidbits::prepare_revision_with_empty(
        working_copy.document_json.clone(),
        working_copy.body_markdown.clone(),
        working_copy.sources.clone(),
        allow_empty_body,
    )?;
    tidbits::validate_source_ids(&write.source_ids, prepared.sources.len())?;
    if let Some(base_revision_id) = working_copy.base_revision_id.as_deref() {
        checkpoint_existing(
            &transaction,
            &working_copy,
            base_revision_id,
            &write,
            &prepared,
        )?;
    } else {
        checkpoint_new(&transaction, &working_copy, &write, &prepared)?;
    }
    transaction.execute("DELETE FROM draft WHERE id = ?1", params![&working_copy.id])?;
    let note = tidbits::load_tidbit(&transaction, &working_copy.note_id)?;
    transaction.commit()?;
    Ok(WorkingCopyCheckpointResult {
        status: WorkingCopyCheckpointStatus::Checkpointed,
        consumed_edit_generation: Some(working_copy.edit_generation),
        note: Some(note),
        working_copy: None,
    })
}

fn checkpoint_new(
    transaction: &Transaction<'_>,
    working_copy: &WorkingCopy,
    write: &CheckpointWorkingCopyWrite,
    prepared: &PreparedRevision,
) -> Result<()> {
    let already_exists = transaction
        .query_row(
            "SELECT 1 FROM tidbit WHERE id = ?1",
            params![&working_copy.note_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if already_exists {
        return Err(DatabaseError::InvalidInput(
            "ephemeral working-copy identity already belongs to a note".into(),
        ));
    }
    transaction.execute(
        "INSERT INTO tidbit(id, created_at, updated_at, current_revision_id)
         VALUES(?1, ?2, ?2, ?3)",
        params![&working_copy.note_id, write.now_ms, &write.revision_id],
    )?;
    tidbits::insert_revision(
        transaction,
        &write.revision_id,
        &working_copy.note_id,
        1,
        write.now_ms,
        prepared,
        &write.source_ids,
    )?;
    media::link_revision_attachments(
        transaction,
        &write.revision_id,
        None,
        &working_copy.id,
        &prepared.attachments,
        &prepared.body_markdown,
        write.now_ms,
    )?;
    passages::insert_author_passages(
        transaction,
        &write.revision_id,
        &prepared.body_markdown,
        write.now_ms,
    )?;
    passages::replace_active_author_passages(transaction, &working_copy.note_id, &write.revision_id)
}

fn checkpoint_existing(
    transaction: &Transaction<'_>,
    working_copy: &WorkingCopy,
    base_revision_id: &str,
    write: &CheckpointWorkingCopyWrite,
    prepared: &PreparedRevision,
) -> Result<()> {
    let current = tidbits::load_current_revision(transaction, &working_copy.note_id)?;
    if current.deleted_at_ms.is_some() {
        return Err(DatabaseError::TidbitDeleted {
            id: working_copy.note_id.clone(),
        });
    }
    if current.revision_id != base_revision_id {
        return Err(DatabaseError::StaleTidbit {
            id: working_copy.note_id.clone(),
            expected_revision_id: base_revision_id.to_owned(),
            actual_revision_id: current.revision_id,
        });
    }
    let revision_number = current
        .revision_number
        .checked_add(1)
        .ok_or_else(|| DatabaseError::InvalidInput("revision number overflow".into()))?;
    let updated_at_ms = tidbits::next_timestamp(current.updated_at_ms, write.now_ms)?;
    tidbits::insert_revision(
        transaction,
        &write.revision_id,
        &working_copy.note_id,
        revision_number,
        updated_at_ms,
        prepared,
        &write.source_ids,
    )?;
    media::link_revision_attachments(
        transaction,
        &write.revision_id,
        Some(base_revision_id),
        &working_copy.id,
        &prepared.attachments,
        &prepared.body_markdown,
        updated_at_ms,
    )?;
    passages::insert_author_passages_allow_empty(
        transaction,
        &write.revision_id,
        &prepared.body_markdown,
        updated_at_ms,
    )?;
    let changed = transaction.execute(
        "UPDATE tidbit
         SET current_revision_id = ?1, updated_at = ?2
         WHERE id = ?3 AND current_revision_id = ?4 AND deleted_at IS NULL",
        params![
            &write.revision_id,
            updated_at_ms,
            &working_copy.note_id,
            base_revision_id,
        ],
    )?;
    if changed != 1 {
        return Err(DatabaseError::StaleTidbit {
            id: working_copy.note_id.clone(),
            expected_revision_id: base_revision_id.to_owned(),
            actual_revision_id: current.revision_id,
        });
    }
    passages::replace_active_author_passages_allow_empty(
        transaction,
        &working_copy.note_id,
        &write.revision_id,
    )
}

fn validate_save_input(input: &SaveWorkingCopyInput) -> Result<()> {
    tidbits::validate_uuid_v7(&input.note_id, "noteId")?;
    validate_generation(input.edit_generation, "editGeneration")?;
    if let Some(base_revision_id) = input.base_revision_id.as_deref() {
        tidbits::validate_uuid_v7(base_revision_id, "baseRevisionId")?;
    }
    document::validate(&input.document_json)?;
    Ok(())
}

fn validate_base_revision(
    transaction: &Transaction<'_>,
    input: &SaveWorkingCopyInput,
) -> Result<()> {
    if let Some(expected_revision_id) = input.base_revision_id.as_deref() {
        let current = tidbits::load_current_revision(transaction, &input.note_id)?;
        if current.deleted_at_ms.is_some() {
            return Err(DatabaseError::TidbitDeleted {
                id: input.note_id.clone(),
            });
        }
        if current.revision_id != expected_revision_id {
            return Err(DatabaseError::StaleTidbit {
                id: input.note_id.clone(),
                expected_revision_id: expected_revision_id.to_owned(),
                actual_revision_id: current.revision_id,
            });
        }
        return Ok(());
    }
    let exists = transaction
        .query_row(
            "SELECT 1 FROM tidbit WHERE id = ?1",
            params![&input.note_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if exists {
        Err(DatabaseError::InvalidInput(
            "an existing note working copy requires its current base revision".into(),
        ))
    } else {
        Ok(())
    }
}

fn validate_generation(value: i64, field: &str) -> Result<()> {
    if (1..=9_007_199_254_740_991).contains(&value) {
        Ok(())
    } else {
        Err(DatabaseError::InvalidInput(format!(
            "{field} must be a positive JavaScript-safe integer"
        )))
    }
}

fn load_in_transaction(
    transaction: &Transaction<'_>,
    note_id: &str,
) -> Result<Option<WorkingCopy>> {
    load_from_connection(transaction, note_id)
}

fn load_from_connection(connection: &Connection, note_id: &str) -> Result<Option<WorkingCopy>> {
    let row = connection
        .query_row(
            "SELECT
                draft.id,
                draft.id,
                draft.base_revision_id,
                draft.edit_generation,
                draft.media_reservation,
                draft.created_at,
                draft.updated_at,
                draft.document_json,
                draft.body_markdown
             FROM draft
             WHERE draft.id = ?1",
            params![note_id],
            |row| {
                Ok(WorkingCopy {
                    id: row.get(0)?,
                    note_id: row.get(1)?,
                    base_revision_id: row.get(2)?,
                    edit_generation: row.get(3)?,
                    media_reservation: row.get(4)?,
                    created_at_ms: row.get(5)?,
                    updated_at_ms: row.get(6)?,
                    document_json: row.get(7)?,
                    body_markdown: row.get(8)?,
                    sources: Vec::new(),
                })
            },
        )
        .optional()?;
    row.map(|mut working_copy| {
        let mut statement = connection.prepare(
            "SELECT label, url
             FROM draft_source
             WHERE draft_id = ?1
             ORDER BY position",
        )?;
        working_copy.sources = statement
            .query_map(params![&working_copy.id], |row| {
                Ok(SourceDraft {
                    label: row.get(0)?,
                    url: row.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(working_copy)
    })
    .transpose()
}

fn same_authored_state(existing: &WorkingCopy, input: &SaveWorkingCopyInput) -> bool {
    existing.base_revision_id == input.base_revision_id
        && existing.document_json == input.document_json
        && existing.body_markdown == input.body_markdown
        && existing.sources == input.sources
}

fn save_result(
    status: WorkingCopySaveStatus,
    working_copy: Option<WorkingCopy>,
) -> WorkingCopySaveResult {
    let accepted_edit_generation = working_copy
        .as_ref()
        .map_or(0, |working_copy| working_copy.edit_generation);
    WorkingCopySaveResult {
        status,
        accepted_edit_generation,
        working_copy,
    }
}

pub(crate) fn has_meaningful_authored_content(markdown: &str) -> bool {
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_MATH;
    Parser::new_ext(markdown, options).any(|event| match event {
        Event::Text(value)
        | Event::Code(value)
        | Event::InlineMath(value)
        | Event::DisplayMath(value) => value.chars().any(|character| !character.is_whitespace()),
        Event::Html(value) | Event::InlineHtml(value) => html_has_visible_text(&value),
        Event::Rule | Event::TaskListMarker(_) => true,
        Event::Start(_)
        | Event::End(_)
        | Event::FootnoteReference(_)
        | Event::SoftBreak
        | Event::HardBreak => false,
    })
}

fn html_has_visible_text(html: &str) -> bool {
    let mut inside_tag = false;
    html.chars().any(|character| match character {
        '<' => {
            inside_tag = true;
            false
        }
        '>' => {
            inside_tag = false;
            false
        }
        _ => !inside_tag && !character.is_whitespace(),
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::database::{
        tidbits::CreateTidbitWrite, AttachmentIngestInput, Database, DatabasePaths,
        SearchPassagesInput, SemanticSearchReadiness, TidbitDraft,
    };

    const NOTE_ID: &str = "019f547b-6200-7000-8000-000000007001";
    const DRAFT_ID_1: &str = "019f547b-6200-7000-8000-000000007002";
    const DRAFT_ID_2: &str = "019f547b-6200-7000-8000-000000007003";
    const REVISION_ID_1: &str = "019f547b-6200-7000-8000-000000007004";
    const REVISION_ID_2: &str = "019f547b-6200-7000-8000-000000007005";
    const SOURCE_ID_1: &str = "019f547b-6200-7000-8000-000000007006";

    struct TestLibrary {
        _root: tempfile::TempDir,
        database: Database,
    }

    impl TestLibrary {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("temporary working-copy library");
            let database = Database::initialize(DatabasePaths::new(root.path()))
                .expect("working-copy database");
            Self {
                _root: root,
                database,
            }
        }

        fn save(
            &self,
            generation: i64,
            body: &str,
            base_revision_id: Option<String>,
            draft_id: &str,
        ) -> WorkingCopySaveResult {
            self.save_with_sources(generation, body, base_revision_id, draft_id, Vec::new())
        }

        fn save_with_sources(
            &self,
            generation: i64,
            body: &str,
            base_revision_id: Option<String>,
            _draft_id: &str,
            sources: Vec<SourceDraft>,
        ) -> WorkingCopySaveResult {
            self.database
                .client()
                .save_working_copy(SaveWorkingCopyWrite {
                    input: SaveWorkingCopyInput {
                        note_id: NOTE_ID.into(),
                        base_revision_id,
                        edit_generation: generation,
                        document_json: super::document::fixture_from_markdown(body),
                        body_markdown: body.into(),
                        sources,
                    },
                    now_ms: 100 + generation,
                    media_limits: media::MediaLimits::default(),
                    allow_empty_ephemeral: false,
                })
                .expect("save working copy")
        }

        fn checkpoint(&self, generation: i64, revision_id: &str) -> WorkingCopyCheckpointResult {
            self.checkpoint_with_sources(generation, revision_id, Vec::new())
        }

        fn checkpoint_with_sources(
            &self,
            generation: i64,
            revision_id: &str,
            source_ids: Vec<String>,
        ) -> WorkingCopyCheckpointResult {
            self.database
                .client()
                .checkpoint_working_copy(CheckpointWorkingCopyWrite {
                    input: CheckpointWorkingCopyInput {
                        note_id: NOTE_ID.into(),
                        expected_edit_generation: generation,
                    },
                    now_ms: 200 + generation,
                    revision_id: revision_id.into(),
                    source_ids,
                })
                .expect("checkpoint working copy")
        }
    }

    #[test]
    fn semantic_empty_detection_distinguishes_structure_math_and_media() {
        for empty in ["", "  \n\t", "# \n\n- ", "$$  $$", "<br><span> </span>"] {
            assert!(!has_meaningful_authored_content(empty), "{empty:?}");
        }
        for contentful in [
            "shower thought",
            "`identifier`",
            "$x$",
            "---",
            "** **",
            "- [ ]",
            "{{kosh:image:019f547b-6200-7000-8000-000000007099}}",
            "<span>formatted text</span>",
        ] {
            assert!(
                has_meaningful_authored_content(contentful),
                "{contentful:?}"
            );
        }
    }

    #[test]
    fn blank_ephemeral_note_never_creates_durable_state() {
        let library = TestLibrary::new();
        let result = library.save(1, "# \n\n- ", None, DRAFT_ID_1);
        assert_eq!(result.status, WorkingCopySaveStatus::Cleared);
        assert_eq!(result.accepted_edit_generation, 1);
        assert_eq!(result.working_copy, None);
        assert_eq!(
            library
                .database
                .client()
                .load_working_copy(NOTE_ID.into())
                .expect("load empty working copy"),
            None
        );
        assert!(matches!(
            library.database.client().load_tidbit(NOTE_ID.into()),
            Err(DatabaseError::NotFound { .. })
        ));
    }

    #[test]
    fn source_only_ephemeral_working_copy_is_cleared() {
        let library = TestLibrary::new();
        let source = SourceDraft {
            label: Some("Discarded source".into()),
            url: Some("https://example.com/discarded".into()),
        };
        let result = library.save_with_sources(1, "", None, DRAFT_ID_1, vec![source]);

        assert_eq!(result.status, WorkingCopySaveStatus::Cleared);
        assert_eq!(result.working_copy, None);
        assert_eq!(
            library
                .database
                .client()
                .load_working_copy(NOTE_ID.into())
                .expect("load source-only working copy")
                .map(|copy| copy.sources),
            None
        );
        assert!(matches!(
            library.database.client().load_tidbit(NOTE_ID.into()),
            Err(DatabaseError::NotFound { .. })
        ));
    }

    #[test]
    fn clearing_the_body_removes_an_existing_source_only_ephemeral_copy() {
        let library = TestLibrary::new();
        let source = SourceDraft {
            label: Some("Temporary source".into()),
            url: Some("https://example.com/temporary".into()),
        };
        assert_eq!(
            library
                .save(1, "Temporary thought", None, DRAFT_ID_1)
                .status,
            WorkingCopySaveStatus::Saved
        );
        assert_eq!(
            library
                .save_with_sources(
                    2,
                    "Temporary thought",
                    None,
                    DRAFT_ID_2,
                    vec![source.clone()],
                )
                .status,
            WorkingCopySaveStatus::Saved
        );

        let cleared = library.save_with_sources(3, "", None, DRAFT_ID_2, vec![source]);

        assert_eq!(cleared.status, WorkingCopySaveStatus::Cleared);
        assert_eq!(cleared.working_copy, None);
        assert_eq!(
            library
                .database
                .client()
                .load_working_copy(NOTE_ID.into())
                .expect("load cleared working copy"),
            None
        );
    }

    #[test]
    fn empty_media_reservation_is_discarded_only_at_its_exact_generation() {
        let library = TestLibrary::new();
        let reservation = library
            .database
            .client()
            .save_working_copy(SaveWorkingCopyWrite {
                input: SaveWorkingCopyInput {
                    note_id: NOTE_ID.into(),
                    base_revision_id: None,
                    edit_generation: 1,
                    document_json: super::document::single_paragraph(""),
                    body_markdown: String::new(),
                    sources: Vec::new(),
                },
                now_ms: 101,
                media_limits: media::MediaLimits::default(),
                allow_empty_ephemeral: true,
            })
            .expect("reserve draft for media");
        assert_eq!(reservation.status, WorkingCopySaveStatus::Saved);
        assert_eq!(
            reservation
                .working_copy
                .as_ref()
                .map(|copy| copy.media_reservation),
            Some(true)
        );
        assert_eq!(
            reservation
                .working_copy
                .as_ref()
                .map(|copy| copy.id.as_str()),
            Some(NOTE_ID)
        );

        library.save(2, "newer authored text", None, DRAFT_ID_2);
        assert!(!library
            .database
            .client()
            .discard_working_copy(
                DiscardWorkingCopyInput {
                    note_id: NOTE_ID.into(),
                    expected_edit_generation: 1,
                },
                103,
            )
            .expect("reject stale discard"));
        assert_eq!(
            library
                .database
                .client()
                .load_working_copy(NOTE_ID.into())
                .expect("load preserved copy")
                .map(|copy| copy.body_markdown),
            Some("newer authored text".into())
        );
        assert_eq!(
            library
                .database
                .client()
                .load_working_copy(NOTE_ID.into())
                .expect("load classified copy")
                .map(|copy| copy.media_reservation),
            Some(false)
        );
        assert!(library
            .database
            .client()
            .discard_working_copy(
                DiscardWorkingCopyInput {
                    note_id: NOTE_ID.into(),
                    expected_edit_generation: 2,
                },
                104,
            )
            .expect("discard exact copy"));
        assert_eq!(
            library
                .database
                .client()
                .load_working_copy(NOTE_ID.into())
                .expect("load discarded copy"),
            None
        );
    }

    #[test]
    fn media_only_working_copy_checkpoints_its_attachment_membership() {
        let library = TestLibrary::new();
        library
            .database
            .client()
            .save_working_copy(SaveWorkingCopyWrite {
                input: SaveWorkingCopyInput {
                    note_id: NOTE_ID.into(),
                    base_revision_id: None,
                    edit_generation: 1,
                    document_json: super::document::single_paragraph(""),
                    body_markdown: String::new(),
                    sources: Vec::new(),
                },
                now_ms: 101,
                media_limits: media::MediaLimits::default(),
                allow_empty_ephemeral: true,
            })
            .expect("reserve media-only note");
        let attachment = library
            .database
            .ingest_attachment(
                AttachmentIngestInput {
                    draft_id: NOTE_ID.into(),
                    display_filename: "shower-thought.txt".into(),
                    media_type: "text/plain".into(),
                    now_ms: 102,
                    limits: media::MediaLimits::default(),
                },
                Cursor::new(b"attachment-only note"),
            )
            .expect("ingest note attachment");
        let body = format!("{{{{kosh:attachment:{}}}}}", attachment.id);
        let saved = library.save(2, &body, None, DRAFT_ID_2);
        assert_eq!(saved.status, WorkingCopySaveStatus::Saved);
        let checkpoint = library.checkpoint(2, REVISION_ID_1);
        assert_eq!(checkpoint.status, WorkingCopyCheckpointStatus::Checkpointed);
        assert_eq!(
            checkpoint
                .note
                .as_ref()
                .map(|note| note.body_markdown.as_str()),
            Some(body.as_str())
        );
        assert_eq!(
            library
                .database
                .open_main_read_only()
                .expect("main reader")
                .query_row(
                    "SELECT count(*) FROM tidbit_revision_attachment WHERE tidbit_revision_id = ?1",
                    params![REVISION_ID_1],
                    |row| row.get::<_, i64>(0),
                )
                .expect("revision attachment count"),
            1
        );
    }

    #[test]
    fn monotonically_newer_generation_wins_and_reuse_is_rejected() {
        let library = TestLibrary::new();
        let newer = library.save(2, "newest exact text", None, DRAFT_ID_1);
        assert_eq!(newer.status, WorkingCopySaveStatus::Saved);
        let stale = library.save(1, "older completion", None, DRAFT_ID_2);
        assert_eq!(stale.status, WorkingCopySaveStatus::Stale);
        assert_eq!(stale.accepted_edit_generation, 2);
        assert_eq!(
            stale
                .working_copy
                .as_ref()
                .map(|copy| copy.body_markdown.as_str()),
            Some("newest exact text")
        );
        let reused = library
            .database
            .client()
            .save_working_copy(SaveWorkingCopyWrite {
                input: SaveWorkingCopyInput {
                    note_id: NOTE_ID.into(),
                    base_revision_id: None,
                    edit_generation: 2,
                    document_json: super::document::single_paragraph("different bytes"),
                    body_markdown: "different bytes".into(),
                    sources: Vec::new(),
                },
                now_ms: 103,
                media_limits: media::MediaLimits::default(),
                allow_empty_ephemeral: false,
            });
        assert!(matches!(reused, Err(DatabaseError::InvalidInput(_))));
    }

    #[test]
    fn exact_checkpoint_creates_titleless_revision_and_consumes_copy_atomically() {
        let library = TestLibrary::new();
        library.save_with_sources(
            1,
            "Remember the blue hour.",
            None,
            DRAFT_ID_1,
            vec![SourceDraft {
                label: Some(" Reference ".into()),
                url: Some("HTTPS://Example.COM:443/reference#fragment".into()),
            }],
        );
        let result = library.checkpoint_with_sources(1, REVISION_ID_1, vec![SOURCE_ID_1.into()]);
        assert_eq!(result.status, WorkingCopyCheckpointStatus::Checkpointed);
        assert_eq!(result.consumed_edit_generation, Some(1));
        let note = result.note.expect("checkpointed note");
        assert_eq!(note.id, NOTE_ID);
        assert_eq!(note.current_revision_id, REVISION_ID_1);
        assert_eq!(note.revision_number, 1);
        assert_eq!(note.body_markdown, "Remember the blue hour.");
        assert_eq!(note.sources.len(), 1);
        assert_eq!(note.sources[0].id, SOURCE_ID_1);
        assert_eq!(note.sources[0].label.as_deref(), Some("Reference"));
        assert_eq!(
            note.sources[0].url.as_deref(),
            Some("https://example.com/reference")
        );
        assert_eq!(
            library
                .database
                .client()
                .load_working_copy(NOTE_ID.into())
                .expect("consumed working copy"),
            None
        );
    }

    #[test]
    fn stale_checkpoint_preserves_the_newest_working_copy() {
        let library = TestLibrary::new();
        library.save(1, "first", None, DRAFT_ID_1);
        library.save(2, "second", None, DRAFT_ID_2);
        let result = library.checkpoint(1, REVISION_ID_1);
        assert_eq!(result.status, WorkingCopyCheckpointStatus::Stale);
        assert_eq!(result.note, None);
        assert_eq!(
            result
                .working_copy
                .map(|copy| (copy.edit_generation, copy.body_markdown)),
            Some((2, "second".into()))
        );
        assert!(matches!(
            library.database.client().load_tidbit(NOTE_ID.into()),
            Err(DatabaseError::NotFound { .. })
        ));
    }

    #[test]
    fn continuous_typing_creates_bounded_revisions_and_empty_edits_do_not_delete() {
        let library = TestLibrary::new();
        library.save(1, "a", None, DRAFT_ID_1);
        library.save(2, "ab", None, DRAFT_ID_2);
        library.save(3, "abc", None, DRAFT_ID_2);
        let first = library
            .checkpoint(3, REVISION_ID_1)
            .note
            .expect("first checkpoint");
        assert_eq!(first.revision_number, 1);

        library.save(4, "", Some(first.current_revision_id.clone()), DRAFT_ID_2);
        let cleared = library
            .checkpoint(4, REVISION_ID_2)
            .note
            .expect("empty checkpoint");
        assert_eq!(cleared.revision_number, 2);
        assert_eq!(cleared.body_markdown, "");
        assert_eq!(cleared.deleted_at_ms, None);
        let connection = library
            .database
            .open_main_read_only()
            .expect("read empty revision passage state");
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM active_passage WHERE tidbit_id = ?1",
                    params![NOTE_ID],
                    |row| row.get::<_, i64>(0),
                )
                .expect("active passage count"),
            0
        );
    }

    #[test]
    fn interrupted_copy_survives_restart_and_becomes_searchable_after_checkpoint() {
        let root = tempfile::tempdir().expect("restart working-copy root");
        let paths = DatabasePaths::new(root.path());
        let database = Database::initialize(paths.clone()).expect("initial database");
        database
            .client()
            .save_working_copy(SaveWorkingCopyWrite {
                input: SaveWorkingCopyInput {
                    note_id: NOTE_ID.into(),
                    base_revision_id: None,
                    edit_generation: 7,
                    document_json: super::document::single_paragraph(
                        "recoverable saffron observation",
                    ),
                    body_markdown: "recoverable saffron observation".into(),
                    sources: Vec::new(),
                },
                now_ms: 300,
                media_limits: media::MediaLimits::default(),
                allow_empty_ephemeral: false,
            })
            .expect("persist interrupted copy");
        database.shutdown().expect("shutdown before recovery");
        drop(database);

        let reopened = Database::initialize(paths).expect("reopen database");
        let recovered = reopened
            .client()
            .list_working_copies()
            .expect("list recovered copies");
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].edit_generation, 7);
        assert!(recovered[0].document_json.contains("native-fixture-block"));
        let recovered_document = recovered[0].document_json.clone();
        let before_checkpoint = reopened
            .client()
            .search_passages_with_semantics(
                SearchPassagesInput {
                    query: "saffron".into(),
                    limit: 10,
                    mode: crate::database::LexicalSearchMode::Default,
                },
                None,
                SemanticSearchReadiness::WaitingForRuntime,
            )
            .expect("search ignores the interrupted copy");
        assert!(before_checkpoint.results.is_empty());
        reopened
            .client()
            .checkpoint_working_copy(CheckpointWorkingCopyWrite {
                input: CheckpointWorkingCopyInput {
                    note_id: NOTE_ID.into(),
                    expected_edit_generation: 7,
                },
                now_ms: 301,
                revision_id: REVISION_ID_1.into(),
                source_ids: Vec::new(),
            })
            .expect("checkpoint recovered copy");
        assert_eq!(
            reopened
                .client()
                .load_tidbit(NOTE_ID.into())
                .expect("load checkpointed note")
                .document_json,
            recovered_document
        );
        let response = reopened
            .client()
            .search_passages_with_semantics(
                SearchPassagesInput {
                    query: "saffron".into(),
                    limit: 10,
                    mode: crate::database::LexicalSearchMode::Default,
                },
                None,
                SemanticSearchReadiness::WaitingForRuntime,
            )
            .expect("search reconciled copy");
        assert_eq!(response.results.len(), 1);
        assert_eq!(
            response.results[0]
                .citation
                .tidbit
                .as_ref()
                .map(|tidbit| tidbit.id.as_str()),
            Some(NOTE_ID)
        );
    }

    #[test]
    fn existing_note_requires_its_exact_current_base() {
        let library = TestLibrary::new();
        let note = library
            .database
            .client()
            .create_tidbit(CreateTidbitWrite {
                input: TidbitDraft {
                    document_json: super::document::single_paragraph("existing body"),
                    body_markdown: "existing body".into(),
                    sources: Vec::new(),
                },
                now_ms: 400,
                tidbit_id: NOTE_ID.into(),
                revision_id: REVISION_ID_1.into(),
                source_ids: Vec::new(),
            })
            .expect("existing note");
        let stale = library
            .database
            .client()
            .save_working_copy(SaveWorkingCopyWrite {
                input: SaveWorkingCopyInput {
                    note_id: NOTE_ID.into(),
                    base_revision_id: Some(REVISION_ID_2.into()),
                    edit_generation: 1,
                    document_json: super::document::single_paragraph("unsafe overwrite"),
                    body_markdown: "unsafe overwrite".into(),
                    sources: Vec::new(),
                },
                now_ms: 401,
                media_limits: media::MediaLimits::default(),
                allow_empty_ephemeral: false,
            });
        assert!(matches!(stale, Err(DatabaseError::StaleTidbit { .. })));
        assert_eq!(
            library
                .save(1, "safe edit", Some(note.current_revision_id), DRAFT_ID_1,)
                .status,
            WorkingCopySaveStatus::Saved
        );
    }
}

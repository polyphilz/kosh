use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;

use super::{media, passages, DatabaseError, Result};

const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
const DISPLAY_TITLE_LIMIT: usize = 96;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceDraft {
    pub label: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TidbitDraft {
    pub body_markdown: String,
    #[serde(default)]
    pub sources: Vec<SourceDraft>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteTidbitInput {
    pub id: String,
    pub expected_revision_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreTidbitInput {
    pub id: String,
    pub expected_revision_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TidbitSource {
    pub id: String,
    pub label: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tidbit {
    pub id: String,
    pub current_revision_id: String,
    pub revision_number: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub deleted_at_ms: Option<i64>,
    pub display_title: String,
    pub body_markdown: String,
    pub sources: Vec<TidbitSource>,
}

#[derive(Clone, Debug)]
pub(super) struct PreparedSource {
    label: Option<String>,
    normalized_url: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct PreparedRevision {
    pub(super) body_markdown: String,
    pub(super) sources: Vec<PreparedSource>,
    pub(super) content_hash: Vec<u8>,
}

pub(crate) struct CreateTidbitWrite {
    pub input: TidbitDraft,
    pub now_ms: i64,
    pub tidbit_id: String,
    pub revision_id: String,
    pub source_ids: Vec<String>,
}

pub(super) fn create_tidbit(
    connection: &mut Connection,
    write: CreateTidbitWrite,
) -> Result<Tidbit> {
    validate_timestamp(write.now_ms, "nowMs")?;
    validate_uuid_v7(&write.tidbit_id, "tidbitId")?;
    validate_uuid_v7(&write.revision_id, "revisionId")?;
    let prepared = prepare_revision(write.input.body_markdown, write.input.sources)?;
    validate_source_ids(&write.source_ids, prepared.sources.len())?;

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "INSERT INTO tidbit(id, created_at, updated_at, current_revision_id)
         VALUES(?1, ?2, ?2, ?3)",
        params![write.tidbit_id, write.now_ms, write.revision_id],
    )?;
    insert_revision(
        &transaction,
        &write.revision_id,
        &write.tidbit_id,
        1,
        write.now_ms,
        &prepared,
        &write.source_ids,
    )?;
    media::link_revision_attachments(
        &transaction,
        &write.revision_id,
        None,
        &write.tidbit_id,
        &prepared.body_markdown,
        write.now_ms,
    )?;
    passages::insert_author_passages(
        &transaction,
        &write.revision_id,
        &prepared.body_markdown,
        write.now_ms,
    )?;
    passages::replace_active_author_passages(&transaction, &write.tidbit_id, &write.revision_id)?;
    transaction.commit()?;

    load_tidbit(connection, &write.tidbit_id)
}

pub(super) fn delete_tidbit(
    connection: &mut Connection,
    input: DeleteTidbitInput,
    now_ms: i64,
) -> Result<Tidbit> {
    validate_timestamp(now_ms, "nowMs")?;
    validate_uuid_v7(&input.id, "id")?;
    validate_uuid_v7(&input.expected_revision_id, "expectedRevisionId")?;

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = load_current_revision(&transaction, &input.id)?;
    if current.deleted_at_ms.is_some() {
        return Err(DatabaseError::TidbitDeleted { id: input.id });
    }
    if current.revision_id != input.expected_revision_id {
        return Err(DatabaseError::StaleTidbit {
            id: input.id,
            expected_revision_id: input.expected_revision_id,
            actual_revision_id: current.revision_id,
        });
    }
    let deleted_at_ms = next_timestamp(current.updated_at_ms, now_ms)?;
    let changed = transaction.execute(
        "UPDATE tidbit
         SET deleted_at = ?1, updated_at = ?1
         WHERE id = ?2
           AND current_revision_id = ?3
           AND deleted_at IS NULL",
        params![deleted_at_ms, input.id, input.expected_revision_id],
    )?;
    if changed != 1 {
        return Err(DatabaseError::StaleTidbit {
            id: input.id,
            expected_revision_id: input.expected_revision_id,
            actual_revision_id: current.revision_id,
        });
    }
    passages::deactivate_tidbit(&transaction, &input.id)?;
    transaction.commit()?;

    load_tidbit(connection, &input.id)
}

pub(super) fn restore_tidbit(
    connection: &mut Connection,
    input: RestoreTidbitInput,
    now_ms: i64,
) -> Result<Tidbit> {
    validate_timestamp(now_ms, "nowMs")?;
    validate_uuid_v7(&input.id, "id")?;
    validate_uuid_v7(&input.expected_revision_id, "expectedRevisionId")?;

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = load_current_revision(&transaction, &input.id)?;
    if current.deleted_at_ms.is_none() {
        return Err(DatabaseError::InvalidInput(format!(
            "tidbit {} is not deleted",
            input.id
        )));
    }
    if current.revision_id != input.expected_revision_id {
        return Err(DatabaseError::StaleTidbit {
            id: input.id,
            expected_revision_id: input.expected_revision_id,
            actual_revision_id: current.revision_id,
        });
    }
    let updated_at_ms = next_timestamp(current.updated_at_ms, now_ms)?;
    let changed = transaction.execute(
        "UPDATE tidbit
         SET deleted_at = NULL, updated_at = ?1
         WHERE id = ?2
           AND current_revision_id = ?3
           AND deleted_at IS NOT NULL",
        params![updated_at_ms, input.id, input.expected_revision_id],
    )?;
    if changed != 1 {
        return Err(DatabaseError::StaleTidbit {
            id: input.id,
            expected_revision_id: input.expected_revision_id,
            actual_revision_id: current.revision_id,
        });
    }
    passages::activate_author_passages_on_restore(
        &transaction,
        &input.id,
        &input.expected_revision_id,
    )?;
    transaction.commit()?;

    load_tidbit(connection, &input.id)
}

pub(crate) fn load_tidbit(connection: &Connection, id: &str) -> Result<Tidbit> {
    validate_uuid_v7(id, "id")?;
    let mut tidbit = connection
        .query_row(
            "SELECT
                tidbit.id,
                tidbit.current_revision_id,
                revision.revision_number,
                tidbit.created_at,
                tidbit.updated_at,
                tidbit.deleted_at,
                revision.body_markdown
             FROM tidbit
             JOIN tidbit_revision AS revision
               ON revision.id = tidbit.current_revision_id
              AND revision.tidbit_id = tidbit.id
             WHERE tidbit.id = ?1",
            params![id],
            tidbit_from_row,
        )
        .optional()?
        .ok_or_else(|| DatabaseError::NotFound {
            entity: "tidbit",
            id: id.to_owned(),
        })?;
    tidbit.sources = load_sources(connection, &tidbit.current_revision_id)?;
    Ok(tidbit)
}

pub(super) fn load_source_url(connection: &Connection, source_id: &str) -> Result<String> {
    validate_uuid_v7(source_id, "sourceId")?;
    connection
        .query_row(
            "SELECT normalized_url FROM source WHERE id = ?1 AND normalized_url IS NOT NULL",
            params![source_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| DatabaseError::NotFound {
            entity: "source URL",
            id: source_id.to_owned(),
        })
}

fn prepare_revision(body_markdown: String, sources: Vec<SourceDraft>) -> Result<PreparedRevision> {
    prepare_revision_with_empty(body_markdown, sources, false)
}

pub(super) fn prepare_revision_with_empty(
    body_markdown: String,
    sources: Vec<SourceDraft>,
    allow_empty_body: bool,
) -> Result<PreparedRevision> {
    if !allow_empty_body && body_markdown.trim().is_empty() {
        return Err(DatabaseError::InvalidInput(
            "bodyMarkdown must contain non-whitespace text".into(),
        ));
    }
    let sources = sources
        .into_iter()
        .map(prepare_source)
        .collect::<Result<Vec<_>>>()?;
    let mut unique_sources = HashSet::with_capacity(sources.len());
    for source in &sources {
        if !unique_sources.insert((source.label.as_deref(), source.normalized_url.as_deref())) {
            return Err(DatabaseError::InvalidInput(
                "sources must not contain duplicates".into(),
            ));
        }
    }
    let content_hash = revision_content_hash(&body_markdown, &sources);
    Ok(PreparedRevision {
        body_markdown,
        sources,
        content_hash,
    })
}

fn prepare_source(source: SourceDraft) -> Result<PreparedSource> {
    let label = normalize_optional_text(source.label);
    let normalized_url = source
        .url
        .and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        })
        .map(|value| normalize_url(&value))
        .transpose()?;
    if label.is_none() && normalized_url.is_none() {
        return Err(DatabaseError::InvalidInput(
            "each source needs a label or HTTP(S) URL".into(),
        ));
    }
    Ok(PreparedSource {
        label,
        normalized_url,
    })
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

fn normalize_url(value: &str) -> Result<String> {
    let mut url = Url::parse(value)
        .map_err(|_| DatabaseError::InvalidInput("source URL is invalid".into()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(DatabaseError::InvalidInput(
            "source URL must use HTTP or HTTPS".into(),
        ));
    }
    url.set_fragment(None);
    Ok(url.to_string())
}

pub(super) fn validate_source_ids(ids: &[String], expected: usize) -> Result<()> {
    if ids.len() != expected {
        return Err(DatabaseError::InvalidInput(
            "source ID count does not match source count".into(),
        ));
    }
    for id in ids {
        validate_uuid_v7(id, "sourceId")?;
    }
    Ok(())
}

pub(super) fn insert_revision(
    transaction: &Transaction<'_>,
    revision_id: &str,
    tidbit_id: &str,
    revision_number: i64,
    created_at_ms: i64,
    revision: &PreparedRevision,
    source_ids: &[String],
) -> Result<()> {
    transaction.execute(
        "INSERT INTO tidbit_revision(
            id, tidbit_id, revision_number, created_at, body_markdown, content_hash
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            revision_id,
            tidbit_id,
            revision_number,
            created_at_ms,
            revision.body_markdown,
            revision.content_hash
        ],
    )?;
    for (sort_order, (source, proposed_id)) in
        revision.sources.iter().zip(source_ids.iter()).enumerate()
    {
        let existing_id = transaction
            .query_row(
                "SELECT id
                 FROM source
                 WHERE label IS ?1 AND normalized_url IS ?2
                 ORDER BY id
                 LIMIT 1",
                params![source.label, source.normalized_url],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let source_id = if let Some(existing_id) = existing_id {
            existing_id
        } else {
            transaction.execute(
                "INSERT INTO source(id, created_at, label, normalized_url)
                 VALUES(?1, ?2, ?3, ?4)",
                params![
                    proposed_id,
                    created_at_ms,
                    source.label,
                    source.normalized_url
                ],
            )?;
            proposed_id.clone()
        };
        transaction.execute(
            "INSERT INTO tidbit_revision_source(tidbit_revision_id, source_id, sort_order)
             VALUES(?1, ?2, ?3)",
            params![revision_id, source_id, sort_order as i64],
        )?;
    }
    Ok(())
}

pub(super) fn load_sources(
    connection: &Connection,
    revision_id: &str,
) -> Result<Vec<TidbitSource>> {
    let mut statement = connection.prepare(
        "SELECT source.id, source.label, source.normalized_url
         FROM tidbit_revision_source AS membership
         JOIN source ON source.id = membership.source_id
         WHERE membership.tidbit_revision_id = ?1
         ORDER BY membership.sort_order",
    )?;
    let rows = statement.query_map(params![revision_id], |row| {
        Ok(TidbitSource {
            id: row.get(0)?,
            label: row.get(1)?,
            url: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn tidbit_from_row(row: &Row<'_>) -> rusqlite::Result<Tidbit> {
    let body_markdown = row.get::<_, String>(6)?;
    Ok(Tidbit {
        id: row.get(0)?,
        current_revision_id: row.get(1)?,
        revision_number: row.get(2)?,
        created_at_ms: row.get(3)?,
        updated_at_ms: row.get(4)?,
        deleted_at_ms: row.get(5)?,
        display_title: derive_display_title(&body_markdown),
        body_markdown,
        sources: Vec::new(),
    })
}

pub(super) struct CurrentRevision {
    pub(super) revision_id: String,
    pub(super) revision_number: i64,
    pub(super) updated_at_ms: i64,
    pub(super) deleted_at_ms: Option<i64>,
}

pub(super) fn load_current_revision(
    transaction: &Transaction<'_>,
    id: &str,
) -> Result<CurrentRevision> {
    transaction
        .query_row(
            "SELECT
                tidbit.current_revision_id,
                revision.revision_number,
                tidbit.updated_at,
                tidbit.deleted_at
             FROM tidbit
             JOIN tidbit_revision AS revision
               ON revision.id = tidbit.current_revision_id
              AND revision.tidbit_id = tidbit.id
             WHERE tidbit.id = ?1",
            params![id],
            |row| {
                Ok(CurrentRevision {
                    revision_id: row.get(0)?,
                    revision_number: row.get(1)?,
                    updated_at_ms: row.get(2)?,
                    deleted_at_ms: row.get(3)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| DatabaseError::NotFound {
            entity: "tidbit",
            id: id.to_owned(),
        })
}

pub(super) fn validate_timestamp(value: i64, field: &str) -> Result<()> {
    if !(0..=MAX_SAFE_INTEGER).contains(&value) {
        return Err(DatabaseError::InvalidInput(format!(
            "{field} must be a non-negative JavaScript-safe integer"
        )));
    }
    Ok(())
}

pub(super) fn next_timestamp(previous: i64, observed: i64) -> Result<i64> {
    let next = if observed > previous {
        observed
    } else {
        previous
            .checked_add(1)
            .ok_or_else(|| DatabaseError::InvalidInput("timestamp overflow".into()))?
    };
    validate_timestamp(next, "updatedAtMs")?;
    Ok(next)
}

pub(super) fn validate_uuid_v7(value: &str, field: &str) -> Result<()> {
    let parsed = Uuid::parse_str(value)
        .map_err(|_| DatabaseError::InvalidInput(format!("{field} must be a UUIDv7")))?;
    if parsed.get_version_num() != 7 || parsed.hyphenated().to_string() != value {
        return Err(DatabaseError::InvalidInput(format!(
            "{field} must be a lowercase UUIDv7"
        )));
    }
    Ok(())
}

fn revision_content_hash(body_markdown: &str, sources: &[PreparedSource]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(b"kosh:note-revision:v1\0");
    hash_text(&mut hasher, body_markdown);
    hasher.update((sources.len() as u64).to_be_bytes());
    for source in sources {
        hash_optional(&mut hasher, source.label.as_deref());
        hash_optional(&mut hasher, source.normalized_url.as_deref());
    }
    hasher.finalize().to_vec()
}

fn hash_optional(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hash_text(hasher, value);
        }
        None => hasher.update([0]),
    }
}

fn hash_text(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

pub(super) fn derive_display_title(body_markdown: &str) -> String {
    body_markdown
        .lines()
        .filter_map(useful_markdown_line)
        .next()
        .map_or_else(
            || "Untitled note".into(),
            |line| truncate(line, DISPLAY_TITLE_LIMIT),
        )
}

fn useful_markdown_line(line: &str) -> Option<&str> {
    let mut value = line.trim();
    if value.is_empty() || value.starts_with("```") || value.starts_with("~~~") {
        return None;
    }
    loop {
        let stripped = value
            .strip_prefix('#')
            .or_else(|| value.strip_prefix('>'))
            .or_else(|| value.strip_prefix('-'))
            .or_else(|| value.strip_prefix('*'))
            .or_else(|| value.strip_prefix('+'));
        let Some(stripped) = stripped else {
            break;
        };
        value = stripped.trim_start();
    }
    (!value.is_empty()).then_some(value)
}

fn truncate(value: &str, limit: usize) -> String {
    let mut characters = value.chars();
    let head = characters.by_ref().take(limit).collect::<String>();
    if characters.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

use rusqlite::{params, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};

use super::{document, DatabaseError, Result};

pub(super) const FTS_VERSION: &str = "block-lexical-v1";

#[derive(Debug)]
struct AttachmentEvidence {
    display_filename: String,
    extracted_text: String,
}

#[derive(Clone, Copy)]
enum MissingAttachmentPolicy {
    Reject,
    SkipBlock,
}

pub(super) fn replace_tidbit_documents(
    transaction: &Transaction<'_>,
    tidbit_id: &str,
) -> Result<()> {
    replace_tidbit_documents_with_policy(transaction, tidbit_id, MissingAttachmentPolicy::Reject)
}

pub(super) fn clear_tidbit_documents(transaction: &Transaction<'_>, tidbit_id: &str) -> Result<()> {
    transaction.execute(
        "DELETE FROM block_search_document WHERE tidbit_id = ?1",
        params![tidbit_id],
    )?;
    Ok(())
}

fn replace_tidbit_documents_with_policy(
    transaction: &Transaction<'_>,
    tidbit_id: &str,
    missing_attachment_policy: MissingAttachmentPolicy,
) -> Result<()> {
    clear_tidbit_documents(transaction, tidbit_id)?;
    let current = transaction
        .query_row(
            "SELECT revision.id, revision.document_json, tidbit.updated_at
             FROM tidbit
             JOIN tidbit_revision AS revision
               ON revision.id = tidbit.current_revision_id
              AND revision.tidbit_id = tidbit.id
             WHERE tidbit.id = ?1 AND tidbit.deleted_at IS NULL",
            params![tidbit_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((revision_id, document_json, updated_at)) = current else {
        return Ok(());
    };
    for block in document::extract_searchable_blocks(&document_json)? {
        let attachment = match block.attachment_id.as_deref() {
            Some(attachment_id) => match load_attachment_evidence(
                transaction,
                tidbit_id,
                &revision_id,
                &block.block_id,
                attachment_id,
            )? {
                Some(evidence) => Some(evidence),
                None if matches!(
                    missing_attachment_policy,
                    MissingAttachmentPolicy::SkipBlock
                ) =>
                {
                    log::warn!(
                        "skipping block {} while rebuilding search because attachment {} is not owned by its current note",
                        block.block_id,
                        attachment_id
                    );
                    continue;
                }
                None => {
                    return Err(DatabaseError::Validation {
                        kind: "main",
                        reason: format!(
                            "current block {} does not own attachment {attachment_id}",
                            block.block_id
                        ),
                    });
                }
            },
            None => None,
        };
        let attachment_names = attachment
            .as_ref()
            .map(|evidence| evidence.display_filename.as_str())
            .unwrap_or_default();
        let extracted_text = attachment
            .as_ref()
            .map(|evidence| evidence.extracted_text.as_str())
            .unwrap_or_default();
        if block.authored_text.trim().is_empty()
            && attachment_names.trim().is_empty()
            && extracted_text.trim().is_empty()
        {
            continue;
        }
        let heading_context = block.heading_context.join("\n");
        let content_hash = search_content_hash(
            &block.block_type,
            &heading_context,
            &block.authored_text,
            attachment_names,
            extracted_text,
        );
        transaction.execute(
            "INSERT INTO block_search_document(
                tidbit_id, tidbit_revision_id, block_id, block_ordinal, block_type,
                heading_context, body, attachment_names, extracted_text,
                content_hash, updated_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                tidbit_id,
                revision_id,
                block.block_id,
                i64::try_from(block.ordinal).map_err(|_| DatabaseError::Validation {
                    kind: "main",
                    reason: "document block ordinal exceeds SQLite".into(),
                })?,
                block.block_type,
                heading_context,
                block.authored_text,
                attachment_names,
                extracted_text,
                content_hash.as_slice(),
                updated_at,
            ],
        )?;
    }
    Ok(())
}

pub(super) fn refresh_attachment_owners(
    transaction: &Transaction<'_>,
    attachment_id: &str,
) -> Result<()> {
    let owner_ids = {
        let mut statement = transaction.prepare(
            "SELECT tidbit.id
             FROM tidbit
             JOIN tidbit_revision_attachment AS membership
               ON membership.tidbit_revision_id = tidbit.current_revision_id
             WHERE membership.attachment_id = ?1
               AND tidbit.deleted_at IS NULL
             ORDER BY tidbit.id",
        )?;
        let rows = statement
            .query_map(params![attachment_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for tidbit_id in owner_ids {
        replace_tidbit_documents(transaction, &tidbit_id)?;
    }
    Ok(())
}

pub(super) fn rebuild_documents(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute("DELETE FROM block_search_document", [])?;
    let tidbit_ids = {
        let mut statement =
            transaction.prepare("SELECT id FROM tidbit WHERE deleted_at IS NULL ORDER BY id")?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for tidbit_id in tidbit_ids {
        replace_tidbit_documents_with_policy(
            transaction,
            &tidbit_id,
            MissingAttachmentPolicy::SkipBlock,
        )?;
    }
    mark_current(transaction)
}

fn load_attachment_evidence(
    transaction: &Transaction<'_>,
    tidbit_id: &str,
    revision_id: &str,
    block_id: &str,
    attachment_id: &str,
) -> Result<Option<AttachmentEvidence>> {
    Ok(transaction
        .query_row(
            "SELECT
                attachment.display_filename,
                coalesce((
                    SELECT group_concat(content, char(10))
                    FROM (
                        SELECT passage.content
                        FROM current_attachment_passage AS current
                        JOIN passage ON passage.id = current.passage_id
                        WHERE current.attachment_id = attachment.id
                          AND passage.locator_kind = 'OCR_REGION'
                        ORDER BY passage.ordinal
                    ) AS ordered_evidence
                ), '')
             FROM tidbit_revision_attachment AS membership
             JOIN attachment ON attachment.id = membership.attachment_id
             WHERE membership.tidbit_revision_id = ?1
               AND membership.attachment_id = ?2
               AND membership.block_id = ?3
               AND attachment.deleted_at IS NULL
               AND EXISTS(
                   SELECT 1
                   FROM tidbit
                   WHERE tidbit.id = ?4
                     AND tidbit.current_revision_id = membership.tidbit_revision_id
                     AND tidbit.deleted_at IS NULL
               )",
            params![revision_id, attachment_id, block_id, tidbit_id],
            |row| {
                Ok(AttachmentEvidence {
                    display_filename: row.get(0)?,
                    extracted_text: row.get(1)?,
                })
            },
        )
        .optional()?)
}

pub(super) fn search_content_hash(
    block_type: &str,
    heading_context: &str,
    body: &str,
    attachment_names: &str,
    extracted_text: &str,
) -> Vec<u8> {
    let mut hasher = Sha256::new();
    for field in [
        block_type,
        heading_context,
        body,
        attachment_names,
        extracted_text,
    ] {
        hasher.update(
            u64::try_from(field.len())
                .expect("field length fits u64")
                .to_be_bytes(),
        );
        hasher.update(field.as_bytes());
    }
    hasher.finalize().to_vec()
}

fn mark_current(transaction: &Transaction<'_>) -> Result<()> {
    let changed = transaction.execute(
        "UPDATE index_state
         SET version = ?1, status = 'IDLE', cursor = NULL, error = NULL
         WHERE name = 'BLOCK_FTS'",
        params![FTS_VERSION],
    )?;
    if changed != 1 {
        return Err(DatabaseError::Validation {
            kind: "main",
            reason: "BLOCK_FTS index state is missing".into(),
        });
    }
    Ok(())
}

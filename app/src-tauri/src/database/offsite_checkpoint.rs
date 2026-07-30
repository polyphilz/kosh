use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use sha2::{Digest, Sha256};

use crate::backup::{
    domain::{
        BackupSetId, CheckpointErrorCode, CheckpointId, CheckpointPhase, ContentSha256,
        ReplicaEpochId,
    },
    litestream::LitestreamTxid,
};

use super::{migrations, DatabaseError, Result};

#[derive(Clone, Debug)]
pub(crate) struct PrepareOffsiteCheckpointInput {
    pub(crate) checkpoint_id: CheckpointId,
    pub(crate) backup_set_id: BackupSetId,
    pub(crate) replica_epoch_id: ReplicaEpochId,
    pub(crate) created_at_ms: i64,
    pub(crate) kosh_version: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CheckpointMediaReference {
    pub(crate) sha256: ContentSha256,
    pub(crate) byte_length: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedOffsiteCheckpoint {
    pub(crate) checkpoint_id: CheckpointId,
    pub(crate) backup_set_id: BackupSetId,
    pub(crate) replica_epoch_id: ReplicaEpochId,
    pub(crate) created_at_ms: i64,
    pub(crate) kosh_version: String,
    pub(crate) config_revision: i64,
    pub(crate) content_revision: u64,
    pub(crate) main_migration_head: u32,
    pub(crate) media_migration_head: u32,
    pub(crate) referenced_media: Vec<CheckpointMediaReference>,
    pub(crate) referenced_total_bytes: u64,
    pub(crate) referenced_hash_set_sha256: ContentSha256,
    pub(crate) litestream_txid: LitestreamTxid,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PublishedOffsiteCheckpoint {
    pub(crate) checkpoint_id: CheckpointId,
    pub(crate) backup_set_id: BackupSetId,
    pub(crate) replica_epoch_id: ReplicaEpochId,
    pub(crate) config_revision: i64,
    pub(crate) content_revision: u64,
    pub(crate) kosh_version: String,
    pub(crate) main_migration_head: u32,
    pub(crate) media_migration_head: u32,
    pub(crate) referenced_hash_count: u64,
    pub(crate) referenced_total_bytes: u64,
    pub(crate) referenced_hash_set_sha256: ContentSha256,
    pub(crate) litestream_txid: LitestreamTxid,
    pub(crate) manifest_object_key: String,
    pub(crate) created_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OffsiteCheckpointScheduleState {
    pub(crate) content_revision: u64,
    pub(crate) last_published: Option<PublishedOffsiteCheckpoint>,
}

pub(crate) trait LocalCheckpointSync: Send + Sync {
    fn sync_local(&self) -> std::result::Result<LitestreamTxid, CheckpointErrorCode>;
}

pub(super) fn prepare_and_fence(
    main: &mut Connection,
    media: &Connection,
    input: PrepareOffsiteCheckpointInput,
    local_sync: Arc<dyn LocalCheckpointSync>,
) -> Result<PreparedOffsiteCheckpoint> {
    if input.created_at_ms < 0
        || input.kosh_version.is_empty()
        || input.kosh_version.len() > 64
        || input.kosh_version.chars().any(char::is_control)
    {
        return Err(invalid("checkpoint metadata is invalid"));
    }

    let transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let config_revision = load_active_config_revision(&transaction, &input)?;
    let referenced_media = load_references(&transaction, &input.backup_set_id, true)?;
    let referenced_total_bytes = referenced_media
        .iter()
        .try_fold(0_u64, |total, reference| {
            total
                .checked_add(reference.byte_length)
                .ok_or_else(|| invalid("referenced media byte total overflowed"))
        })?;
    let referenced_hash_set_sha256 = hash_reference_set(&referenced_media);
    let content_revision = load_content_revision(&transaction)?;
    let heads = migrations::expected_heads();
    let main_migration_head = positive_head(heads.main, "main")?;
    let media_migration_head = positive_head(heads.media, "media")?;
    validate_recorded_head(&transaction, main_migration_head, "main")?;
    validate_recorded_head(media, media_migration_head, "media")?;

    transaction.execute(
        "INSERT INTO offsite_backup_checkpoint (
            checkpoint_id, backup_set_id, replica_epoch_id, phase,
            config_revision, content_revision, created_at, kosh_version,
            main_migration_head, media_migration_head, referenced_hash_count,
            referenced_total_bytes, referenced_hash_set_sha256,
            litestream_txid, manifest_object_key, publication_sequence,
            last_error_code, updated_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
            NULL, NULL, NULL, NULL, ?7
         )",
        params![
            input.checkpoint_id.as_str(),
            input.backup_set_id.as_str(),
            input.replica_epoch_id.as_str(),
            CheckpointPhase::Prepared.as_db_str(),
            config_revision,
            stored_i64(content_revision, "content revision")?,
            input.created_at_ms,
            input.kosh_version,
            main_migration_head,
            media_migration_head,
            stored_i64(
                u64::try_from(referenced_media.len())
                    .map_err(|_| invalid("too many media references"))?,
                "media reference count"
            )?,
            stored_i64(referenced_total_bytes, "media byte total")?,
            referenced_hash_set_sha256.as_bytes().as_slice(),
        ],
    )?;
    transaction.commit()?;

    // The single writer remains inside this message until Litestream has
    // returned the TXID containing the committed PREPARED row. A later write
    // therefore cannot slip between the relational fence and its TXID.
    let litestream_txid = local_sync
        .sync_local()
        .map_err(DatabaseError::OffsiteCheckpointFence)?;
    Ok(PreparedOffsiteCheckpoint {
        checkpoint_id: input.checkpoint_id,
        backup_set_id: input.backup_set_id,
        replica_epoch_id: input.replica_epoch_id,
        created_at_ms: input.created_at_ms,
        kosh_version: input.kosh_version,
        config_revision,
        content_revision,
        main_migration_head,
        media_migration_head,
        referenced_media,
        referenced_total_bytes,
        referenced_hash_set_sha256,
        litestream_txid,
    })
}

pub(super) fn mark_fenced(
    connection: &mut Connection,
    checkpoint_id: &CheckpointId,
    txid: LitestreamTxid,
) -> Result<()> {
    transition(
        connection,
        checkpoint_id,
        CheckpointPhase::Prepared,
        CheckpointPhase::Fenced,
        Some(txid),
    )
}

pub(super) fn mark_replicated(
    connection: &mut Connection,
    checkpoint_id: &CheckpointId,
) -> Result<()> {
    transition(
        connection,
        checkpoint_id,
        CheckpointPhase::Fenced,
        CheckpointPhase::Replicated,
        None,
    )
}

pub(super) fn mark_published(
    connection: &mut Connection,
    checkpoint_id: &CheckpointId,
    manifest_object_key: &str,
) -> Result<()> {
    if manifest_object_key.is_empty() || manifest_object_key.len() > 1_024 {
        return Err(invalid("checkpoint manifest key is invalid"));
    }
    let changed = connection.execute(
        "UPDATE offsite_backup_checkpoint
         SET phase = ?1,
             manifest_object_key = ?2,
             publication_sequence = (
                 SELECT coalesce(max(publication_sequence), 0) + 1
                 FROM offsite_backup_checkpoint
             ),
             updated_at = max(updated_at, ?3)
         WHERE checkpoint_id = ?4 AND phase = ?5
           AND EXISTS (
               SELECT 1
               FROM offsite_backup_config AS config
               WHERE config.singleton_id = 1
                 AND config.enabled = 1
                 AND config.revision = offsite_backup_checkpoint.config_revision
                 AND config.backup_set_id = offsite_backup_checkpoint.backup_set_id
                 AND config.replica_epoch_id = offsite_backup_checkpoint.replica_epoch_id
           )",
        params![
            CheckpointPhase::Published.as_db_str(),
            manifest_object_key,
            now_millis()?,
            checkpoint_id.as_str(),
            CheckpointPhase::Replicated.as_db_str(),
        ],
    )?;
    exactly_one(changed)
}

pub(super) fn mark_failed(
    connection: &mut Connection,
    checkpoint_id: &CheckpointId,
    error_code: CheckpointErrorCode,
) -> Result<()> {
    let changed = connection.execute(
        "UPDATE offsite_backup_checkpoint
         SET phase = ?1, last_error_code = ?2, updated_at = max(updated_at, ?3)
         WHERE checkpoint_id = ?4 AND phase IN (?5, ?6, ?7)",
        params![
            CheckpointPhase::Failed.as_db_str(),
            error_code.as_db_str(),
            now_millis()?,
            checkpoint_id.as_str(),
            CheckpointPhase::Prepared.as_db_str(),
            CheckpointPhase::Fenced.as_db_str(),
            CheckpointPhase::Replicated.as_db_str(),
        ],
    )?;
    exactly_one(changed)
}

pub(super) fn fail_incomplete(
    connection: &mut Connection,
    error_code: CheckpointErrorCode,
) -> Result<u64> {
    let changed = connection.execute(
        "UPDATE offsite_backup_checkpoint
         SET phase = ?1, last_error_code = ?2, updated_at = max(updated_at, ?3)
         WHERE phase IN (?4, ?5, ?6)",
        params![
            CheckpointPhase::Failed.as_db_str(),
            error_code.as_db_str(),
            now_millis()?,
            CheckpointPhase::Prepared.as_db_str(),
            CheckpointPhase::Fenced.as_db_str(),
            CheckpointPhase::Replicated.as_db_str(),
        ],
    )?;
    Ok(changed as u64)
}

pub(super) fn schedule_state(connection: &Connection) -> Result<OffsiteCheckpointScheduleState> {
    let content_revision = load_content_revision(connection)?;
    let stored = connection
        .query_row(
            "SELECT checkpoint_id, backup_set_id, replica_epoch_id,
                    config_revision, content_revision, kosh_version,
                    main_migration_head, media_migration_head,
                    referenced_hash_count, referenced_total_bytes,
                    referenced_hash_set_sha256, litestream_txid,
                    manifest_object_key, created_at
             FROM offsite_backup_checkpoint
             WHERE phase = ?1
             ORDER BY publication_sequence DESC
             LIMIT 1",
            [CheckpointPhase::Published.as_db_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, Vec<u8>>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, i64>(13)?,
                ))
            },
        )
        .optional()?;
    let last_published = stored
        .map(
            |(
                checkpoint_id,
                backup_set_id,
                replica_epoch_id,
                config_revision,
                checkpoint_revision,
                kosh_version,
                main_head,
                media_head,
                hash_count,
                total_bytes,
                hash_set,
                txid,
                manifest_object_key,
                created_at_ms,
            )| {
                let checkpoint_revision =
                    non_negative_u64(checkpoint_revision, "content revision")?;
                let hash_count = non_negative_u64(hash_count, "media reference count")?;
                let total_bytes = non_negative_u64(total_bytes, "media byte total")?;
                let hash_set = parse_sha256(hash_set)?;
                Ok::<PublishedOffsiteCheckpoint, DatabaseError>(PublishedOffsiteCheckpoint {
                    checkpoint_id: CheckpointId::parse(checkpoint_id).map_err(invalid_domain)?,
                    backup_set_id: BackupSetId::parse(backup_set_id).map_err(invalid_domain)?,
                    replica_epoch_id: ReplicaEpochId::parse(replica_epoch_id)
                        .map_err(invalid_domain)?,
                    config_revision,
                    content_revision: checkpoint_revision,
                    kosh_version,
                    main_migration_head: positive_u32(main_head, "main migration head")?,
                    media_migration_head: positive_u32(media_head, "media migration head")?,
                    referenced_hash_count: hash_count,
                    referenced_total_bytes: total_bytes,
                    referenced_hash_set_sha256: hash_set,
                    litestream_txid: txid
                        .parse()
                        .map_err(|_| invalid("stored checkpoint TXID is invalid"))?,
                    manifest_object_key,
                    created_at_ms,
                })
            },
        )
        .transpose()?;
    Ok(OffsiteCheckpointScheduleState {
        content_revision,
        last_published,
    })
}

fn transition(
    connection: &mut Connection,
    checkpoint_id: &CheckpointId,
    expected: CheckpointPhase,
    next: CheckpointPhase,
    txid: Option<LitestreamTxid>,
) -> Result<()> {
    let changed = connection.execute(
        "UPDATE offsite_backup_checkpoint
         SET phase = ?1,
             litestream_txid = coalesce(?2, litestream_txid),
             updated_at = max(updated_at, ?3)
         WHERE checkpoint_id = ?4 AND phase = ?5",
        params![
            next.as_db_str(),
            txid.map(|value| value.to_string()),
            now_millis()?,
            checkpoint_id.as_str(),
            expected.as_db_str(),
        ],
    )?;
    exactly_one(changed)
}

fn load_active_config_revision(
    transaction: &Transaction<'_>,
    input: &PrepareOffsiteCheckpointInput,
) -> Result<i64> {
    transaction
        .query_row(
            "SELECT revision FROM offsite_backup_config
             WHERE singleton_id = 1 AND enabled = 1
               AND backup_set_id = ?1 AND replica_epoch_id = ?2",
            params![
                input.backup_set_id.as_str(),
                input.replica_epoch_id.as_str()
            ],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .filter(|revision| *revision > 0)
        .ok_or_else(|| invalid("checkpoint target is not the active enabled backup"))
}

fn load_references(
    connection: &Connection,
    backup_set_id: &BackupSetId,
    require_uploaded: bool,
) -> Result<Vec<CheckpointMediaReference>> {
    let mut statement = connection.prepare(
        "WITH referenced(sha256, byte_length) AS (
            SELECT attachment.sha256, attachment.byte_length
            FROM attachment
            WHERE attachment.deleted_at IS NULL
               OR EXISTS (
                    SELECT 1 FROM tidbit_revision_attachment
                    WHERE attachment_id = attachment.id
               )
               OR EXISTS (
                    SELECT 1 FROM research_run_attachment
                    WHERE attachment_id = attachment.id
               )
            UNION
            SELECT image.preview_sha256, image.preview_byte_length
            FROM attachment_image AS image
            JOIN attachment ON attachment.id = image.attachment_id
            WHERE attachment.deleted_at IS NULL
               OR EXISTS (
                    SELECT 1 FROM tidbit_revision_attachment
                    WHERE attachment_id = attachment.id
               )
               OR EXISTS (
                    SELECT 1 FROM research_run_attachment
                    WHERE attachment_id = attachment.id
               )
         )
         SELECT referenced.sha256, referenced.byte_length, upload.state
         FROM referenced
         LEFT JOIN offsite_media_upload AS upload
           ON upload.backup_set_id = ?1
          AND upload.sha256 = referenced.sha256
         ORDER BY referenced.sha256",
    )?;
    let mut rows = statement.query([backup_set_id.as_str()])?;
    let mut references: Vec<CheckpointMediaReference> = Vec::new();
    while let Some(row) = rows.next()? {
        let state = row.get::<_, Option<String>>(2)?;
        if state.is_none() || (require_uploaded && state.as_deref() != Some("UPLOADED")) {
            return Err(DatabaseError::OffsiteCheckpointMediaIncomplete);
        }
        let reference = CheckpointMediaReference {
            sha256: parse_sha256(row.get(0)?)?,
            byte_length: positive_u64(row.get(1)?, "referenced media byte length")?,
        };
        if let Some(previous) = references
            .last()
            .filter(|previous| previous.sha256 == reference.sha256)
        {
            if previous.byte_length != reference.byte_length {
                return Err(invalid(
                    "one referenced media hash has conflicting byte lengths",
                ));
            }
            continue;
        }
        references.push(reference);
    }
    Ok(references)
}

fn hash_reference_set(references: &[CheckpointMediaReference]) -> ContentSha256 {
    let mut digest = Sha256::new();
    for reference in references {
        digest.update(reference.sha256.as_bytes());
    }
    ContentSha256::from_bytes(digest.finalize().into())
}

fn parse_sha256(value: Vec<u8>) -> Result<ContentSha256> {
    let bytes: [u8; 32] = value
        .try_into()
        .map_err(|_| invalid("stored SHA-256 must contain 32 bytes"))?;
    Ok(ContentSha256::from_bytes(bytes))
}

fn load_content_revision(connection: &Connection) -> Result<u64> {
    non_negative_u64(
        connection.query_row(
            "SELECT revision FROM offsite_backup_content_clock WHERE singleton_id = 1",
            [],
            |row| row.get(0),
        )?,
        "content revision",
    )
}

fn validate_recorded_head(connection: &Connection, expected: u32, label: &str) -> Result<()> {
    let actual = connection.query_row(
        "SELECT max(version) FROM refinery_schema_history",
        [],
        |row| row.get::<_, Option<i32>>(0),
    )?;
    if actual.and_then(|value| u32::try_from(value).ok()) != Some(expected) {
        return Err(invalid(&format!(
            "{label} migration head changed during checkpoint"
        )));
    }
    Ok(())
}

fn now_millis() -> Result<i64> {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| invalid("system clock predates Unix epoch"))?
            .as_millis(),
    )
    .map_err(|_| invalid("system clock exceeded SQLite limits"))
}

fn positive_head(value: Option<i32>, label: &str) -> Result<u32> {
    value
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid(&format!("{label} migration head is invalid")))
}

fn non_negative_u64(value: i64, field: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| invalid(&format!("{field} is negative")))
}

fn positive_u64(value: i64, field: &str) -> Result<u64> {
    non_negative_u64(value, field)?
        .checked_sub(1)
        .map(|value| value + 1)
        .ok_or_else(|| invalid(&format!("{field} must be positive")))
}

fn positive_u32(value: i64, field: &str) -> Result<u32> {
    u32::try_from(positive_u64(value, field)?)
        .map_err(|_| invalid(&format!("{field} is too large")))
}

fn stored_i64(value: u64, field: &str) -> Result<i64> {
    i64::try_from(value).map_err(|_| invalid(&format!("{field} exceeded SQLite limits")))
}

fn exactly_one(changed: usize) -> Result<()> {
    if changed == 1 {
        Ok(())
    } else {
        Err(DatabaseError::StaleOffsiteCheckpoint)
    }
}

fn invalid_domain(error: impl std::fmt::Display) -> DatabaseError {
    invalid(&error.to_string())
}

fn invalid(reason: &str) -> DatabaseError {
    DatabaseError::InvalidOffsiteCheckpoint(reason.to_owned())
}

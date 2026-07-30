use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use uuid::Variant;

use crate::backup::domain::{BackupSetId, ContentSha256};

use super::{
    backup_state::{self, OffsiteBackupConfig},
    DatabaseError, Result,
};

const REFERENCED_MEDIA_SELECT: &str = "
    SELECT attachment.sha256
    FROM attachment
    WHERE attachment.deleted_at IS NULL
       OR EXISTS (
            SELECT 1
            FROM tidbit_revision_attachment AS membership
            WHERE membership.attachment_id = attachment.id
       )
       OR EXISTS (
            SELECT 1
            FROM research_run_attachment AS research_membership
            WHERE research_membership.attachment_id = attachment.id
       )
    UNION
    SELECT image.preview_sha256
    FROM attachment_image AS image
    JOIN attachment ON attachment.id = image.attachment_id
    WHERE attachment.deleted_at IS NULL
       OR EXISTS (
            SELECT 1
            FROM tidbit_revision_attachment AS membership
            WHERE membership.attachment_id = attachment.id
       )
       OR EXISTS (
            SELECT 1
            FROM research_run_attachment AS research_membership
            WHERE research_membership.attachment_id = attachment.id
       )
";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OffsiteMediaUploadClaim {
    pub(crate) config: OffsiteBackupConfig,
    pub(crate) sha256: ContentSha256,
    pub(crate) lease_id: String,
    pub(crate) attempt_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OffsiteMediaUploadFailureCode {
    Interrupted,
    ConfigurationChanged,
    CredentialsMissing,
    CredentialsUnavailable,
    LocalBlobMissing,
    LocalBlobInvalid,
    LocalBlobUnavailable,
    RemoteConfiguration,
    RemoteNetwork,
    RemoteTimeout,
    RemoteAuthentication,
    RemoteAuthorization,
    RemoteRateLimited,
    RemoteUnavailable,
    RemoteInvalidResponse,
    RemoteObjectMismatch,
}

impl OffsiteMediaUploadFailureCode {
    pub(crate) const fn as_db_str(self) -> &'static str {
        match self {
            Self::Interrupted => "INTERRUPTED",
            Self::ConfigurationChanged => "CONFIGURATION_CHANGED",
            Self::CredentialsMissing => "CREDENTIALS_MISSING",
            Self::CredentialsUnavailable => "CREDENTIALS_UNAVAILABLE",
            Self::LocalBlobMissing => "LOCAL_BLOB_MISSING",
            Self::LocalBlobInvalid => "LOCAL_BLOB_INVALID",
            Self::LocalBlobUnavailable => "LOCAL_BLOB_UNAVAILABLE",
            Self::RemoteConfiguration => "REMOTE_CONFIGURATION",
            Self::RemoteNetwork => "REMOTE_NETWORK",
            Self::RemoteTimeout => "REMOTE_TIMEOUT",
            Self::RemoteAuthentication => "REMOTE_AUTHENTICATION",
            Self::RemoteAuthorization => "REMOTE_AUTHORIZATION",
            Self::RemoteRateLimited => "REMOTE_RATE_LIMITED",
            Self::RemoteUnavailable => "REMOTE_UNAVAILABLE",
            Self::RemoteInvalidResponse => "REMOTE_INVALID_RESPONSE",
            Self::RemoteObjectMismatch => "REMOTE_OBJECT_MISMATCH",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct OffsiteMediaUploadProgress {
    pub(crate) referenced: u64,
    pub(crate) pending: u64,
    pub(crate) running: u64,
    pub(crate) retry_wait: u64,
    pub(crate) uploaded: u64,
    pub(crate) failed: u64,
    pub(crate) untracked: u64,
    pub(crate) next_attempt_at_ms: Option<i64>,
}

pub(super) fn synchronize_for_saved_config(
    connection: &Connection,
    previous: Option<&OffsiteBackupConfig>,
    saved: &OffsiteBackupConfig,
    now_ms: i64,
) -> Result<()> {
    validate_timestamp(now_ms)?;
    connection.execute(
        "DELETE FROM offsite_media_upload WHERE backup_set_id <> ?1",
        [saved.backup_set_id.as_str()],
    )?;
    let destination_changed = previous.is_some_and(|previous| {
        previous.backup_set_id == saved.backup_set_id && previous.target != saved.target
    });
    if destination_changed {
        connection.execute(
            "DELETE FROM offsite_media_upload WHERE backup_set_id = ?1",
            [saved.backup_set_id.as_str()],
        )?;
    }
    if !saved.enabled {
        connection.execute(
            "UPDATE offsite_media_upload
             SET state = 'RETRY_WAIT',
                 next_attempt_at = ?1,
                 lease_id = NULL,
                 started_at = NULL,
                 uploaded_at = NULL,
                 remote_version = NULL,
                 last_error_code = ?2,
                 updated_at = max(updated_at, ?1)
             WHERE backup_set_id = ?3
               AND state = 'RUNNING'",
            params![
                now_ms,
                OffsiteMediaUploadFailureCode::ConfigurationChanged.as_db_str(),
                saved.backup_set_id.as_str(),
            ],
        )?;
    }
    if saved.enabled {
        seed_referenced(connection, &saved.backup_set_id, now_ms)?;
    }
    Ok(())
}

pub(super) fn reconcile(connection: &mut Connection, now_ms: i64) -> Result<u64> {
    validate_timestamp(now_ms)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(config) = backup_state::load(&transaction)? else {
        transaction.commit()?;
        return Ok(0);
    };
    if !config.enabled {
        transaction.commit()?;
        return Ok(0);
    }
    transaction.execute(
        "DELETE FROM offsite_media_upload
         WHERE backup_set_id = ?1
           AND state <> 'RUNNING'
           AND sha256 NOT IN (
               SELECT sha256
               FROM (
                   SELECT attachment.sha256
                   FROM attachment
                   WHERE attachment.deleted_at IS NULL
                      OR EXISTS (
                           SELECT 1
                           FROM tidbit_revision_attachment AS membership
                           WHERE membership.attachment_id = attachment.id
                      )
                      OR EXISTS (
                           SELECT 1
                           FROM research_run_attachment AS research_membership
                           WHERE research_membership.attachment_id = attachment.id
                      )
                   UNION
                   SELECT image.preview_sha256
                   FROM attachment_image AS image
                   JOIN attachment ON attachment.id = image.attachment_id
                   WHERE attachment.deleted_at IS NULL
                      OR EXISTS (
                           SELECT 1
                           FROM tidbit_revision_attachment AS membership
                           WHERE membership.attachment_id = attachment.id
                      )
                      OR EXISTS (
                           SELECT 1
                           FROM research_run_attachment AS research_membership
                           WHERE research_membership.attachment_id = attachment.id
                      )
               )
           )",
        [config.backup_set_id.as_str()],
    )?;
    let inserted = seed_referenced(&transaction, &config.backup_set_id, now_ms)?;
    transaction.commit()?;
    u64::try_from(inserted).map_err(|_| invalid("seed count is outside the supported range"))
}

pub(super) fn recover_interrupted(connection: &Connection, now_ms: i64) -> Result<u64> {
    validate_timestamp(now_ms)?;
    let recovered = connection.execute(
        "UPDATE offsite_media_upload
         SET state = 'RETRY_WAIT',
             next_attempt_at = ?1,
             lease_id = NULL,
             started_at = NULL,
             uploaded_at = NULL,
             remote_version = NULL,
             last_error_code = ?2,
             updated_at = max(updated_at, ?1)
         WHERE state = 'RUNNING'",
        params![
            now_ms,
            OffsiteMediaUploadFailureCode::Interrupted.as_db_str()
        ],
    )?;
    u64::try_from(recovered)
        .map_err(|_| invalid("recovered upload count is outside the supported range"))
}

pub(super) fn claim_next(
    connection: &mut Connection,
    now_ms: i64,
    lease_id: String,
) -> Result<Option<OffsiteMediaUploadClaim>> {
    validate_timestamp(now_ms)?;
    validate_uuid_v7(&lease_id, "leaseId")?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(config) = backup_state::load_enabled(&transaction)? else {
        transaction.commit()?;
        return Ok(None);
    };
    let select = format!(
        "SELECT upload.sha256, upload.attempt_count
         FROM offsite_media_upload AS upload
         WHERE upload.backup_set_id = ?1
           AND upload.state IN ('PENDING', 'RETRY_WAIT')
           AND upload.next_attempt_at <= ?2
           AND upload.sha256 IN ({REFERENCED_MEDIA_SELECT})
         ORDER BY upload.next_attempt_at, upload.sha256
         LIMIT 1"
    );
    let candidate = transaction
        .query_row(
            &select,
            params![config.backup_set_id.as_str(), now_ms],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let Some((sha256, previous_attempt_count)) = candidate else {
        transaction.commit()?;
        return Ok(None);
    };
    let attempt_count = previous_attempt_count
        .checked_add(1)
        .ok_or_else(|| invalid("upload attempt count overflow"))?;
    let changed = transaction.execute(
        "UPDATE offsite_media_upload
         SET state = 'RUNNING',
             attempt_count = ?1,
             next_attempt_at = NULL,
             lease_id = ?2,
             started_at = ?3,
             uploaded_at = NULL,
             remote_version = NULL,
             last_error_code = NULL,
             updated_at = max(updated_at, ?3)
         WHERE backup_set_id = ?4
           AND sha256 = ?5
           AND state IN ('PENDING', 'RETRY_WAIT')
           AND next_attempt_at <= ?3",
        params![
            attempt_count,
            &lease_id,
            now_ms,
            config.backup_set_id.as_str(),
            &sha256,
        ],
    )?;
    if changed != 1 {
        return Err(invalid("eligible media upload could not be leased"));
    }
    transaction.commit()?;
    Ok(Some(OffsiteMediaUploadClaim {
        config,
        sha256: parse_sha256(sha256)?,
        lease_id,
        attempt_count: u32::try_from(attempt_count)
            .map_err(|_| invalid("upload attempt count exceeds the supported range"))?,
    }))
}

pub(super) fn complete(
    connection: &Connection,
    claim: &OffsiteMediaUploadClaim,
    remote_version: &str,
    now_ms: i64,
) -> Result<bool> {
    validate_timestamp(now_ms)?;
    if remote_version.is_empty()
        || remote_version.len() > 256
        || remote_version.chars().any(char::is_control)
    {
        return Err(invalid("remote object version is invalid"));
    }
    let changed = connection.execute(
        "UPDATE offsite_media_upload
         SET state = 'UPLOADED',
             next_attempt_at = NULL,
             lease_id = NULL,
             started_at = NULL,
             uploaded_at = ?1,
             remote_version = ?2,
             last_error_code = NULL,
             updated_at = max(updated_at, ?1)
         WHERE backup_set_id = ?3
           AND sha256 = ?4
           AND state = 'RUNNING'
           AND lease_id = ?5",
        params![
            now_ms,
            remote_version,
            claim.config.backup_set_id.as_str(),
            claim.sha256.as_bytes().as_slice(),
            &claim.lease_id,
        ],
    )?;
    Ok(changed == 1)
}

pub(super) fn authorize_remote_write(
    connection: &Connection,
    claim: &OffsiteMediaUploadClaim,
) -> Result<bool> {
    let Some(config) = backup_state::load_enabled(connection)? else {
        return Ok(false);
    };
    if config.backup_set_id != claim.config.backup_set_id || config.target != claim.config.target {
        return Ok(false);
    }
    let current_lease = connection.query_row(
        "SELECT EXISTS(
                SELECT 1
                FROM offsite_media_upload
                WHERE backup_set_id = ?1
                  AND sha256 = ?2
                  AND state = 'RUNNING'
                  AND lease_id = ?3
             )",
        params![
            claim.config.backup_set_id.as_str(),
            claim.sha256.as_bytes().as_slice(),
            &claim.lease_id,
        ],
        |row| row.get::<_, bool>(0),
    )?;
    if !current_lease {
        return Ok(false);
    }

    let reference_query = format!(
        "SELECT EXISTS(
            SELECT 1
            FROM ({REFERENCED_MEDIA_SELECT}) AS referenced
            WHERE referenced.sha256 = ?1
         )"
    );
    let retained = connection.query_row(
        &reference_query,
        [claim.sha256.as_bytes().as_slice()],
        |row| row.get::<_, bool>(0),
    )?;
    if retained {
        return Ok(true);
    }

    connection.execute(
        "DELETE FROM offsite_media_upload
         WHERE backup_set_id = ?1
           AND sha256 = ?2
           AND state = 'RUNNING'
           AND lease_id = ?3",
        params![
            claim.config.backup_set_id.as_str(),
            claim.sha256.as_bytes().as_slice(),
            &claim.lease_id,
        ],
    )?;
    Ok(false)
}

pub(super) fn fail(
    connection: &Connection,
    claim: &OffsiteMediaUploadClaim,
    code: OffsiteMediaUploadFailureCode,
    retry_at_ms: Option<i64>,
    now_ms: i64,
) -> Result<bool> {
    validate_timestamp(now_ms)?;
    if retry_at_ms.is_some_and(|retry_at_ms| retry_at_ms < now_ms) {
        return Err(invalid("retry timestamp predates the failure"));
    }
    let state = if retry_at_ms.is_some() {
        "RETRY_WAIT"
    } else {
        "FAILED"
    };
    let changed = connection.execute(
        "UPDATE offsite_media_upload
         SET state = ?1,
             next_attempt_at = ?2,
             lease_id = NULL,
             started_at = NULL,
             uploaded_at = NULL,
             remote_version = NULL,
             last_error_code = ?3,
             updated_at = max(updated_at, ?4)
         WHERE backup_set_id = ?5
           AND sha256 = ?6
           AND state = 'RUNNING'
           AND lease_id = ?7",
        params![
            state,
            retry_at_ms,
            code.as_db_str(),
            now_ms,
            claim.config.backup_set_id.as_str(),
            claim.sha256.as_bytes().as_slice(),
            &claim.lease_id,
        ],
    )?;
    Ok(changed == 1)
}

pub(super) fn progress(connection: &Connection) -> Result<OffsiteMediaUploadProgress> {
    let Some(config) = backup_state::load(connection)? else {
        return Ok(OffsiteMediaUploadProgress::default());
    };
    let query = format!(
        "WITH referenced(sha256) AS ({REFERENCED_MEDIA_SELECT})
         SELECT
             count(*),
             coalesce(sum(upload.state = 'PENDING'), 0),
             coalesce(sum(upload.state = 'RUNNING'), 0),
             coalesce(sum(upload.state = 'RETRY_WAIT'), 0),
             coalesce(sum(upload.state = 'UPLOADED'), 0),
             coalesce(sum(upload.state = 'FAILED'), 0),
             coalesce(sum(upload.state IS NULL), 0),
             min(
                 CASE
                     WHEN upload.state IN ('PENDING', 'RETRY_WAIT')
                     THEN upload.next_attempt_at
                 END
             )
         FROM referenced
         LEFT JOIN offsite_media_upload AS upload
           ON upload.backup_set_id = ?1
          AND upload.sha256 = referenced.sha256"
    );
    let values = connection.query_row(&query, [config.backup_set_id.as_str()], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<i64>>(7)?,
        ))
    })?;
    Ok(OffsiteMediaUploadProgress {
        referenced: count(values.0)?,
        pending: count(values.1)?,
        running: count(values.2)?,
        retry_wait: count(values.3)?,
        uploaded: count(values.4)?,
        failed: count(values.5)?,
        untracked: count(values.6)?,
        next_attempt_at_ms: values.7,
    })
}

fn seed_referenced(
    connection: &Connection,
    backup_set_id: &BackupSetId,
    now_ms: i64,
) -> Result<usize> {
    let insert = format!(
        "INSERT OR IGNORE INTO offsite_media_upload(
            backup_set_id,
            sha256,
            state,
            attempt_count,
            next_attempt_at,
            created_at,
            updated_at
         )
         SELECT ?1, referenced.sha256, 'PENDING', 0, ?2, ?2, ?2
         FROM ({REFERENCED_MEDIA_SELECT}) AS referenced"
    );
    connection
        .execute(&insert, params![backup_set_id.as_str(), now_ms])
        .map_err(Into::into)
}

fn parse_sha256(value: Vec<u8>) -> Result<ContentSha256> {
    let bytes: [u8; 32] = value
        .try_into()
        .map_err(|_| invalid("stored media upload digest must contain 32 bytes"))?;
    Ok(ContentSha256::from_bytes(bytes))
}

fn validate_timestamp(value: i64) -> Result<()> {
    if value < 0 {
        return Err(invalid("timestamp must not be negative"));
    }
    Ok(())
}

fn validate_uuid_v7(value: &str, field: &str) -> Result<()> {
    let valid = uuid::Uuid::parse_str(value).ok().is_some_and(|parsed| {
        parsed.get_version_num() == 7
            && parsed.get_variant() == Variant::RFC4122
            && parsed.hyphenated().to_string() == value
    });
    if !valid {
        return Err(DatabaseError::InvalidInput(format!(
            "{field} must be a lowercase RFC UUIDv7"
        )));
    }
    Ok(())
}

fn count(value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| invalid("media upload count is outside the supported range"))
}

fn invalid(reason: impl Into<String>) -> DatabaseError {
    DatabaseError::Validation {
        kind: "offsite backup media",
        reason: reason.into(),
    }
}

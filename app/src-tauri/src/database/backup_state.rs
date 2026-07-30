use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::backup::domain::{
    BackupProvider, BackupSetId, R2AccountId, R2BucketName, R2Jurisdiction, R2Target,
    ReplicaEpochId,
};

use super::{backup_media, DatabaseError, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OffsiteBackupConfig {
    pub(crate) revision: i64,
    pub(crate) backup_set_id: BackupSetId,
    pub(crate) replica_epoch_id: ReplicaEpochId,
    pub(crate) enabled: bool,
    pub(crate) provider: BackupProvider,
    pub(crate) target: R2Target,
    pub(crate) created_at_ms: i64,
    pub(crate) updated_at_ms: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct SaveOffsiteBackupConfigInput {
    pub(crate) expected_revision: i64,
    pub(crate) backup_set_id: BackupSetId,
    pub(crate) replica_epoch_id: ReplicaEpochId,
    pub(crate) enabled: bool,
    pub(crate) target: R2Target,
    pub(crate) now_ms: i64,
}

pub(super) fn load(connection: &Connection) -> Result<Option<OffsiteBackupConfig>> {
    let stored = connection
        .query_row(
            "SELECT
                revision,
                backup_set_id,
                replica_epoch_id,
                enabled,
                provider,
                jurisdiction,
                account_id,
                bucket,
                created_at,
                updated_at
             FROM offsite_backup_config
             WHERE singleton_id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            },
        )
        .optional()?;
    stored.map(parse_stored).transpose()
}

pub(super) fn load_enabled(connection: &Connection) -> Result<Option<OffsiteBackupConfig>> {
    Ok(load(connection)?.filter(|config| config.enabled))
}

pub(super) fn is_current_enabled(
    connection: &Connection,
    expected: &OffsiteBackupConfig,
) -> Result<bool> {
    Ok(load_enabled(connection)?.as_ref() == Some(expected))
}

pub(super) fn save(
    connection: &mut Connection,
    input: SaveOffsiteBackupConfigInput,
) -> Result<OffsiteBackupConfig> {
    if input.expected_revision < 0 {
        return Err(invalid("expected revision must not be negative"));
    }
    if input.now_ms < 0 {
        return Err(invalid("timestamp must not be negative"));
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = load(&transaction)?;
    let previous = current.clone();
    let cleanup_pending = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM offsite_credential_cleanup
            WHERE backup_set_id = ?1
         )",
        [input.backup_set_id.as_str()],
        |row| row.get::<_, bool>(0),
    )?;
    if cleanup_pending {
        return Err(DatabaseError::OffsiteBackupSetPendingCredentialCleanup {
            backup_set_id: input.backup_set_id.to_string(),
        });
    }
    let retired_backup_set_id = current
        .as_ref()
        .filter(|config| config.backup_set_id != input.backup_set_id)
        .map(|config| config.backup_set_id.clone());

    match current {
        None => {
            if input.expected_revision != 0 {
                return Err(DatabaseError::StaleOffsiteBackupConfig);
            }
            transaction.execute(
                "INSERT INTO offsite_backup_config (
                    singleton_id,
                    revision,
                    backup_set_id,
                    replica_epoch_id,
                    enabled,
                    provider,
                    jurisdiction,
                    account_id,
                    bucket,
                    created_at,
                    updated_at
                 ) VALUES (1, 1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    input.backup_set_id.as_str(),
                    input.replica_epoch_id.as_str(),
                    input.enabled,
                    BackupProvider::R2.as_db_str(),
                    input.target.jurisdiction.as_db_str(),
                    input.target.account_id.as_str(),
                    input.target.bucket.as_str(),
                    input.now_ms,
                ],
            )?;
        }
        Some(current) => {
            if input.now_ms < current.created_at_ms {
                return Err(invalid("timestamp predates the stored configuration"));
            }
            let changed = transaction.execute(
                "UPDATE offsite_backup_config
                 SET revision = revision + 1,
                     backup_set_id = ?1,
                     replica_epoch_id = ?2,
                     enabled = ?3,
                     provider = ?4,
                     jurisdiction = ?5,
                     account_id = ?6,
                     bucket = ?7,
                     updated_at = ?8
                 WHERE singleton_id = 1 AND revision = ?9",
                params![
                    input.backup_set_id.as_str(),
                    input.replica_epoch_id.as_str(),
                    input.enabled,
                    BackupProvider::R2.as_db_str(),
                    input.target.jurisdiction.as_db_str(),
                    input.target.account_id.as_str(),
                    input.target.bucket.as_str(),
                    input.now_ms,
                    input.expected_revision,
                ],
            )?;
            if changed != 1 {
                return Err(DatabaseError::StaleOffsiteBackupConfig);
            }
        }
    }

    if let Some(retired_backup_set_id) = retired_backup_set_id {
        transaction.execute(
            "INSERT OR IGNORE INTO offsite_credential_cleanup(backup_set_id, created_at)
             VALUES(?1, ?2)",
            params![retired_backup_set_id.as_str(), input.now_ms],
        )?;
    }
    let saved =
        load(&transaction)?.ok_or_else(|| invalid("saved configuration could not be loaded"))?;
    backup_media::synchronize_for_saved_config(
        &transaction,
        previous.as_ref(),
        &saved,
        input.now_ms,
    )?;
    transaction.commit()?;
    Ok(saved)
}

pub(super) fn load_credential_cleanup(connection: &Connection) -> Result<Vec<BackupSetId>> {
    let mut statement = connection.prepare(
        "SELECT backup_set_id
         FROM offsite_credential_cleanup
         ORDER BY created_at, backup_set_id",
    )?;
    let cleanup = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .map(|value| {
            BackupSetId::parse(value?)
                .map_err(|error| invalid(format!("invalid stored credential cleanup: {error}")))
        })
        .collect();
    cleanup
}

pub(super) fn complete_credential_cleanup(
    connection: &mut Connection,
    backup_set_id: &BackupSetId,
) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let pending = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM offsite_credential_cleanup
            WHERE backup_set_id = ?1
         )",
        [backup_set_id.as_str()],
        |row| row.get::<_, bool>(0),
    )?;
    if !pending {
        transaction.commit()?;
        return Ok(());
    }
    let active = transaction
        .query_row(
            "SELECT backup_set_id
             FROM offsite_backup_config
             WHERE singleton_id = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if active.as_deref() == Some(backup_set_id.as_str()) {
        return Err(DatabaseError::OffsiteCredentialCleanupNotAuthorized {
            backup_set_id: backup_set_id.to_string(),
        });
    }
    transaction.execute(
        "DELETE FROM offsite_credential_cleanup WHERE backup_set_id = ?1",
        [backup_set_id.as_str()],
    )?;
    transaction.commit()?;
    Ok(())
}

fn parse_stored(
    stored: (
        i64,
        String,
        String,
        bool,
        String,
        String,
        String,
        String,
        i64,
        i64,
    ),
) -> Result<OffsiteBackupConfig> {
    let (
        revision,
        backup_set_id,
        replica_epoch_id,
        enabled,
        provider,
        jurisdiction,
        account_id,
        bucket,
        created_at_ms,
        updated_at_ms,
    ) = stored;
    Ok(OffsiteBackupConfig {
        revision,
        backup_set_id: BackupSetId::parse(backup_set_id)
            .map_err(|error| invalid(error.to_string()))?,
        replica_epoch_id: ReplicaEpochId::parse(replica_epoch_id)
            .map_err(|error| invalid(error.to_string()))?,
        enabled,
        provider: BackupProvider::from_db(&provider).map_err(|error| invalid(error.to_string()))?,
        target: R2Target {
            account_id: R2AccountId::parse(account_id)
                .map_err(|error| invalid(error.to_string()))?,
            jurisdiction: R2Jurisdiction::from_db(&jurisdiction)
                .map_err(|error| invalid(error.to_string()))?,
            bucket: R2BucketName::parse(bucket).map_err(|error| invalid(error.to_string()))?,
        },
        created_at_ms,
        updated_at_ms,
    })
}

fn invalid(reason: impl Into<String>) -> DatabaseError {
    DatabaseError::InvalidOffsiteBackupConfig(reason.into())
}

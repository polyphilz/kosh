use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::backup::domain::{
    BackupProvider, BackupSetId, BackupWriterId, R2AccountId, R2BucketName, R2Jurisdiction,
    R2Target, ReplicaEpochId,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CredentialIntentAction {
    Reuse,
    Replace,
}

impl CredentialIntentAction {
    fn as_db_str(self) -> &'static str {
        match self {
            Self::Reuse => "REUSE",
            Self::Replace => "REPLACE",
        }
    }

    fn from_db(value: &str) -> Result<Self> {
        match value {
            "REUSE" => Ok(Self::Reuse),
            "REPLACE" => Ok(Self::Replace),
            _ => Err(invalid("invalid stored credential intent action")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OffsiteOperationState {
    Pending,
    Committed,
}

impl OffsiteOperationState {
    fn from_db(value: &str) -> Result<Self> {
        match value {
            "PENDING" => Ok(Self::Pending),
            "COMMITTED" => Ok(Self::Committed),
            _ => Err(invalid("invalid stored off-site operation state")),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct OffsiteBackupConfigIntent {
    pub(crate) operation_id: String,
    pub(crate) proposed: SaveOffsiteBackupConfigInput,
    pub(crate) credential_action: CredentialIntentAction,
    pub(crate) state: OffsiteOperationState,
}

#[derive(Clone, Debug)]
pub(crate) struct BeginOffsiteBackupConfigIntentInput {
    pub(crate) operation_id: String,
    pub(crate) proposed: SaveOffsiteBackupConfigInput,
    pub(crate) credential_action: CredentialIntentAction,
}

#[derive(Clone, Debug)]
pub(crate) struct OffsiteBackupTakeoverIntent {
    pub(crate) operation_id: String,
    pub(crate) expected_revision: i64,
    pub(crate) backup_set_id: BackupSetId,
    pub(crate) previous_replica_epoch_id: ReplicaEpochId,
    pub(crate) next_replica_epoch_id: ReplicaEpochId,
    pub(crate) expected_owner_replica_epoch_id: ReplicaEpochId,
    pub(crate) expected_owner_writer_id: BackupWriterId,
    pub(crate) expected_owner_version: String,
    pub(crate) next_writer_id: BackupWriterId,
    pub(crate) created_at_ms: i64,
}

pub(crate) type BeginOffsiteBackupTakeoverIntentInput = OffsiteBackupTakeoverIntent;

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
    if operation_pending(connection)? {
        return Ok(None);
    }
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
    if operation_pending(connection)? {
        return Err(DatabaseError::OffsiteBackupOperationPending);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let saved = apply_config(&transaction, input)?;
    transaction.commit()?;
    Ok(saved)
}

fn apply_config(
    transaction: &rusqlite::Transaction<'_>,
    input: SaveOffsiteBackupConfigInput,
) -> Result<OffsiteBackupConfig> {
    if input.expected_revision < 0 {
        return Err(invalid("expected revision must not be negative"));
    }
    if input.now_ms < 0 {
        return Err(invalid("timestamp must not be negative"));
    }
    let current = load(transaction)?;
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
        load(transaction)?.ok_or_else(|| invalid("saved configuration could not be loaded"))?;
    backup_media::synchronize_for_saved_config(
        transaction,
        previous.as_ref(),
        &saved,
        input.now_ms,
    )?;
    Ok(saved)
}

pub(super) fn begin_config_intent(
    connection: &mut Connection,
    mut input: BeginOffsiteBackupConfigIntentInput,
) -> Result<()> {
    validate_operation_id(&input.operation_id)?;
    if input.proposed.enabled {
        return Err(invalid("a recovery target change must remain disabled"));
    }
    if input.proposed.expected_revision < 0 || input.proposed.now_ms < 0 {
        return Err(invalid("invalid recovery target operation metadata"));
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_no_operation(&transaction)?;
    let current = load(&transaction)?;
    if current.as_ref().map_or(0, |config| config.revision) != input.proposed.expected_revision {
        return Err(DatabaseError::StaleOffsiteBackupConfig);
    }
    if current.as_ref().is_some_and(|config| config.enabled) {
        return Err(DatabaseError::OffsiteBackupMustBeDisabled);
    }
    if let Some(current) = &current {
        input.proposed.now_ms = input.proposed.now_ms.max(current.updated_at_ms);
    }
    ensure_backup_set_not_queued(&transaction, &input.proposed.backup_set_id)?;
    transaction.execute(
        "INSERT INTO offsite_backup_config_intent (
            singleton_id,
            operation_id,
            expected_revision,
            backup_set_id,
            replica_epoch_id,
            provider,
            jurisdiction,
            account_id,
            bucket,
            credential_action,
            state,
            created_at,
            updated_at
         ) VALUES (1, ?1, ?2, ?3, ?4, 'R2', ?5, ?6, ?7, ?8, 'PENDING', ?9, ?9)",
        params![
            input.operation_id,
            input.proposed.expected_revision,
            input.proposed.backup_set_id.as_str(),
            input.proposed.replica_epoch_id.as_str(),
            input.proposed.target.jurisdiction.as_db_str(),
            input.proposed.target.account_id.as_str(),
            input.proposed.target.bucket.as_str(),
            input.credential_action.as_db_str(),
            input.proposed.now_ms,
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn load_config_intent(
    connection: &Connection,
) -> Result<Option<OffsiteBackupConfigIntent>> {
    let stored = connection
        .query_row(
            "SELECT
                operation_id,
                expected_revision,
                backup_set_id,
                replica_epoch_id,
                jurisdiction,
                account_id,
                bucket,
                credential_action,
                state,
                created_at
             FROM offsite_backup_config_intent
             WHERE singleton_id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            },
        )
        .optional()?;
    stored
        .map(
            |(
                operation_id,
                expected_revision,
                backup_set_id,
                replica_epoch_id,
                jurisdiction,
                account_id,
                bucket,
                credential_action,
                state,
                created_at_ms,
            )| {
                validate_operation_id(&operation_id)?;
                Ok(OffsiteBackupConfigIntent {
                    operation_id,
                    proposed: SaveOffsiteBackupConfigInput {
                        expected_revision,
                        backup_set_id: BackupSetId::parse(backup_set_id)
                            .map_err(|error| invalid(error.to_string()))?,
                        replica_epoch_id: ReplicaEpochId::parse(replica_epoch_id)
                            .map_err(|error| invalid(error.to_string()))?,
                        enabled: false,
                        target: R2Target {
                            account_id: R2AccountId::parse(account_id)
                                .map_err(|error| invalid(error.to_string()))?,
                            jurisdiction: R2Jurisdiction::from_db(&jurisdiction)
                                .map_err(|error| invalid(error.to_string()))?,
                            bucket: R2BucketName::parse(bucket)
                                .map_err(|error| invalid(error.to_string()))?,
                        },
                        now_ms: created_at_ms,
                    },
                    credential_action: CredentialIntentAction::from_db(&credential_action)?,
                    state: OffsiteOperationState::from_db(&state)?,
                })
            },
        )
        .transpose()
}

pub(super) fn commit_config_intent(
    connection: &mut Connection,
    operation_id: &str,
) -> Result<OffsiteBackupConfig> {
    validate_operation_id(operation_id)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let intent =
        load_config_intent(&transaction)?.ok_or(DatabaseError::OffsiteBackupOperationNotFound)?;
    if intent.operation_id != operation_id {
        return Err(DatabaseError::OffsiteBackupOperationNotFound);
    }
    if intent.state == OffsiteOperationState::Committed {
        return load(&transaction)?.ok_or_else(|| invalid("committed recovery target is missing"));
    }
    let saved = apply_config(&transaction, intent.proposed)?;
    transaction.execute(
        "UPDATE offsite_backup_config_intent
         SET state = 'COMMITTED', updated_at = MAX(updated_at, ?2)
         WHERE singleton_id = 1 AND operation_id = ?1 AND state = 'PENDING'",
        params![operation_id, saved.updated_at_ms],
    )?;
    transaction.commit()?;
    Ok(saved)
}

pub(super) fn complete_config_intent(
    connection: &mut Connection,
    operation_id: &str,
) -> Result<()> {
    validate_operation_id(operation_id)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let intent =
        load_config_intent(&transaction)?.ok_or(DatabaseError::OffsiteBackupOperationNotFound)?;
    if intent.operation_id != operation_id || intent.state != OffsiteOperationState::Committed {
        return Err(DatabaseError::OffsiteBackupOperationNotFound);
    }
    let current =
        load(&transaction)?.ok_or_else(|| invalid("committed recovery target is missing"))?;
    if current.backup_set_id != intent.proposed.backup_set_id
        || current.replica_epoch_id != intent.proposed.replica_epoch_id
        || current.target != intent.proposed.target
        || current.enabled
    {
        return Err(invalid(
            "committed recovery target does not match its intent",
        ));
    }
    transaction.execute(
        "DELETE FROM offsite_backup_config_intent
         WHERE singleton_id = 1 AND operation_id = ?1",
        [operation_id],
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn abort_config_intent(connection: &mut Connection, operation_id: &str) -> Result<()> {
    validate_operation_id(operation_id)?;
    let changed = connection.execute(
        "DELETE FROM offsite_backup_config_intent
         WHERE singleton_id = 1 AND operation_id = ?1 AND state = 'PENDING'",
        [operation_id],
    )?;
    if changed != 1 {
        return Err(DatabaseError::OffsiteBackupOperationNotFound);
    }
    Ok(())
}

pub(super) fn begin_takeover_intent(
    connection: &mut Connection,
    mut input: BeginOffsiteBackupTakeoverIntentInput,
) -> Result<()> {
    validate_takeover_intent(&input)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_no_operation(&transaction)?;
    let current = load(&transaction)?.ok_or(DatabaseError::StaleOffsiteBackupConfig)?;
    if current.revision != input.expected_revision
        || current.backup_set_id != input.backup_set_id
        || current.replica_epoch_id != input.previous_replica_epoch_id
    {
        return Err(DatabaseError::StaleOffsiteBackupConfig);
    }
    if current.enabled {
        return Err(DatabaseError::OffsiteBackupMustBeDisabled);
    }
    input.created_at_ms = input.created_at_ms.max(current.updated_at_ms);
    transaction.execute(
        "INSERT INTO offsite_backup_takeover_intent (
            singleton_id,
            operation_id,
            expected_revision,
            backup_set_id,
            previous_replica_epoch_id,
            next_replica_epoch_id,
            expected_owner_replica_epoch_id,
            expected_owner_writer_id,
            expected_owner_version,
            next_writer_id,
            state,
            created_at,
            updated_at
         ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'PENDING', ?10, ?10)",
        params![
            input.operation_id,
            input.expected_revision,
            input.backup_set_id.as_str(),
            input.previous_replica_epoch_id.as_str(),
            input.next_replica_epoch_id.as_str(),
            input.expected_owner_replica_epoch_id.as_str(),
            input.expected_owner_writer_id.as_str(),
            input.expected_owner_version,
            input.next_writer_id.as_str(),
            input.created_at_ms,
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn load_takeover_intent(
    connection: &Connection,
) -> Result<Option<OffsiteBackupTakeoverIntent>> {
    let stored = connection
        .query_row(
            "SELECT
                operation_id,
                expected_revision,
                backup_set_id,
                previous_replica_epoch_id,
                next_replica_epoch_id,
                expected_owner_replica_epoch_id,
                expected_owner_writer_id,
                expected_owner_version,
                next_writer_id,
                created_at
             FROM offsite_backup_takeover_intent
             WHERE singleton_id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            },
        )
        .optional()?;
    stored
        .map(
            |(
                operation_id,
                expected_revision,
                backup_set_id,
                previous_replica_epoch_id,
                next_replica_epoch_id,
                expected_owner_replica_epoch_id,
                expected_owner_writer_id,
                expected_owner_version,
                next_writer_id,
                created_at_ms,
            )| {
                let intent = OffsiteBackupTakeoverIntent {
                    operation_id,
                    expected_revision,
                    backup_set_id: BackupSetId::parse(backup_set_id)
                        .map_err(|error| invalid(error.to_string()))?,
                    previous_replica_epoch_id: ReplicaEpochId::parse(previous_replica_epoch_id)
                        .map_err(|error| invalid(error.to_string()))?,
                    next_replica_epoch_id: ReplicaEpochId::parse(next_replica_epoch_id)
                        .map_err(|error| invalid(error.to_string()))?,
                    expected_owner_replica_epoch_id: ReplicaEpochId::parse(
                        expected_owner_replica_epoch_id,
                    )
                    .map_err(|error| invalid(error.to_string()))?,
                    expected_owner_writer_id: BackupWriterId::parse(expected_owner_writer_id)
                        .map_err(|error| invalid(error.to_string()))?,
                    expected_owner_version,
                    next_writer_id: BackupWriterId::parse(next_writer_id)
                        .map_err(|error| invalid(error.to_string()))?,
                    created_at_ms,
                };
                validate_takeover_intent(&intent)?;
                Ok(intent)
            },
        )
        .transpose()
}

pub(super) fn commit_takeover_intent(
    connection: &mut Connection,
    operation_id: &str,
) -> Result<OffsiteBackupConfig> {
    validate_operation_id(operation_id)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let intent =
        load_takeover_intent(&transaction)?.ok_or(DatabaseError::OffsiteBackupOperationNotFound)?;
    if intent.operation_id != operation_id {
        return Err(DatabaseError::OffsiteBackupOperationNotFound);
    }
    let current = load(&transaction)?.ok_or(DatabaseError::StaleOffsiteBackupConfig)?;
    let saved = apply_config(
        &transaction,
        SaveOffsiteBackupConfigInput {
            expected_revision: intent.expected_revision,
            backup_set_id: intent.backup_set_id,
            replica_epoch_id: intent.next_replica_epoch_id,
            enabled: false,
            target: current.target,
            now_ms: intent.created_at_ms,
        },
    )?;
    transaction.execute(
        "DELETE FROM offsite_backup_takeover_intent
         WHERE singleton_id = 1 AND operation_id = ?1",
        [operation_id],
    )?;
    transaction.commit()?;
    Ok(saved)
}

pub(super) fn abort_takeover_intent(connection: &mut Connection, operation_id: &str) -> Result<()> {
    validate_operation_id(operation_id)?;
    let changed = connection.execute(
        "DELETE FROM offsite_backup_takeover_intent
         WHERE singleton_id = 1 AND operation_id = ?1 AND state = 'PENDING'",
        [operation_id],
    )?;
    if changed != 1 {
        return Err(DatabaseError::OffsiteBackupOperationNotFound);
    }
    Ok(())
}

fn operation_pending(connection: &Connection) -> Result<bool> {
    connection
        .query_row(
            "SELECT
                EXISTS(SELECT 1 FROM offsite_backup_config_intent)
                OR EXISTS(SELECT 1 FROM offsite_backup_takeover_intent)",
            [],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn ensure_no_operation(connection: &Connection) -> Result<()> {
    if operation_pending(connection)? {
        return Err(DatabaseError::OffsiteBackupOperationPending);
    }
    Ok(())
}

fn ensure_backup_set_not_queued(
    connection: &Connection,
    backup_set_id: &BackupSetId,
) -> Result<()> {
    let cleanup_pending = connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM offsite_credential_cleanup WHERE backup_set_id = ?1
         )",
        [backup_set_id.as_str()],
        |row| row.get::<_, bool>(0),
    )?;
    if cleanup_pending {
        return Err(DatabaseError::OffsiteBackupSetPendingCredentialCleanup {
            backup_set_id: backup_set_id.to_string(),
        });
    }
    Ok(())
}

fn validate_operation_id(operation_id: &str) -> Result<()> {
    let parsed = uuid::Uuid::parse_str(operation_id)
        .map_err(|_| invalid("operation ID is not a canonical UUIDv7"))?;
    if parsed.get_version_num() != 7 || parsed.to_string() != operation_id {
        return Err(invalid("operation ID is not a canonical UUIDv7"));
    }
    Ok(())
}

fn validate_takeover_intent(input: &OffsiteBackupTakeoverIntent) -> Result<()> {
    validate_operation_id(&input.operation_id)?;
    if input.expected_revision <= 0
        || input.created_at_ms < 0
        || input.previous_replica_epoch_id == input.next_replica_epoch_id
        || input.expected_owner_version.is_empty()
        || input.expected_owner_version.len() > 256
        || input.expected_owner_version.chars().any(char::is_control)
    {
        return Err(invalid("invalid backup takeover intent"));
    }
    Ok(())
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

//! Settings-facing disaster-recovery commands.
//!
//! Secret fields are accepted only as write-only command inputs. They are
//! moved immediately into zeroizing credential values, never implement
//! `Debug` or `Serialize`, and never appear in a response or log message.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tauri::State;
use zeroize::Zeroize;

use crate::{
    database::{
        available_storage_bytes, BeginOffsiteBackupConfigIntentInput,
        BeginOffsiteBackupTakeoverIntentInput, CredentialIntentAction, DatabaseClient,
        DatabaseError, DatabasePaths, OffsiteBackupConfig, OffsiteBackupConfigIntent,
        OffsiteBackupTakeoverIntent, OffsiteOperationState, SaveOffsiteBackupConfigInput,
    },
    runtime::RuntimeState,
};

use super::{
    checkpoint::CheckpointBackupStatus,
    credentials::{CredentialError, CredentialStore, MacOsKeychainCredentialStore, R2Credentials},
    domain::{
        BackupSetId, BackupWriterId, CheckpointBackupPhase, CheckpointErrorCode, CheckpointId,
        R2AccountId, R2BucketName, R2Jurisdiction, R2Target, ReplicaEpochId,
    },
    litestream::{
        CommandLitestreamRestore, LitestreamError, LitestreamRuntimePaths, VerifiedLitestreamBinary,
    },
    litestream_runtime::{RelationalBackupPhase, RelationalBackupStatus},
    object_store::{ObjectStoreError, ObjectStoreErrorCode, R2ObjectStore},
    owner::{inspect_remote_owner, resume_remote_takeover, RemoteOwnerError, RemoteOwnerSnapshot},
    probe::{verify_object_store, ObjectStoreProbeError},
    restore::{
        discover_checkpoints, drill_checkpoint, preview_checkpoint, RemoteCheckpoint, RestoreError,
        RestorePreview,
    },
    writer_identity::{
        MacOsInstallationWriterIdentity, WriterIdentityError, WriterIdentityProvider,
    },
};

const RESTORE_COMMAND_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const EXACT_TRANSACTION_RETENTION_DAYS: u32 = 30;
const TAKEOVER_CONFIRMATION: &str = "TAKE OVER";
const RESTORE_DRILL_STORAGE_HEADROOM_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum BackupCredentialState {
    Stored,
    Missing,
    Unavailable,
    Invalid,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupConfigView {
    revision: i64,
    backup_set_id: String,
    replica_epoch_id: String,
    enabled: bool,
    provider: &'static str,
    jurisdiction: R2Jurisdiction,
    account_id: String,
    bucket: String,
    created_at_ms: i64,
    updated_at_ms: i64,
}

impl From<&OffsiteBackupConfig> for BackupConfigView {
    fn from(config: &OffsiteBackupConfig) -> Self {
        Self {
            revision: config.revision,
            backup_set_id: config.backup_set_id.to_string(),
            replica_epoch_id: config.replica_epoch_id.to_string(),
            enabled: config.enabled,
            provider: "R2",
            jurisdiction: config.target.jurisdiction,
            account_id: config.target.account_id.as_str().to_owned(),
            bucket: config.target.bucket.as_str().to_owned(),
            created_at_ms: config.created_at_ms,
            updated_at_ms: config.updated_at_ms,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaBackupStatus {
    referenced: u64,
    pending: u64,
    running: u64,
    retry_wait: u64,
    uploaded: u64,
    failed: u64,
    untracked: u64,
    next_attempt_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupRetentionView {
    exact_transaction_days: u32,
    checkpoint_policy: &'static str,
    media_policy: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupSettingsSnapshot {
    config: Option<BackupConfigView>,
    credential_state: BackupCredentialState,
    credential_cleanup_pending: bool,
    relational: RelationalBackupStatus,
    media: MediaBackupStatus,
    checkpoint: CheckpointBackupStatus,
    retention: BackupRetentionView,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConfigureBackupInput {
    expected_revision: i64,
    backup_set_id: Option<String>,
    account_id: String,
    jurisdiction: R2Jurisdiction,
    bucket: String,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
}

impl Drop for ConfigureBackupInput {
    fn drop(&mut self) {
        zeroize_optional(&mut self.access_key_id);
        zeroize_optional(&mut self.secret_access_key);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TestBackupConnectionInput {
    backup_set_id: Option<String>,
    account_id: String,
    jurisdiction: R2Jurisdiction,
    bucket: String,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
}

impl Drop for TestBackupConnectionInput {
    fn drop(&mut self) {
        zeroize_optional(&mut self.access_key_id);
        zeroize_optional(&mut self.secret_access_key);
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SetBackupEnabledInput {
    expected_revision: i64,
    enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupConnectionTestResult {
    verified: bool,
    cleanup_complete: bool,
    tested_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteCheckpointView {
    checkpoint_id: String,
    replica_epoch_id: String,
    created_at: String,
    kosh_version: String,
    content_revision: u64,
    referenced_media_count: u64,
    referenced_media_bytes: u64,
}

impl From<&RemoteCheckpoint> for RemoteCheckpointView {
    fn from(checkpoint: &RemoteCheckpoint) -> Self {
        Self {
            checkpoint_id: checkpoint.checkpoint_id().to_string(),
            replica_epoch_id: checkpoint.replica_epoch_id().to_string(),
            created_at: checkpoint.created_at().to_owned(),
            kosh_version: checkpoint.kosh_version().to_owned(),
            content_revision: checkpoint.content_revision(),
            referenced_media_count: checkpoint.referenced_hash_count(),
            referenced_media_bytes: checkpoint.referenced_total_bytes(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RestoreCheckpointInput {
    checkpoint_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RestoreOwnerView {
    backup_set_id: String,
    replica_epoch_id: String,
    writer_id: String,
    version: String,
    is_current_installation: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RestorePreviewView {
    checkpoint: RemoteCheckpointView,
    owner: RestoreOwnerView,
    plan_file_count: u64,
    plan_total_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RestoreDrillView {
    checkpoint_id: String,
    restored_media_count: u64,
    restored_media_bytes: u64,
    completed_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TakeOverBackupInput {
    expected_revision: i64,
    expected_owner_backup_set_id: String,
    expected_owner_replica_epoch_id: String,
    expected_owner_writer_id: String,
    expected_owner_version: String,
    confirmation: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum BackupCommandErrorCode {
    InvalidInput,
    StaleConfiguration,
    NotConfigured,
    CredentialsMissing,
    KeychainUnavailable,
    AuthenticationRejected,
    AuthorizationRejected,
    NetworkUnavailable,
    ServiceUnavailable,
    ConnectionFailed,
    BackupMustBeDisabled,
    OwnerChanged,
    CheckpointFailed,
    RestoreUnavailable,
    RestoreFailed,
    DatabaseUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupCommandError {
    code: BackupCommandErrorCode,
    message: String,
}

pub(crate) fn map_checkpoint_command_error(code: CheckpointErrorCode) -> BackupCommandError {
    let (public_code, message) = match code {
        CheckpointErrorCode::Network | CheckpointErrorCode::NetworkTimeout => (
            BackupCommandErrorCode::NetworkUnavailable,
            "R2 could not be reached. Local capture and search still work.",
        ),
        CheckpointErrorCode::RateLimited | CheckpointErrorCode::ServiceUnavailable => (
            BackupCommandErrorCode::ServiceUnavailable,
            "R2 is temporarily unavailable. Try again later.",
        ),
        CheckpointErrorCode::CredentialsMissing => (
            BackupCommandErrorCode::CredentialsMissing,
            "R2 credentials are not stored for this backup set.",
        ),
        CheckpointErrorCode::KeychainUnavailable => (
            BackupCommandErrorCode::KeychainUnavailable,
            "macOS Keychain is unavailable. Local capture and search still work.",
        ),
        CheckpointErrorCode::InvalidConfiguration => (
            BackupCommandErrorCode::NotConfigured,
            "Turn on offsite recovery before creating a recovery point.",
        ),
        CheckpointErrorCode::AuthenticationRejected => (
            BackupCommandErrorCode::AuthenticationRejected,
            "Cloudflare rejected the R2 credentials.",
        ),
        CheckpointErrorCode::AuthorizationRejected => (
            BackupCommandErrorCode::AuthorizationRejected,
            "The R2 token cannot publish a complete recovery point in this bucket.",
        ),
        CheckpointErrorCode::OwnerConflict | CheckpointErrorCode::OwnerInvalid => (
            BackupCommandErrorCode::OwnerChanged,
            "Another installation owns this backup set. Turn backup off and preview a recovery point before takeover.",
        ),
        CheckpointErrorCode::WorkerUnavailable => (
            BackupCommandErrorCode::CheckpointFailed,
            "The background backup worker is unavailable. Local capture and search still work.",
        ),
        CheckpointErrorCode::LitestreamUnavailable
        | CheckpointErrorCode::FenceTimeout
        | CheckpointErrorCode::ReplicaBehind => (
            BackupCommandErrorCode::CheckpointFailed,
            "Relational backup has not reached a verifiable remote transaction yet. Try again.",
        ),
        CheckpointErrorCode::ImmutableObjectConflict
        | CheckpointErrorCode::LocalMediaMissing
        | CheckpointErrorCode::MalformedManifest
        | CheckpointErrorCode::RemoteMediaMissing
        | CheckpointErrorCode::RemoteMediaCorrupt => (
            BackupCommandErrorCode::CheckpointFailed,
            "Kosh could not verify a complete recovery point. Review backup health and try again.",
        ),
    };
    BackupCommandError::new(public_code, message)
}

type BackupCommandResult<T> = Result<T, BackupCommandError>;

impl BackupCommandError {
    fn new(code: BackupCommandErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new(BackupCommandErrorCode::InvalidInput, message)
    }

    fn worker() -> Self {
        Self::new(
            BackupCommandErrorCode::DatabaseUnavailable,
            "The backup operation could not run. Try again.",
        )
    }
}

#[derive(Clone)]
struct BackupContext {
    client: DatabaseClient,
    paths: DatabasePaths,
    data_root: PathBuf,
    resource_dir: Option<PathBuf>,
    gate: Arc<Mutex<()>>,
}

impl BackupContext {
    fn from_state(state: &RuntimeState) -> Self {
        Self {
            client: state.database_client(),
            paths: state.database_paths().clone(),
            data_root: state.data_dir(),
            resource_dir: state.resource_dir(),
            gate: state.backup_operations_gate(),
        }
    }
}

#[tauri::command]
pub(crate) async fn load_backup_settings(
    state: State<'_, RuntimeState>,
) -> BackupCommandResult<BackupSettingsSnapshot> {
    let context = BackupContext::from_state(&state);
    let relational = state.relational_backup_status();
    let checkpoint = state.checkpoint_backup_status();
    let (snapshot, changed) = run_blocking(move || {
        let _guard = lock_gate(&context.gate);
        let changed = match reconcile_pending_backup_operations(&context.client, &context.data_root)
        {
            Ok(changed) => changed,
            Err(error) => {
                log::warn!("durable off-site backup operation remains pending: {error:?}");
                false
            }
        };
        clean_retired_credentials(&context.client);
        load_snapshot(&context.client, relational, checkpoint).map(|snapshot| (snapshot, changed))
    })
    .await?;
    if changed {
        state.reload_backup_configuration();
    }
    Ok(snapshot)
}

#[tauri::command]
pub(crate) async fn test_backup_connection(
    state: State<'_, RuntimeState>,
    input: TestBackupConnectionInput,
) -> BackupCommandResult<BackupConnectionTestResult> {
    let context = BackupContext::from_state(&state);
    let tested_at_ms = state.now_ms();
    run_blocking(move || test_connection_blocking(context, input, tested_at_ms)).await
}

#[tauri::command]
pub(crate) async fn configure_backup(
    state: State<'_, RuntimeState>,
    input: ConfigureBackupInput,
) -> BackupCommandResult<BackupSettingsSnapshot> {
    let context = BackupContext::from_state(&state);
    let now_ms = state.now_ms();
    run_blocking(move || configure_blocking(context, input, now_ms)).await?;
    state.reload_backup_configuration();
    load_backup_settings(state).await
}

#[tauri::command]
pub(crate) async fn set_backup_enabled(
    state: State<'_, RuntimeState>,
    input: SetBackupEnabledInput,
) -> BackupCommandResult<BackupSettingsSnapshot> {
    let context = BackupContext::from_state(&state);
    let now_ms = state.now_ms();
    run_blocking(move || set_enabled_blocking(context, input, now_ms)).await?;
    state.reload_backup_configuration();
    load_backup_settings(state).await
}

#[tauri::command]
pub(crate) async fn list_backup_checkpoints(
    state: State<'_, RuntimeState>,
) -> BackupCommandResult<Vec<RemoteCheckpointView>> {
    let context = BackupContext::from_state(&state);
    run_blocking(move || {
        let _guard = lock_gate(&context.gate);
        let (config, _credentials, store) = open_saved_store(&context.client)?;
        let keyspace = config.target.keyspace(&config.backup_set_id);
        discover_checkpoints(&store, &keyspace, &config.backup_set_id)
            .map(|checkpoints| checkpoints.iter().map(RemoteCheckpointView::from).collect())
            .map_err(map_restore_error)
    })
    .await
}

#[tauri::command]
pub(crate) async fn preview_backup_restore(
    state: State<'_, RuntimeState>,
    input: RestoreCheckpointInput,
) -> BackupCommandResult<RestorePreviewView> {
    let context = BackupContext::from_state(&state);
    run_blocking(move || preview_restore_blocking(context, input)).await
}

#[tauri::command]
pub(crate) async fn drill_backup_restore(
    state: State<'_, RuntimeState>,
    input: RestoreCheckpointInput,
) -> BackupCommandResult<RestoreDrillView> {
    let context = BackupContext::from_state(&state);
    let completed_at_ms = state.now_ms();
    run_blocking(move || drill_restore_blocking(context, input, completed_at_ms)).await
}

#[tauri::command]
pub(crate) async fn take_over_backup(
    state: State<'_, RuntimeState>,
    input: TakeOverBackupInput,
) -> BackupCommandResult<BackupSettingsSnapshot> {
    let relational_phase = state.relational_backup_status().phase;
    let checkpoint_phase = state.checkpoint_backup_status().phase;
    let context = BackupContext::from_state(&state);
    let now_ms = state.now_ms();
    run_blocking(move || {
        takeover_blocking(context, input, relational_phase, checkpoint_phase, now_ms)
    })
    .await?;
    state.reload_backup_configuration();
    load_backup_settings(state).await
}

fn load_snapshot(
    client: &DatabaseClient,
    relational: RelationalBackupStatus,
    checkpoint: CheckpointBackupStatus,
) -> BackupCommandResult<BackupSettingsSnapshot> {
    let config = client
        .load_offsite_backup_config()
        .map_err(map_database_error)?;
    let credential_state = match &config {
        None => BackupCredentialState::Missing,
        Some(config) => match MacOsKeychainCredentialStore.load(&config.backup_set_id) {
            Ok(_) => BackupCredentialState::Stored,
            Err(CredentialError::Missing) => BackupCredentialState::Missing,
            Err(CredentialError::Unavailable) => BackupCredentialState::Unavailable,
            Err(
                CredentialError::InvalidCredential(_)
                | CredentialError::UnsupportedPayloadVersion
                | CredentialError::CorruptPayload,
            ) => BackupCredentialState::Invalid,
        },
    };
    let cleanup_pending = !client
        .load_offsite_credential_cleanup()
        .map_err(map_database_error)?
        .is_empty();
    let progress = client
        .offsite_media_upload_progress()
        .map_err(map_database_error)?;
    Ok(BackupSettingsSnapshot {
        config: config.as_ref().map(BackupConfigView::from),
        credential_state,
        credential_cleanup_pending: cleanup_pending,
        relational,
        media: MediaBackupStatus {
            referenced: progress.referenced,
            pending: progress.pending,
            running: progress.running,
            retry_wait: progress.retry_wait,
            uploaded: progress.uploaded,
            failed: progress.failed,
            untracked: progress.untracked,
            next_attempt_at_ms: progress.next_attempt_at_ms,
        },
        checkpoint,
        retention: BackupRetentionView {
            exact_transaction_days: EXACT_TRANSACTION_RETENTION_DAYS,
            checkpoint_policy: "Complete checkpoint manifests are immutable and are not automatically deleted in v1.",
            media_policy: "Content-addressed media is immutable and is not automatically deleted in v1.",
        },
    })
}

fn test_connection_blocking(
    context: BackupContext,
    mut input: TestBackupConnectionInput,
    tested_at_ms: i64,
) -> BackupCommandResult<BackupConnectionTestResult> {
    let _guard = lock_gate(&context.gate);
    let target = parse_target(
        std::mem::take(&mut input.account_id),
        input.jurisdiction,
        std::mem::take(&mut input.bucket),
    )?;
    let backup_set_id = match input.backup_set_id.take().filter(|value| !value.is_empty()) {
        Some(value) => BackupSetId::parse(value)
            .map_err(|_| BackupCommandError::invalid("Enter a canonical Kosh backup set ID."))?,
        None => context
            .client
            .load_offsite_backup_config()
            .map_err(map_database_error)?
            .map_or_else(BackupSetId::new, |config| config.backup_set_id),
    };
    let supplied = take_credentials(&mut input.access_key_id, &mut input.secret_access_key)?;
    let credentials = match supplied {
        Some(credentials) => credentials,
        None => MacOsKeychainCredentialStore
            .load(&backup_set_id)
            .map_err(map_credential_error)?,
    };
    let keyspace = target.keyspace(&backup_set_id);
    let store = R2ObjectStore::new(target, keyspace.clone(), &credentials)
        .map_err(map_object_store_error)?;
    verify_object_store(&store, &keyspace).map_err(map_probe_error)?;
    Ok(BackupConnectionTestResult {
        verified: true,
        cleanup_complete: true,
        tested_at_ms,
    })
}

fn configure_blocking(
    context: BackupContext,
    mut input: ConfigureBackupInput,
    now_ms: i64,
) -> BackupCommandResult<()> {
    let _guard = lock_gate(&context.gate);
    if input.expected_revision < 0 {
        return Err(BackupCommandError::invalid(
            "The expected backup configuration revision is invalid.",
        ));
    }
    let current = context
        .client
        .load_offsite_backup_config()
        .map_err(map_database_error)?;
    let current_revision = current.as_ref().map_or(0, |config| config.revision);
    if input.expected_revision != current_revision {
        return Err(stale_configuration());
    }
    let target = parse_target(
        std::mem::take(&mut input.account_id),
        input.jurisdiction,
        std::mem::take(&mut input.bucket),
    )?;
    let requested_backup_set = input
        .backup_set_id
        .take()
        .filter(|value| !value.is_empty())
        .map(BackupSetId::parse)
        .transpose()
        .map_err(|_| BackupCommandError::invalid("Enter a canonical Kosh backup set ID."))?;
    let backup_set_id = requested_backup_set.unwrap_or_else(|| {
        current
            .as_ref()
            .map(|config| config.backup_set_id.clone())
            .unwrap_or_else(BackupSetId::new)
    });
    let replica_epoch_id = current
        .as_ref()
        .filter(|config| config.backup_set_id == backup_set_id)
        .map(|config| config.replica_epoch_id.clone())
        .unwrap_or_else(ReplicaEpochId::new);
    let supplied = take_credentials(&mut input.access_key_id, &mut input.secret_access_key)?;
    let credential_action = if supplied.is_some() {
        CredentialIntentAction::Replace
    } else {
        MacOsKeychainCredentialStore
            .load(&backup_set_id)
            .map_err(map_credential_error)?;
        CredentialIntentAction::Reuse
    };
    let operation_id = uuid::Uuid::now_v7().to_string();
    context
        .client
        .begin_offsite_backup_config_intent(BeginOffsiteBackupConfigIntentInput {
            operation_id: operation_id.clone(),
            proposed: SaveOffsiteBackupConfigInput {
                expected_revision: input.expected_revision,
                backup_set_id,
                replica_epoch_id,
                enabled: false,
                target,
                now_ms,
            },
            credential_action,
        })
        .map_err(map_database_error)?;
    if let Some(credentials) = supplied {
        if let Err(error) = MacOsKeychainCredentialStore.stage(&operation_id, &credentials) {
            let _ = MacOsKeychainCredentialStore.remove_staged(&operation_id);
            let _ = context
                .client
                .abort_offsite_backup_config_intent(operation_id);
            return Err(map_credential_error(error));
        }
    }
    reconcile_pending_backup_operations(&context.client, &context.data_root)?;
    clean_retired_credentials(&context.client);
    Ok(())
}

fn set_enabled_blocking(
    context: BackupContext,
    input: SetBackupEnabledInput,
    now_ms: i64,
) -> BackupCommandResult<()> {
    let _guard = lock_gate(&context.gate);
    let config = context
        .client
        .load_offsite_backup_config()
        .map_err(map_database_error)?
        .ok_or_else(not_configured)?;
    if config.revision != input.expected_revision {
        return Err(stale_configuration());
    }
    if input.enabled {
        MacOsKeychainCredentialStore
            .load(&config.backup_set_id)
            .map_err(map_credential_error)?;
    }
    context
        .client
        .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
            expected_revision: input.expected_revision,
            backup_set_id: config.backup_set_id,
            replica_epoch_id: config.replica_epoch_id,
            enabled: input.enabled,
            target: config.target,
            now_ms,
        })
        .map_err(map_database_error)?;
    Ok(())
}

fn preview_restore_blocking(
    context: BackupContext,
    input: RestoreCheckpointInput,
) -> BackupCommandResult<RestorePreviewView> {
    let _guard = lock_gate(&context.gate);
    let checkpoint_id = parse_checkpoint_id(input.checkpoint_id)?;
    let (config, credentials, store) = open_saved_store(&context.client)?;
    let keyspace = config.target.keyspace(&config.backup_set_id);
    let checkpoint = find_checkpoint(&store, &keyspace, &config.backup_set_id, &checkpoint_id)?;
    let binary = resolve_restore_binary(&context)?;
    let runtime = LitestreamRuntimePaths::new(&context.data_root).map_err(map_litestream_error)?;
    let replica_path = keyspace.litestream(checkpoint.replica_epoch_id());
    let engine = CommandLitestreamRestore::new(
        &binary,
        &runtime,
        &config.target,
        &replica_path,
        &context.paths.main,
        &credentials,
        RESTORE_COMMAND_TIMEOUT,
    )
    .map_err(map_litestream_error)?;
    let preview_target = context
        .data_root
        .join(format!(".restore-preview-{}.sqlite3", uuid::Uuid::now_v7()));
    reject_existing_path(&preview_target)?;
    let preview = preview_checkpoint(
        &store,
        &keyspace,
        &config.backup_set_id,
        &checkpoint_id,
        &engine,
        &context.paths.main,
        &preview_target,
    )
    .map_err(map_restore_error)?;
    reject_unexpected_preview_output(&preview_target)?;
    preview_view(&context.data_root, &config, preview)
}

fn drill_restore_blocking(
    context: BackupContext,
    input: RestoreCheckpointInput,
    completed_at_ms: i64,
) -> BackupCommandResult<RestoreDrillView> {
    let _guard = lock_gate(&context.gate);
    let checkpoint_id = parse_checkpoint_id(input.checkpoint_id)?;
    let (config, credentials, store) = open_saved_store(&context.client)?;
    let keyspace = config.target.keyspace(&config.backup_set_id);
    let checkpoint = find_checkpoint(&store, &keyspace, &config.backup_set_id, &checkpoint_id)?;
    let binary = resolve_restore_binary(&context)?;
    let runtime = LitestreamRuntimePaths::new(&context.data_root).map_err(map_litestream_error)?;
    let replica_path = keyspace.litestream(checkpoint.replica_epoch_id());
    let engine = CommandLitestreamRestore::new(
        &binary,
        &runtime,
        &config.target,
        &replica_path,
        &context.paths.main,
        &credentials,
        RESTORE_COMMAND_TIMEOUT,
    )
    .map_err(map_litestream_error)?;
    let preview_target = context
        .data_root
        .join(format!(".restore-preview-{}.sqlite3", uuid::Uuid::now_v7()));
    reject_existing_path(&preview_target)?;
    let preview = preview_checkpoint(
        &store,
        &keyspace,
        &config.backup_set_id,
        &checkpoint_id,
        &engine,
        &context.paths.main,
        &preview_target,
    )
    .map_err(map_restore_error)?;
    reject_unexpected_preview_output(&preview_target)?;
    ensure_restore_drill_capacity_with(
        &context.data_root,
        preview.plan_total_bytes,
        checkpoint.referenced_total_bytes(),
        available_storage_bytes,
    )?;
    let drill_root = context
        .data_root
        .join(format!(".restore-drill-{}", uuid::Uuid::now_v7()));
    reject_existing_path(&drill_root)?;
    let report = drill_checkpoint(
        &store,
        &keyspace,
        &checkpoint,
        &engine,
        &context.paths.main,
        &drill_root,
    )
    .map_err(map_restore_error)?;
    Ok(RestoreDrillView {
        checkpoint_id: report.checkpoint_id.to_string(),
        restored_media_count: report.restored_media_count,
        restored_media_bytes: report.restored_media_bytes,
        completed_at_ms,
    })
}

fn ensure_restore_drill_capacity_with(
    data_root: &Path,
    relational_bytes: u64,
    media_bytes: u64,
    available_space: impl FnOnce(&Path) -> crate::database::Result<u64>,
) -> BackupCommandResult<()> {
    let required = relational_bytes
        .checked_mul(2)
        .and_then(|bytes| {
            media_bytes
                .checked_mul(2)
                .and_then(|media| bytes.checked_add(media))
        })
        .and_then(|bytes| bytes.checked_add(RESTORE_DRILL_STORAGE_HEADROOM_BYTES))
        .ok_or_else(|| {
            BackupCommandError::new(
                BackupCommandErrorCode::RestoreFailed,
                "This recovery point is too large to stage safely.",
            )
        })?;
    let available = available_space(data_root).map_err(|_| {
        BackupCommandError::new(
            BackupCommandErrorCode::RestoreFailed,
            "Available storage could not be verified, so the recovery drill was not started.",
        )
    })?;
    if available < required {
        return Err(BackupCommandError::new(
            BackupCommandErrorCode::RestoreFailed,
            format!(
                "Not enough free storage for this recovery drill. Free at least {} more bytes and try again.",
                required - available
            ),
        ));
    }
    Ok(())
}

fn takeover_blocking(
    context: BackupContext,
    input: TakeOverBackupInput,
    relational_phase: RelationalBackupPhase,
    checkpoint_phase: CheckpointBackupPhase,
    now_ms: i64,
) -> BackupCommandResult<()> {
    let _guard = lock_gate(&context.gate);
    if input.confirmation != TAKEOVER_CONFIRMATION {
        return Err(BackupCommandError::invalid(
            "Type TAKE OVER to confirm the single-writer transfer.",
        ));
    }
    let config = context
        .client
        .load_offsite_backup_config()
        .map_err(map_database_error)?
        .ok_or_else(not_configured)?;
    if config.revision != input.expected_revision {
        return Err(stale_configuration());
    }
    if config.enabled
        || relational_phase != RelationalBackupPhase::Off
        || checkpoint_phase != CheckpointBackupPhase::Off
    {
        return Err(BackupCommandError::new(
            BackupCommandErrorCode::BackupMustBeDisabled,
            "Turn off backup and wait for replication and checkpoint publication to stop before takeover.",
        ));
    }
    let credentials = MacOsKeychainCredentialStore
        .load(&config.backup_set_id)
        .map_err(map_credential_error)?;
    let keyspace = config.target.keyspace(&config.backup_set_id);
    let store = R2ObjectStore::new(config.target.clone(), keyspace.clone(), &credentials)
        .map_err(map_object_store_error)?;
    let owner = inspect_remote_owner(&store, &keyspace).map_err(map_owner_error)?;
    if !owner_matches_input(&owner, &input) {
        return Err(owner_changed());
    }
    let writer = MacOsInstallationWriterIdentity::new(context.data_root.clone())
        .load()
        .map_err(map_writer_identity_error)?;
    let next_epoch = ReplicaEpochId::new();
    let operation_id = uuid::Uuid::now_v7().to_string();
    context
        .client
        .begin_offsite_backup_takeover_intent(BeginOffsiteBackupTakeoverIntentInput {
            operation_id,
            expected_revision: config.revision,
            backup_set_id: config.backup_set_id,
            previous_replica_epoch_id: config.replica_epoch_id,
            next_replica_epoch_id: next_epoch,
            expected_owner_version: owner.version().to_owned(),
            expected_owner_replica_epoch_id: owner.replica_epoch_id,
            expected_owner_writer_id: owner.writer_id,
            next_writer_id: writer,
            created_at_ms: now_ms,
        })
        .map_err(map_database_error)?;
    reconcile_pending_backup_operations(&context.client, &context.data_root)?;
    Ok(())
}

pub(crate) fn reconcile_startup_backup_state(client: &DatabaseClient) -> BackupCommandResult<bool> {
    let reconciliation = reconcile_pending_config_operation(client);
    clean_retired_credentials(client);
    reconciliation
}

fn reconcile_pending_backup_operations(
    client: &DatabaseClient,
    data_root: &Path,
) -> BackupCommandResult<bool> {
    let config_changed = reconcile_pending_config_operation(client)?;
    let takeover_changed = reconcile_deferred_takeover(client, data_root)?;
    Ok(config_changed || takeover_changed)
}

fn reconcile_pending_config_operation(client: &DatabaseClient) -> BackupCommandResult<bool> {
    if let Some(intent) = client
        .load_offsite_backup_config_intent()
        .map_err(map_database_error)?
    {
        reconcile_config_intent(client, &MacOsKeychainCredentialStore, intent)?;
        return Ok(true);
    }
    Ok(false)
}

pub(crate) fn reconcile_deferred_takeover(
    client: &DatabaseClient,
    data_root: &Path,
) -> BackupCommandResult<bool> {
    if let Some(intent) = client
        .load_offsite_backup_takeover_intent()
        .map_err(map_database_error)?
    {
        reconcile_takeover_intent(client, data_root, intent)?;
        return Ok(true);
    }
    Ok(false)
}

fn reconcile_config_intent(
    client: &DatabaseClient,
    credentials: &dyn CredentialStore,
    intent: OffsiteBackupConfigIntent,
) -> BackupCommandResult<()> {
    if intent.state == OffsiteOperationState::Pending {
        let active_credentials = match intent.credential_action {
            CredentialIntentAction::Reuse => {
                match credentials.load(&intent.proposed.backup_set_id) {
                    Ok(credentials) => credentials,
                    Err(CredentialError::Missing) => {
                        client
                            .abort_offsite_backup_config_intent(intent.operation_id)
                            .map_err(map_database_error)?;
                        return Err(map_credential_error(CredentialError::Missing));
                    }
                    Err(error) => return Err(map_credential_error(error)),
                }
            }
            CredentialIntentAction::Replace => {
                match credentials.load_staged(&intent.operation_id) {
                    Ok(credentials) => credentials,
                    Err(CredentialError::Missing) => {
                        client
                            .abort_offsite_backup_config_intent(intent.operation_id)
                            .map_err(map_database_error)?;
                        return Ok(());
                    }
                    Err(error) => return Err(map_credential_error(error)),
                }
            }
        };
        if intent.credential_action == CredentialIntentAction::Replace {
            credentials
                .save(&intent.proposed.backup_set_id, &active_credentials)
                .map_err(map_credential_error)?;
        }
        client
            .commit_offsite_backup_config_intent(intent.operation_id.clone())
            .map_err(map_database_error)?;
    }

    if intent.credential_action == CredentialIntentAction::Replace {
        match credentials.load_staged(&intent.operation_id) {
            Ok(staged) => credentials
                .save(&intent.proposed.backup_set_id, &staged)
                .map_err(map_credential_error)?,
            Err(CredentialError::Missing) => {
                credentials
                    .load(&intent.proposed.backup_set_id)
                    .map_err(map_credential_error)?;
            }
            Err(error) => return Err(map_credential_error(error)),
        }
        credentials
            .remove_staged(&intent.operation_id)
            .map_err(map_credential_error)?;
    } else {
        credentials
            .load(&intent.proposed.backup_set_id)
            .map_err(map_credential_error)?;
    }
    client
        .complete_offsite_backup_config_intent(intent.operation_id)
        .map_err(map_database_error)
}

fn reconcile_takeover_intent(
    client: &DatabaseClient,
    data_root: &Path,
    intent: OffsiteBackupTakeoverIntent,
) -> BackupCommandResult<()> {
    let config = client
        .load_offsite_backup_config()
        .map_err(map_database_error)?
        .ok_or_else(not_configured)?;
    if config.revision != intent.expected_revision
        || config.backup_set_id != intent.backup_set_id
        || config.replica_epoch_id != intent.previous_replica_epoch_id
        || config.enabled
    {
        return Err(stale_configuration());
    }
    let credentials = MacOsKeychainCredentialStore
        .load(&intent.backup_set_id)
        .map_err(map_credential_error)?;
    let keyspace = config.target.keyspace(&intent.backup_set_id);
    let store = R2ObjectStore::new(config.target, keyspace.clone(), &credentials)
        .map_err(map_object_store_error)?;
    let writer = MacOsInstallationWriterIdentity::new(data_root.to_owned())
        .load()
        .map_err(map_writer_identity_error)?;
    if writer != intent.next_writer_id {
        return Err(BackupCommandError::new(
            BackupCommandErrorCode::OwnerChanged,
            "This installation's backup identity changed before takeover could finish.",
        ));
    }
    if let Err(error) = resume_remote_takeover(
        &store,
        &keyspace,
        &intent.backup_set_id,
        &intent.expected_owner_replica_epoch_id,
        &intent.expected_owner_writer_id,
        &intent.expected_owner_version,
        &intent.next_replica_epoch_id,
        &intent.next_writer_id,
    ) {
        if matches!(error, RemoteOwnerError::Conflict) {
            client
                .abort_offsite_backup_takeover_intent(intent.operation_id)
                .map_err(map_database_error)?;
        }
        return Err(map_owner_error(error));
    }
    client
        .commit_offsite_backup_takeover_intent(intent.operation_id)
        .map_err(map_database_error)?;
    Ok(())
}

fn preview_view(
    data_root: &Path,
    config: &OffsiteBackupConfig,
    preview: RestorePreview,
) -> BackupCommandResult<RestorePreviewView> {
    let writer = MacOsInstallationWriterIdentity::new(data_root.to_owned())
        .load()
        .map_err(map_writer_identity_error)?;
    Ok(RestorePreviewView {
        checkpoint: RemoteCheckpointView::from(&preview.checkpoint),
        owner: RestoreOwnerView {
            backup_set_id: preview.owner.backup_set_id().to_string(),
            replica_epoch_id: preview.owner.replica_epoch_id.to_string(),
            writer_id: preview.owner.writer_id.to_string(),
            version: preview.owner.version().to_owned(),
            is_current_installation: is_current_owner(config, &preview.owner, &writer),
        },
        plan_file_count: preview.plan_file_count,
        plan_total_bytes: preview.plan_total_bytes,
    })
}

fn is_current_owner(
    config: &OffsiteBackupConfig,
    owner: &RemoteOwnerSnapshot,
    writer: &BackupWriterId,
) -> bool {
    owner.backup_set_id() == &config.backup_set_id
        && owner.replica_epoch_id == config.replica_epoch_id
        && &owner.writer_id == writer
}

fn owner_matches_input(owner: &RemoteOwnerSnapshot, input: &TakeOverBackupInput) -> bool {
    owner.backup_set_id().as_str() == input.expected_owner_backup_set_id
        && owner.replica_epoch_id.as_str() == input.expected_owner_replica_epoch_id
        && owner.writer_id.as_str() == input.expected_owner_writer_id
        && owner.version() == input.expected_owner_version
}

fn find_checkpoint(
    store: &R2ObjectStore,
    keyspace: &super::domain::R2Keyspace,
    backup_set_id: &BackupSetId,
    checkpoint_id: &CheckpointId,
) -> BackupCommandResult<RemoteCheckpoint> {
    discover_checkpoints(store, keyspace, backup_set_id)
        .map_err(map_restore_error)?
        .into_iter()
        .find(|checkpoint| checkpoint.checkpoint_id() == checkpoint_id)
        .ok_or_else(|| {
            BackupCommandError::new(
                BackupCommandErrorCode::RestoreFailed,
                "That recovery point is no longer available. Refresh recovery points.",
            )
        })
}

fn open_saved_store(
    client: &DatabaseClient,
) -> BackupCommandResult<(OffsiteBackupConfig, R2Credentials, R2ObjectStore)> {
    let config = client
        .load_offsite_backup_config()
        .map_err(map_database_error)?
        .ok_or_else(not_configured)?;
    let credentials = MacOsKeychainCredentialStore
        .load(&config.backup_set_id)
        .map_err(map_credential_error)?;
    let keyspace = config.target.keyspace(&config.backup_set_id);
    let store = R2ObjectStore::new(config.target.clone(), keyspace, &credentials)
        .map_err(map_object_store_error)?;
    Ok((config, credentials, store))
}

fn resolve_restore_binary(
    context: &BackupContext,
) -> BackupCommandResult<super::litestream::ImmutableLitestreamBinary> {
    let resource_dir = context.resource_dir.as_deref().ok_or_else(|| {
        BackupCommandError::new(
            BackupCommandErrorCode::RestoreUnavailable,
            "The bundled recovery runtime is unavailable in this build.",
        )
    })?;
    let runtime = LitestreamRuntimePaths::new(&context.data_root).map_err(map_litestream_error)?;
    VerifiedLitestreamBinary::resolve(resource_dir)
        .and_then(|binary| binary.stage_immutable(&runtime))
        .map_err(map_litestream_error)
}

fn parse_target(
    account_id: String,
    jurisdiction: R2Jurisdiction,
    bucket: String,
) -> BackupCommandResult<R2Target> {
    Ok(R2Target {
        account_id: R2AccountId::parse(account_id).map_err(|_| {
            BackupCommandError::invalid("Enter a 32-character Cloudflare account ID.")
        })?,
        jurisdiction,
        bucket: R2BucketName::parse(bucket).map_err(|_| {
            BackupCommandError::invalid(
                "Enter a valid lowercase R2 bucket name using letters, numbers, and hyphens.",
            )
        })?,
    })
}

fn parse_checkpoint_id(value: String) -> BackupCommandResult<CheckpointId> {
    CheckpointId::parse(value)
        .map_err(|_| BackupCommandError::invalid("The recovery point ID is invalid."))
}

fn take_credentials(
    access_key_id: &mut Option<String>,
    secret_access_key: &mut Option<String>,
) -> BackupCommandResult<Option<R2Credentials>> {
    let access = access_key_id.take().filter(|value| !value.is_empty());
    let secret = secret_access_key.take().filter(|value| !value.is_empty());
    match (access, secret) {
        (None, None) => Ok(None),
        (Some(access), Some(secret)) => R2Credentials::new(access, secret)
            .map(Some)
            .map_err(map_credential_error),
        (mut access, mut secret) => {
            zeroize_optional(&mut access);
            zeroize_optional(&mut secret);
            Err(BackupCommandError::invalid(
                "Enter both the R2 access key ID and secret access key.",
            ))
        }
    }
}

fn zeroize_optional(value: &mut Option<String>) {
    if let Some(value) = value {
        value.zeroize();
    }
}

fn clean_retired_credentials(client: &DatabaseClient) {
    clean_retired_credentials_with(client, &MacOsKeychainCredentialStore);
}

fn clean_retired_credentials_with(client: &DatabaseClient, credentials: &dyn CredentialStore) {
    let Ok(pending) = client.load_offsite_credential_cleanup() else {
        log::warn!("could not inspect queued off-site credential cleanup");
        return;
    };
    for backup_set_id in pending {
        if credentials.remove(&backup_set_id).is_err() {
            log::warn!("queued off-site credential cleanup could not access Keychain");
            continue;
        }
        if client
            .complete_offsite_credential_cleanup(backup_set_id)
            .is_err()
        {
            log::warn!("queued off-site credential cleanup could not be recorded");
        }
    }
}

fn reject_existing_path(path: &Path) -> BackupCommandResult<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(BackupCommandError::new(
            BackupCommandErrorCode::RestoreFailed,
            "A private recovery workspace already exists. Try again.",
        )),
        Err(_) => Err(BackupCommandError::new(
            BackupCommandErrorCode::RestoreFailed,
            "The private recovery workspace could not be inspected.",
        )),
    }
}

fn reject_unexpected_preview_output(path: &Path) -> BackupCommandResult<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(metadata) if metadata.is_file() => {
            let _ = fs::remove_file(path);
            Err(BackupCommandError::new(
                BackupCommandErrorCode::RestoreFailed,
                "The recovery preview unexpectedly wrote data and was discarded.",
            ))
        }
        Ok(_) | Err(_) => Err(BackupCommandError::new(
            BackupCommandErrorCode::RestoreFailed,
            "The recovery preview workspace is invalid.",
        )),
    }
}

fn lock_gate(gate: &Arc<Mutex<()>>) -> std::sync::MutexGuard<'_, ()> {
    gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

async fn run_blocking<T: Send + 'static>(
    operation: impl FnOnce() -> BackupCommandResult<T> + Send + 'static,
) -> BackupCommandResult<T> {
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|_| BackupCommandError::worker())?
}

fn not_configured() -> BackupCommandError {
    BackupCommandError::new(
        BackupCommandErrorCode::NotConfigured,
        "Set up an R2 recovery target first.",
    )
}

fn stale_configuration() -> BackupCommandError {
    BackupCommandError::new(
        BackupCommandErrorCode::StaleConfiguration,
        "Backup settings changed. Refresh and try again.",
    )
}

fn owner_changed() -> BackupCommandError {
    BackupCommandError::new(
        BackupCommandErrorCode::OwnerChanged,
        "The remote owner changed after preview. Preview the recovery point again.",
    )
}

fn map_database_error(error: DatabaseError) -> BackupCommandError {
    match error {
        DatabaseError::StaleOffsiteBackupConfig => stale_configuration(),
        DatabaseError::OffsiteBackupOperationPending
        | DatabaseError::OffsiteBackupOperationNotFound => stale_configuration(),
        DatabaseError::OffsiteBackupMustBeDisabled => BackupCommandError::new(
            BackupCommandErrorCode::BackupMustBeDisabled,
            "Turn off backup and wait for background recovery work to stop before changing its target.",
        ),
        DatabaseError::InvalidInput(_) | DatabaseError::InvalidOffsiteBackupConfig(_) => {
            BackupCommandError::invalid("The backup configuration is invalid.")
        }
        DatabaseError::WriterUnavailable | DatabaseError::WriterPanicked => {
            BackupCommandError::worker()
        }
        _ => BackupCommandError::new(
            BackupCommandErrorCode::DatabaseUnavailable,
            "Backup settings could not access the local database.",
        ),
    }
}

fn map_credential_error(error: CredentialError) -> BackupCommandError {
    match error {
        CredentialError::InvalidCredential("accessKeyId") => BackupCommandError::invalid(
            "The R2 access key ID must be 32 lowercase hexadecimal characters.",
        ),
        CredentialError::InvalidCredential("secretAccessKey") => BackupCommandError::invalid(
            "The R2 secret access key must be 64 lowercase hexadecimal characters.",
        ),
        CredentialError::InvalidCredential(_) => {
            BackupCommandError::invalid("The R2 credentials are invalid.")
        }
        CredentialError::Missing => BackupCommandError::new(
            BackupCommandErrorCode::CredentialsMissing,
            "R2 credentials are not stored for this backup set.",
        ),
        CredentialError::Unavailable => BackupCommandError::new(
            BackupCommandErrorCode::KeychainUnavailable,
            "macOS Keychain is unavailable. Local capture and search still work.",
        ),
        CredentialError::UnsupportedPayloadVersion | CredentialError::CorruptPayload => {
            BackupCommandError::new(
                BackupCommandErrorCode::KeychainUnavailable,
                "The stored R2 credentials are invalid. Enter them again.",
            )
        }
    }
}

fn map_probe_error(error: ObjectStoreProbeError) -> BackupCommandError {
    map_object_store_code(error.code)
}

fn map_object_store_error(error: ObjectStoreError) -> BackupCommandError {
    map_object_store_code(error.code)
}

fn map_object_store_code(code: ObjectStoreErrorCode) -> BackupCommandError {
    match code {
        ObjectStoreErrorCode::AuthenticationRejected => BackupCommandError::new(
            BackupCommandErrorCode::AuthenticationRejected,
            "Cloudflare rejected the R2 credentials.",
        ),
        ObjectStoreErrorCode::AuthorizationRejected => BackupCommandError::new(
            BackupCommandErrorCode::AuthorizationRejected,
            "The R2 token cannot read, write, list, and clean up probe objects in this bucket.",
        ),
        ObjectStoreErrorCode::Network | ObjectStoreErrorCode::Timeout => BackupCommandError::new(
            BackupCommandErrorCode::NetworkUnavailable,
            "R2 could not be reached. Local capture and search still work.",
        ),
        ObjectStoreErrorCode::RateLimited | ObjectStoreErrorCode::ServiceUnavailable => {
            BackupCommandError::new(
                BackupCommandErrorCode::ServiceUnavailable,
                "R2 is temporarily unavailable. Try again later.",
            )
        }
        _ => BackupCommandError::new(
            BackupCommandErrorCode::ConnectionFailed,
            "The R2 target did not satisfy Kosh's read, write, list, and cleanup checks.",
        ),
    }
}

fn map_litestream_error(_error: LitestreamError) -> BackupCommandError {
    BackupCommandError::new(
        BackupCommandErrorCode::RestoreUnavailable,
        "The verified recovery runtime is unavailable.",
    )
}

fn map_restore_error(error: RestoreError) -> BackupCommandError {
    match error {
        RestoreError::Store(error) => map_object_store_error(error),
        RestoreError::Owner(error) => map_owner_error(error),
        RestoreError::CheckpointNotFound => BackupCommandError::new(
            BackupCommandErrorCode::RestoreFailed,
            "That recovery point is no longer available. Refresh recovery points.",
        ),
        RestoreError::Litestream(error) => map_litestream_error(error),
        _ => BackupCommandError::new(
            BackupCommandErrorCode::RestoreFailed,
            "The recovery point failed verification and was not applied.",
        ),
    }
}

fn map_owner_error(error: RemoteOwnerError) -> BackupCommandError {
    match error {
        RemoteOwnerError::Conflict => owner_changed(),
        RemoteOwnerError::Store(error) => map_object_store_error(error),
        RemoteOwnerError::Cancelled | RemoteOwnerError::Invalid => BackupCommandError::new(
            BackupCommandErrorCode::OwnerChanged,
            "The remote owner record is unavailable or invalid.",
        ),
    }
}

fn map_writer_identity_error(_error: WriterIdentityError) -> BackupCommandError {
    BackupCommandError::new(
        BackupCommandErrorCode::RestoreUnavailable,
        "This installation's single-writer identity is unavailable.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::domain::BackupProvider;
    use crate::database::{Database, DatabasePaths};
    use std::{collections::HashMap, sync::Mutex};

    const ACCESS_KEY: &str = "0123456789abcdef0123456789abcdef";
    const SECRET_KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[derive(Default)]
    struct FakeCredentialStore {
        active: Mutex<HashMap<String, (String, String)>>,
        staged: Mutex<HashMap<String, (String, String)>>,
    }

    impl FakeCredentialStore {
        fn pair(credentials: &R2Credentials) -> (String, String) {
            (
                credentials.access_key_id().to_owned(),
                credentials.secret_access_key().to_owned(),
            )
        }

        fn decode(pair: &(String, String)) -> Result<R2Credentials, CredentialError> {
            R2Credentials::new(pair.0.clone(), pair.1.clone())
        }

        fn has_active(&self, backup_set_id: &BackupSetId) -> bool {
            self.active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains_key(backup_set_id.as_str())
        }

        fn has_staged(&self, operation_id: &str) -> bool {
            self.staged
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains_key(operation_id)
        }
    }

    impl CredentialStore for FakeCredentialStore {
        fn save(
            &self,
            backup_set_id: &BackupSetId,
            credentials: &R2Credentials,
        ) -> Result<(), CredentialError> {
            self.active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(backup_set_id.to_string(), Self::pair(credentials));
            Ok(())
        }

        fn load(&self, backup_set_id: &BackupSetId) -> Result<R2Credentials, CredentialError> {
            let active = self
                .active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::decode(
                active
                    .get(backup_set_id.as_str())
                    .ok_or(CredentialError::Missing)?,
            )
        }

        fn remove(&self, backup_set_id: &BackupSetId) -> Result<(), CredentialError> {
            self.active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(backup_set_id.as_str());
            Ok(())
        }

        fn stage(
            &self,
            operation_id: &str,
            credentials: &R2Credentials,
        ) -> Result<(), CredentialError> {
            self.staged
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(operation_id.to_owned(), Self::pair(credentials));
            Ok(())
        }

        fn load_staged(&self, operation_id: &str) -> Result<R2Credentials, CredentialError> {
            let staged = self
                .staged
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::decode(staged.get(operation_id).ok_or(CredentialError::Missing)?)
        }

        fn remove_staged(&self, operation_id: &str) -> Result<(), CredentialError> {
            self.staged
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(operation_id);
            Ok(())
        }
    }

    fn pending_config_fixture(
        client: &DatabaseClient,
        credential_action: CredentialIntentAction,
    ) -> (String, BackupSetId) {
        let operation_id = uuid::Uuid::now_v7().to_string();
        let backup_set_id = BackupSetId::new();
        client
            .begin_offsite_backup_config_intent(BeginOffsiteBackupConfigIntentInput {
                operation_id: operation_id.clone(),
                proposed: SaveOffsiteBackupConfigInput {
                    expected_revision: 0,
                    backup_set_id: backup_set_id.clone(),
                    replica_epoch_id: ReplicaEpochId::new(),
                    enabled: false,
                    target: R2Target {
                        account_id: R2AccountId::parse(ACCESS_KEY).expect("account"),
                        jurisdiction: R2Jurisdiction::Default,
                        bucket: R2BucketName::parse("kosh-test").expect("bucket"),
                    },
                    now_ms: 10,
                },
                credential_action,
            })
            .expect("begin config intent");
        (operation_id, backup_set_id)
    }

    #[test]
    fn command_errors_are_public_and_never_echo_credentials() {
        let access = format!("{ACCESS_KEY}x");
        let secret = format!("{SECRET_KEY}x");
        let error =
            R2Credentials::new(access.clone(), secret.clone()).expect_err("invalid credentials");
        let mapped = map_credential_error(error);
        let serialized = serde_json::to_string(&mapped).expect("serialize public error");
        assert!(!serialized.contains(&access));
        assert!(!serialized.contains(&secret));
        assert!(serialized.contains("INVALID_INPUT"));
    }

    #[test]
    fn pending_staged_credentials_reconcile_to_active_config_and_clear_the_journal() {
        let root = tempfile::TempDir::new().expect("temporary root");
        let database = Database::initialize(DatabasePaths::new(root.path())).expect("database");
        let client = database.client();
        let store = FakeCredentialStore::default();
        let (operation_id, backup_set_id) =
            pending_config_fixture(&client, CredentialIntentAction::Replace);
        let credentials = R2Credentials::new(ACCESS_KEY, SECRET_KEY).expect("credentials");
        store
            .stage(&operation_id, &credentials)
            .expect("stage credentials");
        let intent = client
            .load_offsite_backup_config_intent()
            .expect("intent")
            .expect("pending intent");

        reconcile_config_intent(&client, &store, intent).expect("reconcile intent");

        assert!(store.has_active(&backup_set_id));
        assert!(!store.has_staged(&operation_id));
        assert!(client
            .load_offsite_backup_config_intent()
            .expect("intent")
            .is_none());
        assert_eq!(
            client
                .load_offsite_backup_config()
                .expect("configuration")
                .expect("saved configuration")
                .backup_set_id,
            backup_set_id
        );
    }

    #[test]
    fn missing_staged_credentials_abort_an_uncommitted_config_without_mutation() {
        let root = tempfile::TempDir::new().expect("temporary root");
        let database = Database::initialize(DatabasePaths::new(root.path())).expect("database");
        let client = database.client();
        let store = FakeCredentialStore::default();
        pending_config_fixture(&client, CredentialIntentAction::Replace);
        let intent = client
            .load_offsite_backup_config_intent()
            .expect("intent")
            .expect("pending intent");

        reconcile_config_intent(&client, &store, intent).expect("abort missing stage");

        assert!(client
            .load_offsite_backup_config_intent()
            .expect("intent")
            .is_none());
        assert_eq!(
            client.load_offsite_backup_config().expect("configuration"),
            None
        );
    }

    #[test]
    fn missing_reused_credentials_abort_an_uncommitted_config_and_remain_recoverable() {
        let root = tempfile::TempDir::new().expect("temporary root");
        let database = Database::initialize(DatabasePaths::new(root.path())).expect("database");
        let client = database.client();
        let store = FakeCredentialStore::default();
        pending_config_fixture(&client, CredentialIntentAction::Reuse);
        let intent = client
            .load_offsite_backup_config_intent()
            .expect("intent")
            .expect("pending intent");

        let error =
            reconcile_config_intent(&client, &store, intent).expect_err("missing credentials");

        assert_eq!(error.code, BackupCommandErrorCode::CredentialsMissing);
        assert!(client
            .load_offsite_backup_config_intent()
            .expect("intent")
            .is_none());
        assert_eq!(
            client.load_offsite_backup_config().expect("configuration"),
            None
        );
    }

    #[test]
    fn committed_config_reconciliation_accepts_verified_active_credentials_after_stage_cleanup() {
        let root = tempfile::TempDir::new().expect("temporary root");
        let database = Database::initialize(DatabasePaths::new(root.path())).expect("database");
        let client = database.client();
        let store = FakeCredentialStore::default();
        let (operation_id, backup_set_id) =
            pending_config_fixture(&client, CredentialIntentAction::Replace);
        let credentials = R2Credentials::new(ACCESS_KEY, SECRET_KEY).expect("credentials");
        store
            .save(&backup_set_id, &credentials)
            .expect("active credentials");
        client
            .commit_offsite_backup_config_intent(operation_id.clone())
            .expect("committed config");
        let committed = client
            .load_offsite_backup_config_intent()
            .expect("intent")
            .expect("committed intent");
        assert_eq!(committed.state, OffsiteOperationState::Committed);

        reconcile_config_intent(&client, &store, committed).expect("finish cleanup");

        assert!(client
            .load_offsite_backup_config_intent()
            .expect("intent")
            .is_none());
        assert!(store.has_active(&backup_set_id));
    }

    #[test]
    fn queued_retired_credentials_retry_independently_of_configuration_changes() {
        let root = tempfile::TempDir::new().expect("temporary root");
        let database = Database::initialize(DatabasePaths::new(root.path())).expect("database");
        let client = database.client();
        let first = client
            .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
                expected_revision: 0,
                backup_set_id: BackupSetId::new(),
                replica_epoch_id: ReplicaEpochId::new(),
                enabled: false,
                target: R2Target {
                    account_id: R2AccountId::parse(ACCESS_KEY).expect("account"),
                    jurisdiction: R2Jurisdiction::Default,
                    bucket: R2BucketName::parse("kosh-test").expect("bucket"),
                },
                now_ms: 10,
            })
            .expect("first config");
        let second = client
            .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
                expected_revision: first.revision,
                backup_set_id: BackupSetId::new(),
                replica_epoch_id: ReplicaEpochId::new(),
                enabled: false,
                target: first.target.clone(),
                now_ms: 20,
            })
            .expect("second config");
        let store = FakeCredentialStore::default();
        store
            .save(
                &first.backup_set_id,
                &R2Credentials::new(ACCESS_KEY, SECRET_KEY).expect("credentials"),
            )
            .expect("retired credentials");

        clean_retired_credentials_with(&client, &store);

        assert!(!store.has_active(&first.backup_set_id));
        assert!(client
            .load_offsite_credential_cleanup()
            .expect("cleanup queue")
            .is_empty());
        assert_eq!(
            client.load_offsite_backup_config().expect("active config"),
            Some(second)
        );
    }

    #[test]
    fn synchronous_startup_leaves_remote_takeover_for_deferred_reconciliation() {
        let root = tempfile::TempDir::new().expect("temporary root");
        let database = Database::initialize(DatabasePaths::new(root.path())).expect("database");
        let client = database.client();
        let current = client
            .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
                expected_revision: 0,
                backup_set_id: BackupSetId::new(),
                replica_epoch_id: ReplicaEpochId::new(),
                enabled: false,
                target: R2Target {
                    account_id: R2AccountId::parse(ACCESS_KEY).expect("account"),
                    jurisdiction: R2Jurisdiction::Default,
                    bucket: R2BucketName::parse("kosh-test").expect("bucket"),
                },
                now_ms: 10,
            })
            .expect("configuration");
        client
            .begin_offsite_backup_takeover_intent(BeginOffsiteBackupTakeoverIntentInput {
                operation_id: uuid::Uuid::now_v7().to_string(),
                expected_revision: current.revision,
                backup_set_id: current.backup_set_id,
                previous_replica_epoch_id: current.replica_epoch_id,
                next_replica_epoch_id: ReplicaEpochId::new(),
                expected_owner_replica_epoch_id: ReplicaEpochId::new(),
                expected_owner_writer_id: BackupWriterId::new(),
                expected_owner_version: "owner-version".into(),
                next_writer_id: BackupWriterId::new(),
                created_at_ms: 20,
            })
            .expect("takeover intent");

        assert!(!reconcile_startup_backup_state(&client).expect("local startup reconciliation"));
        assert!(client
            .load_offsite_backup_takeover_intent()
            .expect("takeover intent")
            .is_some());
    }

    #[test]
    fn credential_pairs_are_all_or_nothing_and_consumed_into_zeroizing_values() {
        let mut access = Some(ACCESS_KEY.to_owned());
        let mut secret = Some(SECRET_KEY.to_owned());
        let credentials = take_credentials(&mut access, &mut secret)
            .expect("credentials")
            .expect("pair");
        assert_eq!(credentials.access_key_id(), ACCESS_KEY);
        assert_eq!(credentials.secret_access_key(), SECRET_KEY);
        assert!(access.is_none());
        assert!(secret.is_none());

        let mut access = Some(ACCESS_KEY.to_owned());
        let mut secret = None;
        assert_eq!(
            take_credentials(&mut access, &mut secret)
                .expect_err("partial pair")
                .code,
            BackupCommandErrorCode::InvalidInput
        );
    }

    #[test]
    fn target_and_identifier_validation_fail_closed() {
        assert_eq!(
            parse_target(
                "not-an-account".into(),
                R2Jurisdiction::Default,
                "kosh".into()
            )
            .expect_err("invalid account")
            .code,
            BackupCommandErrorCode::InvalidInput
        );
        assert_eq!(
            parse_checkpoint_id("not-a-checkpoint".into())
                .expect_err("invalid checkpoint")
                .code,
            BackupCommandErrorCode::InvalidInput
        );
    }

    #[test]
    fn recovery_drill_requires_conservative_free_space_before_staging() {
        let root = tempfile::TempDir::new().expect("temporary root");
        let required = (10_u64 * 2) + (20_u64 * 2) + RESTORE_DRILL_STORAGE_HEADROOM_BYTES;
        ensure_restore_drill_capacity_with(root.path(), 10, 20, |_| Ok(required))
            .expect("exact capacity");
        let error = ensure_restore_drill_capacity_with(root.path(), 10, 20, |_| Ok(required - 1))
            .expect_err("insufficient capacity");
        assert_eq!(error.code, BackupCommandErrorCode::RestoreFailed);
        assert!(error.message.contains("1 more bytes"));
    }

    #[test]
    fn recovery_drill_size_overflow_fails_closed_without_querying_the_filesystem() {
        let root = tempfile::TempDir::new().expect("temporary root");
        let queried = std::cell::Cell::new(false);
        let error = ensure_restore_drill_capacity_with(root.path(), u64::MAX, 1, |_| {
            queried.set(true);
            Ok(u64::MAX)
        })
        .expect_err("overflow");
        assert_eq!(error.code, BackupCommandErrorCode::RestoreFailed);
        assert!(!queried.get());
    }

    #[test]
    fn snapshot_json_contains_no_credential_fields() {
        let snapshot = BackupSettingsSnapshot {
            config: None,
            credential_state: BackupCredentialState::Missing,
            credential_cleanup_pending: false,
            relational: RelationalBackupStatus::default(),
            media: MediaBackupStatus::default(),
            checkpoint: CheckpointBackupStatus::default(),
            retention: BackupRetentionView {
                exact_transaction_days: 30,
                checkpoint_policy: "immutable",
                media_policy: "immutable",
            },
        };
        let json = serde_json::to_string(&snapshot).expect("snapshot JSON");
        assert!(!json.contains("accessKey"));
        assert!(!json.contains("secret"));
        assert!(json.contains("\"exactTransactionDays\":30"));
    }

    #[test]
    fn takeover_requires_the_exact_previewed_owner_fields() {
        let backup_set_id = BackupSetId::new();
        let replica_epoch_id = ReplicaEpochId::new();
        let writer_id = BackupWriterId::new();
        let target = R2Target {
            account_id: R2AccountId::parse(ACCESS_KEY).expect("account"),
            jurisdiction: R2Jurisdiction::Default,
            bucket: R2BucketName::parse("kosh-test").expect("bucket"),
        };
        let keyspace = target.keyspace(&backup_set_id);
        let store = super::super::object_store::fake::FakeObjectStore::new(keyspace.clone());
        super::super::owner::claim_remote_owner(
            &store,
            &keyspace,
            &backup_set_id,
            &replica_epoch_id,
            &writer_id,
        )
        .expect("owner");
        let owner = inspect_remote_owner(&store, &keyspace).expect("snapshot");
        let mut config = OffsiteBackupConfig {
            revision: 1,
            backup_set_id: backup_set_id.clone(),
            replica_epoch_id: replica_epoch_id.clone(),
            enabled: false,
            provider: BackupProvider::R2,
            target: target.clone(),
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        assert!(is_current_owner(&config, &owner, &writer_id));
        config.replica_epoch_id = ReplicaEpochId::new();
        assert!(!is_current_owner(&config, &owner, &writer_id));
        let exact = TakeOverBackupInput {
            expected_revision: 1,
            expected_owner_backup_set_id: backup_set_id.to_string(),
            expected_owner_replica_epoch_id: replica_epoch_id.to_string(),
            expected_owner_writer_id: writer_id.to_string(),
            expected_owner_version: owner.version().to_owned(),
            confirmation: TAKEOVER_CONFIRMATION.into(),
        };
        assert!(owner_matches_input(&owner, &exact));
        assert!(!owner_matches_input(
            &owner,
            &TakeOverBackupInput {
                expected_owner_version: "\"stale\"".into(),
                ..exact
            }
        ));
    }
}

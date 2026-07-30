use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tauri::State;

use crate::database::{
    CheckpointMediaReference, DatabaseClient, DatabaseError, OffsiteBackupConfig,
    OffsiteCheckpointScheduleState, PrepareOffsiteCheckpointInput, PreparedOffsiteCheckpoint,
};

use super::{
    credentials::{CredentialError, CredentialStore, MacOsKeychainCredentialStore},
    domain::{
        CheckpointBackupPhase, CheckpointErrorCode, CheckpointId, CheckpointManifestInput,
        CheckpointManifestV1, PublishedCheckpointEvidence, R2ObjectKey, UtcTimestamp,
        OBJECT_FORMAT_VERSION,
    },
    litestream_runtime::LitestreamCheckpointHandle,
    media_reconciler::MediaBackupWakeHandle,
    object_store::{
        ObjectContentType, ObjectMetadata, ObjectStore, ObjectStoreErrorCode, PutCondition,
        PutObjectOutcome, PutObjectRequest, R2ObjectStore,
    },
    owner::{claim_remote_owner_cancellable, RemoteOwnerError},
    writer_identity::{
        MacOsInstallationWriterIdentity, WriterIdentityError, WriterIdentityProvider,
    },
};

const SCHEDULER_POLL_INTERVAL: Duration = Duration::from_secs(5);
const QUIET_DEBOUNCE: Duration = Duration::from_secs(60);
const MAX_DIRTY_DELAY: Duration = Duration::from_secs(5 * 60);
const RETRY_DELAY: Duration = Duration::from_secs(30);
const MANUAL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MEDIA_HEAD_BATCH_SIZE: usize = 8;
const SHUTDOWN_JOIN_GRACE: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckpointBackupStatus {
    pub(crate) phase: CheckpointBackupPhase,
    pub(crate) content_revision: Option<u64>,
    pub(crate) last_published_content_revision: Option<u64>,
    pub(crate) last_published_at_ms: Option<i64>,
    pub(crate) last_error_code: Option<CheckpointErrorCode>,
}

impl Default for CheckpointBackupStatus {
    fn default() -> Self {
        Self {
            phase: CheckpointBackupPhase::Off,
            content_revision: None,
            last_published_content_revision: None,
            last_published_at_ms: None,
            last_error_code: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CheckpointWorkRevision {
    config_revision: i64,
    content_revision: u64,
}

enum CoordinatorSignal {
    BackupNow(mpsc::SyncSender<Result<(), CheckpointErrorCode>>),
    Shutdown,
}

pub(crate) struct CheckpointBackupCoordinator {
    sender: mpsc::Sender<CoordinatorSignal>,
    shutdown: Arc<AtomicBool>,
    status: Arc<Mutex<CheckpointBackupStatus>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub(crate) struct CheckpointBackupHandle {
    sender: mpsc::Sender<CoordinatorSignal>,
    shutdown: Arc<AtomicBool>,
}

impl CheckpointBackupHandle {
    pub(crate) fn backup_now(&self) -> Result<(), CheckpointErrorCode> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err(CheckpointErrorCode::WorkerUnavailable);
        }
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(CoordinatorSignal::BackupNow(reply))
            .map_err(|_| CheckpointErrorCode::WorkerUnavailable)?;
        receiver
            .recv_timeout(MANUAL_TIMEOUT)
            .map_err(|_| CheckpointErrorCode::WorkerUnavailable)?
    }
}

impl CheckpointBackupCoordinator {
    pub(crate) fn start(
        database: DatabaseClient,
        data_root: std::path::PathBuf,
        litestream: LitestreamCheckpointHandle,
        media: MediaBackupWakeHandle,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(CheckpointBackupStatus::default()));
        let worker_shutdown = Arc::clone(&shutdown);
        let worker_status = Arc::clone(&status);
        let worker = thread::Builder::new()
            .name("kosh-checkpoint-backup".into())
            .spawn(move || {
                run_scheduler(
                    database,
                    data_root,
                    litestream,
                    media,
                    receiver,
                    worker_shutdown,
                    worker_status,
                );
            });
        let worker = match worker {
            Ok(worker) => Some(worker),
            Err(_) => {
                *lock_status(&status) = CheckpointBackupStatus {
                    phase: CheckpointBackupPhase::Unavailable,
                    last_error_code: Some(CheckpointErrorCode::WorkerUnavailable),
                    ..CheckpointBackupStatus::default()
                };
                None
            }
        };
        Self {
            sender,
            shutdown,
            status,
            worker: Mutex::new(worker),
        }
    }

    pub(crate) fn disabled() -> Self {
        let (sender, receiver) = mpsc::channel();
        drop(receiver);
        Self {
            sender,
            shutdown: Arc::new(AtomicBool::new(true)),
            status: Arc::new(Mutex::new(CheckpointBackupStatus::default())),
            worker: Mutex::new(None),
        }
    }

    pub(crate) fn status(&self) -> CheckpointBackupStatus {
        lock_status(&self.status).clone()
    }

    pub(crate) fn handle(&self) -> CheckpointBackupHandle {
        CheckpointBackupHandle {
            sender: self.sender.clone(),
            shutdown: Arc::clone(&self.shutdown),
        }
    }

    pub(crate) fn shutdown(&self) {
        if self.shutdown.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.sender.send(CoordinatorSignal::Shutdown);
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let (done, completed) = mpsc::sync_channel(1);
            match thread::Builder::new()
                .name("kosh-checkpoint-backup-reaper".into())
                .spawn(move || {
                    if worker.join().is_err() {
                        log::error!("checkpoint backup coordinator panicked during shutdown");
                    }
                    let _ = done.send(());
                }) {
                Ok(_) => {
                    if completed.recv_timeout(SHUTDOWN_JOIN_GRACE).is_err() {
                        log::warn!(
                            "checkpoint backup coordinator is finishing a bounded operation in the background"
                        );
                    }
                }
                Err(error) => {
                    log::error!("could not start checkpoint backup reaper: {error}");
                }
            }
        }
    }
}

impl Drop for CheckpointBackupCoordinator {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_scheduler(
    database: DatabaseClient,
    data_root: std::path::PathBuf,
    litestream: LitestreamCheckpointHandle,
    media: MediaBackupWakeHandle,
    receiver: mpsc::Receiver<CoordinatorSignal>,
    shutdown: Arc<AtomicBool>,
    status: Arc<Mutex<CheckpointBackupStatus>>,
) {
    let _ = database.fail_incomplete_offsite_checkpoints(CheckpointErrorCode::WorkerUnavailable);
    let writer_identity = MacOsInstallationWriterIdentity::new(data_root);
    let mut first_dirty: Option<Instant> = None;
    let mut last_change: Option<Instant> = None;
    let mut observed_revision: Option<CheckpointWorkRevision> = None;
    let mut retry_not_before = Instant::now();

    loop {
        let signal = match receiver.recv_timeout(SCHEDULER_POLL_INTERVAL) {
            Ok(signal) => Some(signal),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if matches!(signal, Some(CoordinatorSignal::Shutdown)) {
            break;
        }
        let manual_reply = match signal {
            Some(CoordinatorSignal::BackupNow(reply)) => Some(reply),
            Some(CoordinatorSignal::Shutdown) | None => None,
        };

        let Some(config) = load_enabled_config(&database, &status) else {
            first_dirty = None;
            last_change = None;
            observed_revision = None;
            if let Some(reply) = manual_reply {
                let _ = reply.send(Err(CheckpointErrorCode::InvalidConfiguration));
            }
            continue;
        };
        let schedule_state = match database.load_offsite_checkpoint_schedule_state() {
            Ok(state) => state,
            Err(error) => {
                set_failure(&status, map_database_error(&error));
                if let Some(reply) = manual_reply {
                    let _ = reply.send(Err(map_database_error(&error)));
                }
                continue;
            }
        };
        update_schedule_status(&status, &schedule_state);
        let work_revision = CheckpointWorkRevision {
            config_revision: config.revision,
            content_revision: schedule_state.content_revision,
        };
        let already_published = schedule_state
            .last_published
            .as_ref()
            .is_some_and(|published| {
                published.config_revision == config.revision
                    && published.backup_set_id == config.backup_set_id
                    && published.replica_epoch_id == config.replica_epoch_id
                    && published.content_revision == schedule_state.content_revision
            });
        if already_published && manual_reply.is_none() {
            first_dirty = None;
            last_change = None;
            observed_revision = Some(work_revision);
            lock_status(&status).phase = CheckpointBackupPhase::Idle;
            continue;
        }

        let now = Instant::now();
        observe_checkpoint_work(
            &mut observed_revision,
            &mut first_dirty,
            &mut last_change,
            work_revision,
            now,
        );
        let automatic_due = automatic_checkpoint_due(first_dirty, last_change, now);
        if manual_reply.is_none() && !automatic_due {
            lock_status(&status).phase = CheckpointBackupPhase::Idle;
            continue;
        }
        if manual_reply.is_none() && now < retry_not_before {
            continue;
        }

        if manual_reply.is_some() {
            if let Ok(retried) = database.retry_failed_offsite_media_uploads(system_now_ms()) {
                if retried > 0 {
                    media.wake();
                }
            }
        }
        let result = create_checkpoint(
            &database,
            &config,
            &litestream,
            &media,
            &writer_identity,
            &status,
            &shutdown,
        );
        match result {
            Ok(()) => {
                first_dirty = None;
                last_change = None;
                let mut status = lock_status(&status);
                status.phase = CheckpointBackupPhase::Idle;
                status.last_error_code = None;
            }
            Err(code) => {
                retry_not_before = Instant::now() + RETRY_DELAY;
                set_failure(&status, code);
            }
        }
        if let Some(reply) = manual_reply {
            let _ = reply.send(result);
        }
    }
    shutdown.store(true, Ordering::Release);
}

fn load_enabled_config(
    database: &DatabaseClient,
    status: &Mutex<CheckpointBackupStatus>,
) -> Option<OffsiteBackupConfig> {
    match database.load_enabled_offsite_backup_config() {
        Ok(config) => {
            if config.is_none() {
                *lock_status(status) = CheckpointBackupStatus::default();
            }
            config
        }
        Err(error) => {
            set_failure(status, map_database_error(&error));
            None
        }
    }
}

fn create_checkpoint(
    database: &DatabaseClient,
    config: &OffsiteBackupConfig,
    litestream: &LitestreamCheckpointHandle,
    media: &MediaBackupWakeHandle,
    writer_identity: &impl WriterIdentityProvider,
    status: &Mutex<CheckpointBackupStatus>,
    shutdown: &AtomicBool,
) -> Result<(), CheckpointErrorCode> {
    ensure_running(shutdown)?;
    let progress = database
        .offsite_media_upload_progress()
        .map_err(|error| map_database_error(&error))?;
    if progress.pending != 0
        || progress.running != 0
        || progress.retry_wait != 0
        || progress.failed != 0
        || progress.untracked != 0
        || progress.uploaded != progress.referenced
    {
        media.wake();
        lock_status(status).phase = CheckpointBackupPhase::WaitingForMedia;
        return Err(CheckpointErrorCode::LocalMediaMissing);
    }

    let credentials = MacOsKeychainCredentialStore
        .load(&config.backup_set_id)
        .map_err(map_credential_error)?;
    let keyspace = config.target.keyspace(&config.backup_set_id);
    let store = R2ObjectStore::new(config.target.clone(), keyspace.clone(), &credentials)
        .map_err(|error| map_store_error(error.code))?;
    let litestream = litestream.bind(config);
    let writer_id = writer_identity.load().map_err(map_writer_identity_error)?;
    with_current_remote(database, config, || {
        claim_remote_owner_cancellable(
            &store,
            &keyspace,
            &config.backup_set_id,
            &config.replica_epoch_id,
            &writer_id,
            shutdown,
        )
        .map_err(map_owner_error)
    })?;

    let created_at_ms = system_now_ms();
    let created_at = UtcTimestamp::from_unix_millis(created_at_ms)
        .map_err(|_| CheckpointErrorCode::MalformedManifest)?;
    let checkpoint_id = CheckpointId::new();
    lock_status(status).phase = CheckpointBackupPhase::Fencing;
    let prepared = match database.prepare_offsite_checkpoint(
        PrepareOffsiteCheckpointInput {
            checkpoint_id: checkpoint_id.clone(),
            backup_set_id: config.backup_set_id.clone(),
            replica_epoch_id: config.replica_epoch_id.clone(),
            created_at_ms,
            kosh_version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        Arc::new(litestream.clone()),
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            let code = map_database_error(&error);
            if matches!(error, DatabaseError::OffsiteCheckpointFence(_)) {
                let _ = database.mark_offsite_checkpoint_failed(checkpoint_id, code);
            }
            return Err(code);
        }
    };
    ensure_running(shutdown)
        .or_else(|code| fail_checkpoint(database, checkpoint_id.clone(), code))?;
    if let Err(error) =
        database.mark_offsite_checkpoint_fenced(checkpoint_id.clone(), prepared.litestream_txid)
    {
        return fail_checkpoint(database, checkpoint_id, map_database_error(&error));
    }

    lock_status(status).phase = CheckpointBackupPhase::WaitingForReplica;
    let remote = match with_current_remote(database, config, || litestream.sync_remote()) {
        Ok(remote) => remote,
        Err(code) => return fail_checkpoint(database, checkpoint_id, code),
    };
    ensure_running(shutdown)
        .or_else(|code| fail_checkpoint(database, checkpoint_id.clone(), code))?;
    if !replica_covers(remote.replica_txid, prepared.litestream_txid) {
        return fail_checkpoint(database, checkpoint_id, CheckpointErrorCode::ReplicaBehind);
    }
    if let Err(error) = database.mark_offsite_checkpoint_replicated(checkpoint_id.clone()) {
        return fail_checkpoint(database, checkpoint_id, map_database_error(&error));
    }

    lock_status(status).phase = CheckpointBackupPhase::Validating;
    if let Err(code) =
        validate_remote_media(database, config, &store, &keyspace, &prepared, shutdown)
    {
        return fail_checkpoint(database, checkpoint_id, code);
    }

    lock_status(status).phase = CheckpointBackupPhase::Publishing;
    let publication = (|| {
        let manifest = CheckpointManifestV1::new(CheckpointManifestInput {
            backup_set_id: prepared.backup_set_id.clone(),
            replica_epoch_id: prepared.replica_epoch_id.clone(),
            checkpoint_id: prepared.checkpoint_id.clone(),
            created_at,
            kosh_version: prepared.kosh_version.clone(),
            content_revision: prepared.content_revision,
            main_migration_head: prepared.main_migration_head,
            litestream_path: keyspace.litestream(&prepared.replica_epoch_id),
            txid: prepared.litestream_txid.to_string(),
            media_migration_head: prepared.media_migration_head,
            referenced_hash_count: prepared.referenced_media.len() as u64,
            referenced_total_bytes: prepared.referenced_total_bytes,
            referenced_hash_set_sha256: prepared.referenced_hash_set_sha256,
        })
        .map_err(|_| CheckpointErrorCode::MalformedManifest)?;
        let bytes = manifest
            .to_json()
            .map_err(|_| CheckpointErrorCode::MalformedManifest)?;
        let key = manifest
            .object_key(&keyspace)
            .map_err(|_| CheckpointErrorCode::MalformedManifest)?;
        publish_manifest(
            &ManifestPublicationContext {
                database,
                config,
                store: &store,
                keyspace: &keyspace,
                shutdown,
            },
            &key,
            &bytes,
            &manifest,
            &prepared,
        )?;
        Ok::<_, CheckpointErrorCode>(key)
    })();
    let key = match publication {
        Ok(key) => key,
        Err(code) => return fail_checkpoint(database, checkpoint_id, code),
    };
    database
        .mark_offsite_checkpoint_published(checkpoint_id.clone(), key.as_str().to_owned())
        .map_err(|error| map_database_error(&error))
        .or_else(|code| fail_checkpoint(database, checkpoint_id, code))
}

fn automatic_checkpoint_due(
    first_dirty: Option<Instant>,
    last_change: Option<Instant>,
    now: Instant,
) -> bool {
    last_change.is_some_and(|changed| now.duration_since(changed) >= QUIET_DEBOUNCE)
        || first_dirty.is_some_and(|dirty| now.duration_since(dirty) >= MAX_DIRTY_DELAY)
}

fn observe_checkpoint_work(
    observed: &mut Option<CheckpointWorkRevision>,
    first_dirty: &mut Option<Instant>,
    last_change: &mut Option<Instant>,
    current: CheckpointWorkRevision,
    now: Instant,
) {
    if *observed != Some(current) {
        *observed = Some(current);
        first_dirty.get_or_insert(now);
        *last_change = Some(now);
    }
}

fn replica_covers(
    replica_txid: Option<super::litestream::LitestreamTxid>,
    fenced_txid: super::litestream::LitestreamTxid,
) -> bool {
    replica_txid.is_some_and(|replica| replica >= fenced_txid)
}

fn validate_remote_media(
    database: &DatabaseClient,
    config: &OffsiteBackupConfig,
    store: &dyn ObjectStore,
    keyspace: &super::domain::R2Keyspace,
    prepared: &PreparedOffsiteCheckpoint,
    shutdown: &AtomicBool,
) -> Result<(), CheckpointErrorCode> {
    for batch in prepared.referenced_media.chunks(MEDIA_HEAD_BATCH_SIZE) {
        ensure_running(shutdown)?;
        let metadata = with_current_remote(database, config, || {
            thread::scope(|scope| {
                let requests = batch
                    .iter()
                    .map(|reference| {
                        let key = keyspace.media(reference.sha256);
                        scope.spawn(move || {
                            store
                                .head(&key)
                                .map_err(|error| map_media_store_error(error.code))
                        })
                    })
                    .collect::<Vec<_>>();
                requests
                    .into_iter()
                    .map(|request| {
                        request
                            .join()
                            .map_err(|_| CheckpointErrorCode::WorkerUnavailable)?
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
        })?;
        for (reference, metadata) in batch.iter().zip(metadata) {
            let Some(metadata) = metadata else {
                let _ = database.requeue_uploaded_offsite_media(
                    config.backup_set_id.clone(),
                    reference.sha256,
                    system_now_ms(),
                );
                return Err(CheckpointErrorCode::RemoteMediaMissing);
            };
            if !media_metadata_matches(&metadata, reference) {
                let _ = database.requeue_uploaded_offsite_media(
                    config.backup_set_id.clone(),
                    reference.sha256,
                    system_now_ms(),
                );
                return Err(CheckpointErrorCode::RemoteMediaCorrupt);
            }
        }
    }
    Ok(())
}

fn media_metadata_matches(metadata: &ObjectMetadata, reference: &CheckpointMediaReference) -> bool {
    metadata.byte_length == reference.byte_length
        && metadata.content_type == Some(ObjectContentType::Binary)
        && metadata.kosh_sha256 == Some(reference.sha256)
        && metadata.object_format_version == Some(OBJECT_FORMAT_VERSION)
}

struct ManifestPublicationContext<'a> {
    database: &'a DatabaseClient,
    config: &'a OffsiteBackupConfig,
    store: &'a dyn ObjectStore,
    keyspace: &'a super::domain::R2Keyspace,
    shutdown: &'a AtomicBool,
}

fn publish_manifest(
    context: &ManifestPublicationContext<'_>,
    key: &R2ObjectKey,
    expected_bytes: &[u8],
    expected: &CheckpointManifestV1,
    prepared: &PreparedOffsiteCheckpoint,
) -> Result<(), CheckpointErrorCode> {
    let outcome = with_current_remote(context.database, context.config, || {
        ensure_running(context.shutdown)?;
        context
            .store
            .put(PutObjectRequest {
                key: key.clone(),
                bytes: expected_bytes.to_vec(),
                content_type: ObjectContentType::Json,
                kosh_sha256: None,
                condition: PutCondition::IfAbsent,
            })
            .map_err(|error| map_store_error(error.code))
    })?;
    ensure_running(context.shutdown)?;
    let readback = with_current_remote(context.database, context.config, || {
        context
            .store
            .get_bounded(key, MAX_MANIFEST_BYTES)
            .map_err(|error| map_manifest_store_error(error.code))
    })?;
    if readback.metadata.byte_length != expected_bytes.len() as u64
        || readback.metadata.content_type != Some(ObjectContentType::Json)
        || readback.metadata.kosh_sha256.is_some()
        || readback.metadata.object_format_version != Some(OBJECT_FORMAT_VERSION)
        || readback.bytes != expected_bytes
    {
        return Err(match outcome {
            PutObjectOutcome::ConditionNotMet => CheckpointErrorCode::ImmutableObjectConflict,
            PutObjectOutcome::Stored => CheckpointErrorCode::MalformedManifest,
        });
    }
    let decoded = CheckpointManifestV1::from_json(&readback.bytes, context.keyspace)
        .map_err(|_| CheckpointErrorCode::MalformedManifest)?;
    let evidence = PublishedCheckpointEvidence {
        checkpoint_id: &prepared.checkpoint_id,
        backup_set_id: &prepared.backup_set_id,
        replica_epoch_id: &prepared.replica_epoch_id,
        content_revision: prepared.content_revision,
        kosh_version: &prepared.kosh_version,
        main_migration_head: prepared.main_migration_head,
        media_migration_head: prepared.media_migration_head,
        referenced_hash_count: prepared.referenced_media.len() as u64,
        referenced_total_bytes: prepared.referenced_total_bytes,
        referenced_hash_set_sha256: prepared.referenced_hash_set_sha256,
        litestream_txid: &prepared.litestream_txid.to_string(),
    };
    if decoded != *expected
        || decoded
            .object_key(context.keyspace)
            .map_err(|_| CheckpointErrorCode::MalformedManifest)?
            != *key
        || !decoded.matches_published_evidence(&evidence)
    {
        return Err(CheckpointErrorCode::MalformedManifest);
    }
    Ok(())
}

fn ensure_running(shutdown: &AtomicBool) -> Result<(), CheckpointErrorCode> {
    if shutdown.load(Ordering::Acquire) {
        Err(CheckpointErrorCode::WorkerUnavailable)
    } else {
        Ok(())
    }
}

fn with_current_remote<T>(
    database: &DatabaseClient,
    config: &OffsiteBackupConfig,
    operation: impl FnOnce() -> Result<T, CheckpointErrorCode>,
) -> Result<T, CheckpointErrorCode> {
    match database.with_current_offsite_checkpoint(config, operation) {
        Ok(Some(result)) => result,
        Ok(None) => Err(CheckpointErrorCode::InvalidConfiguration),
        Err(error) => Err(map_database_error(&error)),
    }
}

fn fail_checkpoint(
    database: &DatabaseClient,
    checkpoint_id: CheckpointId,
    code: CheckpointErrorCode,
) -> Result<(), CheckpointErrorCode> {
    let _ = database.mark_offsite_checkpoint_failed(checkpoint_id, code);
    Err(code)
}

fn update_schedule_status(
    status: &Mutex<CheckpointBackupStatus>,
    state: &OffsiteCheckpointScheduleState,
) {
    let mut status = lock_status(status);
    status.content_revision = Some(state.content_revision);
    status.last_published_content_revision = state
        .last_published
        .as_ref()
        .map(|value| value.content_revision);
    status.last_published_at_ms = state
        .last_published
        .as_ref()
        .map(|value| value.created_at_ms);
}

fn set_failure(status: &Mutex<CheckpointBackupStatus>, code: CheckpointErrorCode) {
    let mut status = lock_status(status);
    status.phase = match code {
        CheckpointErrorCode::CredentialsMissing => CheckpointBackupPhase::Unavailable,
        CheckpointErrorCode::OwnerConflict | CheckpointErrorCode::OwnerInvalid => {
            CheckpointBackupPhase::Blocked
        }
        CheckpointErrorCode::LocalMediaMissing => CheckpointBackupPhase::WaitingForMedia,
        _ => CheckpointBackupPhase::Degraded,
    };
    status.last_error_code = Some(code);
}

fn map_database_error(error: &DatabaseError) -> CheckpointErrorCode {
    match error {
        DatabaseError::OffsiteCheckpointFence(code) => *code,
        DatabaseError::OffsiteCheckpointMediaIncomplete => CheckpointErrorCode::LocalMediaMissing,
        DatabaseError::InvalidOffsiteBackupConfig(_)
        | DatabaseError::StaleOffsiteBackupConfig
        | DatabaseError::InvalidOffsiteCheckpoint(_)
        | DatabaseError::StaleOffsiteCheckpoint => CheckpointErrorCode::InvalidConfiguration,
        _ => CheckpointErrorCode::WorkerUnavailable,
    }
}

fn map_credential_error(error: CredentialError) -> CheckpointErrorCode {
    match error {
        CredentialError::Missing => CheckpointErrorCode::CredentialsMissing,
        CredentialError::Unavailable => CheckpointErrorCode::KeychainUnavailable,
        CredentialError::InvalidCredential(_)
        | CredentialError::UnsupportedPayloadVersion
        | CredentialError::CorruptPayload => CheckpointErrorCode::InvalidConfiguration,
    }
}

fn map_writer_identity_error(error: WriterIdentityError) -> CheckpointErrorCode {
    match error {
        WriterIdentityError::Unavailable | WriterIdentityError::Invalid => {
            CheckpointErrorCode::InvalidConfiguration
        }
    }
}

fn map_owner_error(error: RemoteOwnerError) -> CheckpointErrorCode {
    match error {
        RemoteOwnerError::Cancelled => CheckpointErrorCode::WorkerUnavailable,
        RemoteOwnerError::Conflict => CheckpointErrorCode::OwnerConflict,
        RemoteOwnerError::Invalid => CheckpointErrorCode::OwnerInvalid,
        RemoteOwnerError::Store(error) => map_store_error(error.code),
    }
}

fn map_store_error(code: ObjectStoreErrorCode) -> CheckpointErrorCode {
    match code {
        ObjectStoreErrorCode::Network => CheckpointErrorCode::Network,
        ObjectStoreErrorCode::Timeout => CheckpointErrorCode::NetworkTimeout,
        ObjectStoreErrorCode::AuthenticationRejected => CheckpointErrorCode::AuthenticationRejected,
        ObjectStoreErrorCode::AuthorizationRejected => CheckpointErrorCode::AuthorizationRejected,
        ObjectStoreErrorCode::RateLimited => CheckpointErrorCode::RateLimited,
        ObjectStoreErrorCode::ServiceUnavailable => CheckpointErrorCode::ServiceUnavailable,
        ObjectStoreErrorCode::NotFound => CheckpointErrorCode::RemoteMediaMissing,
        ObjectStoreErrorCode::Conflict | ObjectStoreErrorCode::PreconditionFailed => {
            CheckpointErrorCode::ImmutableObjectConflict
        }
        ObjectStoreErrorCode::InvalidConfiguration
        | ObjectStoreErrorCode::KeyOutsidePrefix
        | ObjectStoreErrorCode::DeletionNotAuthorized
        | ObjectStoreErrorCode::MediaWriteRequiresVerifiedCreate
        | ObjectStoreErrorCode::ContentHashMismatch
        | ObjectStoreErrorCode::ObjectTooLarge
        | ObjectStoreErrorCode::ResponseTooLarge
        | ObjectStoreErrorCode::InvalidResponse => CheckpointErrorCode::InvalidConfiguration,
    }
}

fn map_media_store_error(code: ObjectStoreErrorCode) -> CheckpointErrorCode {
    match code {
        ObjectStoreErrorCode::NotFound => CheckpointErrorCode::RemoteMediaMissing,
        ObjectStoreErrorCode::InvalidResponse
        | ObjectStoreErrorCode::ContentHashMismatch
        | ObjectStoreErrorCode::ObjectTooLarge
        | ObjectStoreErrorCode::ResponseTooLarge => CheckpointErrorCode::RemoteMediaCorrupt,
        _ => map_store_error(code),
    }
}

fn map_manifest_store_error(code: ObjectStoreErrorCode) -> CheckpointErrorCode {
    match code {
        ObjectStoreErrorCode::NotFound
        | ObjectStoreErrorCode::InvalidResponse
        | ObjectStoreErrorCode::ResponseTooLarge => CheckpointErrorCode::MalformedManifest,
        _ => map_store_error(code),
    }
}

fn system_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

fn lock_status(
    status: &Mutex<CheckpointBackupStatus>,
) -> std::sync::MutexGuard<'_, CheckpointBackupStatus> {
    status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[tauri::command]
pub(crate) fn checkpoint_backup_status(
    state: State<'_, crate::runtime::RuntimeState>,
) -> CheckpointBackupStatus {
    state.checkpoint_backup_status()
}

#[tauri::command]
pub(crate) async fn backup_now(
    state: State<'_, crate::runtime::RuntimeState>,
) -> Result<(), CheckpointErrorCode> {
    let handle = state.checkpoint_backup_handle();
    tauri::async_runtime::spawn_blocking(move || handle.backup_now())
        .await
        .map_err(|_| CheckpointErrorCode::WorkerUnavailable)?
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    use super::*;
    use crate::{
        backup::{
            domain::{
                BackupSetId, ContentSha256, R2AccountId, R2BucketName, R2Jurisdiction, R2Target,
                ReplicaEpochId,
            },
            litestream::LitestreamTxid,
            litestream_runtime::LitestreamRuntimeService,
            media_reconciler::MediaBackupCoordinator,
            object_store::{
                fake::{FakeObjectStore, ObjectOperation},
                PutMediaRequest,
            },
        },
        database::{Database, DatabasePaths, SaveOffsiteBackupConfigInput},
    };

    fn target() -> R2Target {
        R2Target {
            account_id: R2AccountId::parse("0123456789abcdef0123456789abcdef").expect("account"),
            jurisdiction: R2Jurisdiction::Default,
            bucket: R2BucketName::parse("kosh-checkpoint-test").expect("bucket"),
        }
    }

    fn enabled_database(
        backup_set_id: BackupSetId,
        replica_epoch_id: ReplicaEpochId,
        target: R2Target,
    ) -> (TempDir, Database, OffsiteBackupConfig) {
        let root = TempDir::new().expect("temporary database");
        let database = Database::initialize(DatabasePaths::new(root.path())).expect("database");
        let config = database
            .client()
            .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
                expected_revision: 0,
                backup_set_id,
                replica_epoch_id,
                enabled: true,
                target,
                now_ms: 1,
            })
            .expect("enabled backup");
        (root, database, config)
    }

    fn prepared(
        backup_set_id: BackupSetId,
        replica_epoch_id: ReplicaEpochId,
        references: Vec<CheckpointMediaReference>,
    ) -> PreparedOffsiteCheckpoint {
        let referenced_total_bytes = references.iter().map(|item| item.byte_length).sum();
        let mut digest = Sha256::new();
        for reference in &references {
            digest.update(reference.sha256.as_bytes());
        }
        PreparedOffsiteCheckpoint {
            checkpoint_id: CheckpointId::new(),
            backup_set_id,
            replica_epoch_id,
            created_at_ms: 1_000,
            kosh_version: "test".into(),
            config_revision: 1,
            content_revision: 9,
            main_migration_head: 20,
            media_migration_head: 2,
            referenced_media: references,
            referenced_total_bytes,
            referenced_hash_set_sha256: ContentSha256::from_bytes(digest.finalize().into()),
            litestream_txid: LitestreamTxid::from_local(42),
        }
    }

    fn manifest(
        keyspace: &super::super::domain::R2Keyspace,
        prepared: &PreparedOffsiteCheckpoint,
    ) -> (CheckpointManifestV1, R2ObjectKey, Vec<u8>) {
        let manifest = CheckpointManifestV1::new(CheckpointManifestInput {
            backup_set_id: prepared.backup_set_id.clone(),
            replica_epoch_id: prepared.replica_epoch_id.clone(),
            checkpoint_id: prepared.checkpoint_id.clone(),
            created_at: UtcTimestamp::from_unix_millis(prepared.created_at_ms).expect("timestamp"),
            kosh_version: prepared.kosh_version.clone(),
            content_revision: prepared.content_revision,
            main_migration_head: prepared.main_migration_head,
            litestream_path: keyspace.litestream(&prepared.replica_epoch_id),
            txid: prepared.litestream_txid.to_string(),
            media_migration_head: prepared.media_migration_head,
            referenced_hash_count: prepared.referenced_media.len() as u64,
            referenced_total_bytes: prepared.referenced_total_bytes,
            referenced_hash_set_sha256: prepared.referenced_hash_set_sha256,
        })
        .expect("manifest");
        let key = manifest.object_key(keyspace).expect("manifest key");
        let bytes = manifest.to_json().expect("manifest JSON");
        (manifest, key, bytes)
    }

    #[test]
    fn scheduler_honors_both_quiet_debounce_and_maximum_dirty_delay() {
        let start = Instant::now();
        assert!(!automatic_checkpoint_due(
            Some(start),
            Some(start),
            start + Duration::from_secs(59)
        ));
        assert!(automatic_checkpoint_due(
            Some(start),
            Some(start),
            start + QUIET_DEBOUNCE
        ));
        assert!(automatic_checkpoint_due(
            Some(start),
            Some(start + Duration::from_secs(299)),
            start + MAX_DIRTY_DELAY
        ));
    }

    #[test]
    fn scheduler_rearms_when_only_the_backup_configuration_revision_changes() {
        let start = Instant::now();
        let mut observed = None;
        let mut first_dirty = None;
        let mut last_change = None;
        observe_checkpoint_work(
            &mut observed,
            &mut first_dirty,
            &mut last_change,
            CheckpointWorkRevision {
                config_revision: 1,
                content_revision: 7,
            },
            start,
        );
        first_dirty = None;
        last_change = None;

        let changed_at = start + Duration::from_secs(1);
        observe_checkpoint_work(
            &mut observed,
            &mut first_dirty,
            &mut last_change,
            CheckpointWorkRevision {
                config_revision: 2,
                content_revision: 7,
            },
            changed_at,
        );
        assert_eq!(first_dirty, Some(changed_at));
        assert_eq!(last_change, Some(changed_at));
    }

    #[test]
    fn exact_replica_txid_is_required_before_publication() {
        let fence = LitestreamTxid::from_local(42);
        assert!(!replica_covers(None, fence));
        assert!(!replica_covers(Some(LitestreamTxid::from_local(41)), fence));
        assert!(replica_covers(Some(LitestreamTxid::from_local(42)), fence));
        assert!(replica_covers(Some(LitestreamTxid::from_local(43)), fence));
    }

    #[test]
    fn every_media_head_precedes_immutable_manifest_put_and_exact_readback() {
        let backup_set_id = BackupSetId::new();
        let replica_epoch_id = ReplicaEpochId::new();
        let target = target();
        let (_root, database, config) = enabled_database(
            backup_set_id.clone(),
            replica_epoch_id.clone(),
            target.clone(),
        );
        let keyspace = target.keyspace(&backup_set_id);
        let store = FakeObjectStore::new(keyspace.clone());
        let references = (0_u8..9)
            .map(|index| {
                let bytes = format!("checkpoint media {index}").into_bytes();
                let sha256 = ContentSha256::from_bytes(Sha256::digest(&bytes).into());
                store
                    .put_media(
                        PutMediaRequest::new(&keyspace, sha256, bytes.clone())
                            .expect("media request"),
                    )
                    .expect("seed media");
                CheckpointMediaReference {
                    sha256,
                    byte_length: bytes.len() as u64,
                }
            })
            .collect();
        store.clear_operations();
        let prepared = prepared(backup_set_id.clone(), replica_epoch_id.clone(), references);
        let client = database.client();
        let shutdown = AtomicBool::new(false);
        validate_remote_media(&client, &config, &store, &keyspace, &prepared, &shutdown)
            .expect("validate media");
        let (manifest, key, bytes) = manifest(&keyspace, &prepared);
        let publication = ManifestPublicationContext {
            database: &client,
            config: &config,
            store: &store,
            keyspace: &keyspace,
            shutdown: &shutdown,
        };
        publish_manifest(&publication, &key, &bytes, &manifest, &prepared)
            .expect("publish manifest");

        let mut expected_operations = vec![ObjectOperation::Head; 9];
        expected_operations.extend([ObjectOperation::Put, ObjectOperation::Get]);
        assert_eq!(store.operations(), expected_operations);
    }

    #[test]
    fn manifest_publication_is_idempotent_only_for_identical_immutable_bytes() {
        let backup_set_id = BackupSetId::new();
        let replica_epoch_id = ReplicaEpochId::new();
        let target = target();
        let (_root, database, config) = enabled_database(
            backup_set_id.clone(),
            replica_epoch_id.clone(),
            target.clone(),
        );
        let keyspace = target.keyspace(&backup_set_id);
        let prepared = prepared(backup_set_id, replica_epoch_id, Vec::new());
        let store = FakeObjectStore::new(keyspace.clone());
        let (manifest, key, bytes) = manifest(&keyspace, &prepared);
        let client = database.client();
        let shutdown = AtomicBool::new(false);
        let publication = ManifestPublicationContext {
            database: &client,
            config: &config,
            store: &store,
            keyspace: &keyspace,
            shutdown: &shutdown,
        };

        publish_manifest(&publication, &key, &bytes, &manifest, &prepared)
            .expect("first publication");
        publish_manifest(&publication, &key, &bytes, &manifest, &prepared)
            .expect("identical replay");
        assert_eq!(
            publish_manifest(
                &publication,
                &key,
                b"{\"different\":true}",
                &manifest,
                &prepared,
            )
            .expect_err("conflicting immutable bytes"),
            CheckpointErrorCode::ImmutableObjectConflict
        );
    }

    #[test]
    fn absent_or_corrupt_remote_media_blocks_manifest_publication() {
        let backup_set_id = BackupSetId::new();
        let replica_epoch_id = ReplicaEpochId::new();
        let target = target();
        let (_root, database, config) = enabled_database(
            backup_set_id.clone(),
            replica_epoch_id.clone(),
            target.clone(),
        );
        let keyspace = target.keyspace(&backup_set_id);
        let store = FakeObjectStore::new(keyspace.clone());
        let prepared = prepared(
            backup_set_id.clone(),
            replica_epoch_id,
            vec![CheckpointMediaReference {
                sha256: ContentSha256::from_bytes([0xab; 32]),
                byte_length: 10,
            }],
        );
        let shutdown = AtomicBool::new(false);
        assert_eq!(
            validate_remote_media(
                &database.client(),
                &config,
                &store,
                &keyspace,
                &prepared,
                &shutdown,
            )
            .expect_err("missing media"),
            CheckpointErrorCode::RemoteMediaMissing
        );
        assert_eq!(store.operations(), [ObjectOperation::Head]);
    }

    #[test]
    fn manifest_readback_transport_failure_is_not_recorded_as_published() {
        let backup_set_id = BackupSetId::new();
        let replica_epoch_id = ReplicaEpochId::new();
        let target = target();
        let (_root, database, config) = enabled_database(
            backup_set_id.clone(),
            replica_epoch_id.clone(),
            target.clone(),
        );
        let keyspace = target.keyspace(&backup_set_id);
        let prepared = prepared(backup_set_id, replica_epoch_id, Vec::new());
        let store = FakeObjectStore::new(keyspace.clone());
        let (manifest, key, bytes) = manifest(&keyspace, &prepared);
        store.fail_next(ObjectOperation::Get, ObjectStoreErrorCode::Network);
        let client = database.client();
        let shutdown = AtomicBool::new(false);
        let publication = ManifestPublicationContext {
            database: &client,
            config: &config,
            store: &store,
            keyspace: &keyspace,
            shutdown: &shutdown,
        };
        assert_eq!(
            publish_manifest(&publication, &key, &bytes, &manifest, &prepared,)
                .expect_err("readback failure"),
            CheckpointErrorCode::Network
        );
    }

    #[test]
    fn manual_backup_request_round_trips_through_worker_without_an_enabled_target() {
        let root = TempDir::new().expect("temporary database");
        let database = Database::initialize(DatabasePaths::new(root.path())).expect("database");
        let litestream = LitestreamRuntimeService::disabled();
        let media = MediaBackupCoordinator::disabled();
        let coordinator = CheckpointBackupCoordinator::start(
            database.client(),
            root.path().to_owned(),
            litestream.checkpoint_handle(),
            media.wake_handle(),
        );

        assert_eq!(
            coordinator
                .handle()
                .backup_now()
                .expect_err("disabled target"),
            CheckpointErrorCode::InvalidConfiguration
        );
        assert_eq!(coordinator.status().phase, CheckpointBackupPhase::Off);
        coordinator.shutdown();
    }
}

use std::{
    io::Read,
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, OptionalExtension, MAIN_DB};
use sha2::{Digest, Sha256};

use crate::database::{
    connection::{self, DatabaseKind, MAX_MEDIA_BLOB_BYTES},
    DatabaseClient, DatabasePaths, OffsiteMediaUploadClaim, OffsiteMediaUploadFailureCode,
};

use super::{
    credentials::{CredentialError, CredentialStore, MacOsKeychainCredentialStore},
    domain::{ContentSha256, OBJECT_FORMAT_VERSION},
    object_store::{
        ObjectContentType, ObjectMetadata, ObjectStore, ObjectStoreError, ObjectStoreErrorCode,
        PutMediaRequest, R2ObjectStore,
    },
};

const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(30);
const SHUTDOWN_JOIN_GRACE: Duration = Duration::from_millis(250);
const INITIAL_RETRY_DELAY_MS: i64 = 5_000;
const MAX_RETRY_DELAY_MS: i64 = 15 * 60 * 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MediaUploadDisposition {
    Uploaded,
    RetryScheduled,
    PermanentlyFailed,
    Stale,
}

#[derive(Default)]
struct CoordinatorControlState {
    shutdown: bool,
    generation: u64,
}

#[derive(Default)]
struct CoordinatorControl {
    state: Mutex<CoordinatorControlState>,
    changed: Condvar,
}

pub(crate) struct MediaBackupCoordinator {
    control: Arc<CoordinatorControl>,
    worker: Option<JoinHandle<()>>,
    completed: Option<Mutex<Receiver<()>>>,
}

#[derive(Clone)]
pub(crate) struct MediaBackupWakeHandle {
    control: Arc<CoordinatorControl>,
}

impl MediaBackupWakeHandle {
    pub(crate) fn wake(&self) {
        wake_control(&self.control);
    }
}

impl MediaBackupCoordinator {
    pub(crate) fn start(
        client: DatabaseClient,
        paths: DatabasePaths,
    ) -> crate::database::Result<Self> {
        let control = Arc::new(CoordinatorControl::default());
        let worker_control = Arc::clone(&control);
        let (completed_sender, completed_receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("kosh-offsite-media".into())
            .spawn(move || {
                run_coordinator(client, paths, worker_control);
                let _ = completed_sender.send(());
            })?;
        Ok(Self {
            control,
            worker: Some(worker),
            completed: Some(Mutex::new(completed_receiver)),
        })
    }

    pub(crate) fn disabled() -> Self {
        Self {
            control: Arc::new(CoordinatorControl::default()),
            worker: None,
            completed: None,
        }
    }

    pub(crate) fn wake(&self) {
        wake_control(&self.control);
    }

    pub(crate) fn wake_handle(&self) -> MediaBackupWakeHandle {
        MediaBackupWakeHandle {
            control: Arc::clone(&self.control),
        }
    }
}

fn wake_control(control: &CoordinatorControl) {
    let mut state = control
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.generation = state.generation.wrapping_add(1);
    control.changed.notify_one();
}

impl Drop for MediaBackupCoordinator {
    fn drop(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        {
            let mut state = self
                .control
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.shutdown = true;
            state.generation = state.generation.wrapping_add(1);
            self.control.changed.notify_all();
        }
        let finished = self.completed.take().is_some_and(|completed| {
            let completed = completed
                .into_inner()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match completed.recv_timeout(SHUTDOWN_JOIN_GRACE) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => true,
                Err(RecvTimeoutError::Timeout) => false,
            }
        });
        if !finished {
            log::warn!(
                "off-site media worker did not stop within the shutdown grace period; detaching it"
            );
            return;
        }
        if worker.join().is_err() {
            log::warn!("off-site media worker stopped unexpectedly");
        }
    }
}

fn run_coordinator(client: DatabaseClient, paths: DatabasePaths, control: Arc<CoordinatorControl>) {
    let now_ms = system_now_ms();
    if let Err(error) = client.recover_interrupted_offsite_media_uploads(now_ms) {
        log::warn!("off-site media upload recovery is degraded: {error}");
    }
    if let Err(error) = client.reconcile_offsite_media_uploads(now_ms) {
        log::warn!("off-site media reconciliation is degraded: {error}");
    }

    loop {
        let observed_generation = {
            let state = control
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.shutdown {
                break;
            }
            state.generation
        };
        let disposition = process_next_production(&client, &paths);
        if matches!(
            disposition,
            Some(MediaUploadDisposition::Uploaded | MediaUploadDisposition::PermanentlyFailed)
        ) {
            continue;
        }

        let state = control
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.shutdown {
            break;
        }
        if state.generation == observed_generation {
            let _ = control
                .changed
                .wait_timeout(state, IDLE_POLL_INTERVAL)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
}

fn process_next_production(
    client: &DatabaseClient,
    paths: &DatabasePaths,
) -> Option<MediaUploadDisposition> {
    let claim = match client
        .claim_next_offsite_media_upload(system_now_ms(), uuid::Uuid::now_v7().to_string())
    {
        Ok(Some(claim)) => claim,
        Ok(None) => return None,
        Err(error) => {
            log::warn!("off-site media upload claim is unavailable: {error}");
            return None;
        }
    };
    let credentials = match MacOsKeychainCredentialStore.load(&claim.config.backup_set_id) {
        Ok(credentials) => credentials,
        Err(error) => {
            let code = match error {
                CredentialError::Missing => OffsiteMediaUploadFailureCode::CredentialsMissing,
                _ => OffsiteMediaUploadFailureCode::CredentialsUnavailable,
            };
            return Some(record_failure(client, claim, code, true, system_now_ms()));
        }
    };
    let keyspace = claim.config.target.keyspace(&claim.config.backup_set_id);
    let store = match R2ObjectStore::new(claim.config.target.clone(), keyspace, &credentials) {
        Ok(store) => store,
        Err(error) => {
            let (code, retryable) = classify_object_store_error(error);
            return Some(record_failure(
                client,
                claim,
                code,
                retryable,
                system_now_ms(),
            ));
        }
    };
    Some(process_claim(client, paths, &store, claim, &system_now_ms))
}

pub(crate) fn process_claim(
    client: &DatabaseClient,
    paths: &DatabasePaths,
    store: &dyn ObjectStore,
    claim: OffsiteMediaUploadClaim,
    clock: &dyn Fn() -> i64,
) -> MediaUploadDisposition {
    let bytes = match load_local_blob(paths, claim.sha256) {
        Ok(bytes) => bytes,
        Err(LocalBlobError::Missing) => {
            return record_failure(
                client,
                claim,
                OffsiteMediaUploadFailureCode::LocalBlobMissing,
                false,
                clock(),
            );
        }
        Err(LocalBlobError::Invalid) => {
            return record_failure(
                client,
                claim,
                OffsiteMediaUploadFailureCode::LocalBlobInvalid,
                false,
                clock(),
            );
        }
        Err(LocalBlobError::Unavailable(error)) => {
            log::warn!("off-site media read is temporarily unavailable: {error}");
            return record_failure(
                client,
                claim,
                OffsiteMediaUploadFailureCode::LocalBlobUnavailable,
                true,
                clock(),
            );
        }
    };
    let keyspace = claim.config.target.keyspace(&claim.config.backup_set_id);
    let expected_length = u64::try_from(bytes.len()).expect("media object length fits u64");
    let request = match PutMediaRequest::new(&keyspace, claim.sha256, bytes) {
        Ok(request) => request,
        Err(error) => {
            let (code, retryable) = classify_object_store_error(error);
            return record_failure(client, claim, code, retryable, clock());
        }
    };
    let key = keyspace.media(claim.sha256);
    let remote = client.with_current_offsite_media_upload(&claim, || {
        store.put_media(request)?;
        store
            .head(&key)?
            .ok_or_else(|| ObjectStoreError::new(ObjectStoreErrorCode::InvalidResponse))
    });
    let metadata = match remote {
        Ok(Some(Ok(metadata))) => metadata,
        Ok(Some(Err(error))) => {
            let (code, retryable) = classify_object_store_error(error);
            return record_failure(client, claim, code, retryable, clock());
        }
        Ok(None) => return MediaUploadDisposition::Stale,
        Err(error) => {
            log::warn!("off-site media upload fence could not be validated: {error}");
            return MediaUploadDisposition::Stale;
        }
    };
    if !remote_media_matches(&metadata, claim.sha256, expected_length) {
        return record_failure(
            client,
            claim,
            OffsiteMediaUploadFailureCode::RemoteObjectMismatch,
            false,
            clock(),
        );
    }
    match client.complete_offsite_media_upload(claim, metadata.version.as_str().to_owned(), clock())
    {
        Ok(true) => MediaUploadDisposition::Uploaded,
        Ok(false) => MediaUploadDisposition::Stale,
        Err(error) => {
            log::warn!("off-site media completion could not be recorded: {error}");
            MediaUploadDisposition::Stale
        }
    }
}

fn remote_media_matches(
    metadata: &ObjectMetadata,
    sha256: ContentSha256,
    byte_length: u64,
) -> bool {
    metadata.byte_length == byte_length
        && metadata.content_type == Some(ObjectContentType::Binary)
        && metadata.kosh_sha256 == Some(sha256)
        && metadata.object_format_version == Some(OBJECT_FORMAT_VERSION)
}

fn record_failure(
    client: &DatabaseClient,
    claim: OffsiteMediaUploadClaim,
    code: OffsiteMediaUploadFailureCode,
    retryable: bool,
    now_ms: i64,
) -> MediaUploadDisposition {
    let retry_at_ms = retryable.then(|| now_ms.saturating_add(retry_delay_ms(claim.attempt_count)));
    match client.fail_offsite_media_upload(claim, code, retry_at_ms, now_ms) {
        Ok(true) if retryable => MediaUploadDisposition::RetryScheduled,
        Ok(true) => MediaUploadDisposition::PermanentlyFailed,
        Ok(false) => MediaUploadDisposition::Stale,
        Err(error) => {
            log::warn!("off-site media failure could not be recorded: {error}");
            MediaUploadDisposition::Stale
        }
    }
}

fn retry_delay_ms(attempt_count: u32) -> i64 {
    let exponent = attempt_count.saturating_sub(1).min(30);
    INITIAL_RETRY_DELAY_MS
        .saturating_mul(1_i64.checked_shl(exponent).unwrap_or(i64::MAX))
        .min(MAX_RETRY_DELAY_MS)
}

fn classify_object_store_error(error: ObjectStoreError) -> (OffsiteMediaUploadFailureCode, bool) {
    match error.code {
        ObjectStoreErrorCode::InvalidConfiguration
        | ObjectStoreErrorCode::KeyOutsidePrefix
        | ObjectStoreErrorCode::DeletionNotAuthorized
        | ObjectStoreErrorCode::MediaWriteRequiresVerifiedCreate => {
            (OffsiteMediaUploadFailureCode::RemoteConfiguration, false)
        }
        ObjectStoreErrorCode::ContentHashMismatch | ObjectStoreErrorCode::ObjectTooLarge => {
            (OffsiteMediaUploadFailureCode::LocalBlobInvalid, false)
        }
        ObjectStoreErrorCode::Network => (OffsiteMediaUploadFailureCode::RemoteNetwork, true),
        ObjectStoreErrorCode::Timeout => (OffsiteMediaUploadFailureCode::RemoteTimeout, true),
        ObjectStoreErrorCode::AuthenticationRejected => {
            (OffsiteMediaUploadFailureCode::RemoteAuthentication, true)
        }
        ObjectStoreErrorCode::AuthorizationRejected => {
            (OffsiteMediaUploadFailureCode::RemoteAuthorization, true)
        }
        ObjectStoreErrorCode::RateLimited => {
            (OffsiteMediaUploadFailureCode::RemoteRateLimited, true)
        }
        ObjectStoreErrorCode::ServiceUnavailable => {
            (OffsiteMediaUploadFailureCode::RemoteUnavailable, true)
        }
        ObjectStoreErrorCode::NotFound
        | ObjectStoreErrorCode::Conflict
        | ObjectStoreErrorCode::PreconditionFailed
        | ObjectStoreErrorCode::ResponseTooLarge
        | ObjectStoreErrorCode::InvalidResponse => {
            (OffsiteMediaUploadFailureCode::RemoteInvalidResponse, true)
        }
    }
}

enum LocalBlobError {
    Missing,
    Invalid,
    Unavailable(crate::database::DatabaseError),
}

fn load_local_blob(
    paths: &DatabasePaths,
    expected_sha256: ContentSha256,
) -> Result<Vec<u8>, LocalBlobError> {
    let connection = connection::open_read_only(&paths.media, DatabaseKind::Media)
        .map_err(LocalBlobError::Unavailable)?;
    let stored = connection
        .query_row(
            "SELECT rowid, byte_length, length(bytes)
             FROM media_blob
             WHERE sha256 = ?1",
            params![expected_sha256.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(crate::database::DatabaseError::from)
        .map_err(LocalBlobError::Unavailable)?;
    let Some((rowid, byte_length, stored_length)) = stored else {
        return Err(LocalBlobError::Missing);
    };
    if byte_length <= 0 || byte_length > MAX_MEDIA_BLOB_BYTES || stored_length != byte_length {
        return Err(LocalBlobError::Invalid);
    }
    let mut blob = connection
        .blob_open(MAIN_DB, "media_blob", "bytes", rowid, true)
        .map_err(crate::database::DatabaseError::from)
        .map_err(LocalBlobError::Unavailable)?;
    let mut bytes =
        Vec::with_capacity(usize::try_from(byte_length).map_err(|_| LocalBlobError::Invalid)?);
    blob.read_to_end(&mut bytes)
        .map_err(crate::database::DatabaseError::from)
        .map_err(LocalBlobError::Unavailable)?;
    if bytes.len() != usize::try_from(byte_length).map_err(|_| LocalBlobError::Invalid)?
        || ContentSha256::from_bytes(Sha256::digest(&bytes).into()) != expected_sha256
    {
        return Err(LocalBlobError::Invalid);
    }
    Ok(bytes)
}

fn system_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::{
        io::Cursor,
        sync::{
            mpsc::{self, Receiver, SyncSender},
            Arc, Mutex,
        },
        thread,
        time::{Duration, Instant},
    };

    use super::{
        process_claim, retry_delay_ms, CoordinatorControl, MediaBackupCoordinator,
        MediaUploadDisposition, SHUTDOWN_JOIN_GRACE,
    };
    use crate::{
        backup::{
            domain::{
                BackupSetId, R2AccountId, R2BucketName, R2Jurisdiction, R2Keyspace, R2ListPrefix,
                R2ObjectKey, R2Target, ReplicaEpochId,
            },
            object_store::{
                fake::{FakeObjectStore, ObjectOperation},
                ContinuationToken, GetObjectResult, ListObjectsPage, ObjectMetadata, ObjectStore,
                ObjectStoreError, ProbeDeleteAuthorization, PutMediaRequest, PutObjectOutcome,
                PutObjectRequest,
            },
        },
        database::{
            drafts::SaveDraftWrite,
            media::{IngestAttachmentMetadata, StagedAttachment},
            Database, DatabasePaths, MediaLimits, SaveDraftInput, SaveOffsiteBackupConfigInput,
        },
    };
    use sha2::Digest as _;
    use tempfile::TempDir;

    const ACCOUNT_ID: &str = "0123456789abcdef0123456789abcdef";
    const DRAFT_ID: &str = "019f547b-6200-7000-8000-000000009001";

    struct TestLibrary {
        _root: TempDir,
        paths: DatabasePaths,
        database: Database,
        payload: Vec<u8>,
        sha256: crate::backup::domain::ContentSha256,
        keyspace: R2Keyspace,
    }

    impl TestLibrary {
        fn new(payload: &[u8]) -> Self {
            let root = tempfile::tempdir().expect("temporary media backup root");
            let paths = DatabasePaths::new(root.path());
            let database = Database::initialize(paths.clone()).expect("database");
            let client = database.client();
            client
                .save_draft(SaveDraftWrite {
                    input: SaveDraftInput {
                        context_key: "capture".into(),
                        tidbit_id: None,
                        base_revision_id: None,
                        title: None,
                        body_markdown: String::new(),
                        sources: Vec::new(),
                    },
                    now_ms: 10,
                    draft_id: DRAFT_ID.into(),
                    media_limits: MediaLimits::default(),
                })
                .expect("draft");
            let backup_set_id = BackupSetId::new();
            let target = target();
            client
                .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
                    expected_revision: 0,
                    backup_set_id: backup_set_id.clone(),
                    replica_epoch_id: ReplicaEpochId::new(),
                    enabled: true,
                    target: target.clone(),
                    now_ms: 20,
                })
                .expect("config");
            let staged = StagedAttachment::from_reader(
                Cursor::new(payload),
                &root.path().join("staging"),
                &id(4),
                MediaLimits::default().max_attachment_bytes,
            )
            .expect("staged media");
            client
                .ingest_attachment(staged.write(IngestAttachmentMetadata {
                    attachment_id: id(2),
                    ingest_lease_id: id(3),
                    draft_id: DRAFT_ID.into(),
                    display_filename: "evidence.bin".into(),
                    media_type: "application/octet-stream".into(),
                    now_ms: 30,
                    limits: MediaLimits::default(),
                }))
                .expect("attachment");
            let sha256 = crate::backup::domain::ContentSha256::from_bytes(
                sha2::Sha256::digest(payload).into(),
            );
            Self {
                _root: root,
                paths,
                database,
                payload: payload.to_vec(),
                sha256,
                keyspace: target.keyspace(&backup_set_id),
            }
        }

        fn claim(&self, now_ms: i64, suffix: u64) -> crate::database::OffsiteMediaUploadClaim {
            self.database
                .client()
                .claim_next_offsite_media_upload(now_ms, id(suffix))
                .expect("claim")
                .expect("pending media")
        }
    }

    fn target() -> R2Target {
        R2Target {
            account_id: R2AccountId::parse(ACCOUNT_ID).expect("account"),
            jurisdiction: R2Jurisdiction::Default,
            bucket: R2BucketName::parse("kosh-local").expect("bucket"),
        }
    }

    fn id(suffix: u64) -> String {
        format!("019f547b-6200-7000-8000-{suffix:012x}")
    }

    #[test]
    fn retry_backoff_is_bounded() {
        assert_eq!(retry_delay_ms(1), 5_000);
        assert_eq!(retry_delay_ms(2), 10_000);
        assert_eq!(retry_delay_ms(3), 20_000);
        assert_eq!(retry_delay_ms(100), 15 * 60 * 1_000);
    }

    #[test]
    fn verified_upload_is_idempotent_after_crash_before_completion() {
        let library = TestLibrary::new(b"crash-safe media");
        let client = library.database.client();
        let store = FakeObjectStore::new(library.keyspace.clone());
        let first = library.claim(40, 10);
        assert_eq!(
            store
                .put_media(
                    PutMediaRequest::new(
                        &library.keyspace,
                        library.sha256,
                        library.payload.clone(),
                    )
                    .expect("verified request"),
                )
                .expect("pre-crash upload"),
            PutObjectOutcome::Stored
        );

        assert_eq!(
            client
                .recover_interrupted_offsite_media_uploads(50)
                .expect("recover claim"),
            1
        );
        let replay = library.claim(50, 11);
        assert_eq!(replay.sha256, first.sha256);
        assert_eq!(
            process_claim(&client, &library.paths, &store, replay, &|| 60),
            MediaUploadDisposition::Uploaded
        );
        assert_eq!(
            store
                .get(&library.keyspace.media(library.sha256))
                .expect("stored object")
                .bytes,
            library.payload
        );
        let progress = client
            .offsite_media_upload_progress()
            .expect("upload progress");
        assert_eq!(progress.uploaded, 1);
        assert_eq!(
            store.operations(),
            [
                ObjectOperation::Put,
                ObjectOperation::Put,
                ObjectOperation::Head,
                ObjectOperation::Get,
            ]
        );
    }

    #[test]
    fn offline_retry_survives_database_restart() {
        let library = TestLibrary::new(b"offline media");
        let client = library.database.client();
        let store = FakeObjectStore::new(library.keyspace.clone());
        store.fail_next(
            ObjectOperation::Put,
            crate::backup::object_store::ObjectStoreErrorCode::Network,
        );
        let claim = library.claim(100, 20);
        assert_eq!(
            process_claim(&client, &library.paths, &store, claim, &|| 200),
            MediaUploadDisposition::RetryScheduled
        );
        let progress = client
            .offsite_media_upload_progress()
            .expect("retry progress");
        assert_eq!(progress.retry_wait, 1);
        assert_eq!(progress.next_attempt_at_ms, Some(5_200));

        let TestLibrary {
            _root,
            paths,
            database,
            payload,
            sha256,
            keyspace,
        } = library;
        database.shutdown().expect("shutdown");
        drop(database);
        let reopened = Database::initialize(paths.clone()).expect("reopen database");
        let client = reopened.client();
        assert!(client
            .claim_next_offsite_media_upload(5_199, id(21))
            .expect("early claim")
            .is_none());
        let retry = client
            .claim_next_offsite_media_upload(5_200, id(22))
            .expect("retry claim")
            .expect("due upload");
        assert_eq!(
            process_claim(&client, &paths, &store, retry, &|| 5_300),
            MediaUploadDisposition::Uploaded
        );
        assert_eq!(
            store
                .get(&keyspace.media(sha256))
                .expect("uploaded after restart")
                .bytes,
            payload
        );
    }

    #[test]
    fn remote_metadata_mismatch_fails_closed() {
        let library = TestLibrary::new(b"metadata-bound media");
        let client = library.database.client();
        let store = MismatchedMetadataStore {
            inner: FakeObjectStore::new(library.keyspace.clone()),
        };
        let claim = library.claim(100, 30);
        assert_eq!(
            process_claim(&client, &library.paths, &store, claim, &|| 200),
            MediaUploadDisposition::PermanentlyFailed
        );
        let progress = client
            .offsite_media_upload_progress()
            .expect("failed progress");
        assert_eq!(progress.failed, 1);
        assert_eq!(progress.uploaded, 0);
    }

    #[test]
    fn configuration_change_before_remote_write_fences_the_stale_claim() {
        let library = TestLibrary::new(b"configuration-fenced media");
        let client = library.database.client();
        let store = FakeObjectStore::new(library.keyspace.clone());
        let claim = library.claim(100, 35);
        let config = client
            .load_offsite_backup_config()
            .expect("load config")
            .expect("stored config");
        client
            .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
                expected_revision: config.revision,
                backup_set_id: config.backup_set_id,
                replica_epoch_id: config.replica_epoch_id,
                enabled: false,
                target: config.target,
                now_ms: 150,
            })
            .expect("disable backup");

        assert_eq!(
            process_claim(&client, &library.paths, &store, claim, &|| 200),
            MediaUploadDisposition::Stale
        );
        assert!(
            store.operations().is_empty(),
            "a stale claim must not reach the remote store"
        );
    }

    #[test]
    fn blocked_network_upload_does_not_block_the_database_writer() {
        let library = TestLibrary::new(b"nonblocking authored data");
        let client = library.database.client();
        let claim = library.claim(100, 40);
        let (started_sender, started_receiver) = mpsc::sync_channel(1);
        let (release_sender, release_receiver) = mpsc::sync_channel(1);
        let store = Arc::new(BlockingStore {
            inner: FakeObjectStore::new(library.keyspace.clone()),
            started: started_sender,
            release: Mutex::new(release_receiver),
        });
        let worker_client = client.clone();
        let worker_paths = library.paths.clone();
        let worker_store = Arc::clone(&store);
        let worker = thread::spawn(move || {
            process_claim(
                &worker_client,
                &worker_paths,
                worker_store.as_ref(),
                claim,
                &|| 200,
            )
        });
        started_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("upload reached network");

        let diagnostics = client.diagnostics().expect("writer remains responsive");
        assert!(diagnostics.main_foreign_keys);
        assert!(diagnostics.media_foreign_keys);

        let config = client
            .load_offsite_backup_config()
            .expect("load config")
            .expect("stored config");
        let config_client = client.clone();
        let (saved_sender, saved_receiver) = mpsc::sync_channel(1);
        let config_worker = thread::spawn(move || {
            let result = config_client.save_offsite_backup_config(SaveOffsiteBackupConfigInput {
                expected_revision: config.revision,
                backup_set_id: config.backup_set_id,
                replica_epoch_id: config.replica_epoch_id,
                enabled: false,
                target: config.target,
                now_ms: 150,
            });
            saved_sender.send(result).expect("report config save");
        });
        assert!(
            saved_receiver
                .recv_timeout(Duration::from_millis(100))
                .is_err(),
            "configuration changes must wait for an in-flight remote operation"
        );

        release_sender.send(()).expect("release upload");
        assert!(matches!(
            worker.join().expect("worker"),
            MediaUploadDisposition::Uploaded | MediaUploadDisposition::Stale
        ));
        saved_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("configuration save completed after upload")
            .expect("disable backup");
        config_worker.join().expect("configuration worker");
    }

    #[test]
    fn retention_removal_waits_until_an_in_flight_remote_upload_finishes() {
        let library = TestLibrary::new(b"retention-fenced media");
        let client = library.database.client();
        let claim = library.claim(100, 45);
        let attachment_id = id(2);
        let (started_sender, started_receiver) = mpsc::sync_channel(1);
        let (release_sender, release_receiver) = mpsc::sync_channel(1);
        let store = Arc::new(BlockingStore {
            inner: FakeObjectStore::new(library.keyspace.clone()),
            started: started_sender,
            release: Mutex::new(release_receiver),
        });
        let upload_client = client.clone();
        let upload_paths = library.paths.clone();
        let upload_store = Arc::clone(&store);
        let upload = thread::spawn(move || {
            process_claim(
                &upload_client,
                &upload_paths,
                upload_store.as_ref(),
                claim,
                &|| 200,
            )
        });
        started_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("upload reached network");

        let recovery_client = client.clone();
        let (recovered_sender, recovered_receiver) = mpsc::sync_channel(1);
        let recovery = thread::spawn(move || {
            let result = recovery_client
                .schedule_media_lifecycle_recovery(100_000_000, MediaLimits::default());
            recovered_sender
                .send(result)
                .expect("report lifecycle recovery");
        });
        assert!(
            recovered_receiver
                .recv_timeout(Duration::from_millis(100))
                .is_err(),
            "retention removal must wait for an in-flight remote operation"
        );
        assert!(
            client
                .diagnostics()
                .expect("writer remains responsive")
                .main_foreign_keys
        );

        release_sender.send(()).expect("release upload");
        assert_eq!(
            upload.join().expect("upload worker"),
            MediaUploadDisposition::Uploaded
        );
        recovered_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("lifecycle recovery completed after upload")
            .expect("recover expired media");
        recovery.join().expect("recovery worker");
        assert!(matches!(
            client.load_media_payload(attachment_id, 100_000_000, None, 64),
            Err(crate::database::DatabaseError::NotFound { .. })
        ));
    }

    #[test]
    fn coordinator_shutdown_detaches_an_uninterruptible_worker_after_a_bounded_grace() {
        let control = Arc::new(CoordinatorControl::default());
        let (release_sender, release_receiver) = mpsc::sync_channel(1);
        let (completed_sender, completed_receiver) = mpsc::sync_channel(1);
        let (exited_sender, exited_receiver) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            release_receiver.recv().expect("release detached worker");
            let _ = completed_sender.send(());
            exited_sender.send(()).expect("report detached worker exit");
        });
        let coordinator = MediaBackupCoordinator {
            control,
            worker: Some(worker),
            completed: Some(Mutex::new(completed_receiver)),
        };

        let started = Instant::now();
        drop(coordinator);
        assert!(
            started.elapsed() < SHUTDOWN_JOIN_GRACE + Duration::from_millis(500),
            "coordinator shutdown exceeded its bounded grace"
        );

        release_sender.send(()).expect("release detached worker");
        exited_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("detached worker exited");
    }

    struct MismatchedMetadataStore {
        inner: FakeObjectStore,
    }

    impl ObjectStore for MismatchedMetadataStore {
        fn head(&self, key: &R2ObjectKey) -> Result<Option<ObjectMetadata>, ObjectStoreError> {
            let mut metadata = self.inner.head(key)?;
            if let Some(metadata) = &mut metadata {
                metadata.kosh_sha256 =
                    Some(crate::backup::domain::ContentSha256::from_bytes([0xab; 32]));
            }
            Ok(metadata)
        }

        fn get(&self, key: &R2ObjectKey) -> Result<GetObjectResult, ObjectStoreError> {
            self.inner.get(key)
        }

        fn put(&self, request: PutObjectRequest) -> Result<PutObjectOutcome, ObjectStoreError> {
            self.inner.put(request)
        }

        fn put_media(
            &self,
            request: PutMediaRequest,
        ) -> Result<PutObjectOutcome, ObjectStoreError> {
            self.inner.put_media(request)
        }

        fn list(
            &self,
            prefix: &R2ListPrefix,
            continuation: Option<&ContinuationToken>,
        ) -> Result<ListObjectsPage, ObjectStoreError> {
            self.inner.list(prefix, continuation)
        }

        fn delete_probe(
            &self,
            authorization: &ProbeDeleteAuthorization,
            key: &R2ObjectKey,
        ) -> Result<(), ObjectStoreError> {
            self.inner.delete_probe(authorization, key)
        }
    }

    struct BlockingStore {
        inner: FakeObjectStore,
        started: SyncSender<()>,
        release: Mutex<Receiver<()>>,
    }

    impl ObjectStore for BlockingStore {
        fn head(&self, key: &R2ObjectKey) -> Result<Option<ObjectMetadata>, ObjectStoreError> {
            self.inner.head(key)
        }

        fn get(&self, key: &R2ObjectKey) -> Result<GetObjectResult, ObjectStoreError> {
            self.inner.get(key)
        }

        fn put(&self, request: PutObjectRequest) -> Result<PutObjectOutcome, ObjectStoreError> {
            self.inner.put(request)
        }

        fn put_media(
            &self,
            request: PutMediaRequest,
        ) -> Result<PutObjectOutcome, ObjectStoreError> {
            self.started.send(()).expect("signal blocked upload");
            self.release
                .lock()
                .expect("release lock")
                .recv()
                .expect("release upload");
            self.inner.put_media(request)
        }

        fn list(
            &self,
            prefix: &R2ListPrefix,
            continuation: Option<&ContinuationToken>,
        ) -> Result<ListObjectsPage, ObjectStoreError> {
            self.inner.list(prefix, continuation)
        }

        fn delete_probe(
            &self,
            authorization: &ProbeDeleteAuthorization,
            key: &R2ObjectKey,
        ) -> Result<(), ObjectStoreError> {
            self.inner.delete_probe(authorization, key)
        }
    }
}

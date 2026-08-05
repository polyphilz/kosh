use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::{
    database::{Database, DatabaseClient, DatabasePaths, MediaLimits},
    embedding_runtime::{
        EmbeddingRuntime, SemanticRuntimeError, SemanticRuntimeLogs, SemanticRuntimeStatus,
    },
    passage_embedding_indexer::{PassageEmbeddingIndexStatus, PassageEmbeddingIndexer},
};

pub(crate) trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

pub(crate) trait IdGenerator: Send + Sync {
    fn next_id(&self) -> String;
}

struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or_default()
    }
}

struct UuidV7Generator;

impl IdGenerator for UuidV7Generator {
    fn next_id(&self) -> String {
        uuid::Uuid::now_v7().to_string()
    }
}

pub(crate) struct RuntimeState {
    data_dir: PathBuf,
    resource_dir: Option<PathBuf>,
    passage_embedding_indexer: PassageEmbeddingIndexer,
    litestream_backup: crate::backup::litestream_runtime::LitestreamRuntimeService,
    media_backup: crate::backup::media_reconciler::MediaBackupCoordinator,
    checkpoint_backup: crate::backup::checkpoint::CheckpointBackupCoordinator,
    database: Arc<Database>,
    embedding_runtime: Arc<EmbeddingRuntime>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    media_limits: MediaLimits,
    image_ocr: crate::media::ImageOcrCoordinator,
    pending_clipboard_images: Mutex<HashMap<String, PendingClipboardImage>>,
    pending_image_drops: Mutex<HashMap<String, PendingImageDrop>>,
    pending_file_selections: Mutex<HashMap<String, PendingFileSelection>>,
    file_drop_consumers: Mutex<HashSet<String>>,
    maintenance_gate: Arc<Mutex<()>>,
    backup_operations_gate: Arc<Mutex<()>>,
}

struct PendingClipboardImage {
    created_at_ms: i64,
    bytes: Vec<u8>,
}

struct PendingImageDrop {
    created_at_ms: i64,
    paths: Vec<PathBuf>,
}

struct PendingFileSelection {
    created_at_ms: i64,
    drop_consumer: Option<String>,
    path: PathBuf,
}

const CLIPBOARD_IMAGE_TTL_MS: i64 = 5 * 60 * 1_000;
const MAX_PENDING_CLIPBOARD_IMAGES: usize = 4;
const IMAGE_DROP_TTL_MS: i64 = 5 * 60 * 1_000;
const MAX_PENDING_IMAGE_DROPS: usize = 16;
const MAX_FILES_PER_IMAGE_DROP: usize = 32;
const FILE_SELECTION_TTL_MS: i64 = 5 * 60 * 1_000;
const MAX_PENDING_FILE_SELECTIONS: usize = 8;

fn start_optional_image_ocr(client: DatabaseClient) -> crate::media::ImageOcrCoordinator {
    match crate::media::ImageOcrCoordinator::start(client) {
        Ok(coordinator) => coordinator,
        Err(error) => {
            log::warn!("image OCR is unavailable; Kosh will continue without it: {error}");
            crate::media::ImageOcrCoordinator::disabled()
        }
    }
}

fn start_optional_media_backup(
    client: DatabaseClient,
    paths: DatabasePaths,
) -> crate::backup::media_reconciler::MediaBackupCoordinator {
    match crate::backup::media_reconciler::MediaBackupCoordinator::start(client, paths) {
        Ok(coordinator) => coordinator,
        Err(error) => {
            log::warn!("off-site media backup is unavailable; Kosh will continue locally: {error}");
            crate::backup::media_reconciler::MediaBackupCoordinator::disabled()
        }
    }
}

impl RuntimeState {
    pub(crate) fn production(
        data_dir: PathBuf,
        resource_dir: Option<PathBuf>,
    ) -> crate::database::Result<Self> {
        let database = Database::initialize(DatabasePaths::new(&data_dir))?;
        let media_limits = MediaLimits::default().validate()?;
        let startup_now_ms = SystemClock.now_ms();
        if let Err(error) = database
            .client()
            .schedule_media_lifecycle_recovery(startup_now_ms, media_limits)
        {
            log::warn!("startup media lifecycle recovery could not complete: {error}");
        }
        if let Err(error) =
            crate::backup::settings::reconcile_startup_backup_state(&database.client())
        {
            log::warn!("startup off-site backup reconciliation remains pending: {error:?}");
        }
        let litestream_backup = crate::backup::litestream_runtime::LitestreamRuntimeService::start(
            database.client(),
            data_dir.clone(),
            database.paths().main.clone(),
            resource_dir.clone(),
        );
        let embedding_runtime = Arc::new(EmbeddingRuntime::new(&data_dir, resource_dir.as_deref()));
        let passage_embedding_indexer =
            PassageEmbeddingIndexer::start(database.client(), Arc::clone(&embedding_runtime));
        let image_ocr = start_optional_image_ocr(database.client());
        let media_backup = start_optional_media_backup(database.client(), database.paths().clone());
        let checkpoint_backup = crate::backup::checkpoint::CheckpointBackupCoordinator::start(
            database.client(),
            data_dir.clone(),
            litestream_backup.checkpoint_handle(),
            media_backup.wake_handle(),
        );
        let state = Self {
            data_dir,
            resource_dir,
            passage_embedding_indexer,
            litestream_backup,
            media_backup,
            checkpoint_backup,
            database: Arc::new(database),
            embedding_runtime,
            clock: Arc::new(SystemClock),
            ids: Arc::new(UuidV7Generator),
            media_limits,
            image_ocr,
            pending_clipboard_images: Mutex::new(HashMap::new()),
            pending_image_drops: Mutex::new(HashMap::new()),
            pending_file_selections: Mutex::new(HashMap::new()),
            file_drop_consumers: Mutex::new(HashSet::new()),
            maintenance_gate: Arc::new(Mutex::new(())),
            backup_operations_gate: Arc::new(Mutex::new(())),
        };
        if let Err(error) =
            crate::media::recover_staging_directory(&state.media_staging_directory())
        {
            log::warn!("startup media staging recovery could not complete: {error}");
        }
        if let Err(error) = crate::pdf::recover_pdf_open_directory(&state.pdf_open_directory()) {
            log::warn!("startup PDF materialization recovery could not complete: {error}");
        }
        if let Err(error) = crate::attachments::recover_attachment_open_directory(
            &state.attachment_open_directory(),
        ) {
            log::warn!("startup attachment materialization recovery could not complete: {error}");
        }
        log::debug!(
            "relational backup supervisor initialized in {:?} phase",
            state.relational_backup_status().phase
        );
        Ok(state)
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn deterministic(
        data_dir: PathBuf,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        let database =
            Database::initialize(DatabasePaths::new(&data_dir)).expect("temporary Kosh database");
        Self {
            embedding_runtime: Arc::new(EmbeddingRuntime::without_sidecar(&data_dir)),
            data_dir,
            resource_dir: None,
            passage_embedding_indexer: PassageEmbeddingIndexer::disabled(),
            litestream_backup:
                crate::backup::litestream_runtime::LitestreamRuntimeService::disabled(),
            media_backup: crate::backup::media_reconciler::MediaBackupCoordinator::disabled(),
            checkpoint_backup: crate::backup::checkpoint::CheckpointBackupCoordinator::disabled(),
            database: Arc::new(database),
            clock,
            ids,
            media_limits: MediaLimits::default(),
            image_ocr: crate::media::ImageOcrCoordinator::disabled(),
            pending_clipboard_images: Mutex::new(HashMap::new()),
            pending_image_drops: Mutex::new(HashMap::new()),
            pending_file_selections: Mutex::new(HashMap::new()),
            file_drop_consumers: Mutex::new(HashSet::new()),
            maintenance_gate: Arc::new(Mutex::new(())),
            backup_operations_gate: Arc::new(Mutex::new(())),
        }
    }

    pub(crate) fn database_client(&self) -> DatabaseClient {
        self.database.client()
    }

    pub(crate) fn database_paths(&self) -> &DatabasePaths {
        self.database.paths()
    }

    pub(crate) fn maintenance_gate(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.maintenance_gate)
    }

    pub(crate) fn backup_operations_gate(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.backup_operations_gate)
    }

    pub(crate) fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    pub(crate) fn resource_dir(&self) -> Option<PathBuf> {
        self.resource_dir.clone()
    }

    pub(crate) fn relational_backup_status(
        &self,
    ) -> crate::backup::litestream_runtime::RelationalBackupStatus {
        self.litestream_backup.status()
    }

    pub(crate) fn checkpoint_backup_status(
        &self,
    ) -> crate::backup::checkpoint::CheckpointBackupStatus {
        self.checkpoint_backup.status()
    }

    pub(crate) fn checkpoint_backup_handle(
        &self,
    ) -> crate::backup::checkpoint::CheckpointBackupHandle {
        self.checkpoint_backup.handle()
    }

    pub(crate) fn reload_backup_configuration(&self) {
        self.litestream_backup.reload_configuration();
        self.media_backup.wake();
    }

    pub(crate) fn reconcile_backup_takeover_async(&self) {
        let client = self.database.client();
        let data_dir = self.data_dir.clone();
        let gate = Arc::clone(&self.backup_operations_gate);
        if let Err(error) = std::thread::Builder::new()
            .name("kosh-backup-takeover-recovery".into())
            .spawn(move || {
                let _guard = gate
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Err(error) =
                    crate::backup::settings::reconcile_deferred_takeover(&client, &data_dir)
                {
                    log::warn!(
                        "deferred off-site backup takeover reconciliation remains pending: {error:?}"
                    );
                }
            })
        {
            log::warn!("could not start deferred backup takeover reconciliation: {error}");
        }
    }

    pub(crate) fn shutdown_for_exit(&self) {
        self.checkpoint_backup.shutdown();
        if let Err(error) = self.database.shutdown() {
            log::error!("could not quiesce the database writer before backup shutdown: {error}");
        }
        self.litestream_backup.shutdown();
    }

    pub(crate) fn embedding_runtime(&self) -> Arc<EmbeddingRuntime> {
        Arc::clone(&self.embedding_runtime)
    }

    pub(crate) fn passage_embedding_index_status(
        &self,
    ) -> crate::database::Result<PassageEmbeddingIndexStatus> {
        let progress = self.database.client().passage_embedding_index_progress()?;
        let runtime = self.embedding_runtime.status();
        Ok(self
            .passage_embedding_indexer
            .status(progress, runtime.phase, runtime.message))
    }

    pub(crate) fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }

    pub(crate) fn next_ids(&self, count: usize) -> Vec<String> {
        (0..count).map(|_| self.ids.next_id()).collect()
    }

    pub(crate) fn media_limits(&self) -> MediaLimits {
        self.media_limits
    }

    pub(crate) fn media_staging_directory(&self) -> PathBuf {
        self.data_dir.join("media-staging")
    }

    pub(crate) fn wake_image_ocr(&self) {
        self.image_ocr.wake();
    }

    pub(crate) fn wake_media_backup(&self) {
        self.media_backup.wake();
    }

    pub(crate) fn pdf_open_directory(&self) -> PathBuf {
        self.data_dir.join("pdf-open")
    }

    pub(crate) fn attachment_open_directory(&self) -> PathBuf {
        self.data_dir.join("attachment-open")
    }

    pub(crate) fn register_clipboard_image(
        &self,
        bytes: Vec<u8>,
    ) -> crate::database::Result<String> {
        if bytes.is_empty() || bytes.len() > crate::media::MAX_SOURCE_IMAGE_BYTES {
            return Err(crate::database::DatabaseError::InvalidInput(format!(
                "the pasted image must contain between 1 and {} bytes",
                crate::media::MAX_SOURCE_IMAGE_BYTES
            )));
        }
        let now_ms = self.now_ms();
        let mut pending = self
            .pending_clipboard_images
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pending.retain(|_, image| {
            now_ms.saturating_sub(image.created_at_ms) <= CLIPBOARD_IMAGE_TTL_MS
        });
        if pending.len() >= MAX_PENDING_CLIPBOARD_IMAGES {
            return Err(crate::database::DatabaseError::InvalidInput(
                "too many pasted images are awaiting ingestion".into(),
            ));
        }
        let capture_id = self
            .next_ids(1)
            .into_iter()
            .next()
            .expect("requested clipboard image capture ID");
        pending.insert(
            capture_id.clone(),
            PendingClipboardImage {
                created_at_ms: now_ms,
                bytes,
            },
        );
        Ok(capture_id)
    }

    pub(crate) fn take_clipboard_image(
        &self,
        capture_id: &str,
    ) -> crate::database::Result<Vec<u8>> {
        validate_capability_id(capture_id, "captureId")?;
        let now_ms = self.now_ms();
        let mut pending = self
            .pending_clipboard_images
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pending.retain(|_, image| {
            now_ms.saturating_sub(image.created_at_ms) <= CLIPBOARD_IMAGE_TTL_MS
        });
        pending
            .remove(capture_id)
            .map(|image| image.bytes)
            .ok_or_else(|| crate::database::DatabaseError::NotFound {
                entity: "clipboard image capture",
                id: capture_id.into(),
            })
    }

    pub(crate) fn register_image_drop(
        &self,
        paths: &[PathBuf],
    ) -> Option<crate::media::ImageDropNotice> {
        let now_ms = self.now_ms();
        let mut pending = self
            .pending_image_drops
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pending.retain(|_, drop| now_ms.saturating_sub(drop.created_at_ms) <= IMAGE_DROP_TTL_MS);
        if pending.len() >= MAX_PENDING_IMAGE_DROPS {
            return None;
        }
        let paths = paths
            .iter()
            .filter(|path| path.is_file())
            .take(MAX_FILES_PER_IMAGE_DROP)
            .cloned()
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return None;
        }
        let filenames = paths
            .iter()
            .map(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Dropped image")
                    .to_owned()
            })
            .collect();
        let drop_id = self
            .next_ids(1)
            .into_iter()
            .next()
            .expect("requested image drop ID");
        pending.insert(
            drop_id.clone(),
            PendingImageDrop {
                created_at_ms: now_ms,
                paths,
            },
        );
        Some(crate::media::ImageDropNotice { drop_id, filenames })
    }

    pub(crate) fn take_image_drop(&self, drop_id: &str) -> crate::database::Result<Vec<PathBuf>> {
        validate_capability_id(drop_id, "dropId")?;
        let now_ms = self.now_ms();
        let mut pending = self
            .pending_image_drops
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pending.retain(|_, drop| now_ms.saturating_sub(drop.created_at_ms) <= IMAGE_DROP_TTL_MS);
        pending
            .remove(drop_id)
            .map(|drop| drop.paths)
            .ok_or_else(|| crate::database::DatabaseError::NotFound {
                entity: "image drop",
                id: drop_id.into(),
            })
    }

    pub(crate) fn register_file_selection(&self, path: PathBuf) -> crate::database::Result<String> {
        self.register_file_selection_with_origin(path, None)
    }

    pub(crate) fn register_dropped_file_selection(
        &self,
        consumer: &str,
        path: PathBuf,
    ) -> crate::database::Result<String> {
        if !self.file_drop_consumer_active(consumer) {
            return Err(crate::database::DatabaseError::InvalidInput(
                "no editor is accepting dropped files".into(),
            ));
        }
        self.register_file_selection_with_origin(path, Some(consumer))
    }

    fn register_file_selection_with_origin(
        &self,
        path: PathBuf,
        drop_consumer: Option<&str>,
    ) -> crate::database::Result<String> {
        if !path.is_file() {
            return Err(crate::database::DatabaseError::InvalidInput(
                "the selected file is not a regular file".into(),
            ));
        }
        let now_ms = self.now_ms();
        let mut pending = self
            .pending_file_selections
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pending.retain(|_, selection| {
            now_ms.saturating_sub(selection.created_at_ms) <= FILE_SELECTION_TTL_MS
        });
        if let Some(consumer) = drop_consumer {
            if !self.file_drop_consumer_active(consumer) {
                return Err(crate::database::DatabaseError::InvalidInput(
                    "no editor is accepting dropped files".into(),
                ));
            }
        }
        if pending.len() >= MAX_PENDING_FILE_SELECTIONS {
            return Err(crate::database::DatabaseError::InvalidInput(
                "too many files are awaiting ingestion".into(),
            ));
        }
        let selection_id = self
            .next_ids(1)
            .into_iter()
            .next()
            .expect("requested file selection ID");
        pending.insert(
            selection_id.clone(),
            PendingFileSelection {
                created_at_ms: now_ms,
                drop_consumer: drop_consumer.map(str::to_owned),
                path,
            },
        );
        Ok(selection_id)
    }

    pub(crate) fn set_file_drop_consumer_active(&self, consumer: &str, active: bool) {
        let mut consumers = self
            .file_drop_consumers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active {
            consumers.insert(consumer.to_owned());
            return;
        }
        consumers.remove(consumer);
        drop(consumers);
        let mut pending = self
            .pending_file_selections
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pending.retain(|_, selection| selection.drop_consumer.as_deref() != Some(consumer));
    }

    pub(crate) fn file_drop_consumer_active(&self, consumer: &str) -> bool {
        self.file_drop_consumers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(consumer)
    }

    pub(crate) fn discard_file_drop_selections(
        &self,
        consumer: &str,
        selection_ids: &[String],
    ) -> crate::database::Result<()> {
        if selection_ids.len() > MAX_PENDING_FILE_SELECTIONS {
            return Err(crate::database::DatabaseError::InvalidInput(
                "too many file selections were provided".into(),
            ));
        }
        for selection_id in selection_ids {
            validate_capability_id(selection_id, "selectionId")?;
        }
        let mut pending = self
            .pending_file_selections
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for selection_id in selection_ids {
            if pending
                .get(selection_id)
                .is_some_and(|selection| selection.drop_consumer.as_deref() == Some(consumer))
            {
                pending.remove(selection_id);
            }
        }
        Ok(())
    }

    pub(crate) fn take_file_selection(
        &self,
        consumer: &str,
        selection_id: &str,
    ) -> crate::database::Result<PathBuf> {
        validate_capability_id(selection_id, "selectionId")?;
        let now_ms = self.now_ms();
        let mut pending = self
            .pending_file_selections
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pending.retain(|_, selection| {
            now_ms.saturating_sub(selection.created_at_ms) <= FILE_SELECTION_TTL_MS
        });
        let authorized = pending.get(selection_id).is_some_and(|selection| {
            selection
                .drop_consumer
                .as_deref()
                .is_none_or(|owner| owner == consumer)
        });
        authorized
            .then(|| pending.remove(selection_id))
            .flatten()
            .map(|selection| selection.path)
            .ok_or_else(|| crate::database::DatabaseError::NotFound {
                entity: "file selection",
                id: selection_id.into(),
            })
    }
}

fn validate_capability_id(id: &str, field: &str) -> crate::database::Result<()> {
    uuid::Uuid::parse_str(id)
        .ok()
        .filter(|parsed| {
            parsed.get_version_num() == 7 && parsed.hyphenated().to_string().as_str() == id
        })
        .map(|_| ())
        .ok_or_else(|| {
            crate::database::DatabaseError::InvalidInput(format!(
                "{field} must be a lowercase UUIDv7"
            ))
        })
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeProbe {
    pub data_dir: String,
    pub now_ms: i64,
    pub request_id: String,
    pub startup_smoke_canary: Option<String>,
    pub startup_smoke_capture: bool,
}

#[tauri::command]
pub(crate) fn runtime_probe(state: State<'_, RuntimeState>) -> RuntimeProbe {
    RuntimeProbe {
        data_dir: state.data_dir.to_string_lossy().into_owned(),
        now_ms: state.clock.now_ms(),
        request_id: state.ids.next_id(),
        startup_smoke_canary: crate::startup_smoke::canary_query_if_requested(),
        startup_smoke_capture: crate::startup_smoke::capture_requested_if_requested(),
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum SemanticRuntimeErrorCode {
    Unavailable,
    InvalidArtifact,
    OperationInProgress,
    Failed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SemanticRuntimeCommandError {
    code: SemanticRuntimeErrorCode,
    message: String,
}

impl From<SemanticRuntimeError> for SemanticRuntimeCommandError {
    fn from(error: SemanticRuntimeError) -> Self {
        let code = match &error {
            SemanticRuntimeError::RuntimeUnavailable(_) => SemanticRuntimeErrorCode::Unavailable,
            SemanticRuntimeError::InvalidArtifact(_) => SemanticRuntimeErrorCode::InvalidArtifact,
            SemanticRuntimeError::OperationInProgress => {
                SemanticRuntimeErrorCode::OperationInProgress
            }
            _ => SemanticRuntimeErrorCode::Failed,
        };
        Self {
            code,
            message: error.public_message(),
        }
    }
}

type SemanticCommandResult<T> = Result<T, SemanticRuntimeCommandError>;

#[tauri::command]
pub(crate) fn semantic_runtime_status(state: State<'_, RuntimeState>) -> SemanticRuntimeStatus {
    state.embedding_runtime.status()
}

#[tauri::command]
pub(crate) fn passage_embedding_index_status(
    state: State<'_, RuntimeState>,
) -> Result<PassageEmbeddingIndexStatus, String> {
    state
        .passage_embedding_index_status()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn prepare_semantic_runtime(
    state: State<'_, RuntimeState>,
) -> SemanticCommandResult<SemanticRuntimeStatus> {
    let runtime = state.embedding_runtime();
    run_semantic_operation(move || runtime.prepare()).await
}

#[tauri::command]
pub(crate) async fn retry_semantic_runtime(
    state: State<'_, RuntimeState>,
) -> SemanticCommandResult<SemanticRuntimeStatus> {
    let runtime = state.embedding_runtime();
    run_semantic_operation(move || runtime.retry()).await
}

#[tauri::command]
pub(crate) async fn repair_semantic_runtime(
    state: State<'_, RuntimeState>,
) -> SemanticCommandResult<SemanticRuntimeStatus> {
    let runtime = state.embedding_runtime();
    run_semantic_operation(move || runtime.repair()).await
}

#[tauri::command]
pub(crate) async fn semantic_runtime_logs(
    state: State<'_, RuntimeState>,
) -> SemanticCommandResult<SemanticRuntimeLogs> {
    let runtime = state.embedding_runtime();
    run_semantic_operation(move || runtime.logs()).await
}

async fn run_semantic_operation<T>(
    operation: impl FnOnce() -> Result<T, SemanticRuntimeError> + Send + 'static,
) -> SemanticCommandResult<T>
where
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|error| SemanticRuntimeCommandError {
            code: SemanticRuntimeErrorCode::Failed,
            message: format!("semantic runtime command worker failed: {error}"),
        })?
        .map_err(Into::into)
}

#[cfg(feature = "test-support")]
pub(crate) mod deterministic {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use super::{Clock, IdGenerator};

    pub(crate) struct FixedClock(pub(crate) i64);

    impl Clock for FixedClock {
        fn now_ms(&self) -> i64 {
            self.0
        }
    }

    pub(crate) struct SequenceIds {
        values: Mutex<VecDeque<String>>,
    }

    impl SequenceIds {
        pub(crate) fn new(values: impl IntoIterator<Item = String>) -> Arc<Self> {
            Arc::new(Self {
                values: Mutex::new(values.into_iter().collect()),
            })
        }
    }

    impl IdGenerator for SequenceIds {
        fn next_id(&self) -> String {
            self.values
                .lock()
                .expect("sequence ID mutex poisoned")
                .pop_front()
                .expect("deterministic ID sequence exhausted")
        }
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;

    #[test]
    fn optional_ocr_recovery_is_deferred_until_after_coordinator_construction() {
        let directory = tempfile::tempdir().expect("temporary optional OCR database");
        let database =
            Database::initialize(DatabasePaths::new(directory.path())).expect("OCR database");
        let unavailable_client = database.client();
        database.shutdown().expect("stop OCR database writer");

        let coordinator = start_optional_image_ocr(unavailable_client);

        assert!(!coordinator.is_disabled());
    }

    #[test]
    fn exit_shutdown_fences_database_writes_before_relational_backup_stops() {
        let directory = tempfile::tempdir().expect("temporary exit database");
        let state = RuntimeState::deterministic(
            directory.path().join("data"),
            Arc::new(deterministic::FixedClock(100)),
            deterministic::SequenceIds::new([]),
        );
        let client = state.database_client();

        state.shutdown_for_exit();
        state.shutdown_for_exit();

        assert!(
            client.diagnostics().is_err(),
            "the database writer must be closed before final replication shutdown"
        );
    }

    #[test]
    fn native_image_drops_expose_only_opaque_single_use_capabilities() {
        let directory = tempfile::tempdir().expect("temporary image drop directory");
        let image = directory.path().join("private shower thought.png");
        std::fs::write(&image, b"image bytes").expect("image drop fixture");
        let drop_id = "019f547b-6200-7000-8000-000000000991".to_owned();
        let state = RuntimeState::deterministic(
            directory.path().join("data"),
            Arc::new(deterministic::FixedClock(100)),
            deterministic::SequenceIds::new([drop_id.clone()]),
        );

        let notice = state
            .register_image_drop(&[image.clone(), directory.path().to_owned()])
            .expect("registered native image drop");
        assert_eq!(notice.drop_id, drop_id);
        assert_eq!(notice.filenames, ["private shower thought.png"]);
        let serialized = serde_json::to_string(&notice).expect("serialize drop notice");
        assert!(!serialized.contains(directory.path().to_string_lossy().as_ref()));
        assert_eq!(
            state.take_image_drop(&drop_id).expect("consume drop"),
            [image]
        );
        assert!(matches!(
            state.take_image_drop(&drop_id),
            Err(crate::database::DatabaseError::NotFound { .. })
        ));
        assert!(matches!(
            state.take_image_drop("not-a-capability"),
            Err(crate::database::DatabaseError::InvalidInput(_))
        ));
    }

    #[test]
    fn file_drop_capabilities_require_and_follow_an_active_consumer() {
        let directory = tempfile::tempdir().expect("temporary file drop directory");
        let picker_pdf = directory.path().join("picker.pdf");
        let dropped_pdf = directory.path().join("dropped.txt");
        let quick_drop = directory.path().join("quick.txt");
        std::fs::write(&picker_pdf, b"%PDF-picker").expect("picker file fixture");
        std::fs::write(&dropped_pdf, b"dropped text").expect("dropped file fixture");
        std::fs::write(&quick_drop, b"quick text").expect("quick drop fixture");
        let picker_id = "019f547b-6200-7000-8000-000000000993".to_owned();
        let dropped_id = "019f547b-6200-7000-8000-000000000994".to_owned();
        let quick_drop_id = "019f547b-6200-7000-8000-000000000995".to_owned();
        let state = RuntimeState::deterministic(
            directory.path().join("data"),
            Arc::new(deterministic::FixedClock(100)),
            deterministic::SequenceIds::new([
                picker_id.clone(),
                dropped_id.clone(),
                quick_drop_id.clone(),
            ]),
        );

        assert!(matches!(
            state.register_dropped_file_selection("main", dropped_pdf.clone()),
            Err(crate::database::DatabaseError::InvalidInput(_))
        ));
        assert_eq!(
            state
                .register_file_selection(picker_pdf.clone())
                .expect("picker selection"),
            picker_id
        );
        state.set_file_drop_consumer_active("main", true);
        assert_eq!(
            state
                .register_dropped_file_selection("main", dropped_pdf)
                .expect("dropped selection"),
            dropped_id
        );
        state.set_file_drop_consumer_active("quick-add", true);
        assert_eq!(
            state
                .register_dropped_file_selection("quick-add", quick_drop.clone())
                .expect("quick-add dropped selection"),
            quick_drop_id
        );

        state.set_file_drop_consumer_active("main", false);
        assert_eq!(
            state
                .take_file_selection("main", &picker_id)
                .expect("picker survives"),
            picker_pdf
        );
        assert!(matches!(
            state.take_file_selection("main", &dropped_id),
            Err(crate::database::DatabaseError::NotFound { .. })
        ));
        assert!(matches!(
            state.take_file_selection("main", &quick_drop_id),
            Err(crate::database::DatabaseError::NotFound { .. })
        ));
        state
            .discard_file_drop_selections("main", std::slice::from_ref(&quick_drop_id))
            .expect("another window cannot revoke the selection");
        assert_eq!(
            state
                .take_file_selection("quick-add", &quick_drop_id)
                .expect("other window drop survives"),
            quick_drop
        );
    }

    #[test]
    fn pasted_images_are_snapshotted_in_opaque_single_use_capabilities() {
        let directory = tempfile::tempdir().expect("temporary clipboard image directory");
        let capture_id = "019f547b-6200-7000-8000-000000000992".to_owned();
        let state = RuntimeState::deterministic(
            directory.path().join("data"),
            Arc::new(deterministic::FixedClock(100)),
            deterministic::SequenceIds::new([capture_id.clone()]),
        );

        assert_eq!(
            state
                .register_clipboard_image(b"first clipboard image".to_vec())
                .expect("capture pasted image"),
            capture_id
        );
        assert_eq!(
            state
                .take_clipboard_image(&capture_id)
                .expect("consume pasted image"),
            b"first clipboard image"
        );
        assert!(matches!(
            state.take_clipboard_image(&capture_id),
            Err(crate::database::DatabaseError::NotFound { .. })
        ));
        assert!(matches!(
            state.take_clipboard_image("not-a-capability"),
            Err(crate::database::DatabaseError::InvalidInput(_))
        ));
    }

    #[test]
    fn production_runtime_starts_without_a_resolved_resource_directory() {
        let directory = tempfile::tempdir().expect("temporary production data directory");

        let state =
            RuntimeState::production(directory.path().to_owned(), None).expect("runtime state");

        assert!(directory.path().join("kosh.sqlite3").is_file());
        assert!(directory.path().join("media.sqlite3").is_file());
        assert_eq!(
            state.embedding_runtime.status().phase,
            crate::embedding_runtime::SemanticRuntimePhase::Unavailable
        );
        assert_eq!(
            state.relational_backup_status().phase,
            crate::backup::litestream_runtime::RelationalBackupPhase::Off
        );
    }
}

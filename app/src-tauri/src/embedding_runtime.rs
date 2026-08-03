use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions, TryLockError},
    io::{Read, Seek, SeekFrom, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, TryLockError as MutexTryLockError,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, UNIX_EPOCH},
};

#[cfg(debug_assertions)]
use std::env;

use reqwest::{
    blocking::Client,
    header::{CONTENT_RANGE, RANGE},
    StatusCode,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(target_os = "macos")]
use crate::distribution_signing::{verify_distribution_sidecar, DistributionSidecar};
use crate::embedding::{
    self, TextEmbeddingManifest, JINA_V1_GOLDEN_JSON, JINA_V1_MANIFEST_JSON,
    LLAMA_SERVER_V1_PIN_JSON,
};

const MODEL_OVERRIDE_ENV: &str = "KOSH_EMBEDDING_MODEL_PATH";
#[cfg(debug_assertions)]
const SIDECAR_OVERRIDE_ENV: &str = "KOSH_LLAMA_SERVER_PATH";
const LLAMA_DEVICE_ENV: &str = "KOSH_LLAMA_DEVICE";
const LLAMA_GPU_LAYERS_ENV: &str = "KOSH_LLAMA_GPU_LAYERS";
const BUNDLED_SIDECAR_PATH: &str = "bin/llama-server";
const BUNDLED_SIDECAR_COMPONENT: &str = "llama-server";
const BUNDLED_RELEASE_MANIFEST_PATH: &str = "release/llama-server.json";
const LIFECYCLE_LOCK_FILE: &str = "semantic-search.lock";
const VERIFICATION_RECEIPT_FILE: &str = "semantic-search-verification.json";
const VERIFICATION_RECEIPT_VERSION: u32 = 1;
const LLAMA_EMBEDDING_NORMALIZATION: &str = "2";
const LLAMA_PARALLEL_SLOTS: &str = "1";
const SIDECAR_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const SIDECAR_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_READ_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTH_POLL_DELAY: Duration = Duration::from_millis(100);
const IDLE_REAPER_POLL_DELAY: Duration = Duration::from_secs(1);
const EMBEDDING_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const LIFECYCLE_LOCK_WAIT: Duration = Duration::from_secs(30);
const LIFECYCLE_LOCK_POLL_DELAY: Duration = Duration::from_millis(50);
const SIDECAR_LOG_FILE_NAME: &str = "llama-server.log";
const SIDECAR_LOG_MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
const SIDECAR_LOG_ARCHIVE_COUNT: usize = 2;
const SIDECAR_LOG_COPY_BUFFER_BYTES: usize = 8 * 1024;
const SIDECAR_LOG_READ_BYTES: usize = 128 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SemanticRuntimePhase {
    NotDownloaded,
    VerificationRequired,
    Downloading,
    Verifying,
    Starting,
    Ready,
    Unavailable,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticRuntimeStatus {
    pub phase: SemanticRuntimePhase,
    pub downloaded_bytes: u64,
    pub model_bytes: u64,
    pub model_disk_usage_bytes: u64,
    pub runtime_running: bool,
    pub verified: bool,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticRuntimeLogs {
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SemanticRuntimeError {
    #[error("semantic runtime I/O failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("semantic runtime HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("semantic runtime JSON failed: {0}")]
    Json(#[from] serde_json::Error),

    #[error("model artifact is invalid: {0}")]
    InvalidArtifact(String),

    #[error("llama.cpp sidecar is unavailable: {0}")]
    RuntimeUnavailable(String),

    #[error("llama.cpp sidecar failed: {0}")]
    Runtime(String),

    #[error("a semantic runtime operation is already in progress")]
    OperationInProgress,
}

impl SemanticRuntimeError {
    pub(crate) fn public_message(&self) -> String {
        redacted_runtime_error(self)
    }
}

struct RuntimeContract {
    manifest: TextEmbeddingManifest,
    manifest_json: String,
    golden_json: String,
    download_url: String,
}

struct EmbeddingRuntimeInner {
    contract: RuntimeContract,
    model_path: PathBuf,
    model_override: Option<PathBuf>,
    sidecar_path: Option<PathBuf>,
    sidecar_expectation: Option<BundledSidecarExpectation>,
    runtime_settings: LlamaRuntimeSettings,
    data_root: PathBuf,
    http: Client,
    status: Mutex<SemanticRuntimeStatus>,
    runtime: Mutex<SidecarRuntime>,
    sidecar_startup: Mutex<()>,
    operation: Mutex<()>,
    failure_recording: Mutex<()>,
    lifecycle_lock: Mutex<Option<File>>,
    idle_timeout: Duration,
    startup_timeout: Duration,
    sidecar_environment: BTreeMap<String, String>,
    shutdown: AtomicBool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LlamaRuntimeSettings {
    device: Option<String>,
    gpu_layers: String,
    pooling: String,
    embedding_normalization: String,
    parallel_slots: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VerificationFileFingerprint {
    canonical_path: String,
    byte_length: u64,
    modified_at_unix_nanos: u64,
    unix_device: Option<u64>,
    unix_inode: Option<u64>,
    unix_change_time_seconds: Option<i64>,
    unix_change_time_nanoseconds: Option<i64>,
    unix_mode: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VerificationReceipt {
    receipt_version: u32,
    manifest_sha256: String,
    golden_fixtures_sha256: String,
    sidecar_pin_sha256: String,
    model: VerificationFileFingerprint,
    sidecar: VerificationFileFingerprint,
    bundled_sidecar: Option<BundledSidecarExpectation>,
    runtime: LlamaRuntimeSettings,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BundledSidecarExpectation {
    sha256: String,
    size: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BundledReleaseManifest {
    binary: BundledReleaseBinary,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BundledReleaseBinary {
    bundle_path: String,
    sha256: String,
    size: u64,
}

struct ResolvedSidecar {
    path: PathBuf,
    expectation: Option<BundledSidecarExpectation>,
}

pub struct EmbeddingRuntime {
    inner: Arc<EmbeddingRuntimeInner>,
    reaper: Mutex<Option<JoinHandle<()>>>,
}

impl EmbeddingRuntime {
    pub fn new(data_root: &Path, resource_dir: Option<&Path>) -> Self {
        let (sidecar_path, sidecar_expectation, sidecar_resolution_error) =
            match resolve_sidecar_artifact(resource_dir) {
                Ok(Some(sidecar)) => (Some(sidecar.path), sidecar.expectation, None),
                Ok(None) => (None, None, None),
                Err(error) => (None, None, Some(error.public_message())),
            };
        Self::start(RuntimeConfiguration {
            contract: jina_contract(),
            data_root: data_root.to_owned(),
            model_override: development_path_override(MODEL_OVERRIDE_ENV),
            sidecar_path,
            sidecar_expectation,
            sidecar_resolution_error,
            runtime_settings: LlamaRuntimeSettings {
                device: development_string_override(LLAMA_DEVICE_ENV),
                gpu_layers: development_string_override(LLAMA_GPU_LAYERS_ENV).unwrap_or_else(
                    || {
                        if cfg!(target_os = "macos") {
                            "all".into()
                        } else {
                            "0".into()
                        }
                    },
                ),
                pooling: "last".into(),
                embedding_normalization: LLAMA_EMBEDDING_NORMALIZATION.into(),
                parallel_slots: LLAMA_PARALLEL_SLOTS.into(),
            },
            idle_timeout: SIDECAR_IDLE_TIMEOUT,
            startup_timeout: SIDECAR_STARTUP_TIMEOUT,
            sidecar_environment: BTreeMap::new(),
        })
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn without_sidecar(data_root: &Path) -> Self {
        Self::start(RuntimeConfiguration {
            contract: jina_contract(),
            data_root: data_root.to_owned(),
            model_override: None,
            sidecar_path: None,
            sidecar_expectation: None,
            sidecar_resolution_error: None,
            runtime_settings: LlamaRuntimeSettings {
                device: None,
                gpu_layers: "0".into(),
                pooling: "last".into(),
                embedding_normalization: LLAMA_EMBEDDING_NORMALIZATION.into(),
                parallel_slots: LLAMA_PARALLEL_SLOTS.into(),
            },
            idle_timeout: SIDECAR_IDLE_TIMEOUT,
            startup_timeout: SIDECAR_STARTUP_TIMEOUT,
            sidecar_environment: BTreeMap::new(),
        })
    }

    fn start(configuration: RuntimeConfiguration) -> Self {
        let RuntimeConfiguration {
            contract,
            data_root,
            model_override,
            sidecar_path,
            sidecar_expectation,
            sidecar_resolution_error,
            runtime_settings,
            idle_timeout,
            startup_timeout,
            sidecar_environment,
        } = configuration;
        let model_file = contract.manifest.config.model_file.clone();
        let model_bytes = contract.manifest.config.model_file_size;
        let model_path = data_root.join("models").join(model_file);
        let http = build_http_client(Duration::from_secs(10), HTTP_READ_IDLE_TIMEOUT);
        let lifecycle_lock = acquire_lifecycle_lock(&data_root, Duration::ZERO);
        let model_candidate = model_override.as_deref().unwrap_or(&model_path);
        let downloaded_bytes = file_size_if_present(model_candidate).unwrap_or_default();
        let partial_bytes = if model_override.is_none() {
            file_size_if_present(&model_path.with_extension("gguf.part")).unwrap_or_default()
        } else {
            0
        };
        let initial_phase = if sidecar_path.is_none() {
            SemanticRuntimePhase::Unavailable
        } else if downloaded_bytes == model_bytes {
            SemanticRuntimePhase::VerificationRequired
        } else {
            SemanticRuntimePhase::NotDownloaded
        };
        let initial_message = sidecar_resolution_error
            .or_else(|| lifecycle_lock.as_ref().err().map(redacted_runtime_error))
            .or_else(|| {
                sidecar_path
                    .is_none()
                    .then(|| missing_sidecar_error().to_string())
            });
        let inner = Arc::new(EmbeddingRuntimeInner {
            contract,
            model_path,
            model_override,
            sidecar_path,
            sidecar_expectation,
            runtime_settings,
            data_root: data_root.to_owned(),
            http,
            status: Mutex::new(SemanticRuntimeStatus {
                phase: initial_phase,
                downloaded_bytes: downloaded_bytes.max(partial_bytes),
                model_bytes,
                model_disk_usage_bytes: downloaded_bytes.saturating_add(partial_bytes),
                runtime_running: false,
                verified: false,
                message: initial_message,
            }),
            runtime: Mutex::new(SidecarRuntime::default()),
            sidecar_startup: Mutex::new(()),
            operation: Mutex::new(()),
            failure_recording: Mutex::new(()),
            lifecycle_lock: Mutex::new(lifecycle_lock.ok()),
            idle_timeout,
            startup_timeout,
            sidecar_environment,
            shutdown: AtomicBool::new(false),
        });
        restore_verified_status(&inner);
        let reaper_inner = Arc::clone(&inner);
        let reaper = thread::Builder::new()
            .name("kosh-semantic-idle-reaper".into())
            .spawn(move || idle_reaper(reaper_inner))
            .ok();
        if reaper.is_none() {
            update_status(&inner, |status| {
                status.phase = SemanticRuntimePhase::Unavailable;
                status.message = Some("could not start the semantic runtime monitor".into());
            });
        }
        Self {
            inner,
            reaper: Mutex::new(reaper),
        }
    }

    pub fn status(&self) -> SemanticRuntimeStatus {
        refresh_runtime_status(&self.inner);
        let mut status = lock(&self.inner.status).clone();
        status.model_disk_usage_bytes = self.model_disk_usage_bytes().unwrap_or_default();
        status
    }

    pub fn model_name(&self) -> &str {
        &self.inner.contract.manifest.model_name
    }

    pub fn model_disk_usage_bytes(&self) -> std::io::Result<u64> {
        if let Some(model_override) = self.inner.model_override.as_deref() {
            return file_size_if_present(model_override);
        }
        let partial_path = self.inner.model_path.with_extension("gguf.part");
        Ok(file_size_if_present(&self.inner.model_path)?
            .saturating_add(file_size_if_present(&partial_path)?))
    }

    pub fn prepare(&self) -> Result<SemanticRuntimeStatus, SemanticRuntimeError> {
        let _operation = self.operation_guard()?;
        self.prepare_locked(true)?;
        Ok(self.status())
    }

    pub fn retry(&self) -> Result<SemanticRuntimeStatus, SemanticRuntimeError> {
        let _operation = self.operation_guard()?;
        self.prepare_locked(false)?;
        Ok(self.status())
    }

    pub fn repair(&self) -> Result<SemanticRuntimeStatus, SemanticRuntimeError> {
        let _operation = self.operation_guard()?;
        ensure_lifecycle_lock(&self.inner)?;
        if self.inner.model_override.is_some() {
            return Err(SemanticRuntimeError::InvalidArtifact(
                "development model overrides must be repaired outside Kosh".into(),
            ));
        }
        stop_sidecar(&self.inner);
        invalidate_verification_receipt(&self.inner.data_root);
        remove_file_if_present(&self.inner.model_path)?;
        remove_file_if_present(&self.inner.model_path.with_extension("gguf.part"))?;
        update_status(&self.inner, |status| {
            status.phase = SemanticRuntimePhase::NotDownloaded;
            status.downloaded_bytes = 0;
            status.model_disk_usage_bytes = 0;
            status.runtime_running = false;
            status.verified = false;
            status.message = Some("Repairing the local semantic runtime".into());
        });
        self.prepare_locked(false)?;
        Ok(self.status())
    }

    pub fn embed_query(&self, query: &str) -> Result<Vec<f32>, SemanticRuntimeError> {
        self.embed_prefixed(self.inner.contract.manifest.query_input(query))
    }

    pub fn embed_document(&self, document: &str) -> Result<Vec<f32>, SemanticRuntimeError> {
        self.embed_prefixed(self.inner.contract.manifest.document_input(document))
    }

    pub fn logs(&self) -> Result<SemanticRuntimeLogs, SemanticRuntimeError> {
        read_sidecar_logs(&self.inner.data_root)
    }

    fn operation_guard(&self) -> Result<std::sync::MutexGuard<'_, ()>, SemanticRuntimeError> {
        match self.inner.operation.try_lock() {
            Ok(guard) => Ok(guard),
            Err(MutexTryLockError::WouldBlock) => Err(SemanticRuntimeError::OperationInProgress),
            Err(MutexTryLockError::Poisoned(poisoned)) => Ok(poisoned.into_inner()),
        }
    }

    fn prepare_locked(&self, allow_cached_verification: bool) -> Result<(), SemanticRuntimeError> {
        if self.inner.shutdown.load(Ordering::Acquire) {
            return Err(SemanticRuntimeError::Runtime(
                "semantic runtime is shutting down".into(),
            ));
        }
        ensure_lifecycle_lock(&self.inner)?;
        match prepare_and_verify(&self.inner, allow_cached_verification) {
            Ok(()) => {
                let runtime_running = runtime_is_running(&self.inner);
                update_status(&self.inner, |status| {
                    status.phase = SemanticRuntimePhase::Ready;
                    status.downloaded_bytes = status.model_bytes;
                    status.model_disk_usage_bytes = status.model_bytes;
                    status.runtime_running = runtime_running;
                    status.verified = true;
                    status.message = None;
                });
                Ok(())
            }
            Err(error) => {
                set_failure(&self.inner, &error);
                stop_sidecar(&self.inner);
                Err(error)
            }
        }
    }

    fn embed_prefixed(&self, input: String) -> Result<Vec<f32>, SemanticRuntimeError> {
        if self.status().phase != SemanticRuntimePhase::Ready {
            return Err(SemanticRuntimeError::RuntimeUnavailable(
                "the semantic runtime is not verified; lexical search remains available".into(),
            ));
        }
        match embed(&self.inner, &input) {
            Ok(embedding) => Ok(embedding),
            Err(error) => {
                set_failure(&self.inner, &error);
                stop_sidecar(&self.inner);
                Err(error)
            }
        }
    }

    pub fn shutdown(&self) {
        {
            let _failure_recording = lock(&self.inner.failure_recording);
            if self.inner.shutdown.swap(true, Ordering::SeqCst) {
                return;
            }
        }
        stop_sidecar(&self.inner);
        drop(lock(&self.inner.sidecar_startup));
        if let Some(reaper) = lock(&self.reaper).take() {
            if let Err(error) = reaper.join() {
                log::error!("semantic runtime monitor panicked during shutdown: {error:?}");
            }
        }
        release_lifecycle_lock(&self.inner);
    }
}

impl Drop for EmbeddingRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn jina_contract() -> RuntimeContract {
    let manifest = embedding::jina_v1_manifest();
    RuntimeContract {
        download_url: format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            manifest.model_name, manifest.model_revision, manifest.config.model_file
        ),
        manifest_json: JINA_V1_MANIFEST_JSON.into(),
        golden_json: JINA_V1_GOLDEN_JSON.into(),
        manifest,
    }
}

struct RuntimeConfiguration {
    contract: RuntimeContract,
    data_root: PathBuf,
    model_override: Option<PathBuf>,
    sidecar_path: Option<PathBuf>,
    sidecar_expectation: Option<BundledSidecarExpectation>,
    sidecar_resolution_error: Option<String>,
    runtime_settings: LlamaRuntimeSettings,
    idle_timeout: Duration,
    startup_timeout: Duration,
    sidecar_environment: BTreeMap<String, String>,
}

fn file_size_if_present(path: &Path) -> std::io::Result<u64> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

fn build_http_client(connect_timeout: Duration, read_idle_timeout: Duration) -> Client {
    // Reqwest's blocking timeout applies while awaiting the response and afresh
    // to every Response::read, bounding idle reads without limiting total bytes.
    Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(read_idle_timeout)
        .build()
        .unwrap_or_else(|_| Client::new())
}

fn restore_verified_status(inner: &EmbeddingRuntimeInner) {
    let Some(sidecar_path) = inner.sidecar_path.as_deref() else {
        return;
    };
    if lock(&inner.lifecycle_lock).is_none() {
        return;
    }
    let model_path = inner
        .model_override
        .as_deref()
        .unwrap_or(inner.model_path.as_path());
    if !model_path.is_file() {
        return;
    }
    sweep_stale_sidecar(&inner.data_root, sidecar_path, model_path);
    let Ok(expected) = build_verification_receipt(inner, model_path, sidecar_path) else {
        return;
    };
    if !verification_receipt_matches(&inner.data_root, &expected) {
        return;
    }
    update_status(inner, |status| {
        status.phase = SemanticRuntimePhase::Ready;
        status.downloaded_bytes = inner.contract.manifest.config.model_file_size;
        status.model_disk_usage_bytes = inner.contract.manifest.config.model_file_size;
        status.runtime_running = false;
        status.verified = true;
        status.message = None;
    });
}

fn refresh_runtime_status(inner: &EmbeddingRuntimeInner) {
    let mut runtime = lock(&inner.runtime);
    let exited = runtime
        .child
        .as_mut()
        .and_then(|child| child.try_wait().ok().flatten());
    if exited.is_some() {
        stop_runtime(&mut runtime, &inner.data_root);
    }
    let running = runtime.child.is_some();
    drop(runtime);
    update_status(inner, |status| {
        status.runtime_running = running;
        if exited.is_some() && !inner.shutdown.load(Ordering::Acquire) {
            status.phase = SemanticRuntimePhase::Failed;
            status.message =
                Some("llama-server exited unexpectedly; lexical search remains available".into());
        }
    });
}

fn runtime_is_running(inner: &EmbeddingRuntimeInner) -> bool {
    refresh_runtime_status(inner);
    lock(&inner.status).runtime_running
}

fn idle_reaper(inner: Arc<EmbeddingRuntimeInner>) {
    while !inner.shutdown.load(Ordering::Relaxed) {
        reap_idle_sidecar(&inner);
        interruptible_sleep(
            &inner.shutdown,
            IDLE_REAPER_POLL_DELAY.min(inner.idle_timeout),
        );
    }
}

fn redacted_runtime_error(error: &SemanticRuntimeError) -> String {
    let SemanticRuntimeError::Http(source) = error else {
        return error.to_string();
    };
    if let Some(status) = source.status() {
        return format!("semantic runtime HTTP request failed with status {status}");
    }
    if source.is_timeout() {
        return "semantic runtime HTTP request timed out".into();
    }
    if source.is_connect() {
        return "semantic runtime HTTP connection failed".into();
    }
    if source.is_decode() || source.is_body() {
        return "semantic runtime HTTP response failed".into();
    }
    "semantic runtime HTTP request failed".into()
}

fn prepare_and_verify(
    inner: &EmbeddingRuntimeInner,
    allow_cached_verification: bool,
) -> Result<(), SemanticRuntimeError> {
    let sidecar_path = inner
        .sidecar_path
        .as_deref()
        .ok_or_else(missing_sidecar_error)?;
    let candidate_artifact_path = inner
        .model_override
        .as_deref()
        .unwrap_or(inner.model_path.as_path());
    if allow_cached_verification && candidate_artifact_path.is_file() {
        match build_verification_receipt(inner, candidate_artifact_path, sidecar_path) {
            Ok(expected) if verification_receipt_matches(&inner.data_root, &expected) => {
                update_status(inner, |status| {
                    status.phase = SemanticRuntimePhase::Verifying;
                    status.downloaded_bytes = inner.contract.manifest.config.model_file_size;
                    status.message = Some("Using cached semantic-search verification".into());
                });
                log::info!("semantic-search verification receipt matched");
                return Ok(());
            }
            Ok(_) => {}
            Err(error) => {
                log::warn!("could not inspect semantic-search verification inputs: {error}");
            }
        }
    }

    let artifact_path = if let Some(override_path) = inner.model_override.as_deref() {
        update_status(inner, |status| {
            status.phase = SemanticRuntimePhase::Verifying;
            status.message = Some(format!("Verifying {}", override_path.display()));
        });
        verify_artifact(override_path, &inner.contract.manifest)?;
        override_path.to_owned()
    } else {
        prepare_managed_artifact(inner)?
    };
    let receipt_before = build_verification_receipt(inner, &artifact_path, sidecar_path)?;
    update_status(inner, |status| {
        status.phase = SemanticRuntimePhase::Starting;
        status.downloaded_bytes = inner.contract.manifest.config.model_file_size;
        status.verified = false;
        status.message = Some("Verifying llama.cpp compatibility".into());
    });
    verify_golden_fixtures(inner, &artifact_path)?;
    let receipt_after = build_verification_receipt(inner, &artifact_path, sidecar_path)?;
    if receipt_before != receipt_after {
        return Err(SemanticRuntimeError::InvalidArtifact(
            "the model or llama-server changed during verification".into(),
        ));
    }
    write_verification_receipt(&inner.data_root, &receipt_after)?;
    Ok(())
}

fn prepare_managed_artifact(
    inner: &EmbeddingRuntimeInner,
) -> Result<PathBuf, SemanticRuntimeError> {
    if inner.model_path.is_file()
        && verify_artifact(&inner.model_path, &inner.contract.manifest).is_ok()
    {
        update_status(inner, |status| {
            status.phase = SemanticRuntimePhase::Verifying;
            status.downloaded_bytes = inner.contract.manifest.config.model_file_size;
            status.message = Some("Verified local semantic-search model".into());
        });
        return Ok(inner.model_path.clone());
    }
    let parent = inner.model_path.parent().ok_or_else(|| {
        SemanticRuntimeError::InvalidArtifact("model path has no parent directory".into())
    })?;
    fs::create_dir_all(parent)?;
    let partial_path = inner.model_path.with_extension("gguf.part");
    let mut downloaded = partial_path
        .metadata()
        .map(|value| value.len())
        .unwrap_or(0);
    if downloaded > inner.contract.manifest.config.model_file_size {
        OpenOptions::new()
            .write(true)
            .open(&partial_path)?
            .set_len(0)?;
        downloaded = 0;
    }
    if downloaded == inner.contract.manifest.config.model_file_size {
        if verify_artifact(&partial_path, &inner.contract.manifest).is_ok() {
            fs::rename(&partial_path, &inner.model_path)?;
            sync_directory(parent)?;
            return Ok(inner.model_path.clone());
        }
        OpenOptions::new()
            .write(true)
            .open(&partial_path)?
            .set_len(0)?;
        downloaded = 0;
    }
    update_status(inner, |status| {
        status.phase = SemanticRuntimePhase::Downloading;
        status.downloaded_bytes = downloaded;
        status.message = Some("Downloading the semantic-search model".into());
    });

    let mut request = inner.http.get(&inner.contract.download_url);
    if downloaded > 0 {
        request = request.header(RANGE, format!("bytes={downloaded}-"));
    }
    let mut response = request.send()?;
    if response.status() == StatusCode::RANGE_NOT_SATISFIABLE
        && downloaded == inner.contract.manifest.config.model_file_size
    {
        verify_artifact(&partial_path, &inner.contract.manifest)?;
    } else {
        response = response.error_for_status()?;
        let resumed = response.status() == StatusCode::PARTIAL_CONTENT;
        if downloaded > 0 && !resumed {
            downloaded = 0;
        }
        if resumed {
            validate_content_range(response.headers().get(CONTENT_RANGE), downloaded)?;
        }
        let mut output = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(!resumed)
            .open(&partial_path)?;
        if resumed {
            output.seek(SeekFrom::End(0))?;
        }
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            if inner.shutdown.load(Ordering::Relaxed) {
                return Err(SemanticRuntimeError::Runtime(
                    "download canceled during shutdown".into(),
                ));
            }
            let read = response.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            output.write_all(&buffer[..read])?;
            downloaded = downloaded.saturating_add(read as u64);
            if downloaded > inner.contract.manifest.config.model_file_size {
                return Err(SemanticRuntimeError::InvalidArtifact(
                    "model download exceeded the declared byte length".into(),
                ));
            }
            update_status(inner, |status| {
                status.downloaded_bytes = downloaded;
                status.model_disk_usage_bytes = downloaded;
            });
        }
        output.sync_all()?;
        verify_artifact(&partial_path, &inner.contract.manifest)?;
    }
    fs::rename(&partial_path, &inner.model_path)?;
    sync_directory(parent)?;
    update_status(inner, |status| {
        status.phase = SemanticRuntimePhase::Verifying;
        status.downloaded_bytes = inner.contract.manifest.config.model_file_size;
        status.message = Some("Verifying the semantic-search model".into());
    });
    Ok(inner.model_path.clone())
}

fn validate_content_range(
    header: Option<&reqwest::header::HeaderValue>,
    expected_start: u64,
) -> Result<(), SemanticRuntimeError> {
    let value = header
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            SemanticRuntimeError::InvalidArtifact("resume response omitted Content-Range".into())
        })?;
    let expected_prefix = format!("bytes {expected_start}-");
    if !value.starts_with(&expected_prefix) {
        return Err(SemanticRuntimeError::InvalidArtifact(format!(
            "resume response started at the wrong byte: {value}"
        )));
    }
    Ok(())
}

fn verify_artifact(
    path: &Path,
    manifest: &TextEmbeddingManifest,
) -> Result<(), SemanticRuntimeError> {
    let metadata = path.metadata().map_err(|error| {
        SemanticRuntimeError::InvalidArtifact(format!("cannot read {}: {error}", path.display()))
    })?;
    if metadata.len() != manifest.config.model_file_size {
        return Err(SemanticRuntimeError::InvalidArtifact(format!(
            "{} has {} bytes; expected {}",
            path.display(),
            metadata.len(),
            manifest.config.model_file_size
        )));
    }
    let observed = sha256_file(path)?;
    if observed != manifest.model_file_sha256 {
        return Err(SemanticRuntimeError::InvalidArtifact(format!(
            "{} has SHA-256 {observed}; expected {}",
            path.display(),
            manifest.model_file_sha256
        )));
    }
    Ok(())
}

fn verify_bundled_sidecar_artifact(
    inner: &EmbeddingRuntimeInner,
    path: &Path,
) -> Result<(), SemanticRuntimeError> {
    let Some(expectation) = inner.sidecar_expectation.as_ref() else {
        return Ok(());
    };
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        SemanticRuntimeError::InvalidArtifact(format!(
            "cannot inspect bundled llama-server: {error}"
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(SemanticRuntimeError::InvalidArtifact(
            "bundled llama-server is not a regular file".into(),
        ));
    }
    let size_matches = metadata.len() == expectation.size;
    let observed = sha256_file(path)?;
    if size_matches && observed == expectation.sha256 {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    if verify_distribution_sidecar(
        path,
        DistributionSidecar::LlamaServer,
        BUNDLED_SIDECAR_COMPONENT,
        BUNDLED_SIDECAR_PATH,
    )
    .is_ok_and(|verified| verified.size == metadata.len())
    {
        return Ok(());
    }

    if !size_matches {
        Err(SemanticRuntimeError::InvalidArtifact(format!(
            "bundled llama-server has {} bytes; expected {}",
            metadata.len(),
            expectation.size
        )))
    } else {
        Err(SemanticRuntimeError::InvalidArtifact(format!(
            "bundled llama-server has SHA-256 {observed}; expected {}",
            expectation.sha256
        )))
    }
}

fn sha256_file(path: &Path) -> Result<String, SemanticRuntimeError> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex(&digest.finalize()))
}

fn build_verification_receipt(
    inner: &EmbeddingRuntimeInner,
    model_path: &Path,
    sidecar_path: &Path,
) -> Result<VerificationReceipt, SemanticRuntimeError> {
    let mut receipt = build_verification_receipt_for_inputs(
        &inner.runtime_settings,
        &inner.contract.manifest_json,
        &inner.contract.golden_json,
        LLAMA_SERVER_V1_PIN_JSON,
        model_path,
        sidecar_path,
    )?;
    receipt.bundled_sidecar = inner.sidecar_expectation.clone();
    Ok(receipt)
}

fn build_verification_receipt_for_inputs(
    runtime_settings: &LlamaRuntimeSettings,
    manifest_json: &str,
    golden_json: &str,
    sidecar_pin_json: &str,
    model_path: &Path,
    sidecar_path: &Path,
) -> Result<VerificationReceipt, SemanticRuntimeError> {
    Ok(VerificationReceipt {
        receipt_version: VERIFICATION_RECEIPT_VERSION,
        manifest_sha256: sha256_text(manifest_json),
        golden_fixtures_sha256: sha256_text(golden_json),
        sidecar_pin_sha256: sha256_text(sidecar_pin_json),
        model: verification_file_fingerprint(model_path)?,
        sidecar: verification_file_fingerprint(sidecar_path)?,
        bundled_sidecar: None,
        runtime: runtime_settings.clone(),
    })
}

fn verification_file_fingerprint(
    path: &Path,
) -> Result<VerificationFileFingerprint, SemanticRuntimeError> {
    let canonical_path = fs::canonicalize(path).map_err(|error| {
        SemanticRuntimeError::InvalidArtifact(format!("cannot resolve {}: {error}", path.display()))
    })?;
    let metadata = canonical_path.metadata().map_err(|error| {
        SemanticRuntimeError::InvalidArtifact(format!(
            "cannot inspect {}: {error}",
            canonical_path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(SemanticRuntimeError::InvalidArtifact(format!(
            "{} is not a file",
            canonical_path.display()
        )));
    }
    let modified_at_unix_nanos = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            SemanticRuntimeError::InvalidArtifact(format!(
                "{} has a modification time before the Unix epoch",
                canonical_path.display()
            ))
        })?
        .as_nanos()
        .try_into()
        .map_err(|_| {
            SemanticRuntimeError::InvalidArtifact(format!(
                "{} has an unsupported modification time",
                canonical_path.display()
            ))
        })?;

    #[cfg(unix)]
    let (
        unix_device,
        unix_inode,
        unix_change_time_seconds,
        unix_change_time_nanoseconds,
        unix_mode,
    ) = {
        use std::os::unix::fs::MetadataExt;
        (
            Some(metadata.dev()),
            Some(metadata.ino()),
            Some(metadata.ctime()),
            Some(metadata.ctime_nsec()),
            Some(metadata.mode()),
        )
    };
    #[cfg(not(unix))]
    let (
        unix_device,
        unix_inode,
        unix_change_time_seconds,
        unix_change_time_nanoseconds,
        unix_mode,
    ) = (None, None, None, None, None);

    Ok(VerificationFileFingerprint {
        canonical_path: canonical_path.to_string_lossy().into_owned(),
        byte_length: metadata.len(),
        modified_at_unix_nanos,
        unix_device,
        unix_inode,
        unix_change_time_seconds,
        unix_change_time_nanoseconds,
        unix_mode,
    })
}

fn verification_receipt_matches(data_root: &Path, expected: &VerificationReceipt) -> bool {
    match read_verification_receipt(data_root) {
        Ok(Some(cached)) => cached == *expected,
        Ok(None) => false,
        Err(error) => {
            log::warn!("ignoring invalid semantic-search verification receipt: {error}");
            false
        }
    }
}

fn read_verification_receipt(
    data_root: &Path,
) -> Result<Option<VerificationReceipt>, SemanticRuntimeError> {
    let path = verification_receipt_path(data_root);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    serde_json::from_slice(&bytes).map(Some).map_err(Into::into)
}

fn write_verification_receipt(
    data_root: &Path,
    receipt: &VerificationReceipt,
) -> Result<(), SemanticRuntimeError> {
    fs::create_dir_all(data_root)?;
    let path = verification_receipt_path(data_root);
    let temporary = path.with_extension("json.tmp");
    let mut bytes = serde_json::to_vec_pretty(receipt)?;
    bytes.push(b'\n');
    let mut output = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&temporary)?;
    output.write_all(&bytes)?;
    output.sync_all()?;
    drop(output);
    fs::rename(&temporary, &path)?;
    sync_directory(data_root)?;
    Ok(())
}

fn invalidate_verification_receipt(data_root: &Path) {
    let path = verification_receipt_path(data_root);
    if let Err(error) = fs::remove_file(&path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            log::warn!(
                "could not invalidate semantic-search verification receipt {}: {error}",
                path.display()
            );
        }
    }
}

fn verification_receipt_path(data_root: &Path) -> PathBuf {
    data_root.join(VERIFICATION_RECEIPT_FILE)
}

fn sha256_text(value: &str) -> String {
    hex(&Sha256::digest(value.as_bytes()))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GoldenFixtureFile {
    fixture_version: u32,
    model_file_sha256: String,
    generated_with: GoldenRuntime,
    tolerance: GoldenTolerance,
    cases: Vec<GoldenCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GoldenRuntime {
    runtime: String,
    revision: String,
    build: u32,
    device: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GoldenTolerance {
    minimum_cosine_similarity: f64,
    maximum_absolute_difference: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GoldenCase {
    name: String,
    input: String,
    embedding: Vec<f32>,
}

fn verify_golden_fixtures(
    inner: &EmbeddingRuntimeInner,
    model_path: &Path,
) -> Result<(), SemanticRuntimeError> {
    let fixtures: GoldenFixtureFile = serde_json::from_str(&inner.contract.golden_json)?;
    if fixtures.fixture_version != 1
        || fixtures.model_file_sha256 != inner.contract.manifest.model_file_sha256
    {
        return Err(SemanticRuntimeError::InvalidArtifact(
            "golden fixtures do not match the active model manifest".into(),
        ));
    }
    log::info!(
        "verifying {} golden fixtures generated by {} {} build {} on {}",
        fixtures.cases.len(),
        fixtures.generated_with.runtime,
        fixtures.generated_with.revision,
        fixtures.generated_with.build,
        fixtures.generated_with.device,
    );
    for case in fixtures.cases {
        let observed = embed_with_model(inner, model_path, &case.input, false)?;
        validate_embedding(&observed, inner.contract.manifest.dimension as usize)?;
        validate_embedding(&case.embedding, inner.contract.manifest.dimension as usize)?;
        let cosine = case
            .embedding
            .iter()
            .zip(&observed)
            .map(|(expected, actual)| f64::from(*expected) * f64::from(*actual))
            .sum::<f64>();
        let maximum_difference = case
            .embedding
            .iter()
            .zip(&observed)
            .map(|(expected, actual)| (f64::from(*expected) - f64::from(*actual)).abs())
            .fold(0.0_f64, f64::max);
        if cosine < fixtures.tolerance.minimum_cosine_similarity
            || maximum_difference > fixtures.tolerance.maximum_absolute_difference
        {
            return Err(SemanticRuntimeError::InvalidArtifact(format!(
                "golden fixture {} failed (cosine {cosine}, max difference {maximum_difference})",
                case.name
            )));
        }
    }
    Ok(())
}

#[derive(Default)]
struct SidecarRuntime {
    child: Option<Child>,
    endpoint: Option<String>,
    model_path: Option<PathBuf>,
    model_fingerprint: Option<VerificationFileFingerprint>,
    sidecar_fingerprint: Option<VerificationFileFingerprint>,
    last_used: Option<Instant>,
    log_threads: Vec<JoinHandle<()>>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

fn validate_embedding(embedding: &[f32], dimension: usize) -> Result<(), SemanticRuntimeError> {
    if embedding.len() != dimension {
        return Err(SemanticRuntimeError::Runtime(format!(
            "embedding has {} dimensions; expected {dimension}",
            embedding.len()
        )));
    }
    if embedding.iter().any(|value| !value.is_finite()) {
        return Err(SemanticRuntimeError::Runtime(
            "embedding contains a non-finite value".into(),
        ));
    }
    let norm = embedding
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    if (norm - 1.0).abs() > 0.001 {
        return Err(SemanticRuntimeError::Runtime(format!(
            "embedding must be L2-normalized; observed norm {norm}"
        )));
    }
    Ok(())
}

fn embed(inner: &EmbeddingRuntimeInner, input: &str) -> Result<Vec<f32>, SemanticRuntimeError> {
    let model_path = inner
        .model_override
        .as_deref()
        .unwrap_or(inner.model_path.as_path());
    embed_with_model(inner, model_path, input, true)
}

fn embed_with_model(
    inner: &EmbeddingRuntimeInner,
    model_path: &Path,
    input: &str,
    require_verified_inputs: bool,
) -> Result<Vec<f32>, SemanticRuntimeError> {
    let endpoint = ensure_sidecar(inner, model_path, require_verified_inputs)?;
    let response = inner
        .http
        .post(format!("{endpoint}/v1/embeddings"))
        .timeout(EMBEDDING_REQUEST_TIMEOUT)
        .json(&serde_json::json!({ "input": input }))
        .send()?
        .error_for_status()?
        .json::<EmbeddingResponse>()?;
    let embedding = response
        .data
        .into_iter()
        .next()
        .ok_or_else(|| SemanticRuntimeError::Runtime("embedding response had no vectors".into()))?
        .embedding;
    validate_embedding(&embedding, inner.contract.manifest.dimension as usize)?;
    let mut runtime = lock(&inner.runtime);
    runtime.last_used = Some(Instant::now());
    Ok(embedding)
}

fn ensure_sidecar(
    inner: &EmbeddingRuntimeInner,
    model_path: &Path,
    require_verified_inputs: bool,
) -> Result<String, SemanticRuntimeError> {
    let _startup = lock(&inner.sidecar_startup);
    if inner.shutdown.load(Ordering::Acquire) {
        return Err(SemanticRuntimeError::Runtime(
            "sidecar start canceled".into(),
        ));
    }
    let sidecar_path = inner.sidecar_path.as_deref().ok_or_else(|| {
        SemanticRuntimeError::RuntimeUnavailable("no llama-server executable was found".into())
    })?;
    let (model_fingerprint, sidecar_fingerprint) =
        sidecar_launch_fingerprints(inner, model_path, sidecar_path, require_verified_inputs)?;
    let mut runtime = lock(&inner.runtime);
    if let Some(child) = runtime.child.as_mut() {
        if child.try_wait()?.is_none()
            && runtime.model_path.as_deref() == Some(model_path)
            && runtime.model_fingerprint.as_ref() == Some(&model_fingerprint)
            && runtime.sidecar_fingerprint.as_ref() == Some(&sidecar_fingerprint)
        {
            runtime.last_used = Some(Instant::now());
            update_status(inner, |status| status.runtime_running = true);
            return runtime
                .endpoint
                .clone()
                .ok_or_else(|| SemanticRuntimeError::Runtime("sidecar endpoint was lost".into()));
        }
        stop_runtime(&mut runtime, &inner.data_root);
    }
    drop(runtime);

    verify_bundled_sidecar_artifact(inner, sidecar_path)?;
    let (model_fingerprint, sidecar_fingerprint) =
        sidecar_launch_fingerprints(inner, model_path, sidecar_path, require_verified_inputs)?;
    sweep_stale_sidecar(&inner.data_root, sidecar_path, model_path);
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let log_dir = inner.data_root.join("logs");
    let log_path = log_dir.join(SIDECAR_LOG_FILE_NAME);
    let log_writer = Arc::new(Mutex::new(BoundedSidecarLog::open(&log_dir)?));
    update_status(inner, |status| {
        status.phase = SemanticRuntimePhase::Starting;
        status.runtime_running = false;
        status.message = Some("Starting the local embedding runtime".into());
    });
    let mut command = Command::new(sidecar_path);
    command
        .arg("--model")
        .arg(model_path)
        .arg("--embedding")
        .arg("--pooling")
        .arg(&inner.runtime_settings.pooling)
        .arg("--embd-normalize")
        .arg(&inner.runtime_settings.embedding_normalization)
        .arg("--n-gpu-layers")
        .arg(&inner.runtime_settings.gpu_layers)
        .arg("--parallel")
        .arg(&inner.runtime_settings.parallel_slots)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.envs(&inner.sidecar_environment);
    if let Some(device) = inner.runtime_settings.device.as_deref() {
        command.arg("--device").arg(device);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let mut runtime = lock(&inner.runtime);
    let log_threads = match start_sidecar_log_pumps(&mut child, log_writer) {
        Ok(threads) => threads,
        Err(error) => {
            terminate_process_group(&mut child);
            return Err(error.into());
        }
    };
    let pid = child.id();
    if let Err(error) = write_pidfile(&inner.data_root, pid, sidecar_path, model_path) {
        terminate_process_group(&mut child);
        join_sidecar_log_threads(log_threads);
        return Err(error);
    }
    let endpoint = format!("http://127.0.0.1:{port}");
    runtime.child = Some(child);
    runtime.endpoint = Some(endpoint.clone());
    runtime.model_path = Some(model_path.to_owned());
    runtime.model_fingerprint = Some(model_fingerprint);
    runtime.sidecar_fingerprint = Some(sidecar_fingerprint);
    runtime.last_used = Some(Instant::now());
    runtime.log_threads = log_threads;
    drop(runtime);

    let startup_deadline = Instant::now() + inner.startup_timeout;
    while Instant::now() < startup_deadline {
        if inner.shutdown.load(Ordering::Relaxed) {
            stop_sidecar(inner);
            return Err(SemanticRuntimeError::Runtime(
                "sidecar start canceled".into(),
            ));
        }
        {
            let mut runtime = lock(&inner.runtime);
            let status = runtime
                .child
                .as_mut()
                .map(Child::try_wait)
                .transpose()?
                .flatten();
            if let Some(status) = status {
                stop_runtime(&mut runtime, &inner.data_root);
                return Err(SemanticRuntimeError::Runtime(format!(
                    "llama-server exited during startup with {status}; see {}",
                    log_path.display()
                )));
            }
        }
        if inner
            .http
            .get(format!("{endpoint}/health"))
            .timeout(Duration::from_secs(1))
            .send()
            .is_ok_and(|response| response.status().is_success())
        {
            update_status(inner, |status| {
                status.runtime_running = true;
                if status.verified {
                    status.phase = SemanticRuntimePhase::Ready;
                    status.message = None;
                }
            });
            return Ok(endpoint);
        }
        thread::sleep(HEALTH_POLL_DELAY);
    }
    stop_sidecar(inner);
    Err(SemanticRuntimeError::Runtime(format!(
        "llama-server did not become healthy; see {}",
        log_path.display()
    )))
}

fn sidecar_launch_fingerprints(
    inner: &EmbeddingRuntimeInner,
    model_path: &Path,
    sidecar_path: &Path,
    require_verified_inputs: bool,
) -> Result<(VerificationFileFingerprint, VerificationFileFingerprint), SemanticRuntimeError> {
    if require_verified_inputs {
        let expected = build_verification_receipt(inner, model_path, sidecar_path)?;
        if !verification_receipt_matches(&inner.data_root, &expected) {
            return Err(SemanticRuntimeError::InvalidArtifact(
                "the verified model or llama-server changed; retry verification".into(),
            ));
        }
        Ok((expected.model, expected.sidecar))
    } else {
        Ok((
            verification_file_fingerprint(model_path)?,
            verification_file_fingerprint(sidecar_path)?,
        ))
    }
}

fn reap_idle_sidecar(inner: &EmbeddingRuntimeInner) {
    let mut runtime = lock(&inner.runtime);
    if runtime
        .last_used
        .is_some_and(|last_used| last_used.elapsed() >= inner.idle_timeout)
    {
        stop_runtime(&mut runtime, &inner.data_root);
        drop(runtime);
        update_status(inner, |status| {
            status.runtime_running = false;
            if status.verified {
                status.phase = SemanticRuntimePhase::Ready;
                status.message = None;
            }
        });
    }
}

fn stop_sidecar(inner: &EmbeddingRuntimeInner) {
    let mut runtime = lock(&inner.runtime);
    stop_runtime(&mut runtime, &inner.data_root);
    drop(runtime);
    update_status(inner, |status| status.runtime_running = false);
}

fn stop_runtime(runtime: &mut SidecarRuntime, data_root: &Path) {
    if let Some(mut child) = runtime.child.take() {
        let pid = child.id();
        terminate_process_group(&mut child);
        remove_pidfile_if_owned(data_root, pid);
    }
    join_sidecar_log_threads(runtime.log_threads.drain(..));
    runtime.endpoint = None;
    runtime.model_path = None;
    runtime.model_fingerprint = None;
    runtime.sidecar_fingerprint = None;
    runtime.last_used = None;
}

fn start_sidecar_log_pumps(
    child: &mut Child,
    writer: Arc<Mutex<BoundedSidecarLog>>,
) -> std::io::Result<Vec<JoinHandle<()>>> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("llama-server stdout pipe was unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("llama-server stderr pipe was unavailable"))?;
    let stdout_writer = Arc::clone(&writer);
    let stdout_thread = thread::Builder::new()
        .name("kosh-llama-stdout".into())
        .spawn(move || copy_sidecar_log(stdout, stdout_writer))?;
    let stderr_thread = match thread::Builder::new()
        .name("kosh-llama-stderr".into())
        .spawn(move || copy_sidecar_log(stderr, writer))
    {
        Ok(thread) => thread,
        Err(error) => {
            terminate_process_group(child);
            join_sidecar_log_threads([stdout_thread]);
            return Err(error);
        }
    };
    Ok(vec![stdout_thread, stderr_thread])
}

fn copy_sidecar_log(mut source: impl Read, writer: Arc<Mutex<BoundedSidecarLog>>) {
    let mut buffer = [0_u8; SIDECAR_LOG_COPY_BUFFER_BYTES];
    loop {
        let read = match source.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => read,
            Err(error) => {
                log::error!("could not read semantic sidecar output: {error}");
                return;
            }
        };
        if let Err(error) = lock(&writer).write_all(&buffer[..read]) {
            log::error!("could not persist semantic sidecar output: {error}");
            return;
        }
    }
}

fn join_sidecar_log_threads(threads: impl IntoIterator<Item = JoinHandle<()>>) {
    for thread in threads {
        if thread.join().is_err() {
            log::error!("semantic sidecar log worker panicked");
        }
    }
}

struct BoundedSidecarLog {
    directory: PathBuf,
    max_file_bytes: u64,
    archive_count: usize,
    active_file: Option<File>,
    active_bytes: u64,
}

impl BoundedSidecarLog {
    fn open(directory: &Path) -> std::io::Result<Self> {
        Self::open_with_limits(
            directory,
            SIDECAR_LOG_MAX_FILE_BYTES,
            SIDECAR_LOG_ARCHIVE_COUNT,
        )
    }

    fn open_with_limits(
        directory: &Path,
        max_file_bytes: u64,
        archive_count: usize,
    ) -> std::io::Result<Self> {
        if max_file_bytes == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "sidecar log limit must be greater than zero",
            ));
        }
        fs::create_dir_all(directory)?;
        remove_excess_sidecar_archives(directory, archive_count)?;
        for archive in 1..=archive_count {
            trim_log_to_tail(&sidecar_log_path(directory, Some(archive)), max_file_bytes)?;
        }
        let active_path = sidecar_log_path(directory, None);
        trim_log_to_tail(&active_path, max_file_bytes)?;
        if active_path
            .metadata()
            .is_ok_and(|metadata| metadata.len() >= max_file_bytes)
        {
            rotate_sidecar_log_files(directory, archive_count)?;
        }
        let active_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&active_path)?;
        let active_bytes = active_file.metadata()?.len();
        Ok(Self {
            directory: directory.to_owned(),
            max_file_bytes,
            archive_count,
            active_file: Some(active_file),
            active_bytes,
        })
    }

    fn write_all(&mut self, mut bytes: &[u8]) -> std::io::Result<()> {
        while !bytes.is_empty() {
            if self.active_bytes >= self.max_file_bytes {
                self.rotate()?;
            }
            let remaining = self.max_file_bytes - self.active_bytes;
            let write_length = bytes
                .len()
                .min(usize::try_from(remaining).unwrap_or(usize::MAX));
            let active_file = self
                .active_file
                .as_mut()
                .ok_or_else(|| std::io::Error::other("sidecar log file is closed"))?;
            active_file.write_all(&bytes[..write_length])?;
            self.active_bytes += u64::try_from(write_length)
                .map_err(|_| std::io::Error::other("sidecar log write length overflowed"))?;
            bytes = &bytes[write_length..];
        }
        Ok(())
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        if let Some(mut file) = self.active_file.take() {
            file.flush()?;
        }
        rotate_sidecar_log_files(&self.directory, self.archive_count)?;
        self.active_file = Some(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(sidecar_log_path(&self.directory, None))?,
        );
        self.active_bytes = 0;
        Ok(())
    }
}

fn sidecar_log_path(directory: &Path, archive: Option<usize>) -> PathBuf {
    match archive {
        Some(index) => directory.join(format!("llama-server.{index}.log")),
        None => directory.join(SIDECAR_LOG_FILE_NAME),
    }
}

fn read_sidecar_logs(data_root: &Path) -> Result<SemanticRuntimeLogs, SemanticRuntimeError> {
    let directory = data_root.join("logs");
    let paths = [
        sidecar_log_path(&directory, Some(2)),
        sidecar_log_path(&directory, Some(1)),
        sidecar_log_path(&directory, None),
    ];
    let total_bytes = paths
        .iter()
        .filter_map(|path| path.metadata().ok())
        .map(|metadata| metadata.len())
        .sum::<u64>();
    let mut remaining = SIDECAR_LOG_READ_BYTES;
    let mut newest_first = Vec::new();
    for path in paths.iter().rev() {
        if remaining == 0 {
            break;
        }
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let length = file.metadata()?.len();
        let take = usize::try_from(length.min(remaining as u64)).unwrap_or(remaining);
        if length > take as u64 {
            file.seek(SeekFrom::Start(length - take as u64))?;
        }
        let mut bytes = Vec::with_capacity(take);
        file.take(take as u64).read_to_end(&mut bytes)?;
        remaining -= bytes.len();
        newest_first.push(bytes);
    }
    newest_first.reverse();
    let bytes = newest_first.concat();
    Ok(SemanticRuntimeLogs {
        text: String::from_utf8_lossy(&bytes).into_owned(),
        truncated: total_bytes > bytes.len() as u64,
    })
}

fn rotate_sidecar_log_files(directory: &Path, archive_count: usize) -> std::io::Result<()> {
    let active_path = sidecar_log_path(directory, None);
    if archive_count == 0 {
        return remove_file_if_present(&active_path);
    }
    for archive in (1..=archive_count).rev() {
        let source = if archive == 1 {
            active_path.clone()
        } else {
            sidecar_log_path(directory, Some(archive - 1))
        };
        let destination = sidecar_log_path(directory, Some(archive));
        remove_file_if_present(&destination)?;
        if source.exists() {
            fs::rename(source, destination)?;
        }
    }
    Ok(())
}

fn remove_excess_sidecar_archives(directory: &Path, archive_count: usize) -> std::io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(index) = name
            .strip_prefix("llama-server.")
            .and_then(|name| name.strip_suffix(".log"))
            .and_then(|index| index.parse::<usize>().ok())
        else {
            continue;
        };
        if index == 0 || index > archive_count {
            remove_file_if_present(&path)?;
        }
    }
    Ok(())
}

fn trim_log_to_tail(path: &Path, max_file_bytes: u64) -> std::io::Result<()> {
    let length = match path.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if length <= max_file_bytes {
        return Ok(());
    }
    let mut source = File::open(path)?;
    source.seek(SeekFrom::Start(length - max_file_bytes))?;
    let capacity = usize::try_from(max_file_bytes)
        .map_err(|_| std::io::Error::other("sidecar log limit exceeds addressable memory"))?;
    let mut tail = Vec::with_capacity(capacity);
    source.read_to_end(&mut tail)?;
    drop(source);
    let mut destination = OpenOptions::new().write(true).truncate(true).open(path)?;
    destination.write_all(&tail)?;
    destination.flush()
}

fn remove_file_if_present(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn terminate_process_group(child: &mut Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    let process_group = -(child.id() as i32);
    // SAFETY: kill receives a process-group ID created by process_group(0).
    unsafe {
        libc::kill(process_group, libc::SIGTERM);
    }
    for _ in 0..20 {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    // SAFETY: the child still belongs to the process group created above.
    unsafe {
        libc::kill(process_group, libc::SIGKILL);
    }
    let _ = child.wait();
}

#[cfg(not(unix))]
fn terminate_process_group(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn sweep_stale_sidecar(data_root: &Path, sidecar_path: &Path, model_path: &Path) {
    let Ok(contents) = fs::read_to_string(pidfile_path(data_root)) else {
        return;
    };
    let mut lines = contents.lines();
    let Some(pid) = lines.next().and_then(|value| value.parse::<u32>().ok()) else {
        return;
    };
    let recorded_sidecar = lines.next().unwrap_or_default();
    let recorded_model = lines.next().unwrap_or_default();
    if recorded_sidecar != sidecar_path.to_string_lossy()
        || recorded_model != model_path.to_string_lossy()
    {
        return;
    }
    if !process_matches_sidecar(pid, recorded_sidecar, recorded_model) {
        return;
    }
    terminate_stale_process_group(pid, recorded_sidecar, recorded_model);
    remove_pidfile_if_owned(data_root, pid);
}

fn process_matches_sidecar(pid: u32, sidecar_path: &str, model_path: &str) -> bool {
    let Ok(output) = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
    else {
        return false;
    };
    let command = String::from_utf8_lossy(&output.stdout);
    output.status.success()
        && command.contains(sidecar_path)
        && command.contains(model_path)
        && command.contains("--embedding")
}

#[cfg(unix)]
fn terminate_stale_process_group(pid: u32, sidecar_path: &str, model_path: &str) {
    let Ok(pid) = i32::try_from(pid) else {
        return;
    };
    // SAFETY: the caller verified that this process group is Kosh's sidecar.
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    for _ in 0..20 {
        if !process_exists(pid) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    if !process_matches_sidecar(pid as u32, sidecar_path, model_path) {
        return;
    }
    // SAFETY: the verified sidecar did not exit after SIGTERM.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    for _ in 0..20 {
        if !process_exists(pid) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(unix)]
fn process_exists(pid: i32) -> bool {
    // SAFETY: signal 0 checks for a process without sending a signal.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn terminate_stale_process_group(_pid: u32, _sidecar_path: &str, _model_path: &str) {}

fn write_pidfile(
    data_root: &Path,
    pid: u32,
    sidecar_path: &Path,
    model_path: &Path,
) -> Result<(), SemanticRuntimeError> {
    let path = pidfile_path(data_root);
    let temporary = path.with_extension("tmp");
    fs::write(
        &temporary,
        format!(
            "{pid}\n{}\n{}\n",
            sidecar_path.display(),
            model_path.display()
        ),
    )?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn pidfile_path(data_root: &Path) -> PathBuf {
    data_root.join("llama-server.pid")
}

fn remove_pidfile_if_owned(data_root: &Path, expected_pid: u32) {
    let path = pidfile_path(data_root);
    let Ok(contents) = fs::read_to_string(&path) else {
        return;
    };
    if contents
        .lines()
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        == Some(expected_pid)
    {
        let _ = fs::remove_file(path);
    }
}

fn resolve_sidecar_artifact(
    resource_dir: Option<&Path>,
) -> Result<Option<ResolvedSidecar>, SemanticRuntimeError> {
    let Some(resource_dir) = resource_dir else {
        return Ok(None);
    };

    #[cfg(debug_assertions)]
    if let Some(path) = development_path_override(SIDECAR_OVERRIDE_ENV) {
        return if path.is_file() {
            Ok(Some(ResolvedSidecar {
                path,
                expectation: None,
            }))
        } else {
            Err(SemanticRuntimeError::RuntimeUnavailable(format!(
                "{SIDECAR_OVERRIDE_ENV} does not point to a file"
            )))
        };
    }

    let bundled = resource_dir.join(BUNDLED_SIDECAR_PATH);
    if bundled.is_file() {
        let expectation = read_bundled_sidecar_expectation(resource_dir)?;
        return Ok(Some(ResolvedSidecar {
            path: bundled,
            expectation: Some(expectation),
        }));
    }

    #[cfg(debug_assertions)]
    {
        let mut candidates = vec![resource_dir.join("llama-server")];
        if let Some(sibling) = env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join("llama-server")))
        {
            candidates.push(sibling);
        }
        Ok(candidates
            .into_iter()
            .find(|candidate| candidate.is_file())
            .map(|path| ResolvedSidecar {
                path,
                expectation: None,
            })
            .or_else(|| {
                Command::new("which")
                    .arg("llama-server")
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .and_then(|output| {
                        let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                        let path = PathBuf::from(value);
                        path.is_file().then_some(ResolvedSidecar {
                            path,
                            expectation: None,
                        })
                    })
            }))
    }

    #[cfg(not(debug_assertions))]
    {
        Ok(None)
    }
}

fn read_bundled_sidecar_expectation(
    resource_dir: &Path,
) -> Result<BundledSidecarExpectation, SemanticRuntimeError> {
    let manifest_path = resource_dir.join(BUNDLED_RELEASE_MANIFEST_PATH);
    let manifest: BundledReleaseManifest =
        serde_json::from_slice(&fs::read(&manifest_path).map_err(|error| {
            SemanticRuntimeError::InvalidArtifact(format!(
                "cannot read bundled llama-server release manifest: {error}"
            ))
        })?)
        .map_err(|error| {
            SemanticRuntimeError::InvalidArtifact(format!(
                "cannot parse bundled llama-server release manifest: {error}"
            ))
        })?;
    let binary = manifest.binary;
    if binary.bundle_path != BUNDLED_SIDECAR_PATH {
        return Err(SemanticRuntimeError::InvalidArtifact(format!(
            "bundled llama-server release manifest names unexpected path {}",
            binary.bundle_path
        )));
    }
    if binary.size == 0 {
        return Err(SemanticRuntimeError::InvalidArtifact(
            "bundled llama-server release manifest declares an empty binary".into(),
        ));
    }
    if binary.sha256.len() != 64
        || !binary
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(SemanticRuntimeError::InvalidArtifact(
            "bundled llama-server release manifest has an invalid SHA-256".into(),
        ));
    }
    Ok(BundledSidecarExpectation {
        sha256: binary.sha256,
        size: binary.size,
    })
}

#[cfg(debug_assertions)]
fn development_path_override(name: &str) -> Option<PathBuf> {
    env::var_os(name).map(PathBuf::from)
}

#[cfg(not(debug_assertions))]
fn development_path_override(_name: &str) -> Option<PathBuf> {
    None
}

#[cfg(debug_assertions)]
fn development_string_override(name: &str) -> Option<String> {
    env::var(name).ok()
}

#[cfg(not(debug_assertions))]
fn development_string_override(_name: &str) -> Option<String> {
    None
}

#[cfg(debug_assertions)]
fn missing_sidecar_error() -> SemanticRuntimeError {
    SemanticRuntimeError::RuntimeUnavailable(format!(
        "llama-server was not found; set {SIDECAR_OVERRIDE_ENV} during development or stage the release resources"
    ))
}

#[cfg(not(debug_assertions))]
fn missing_sidecar_error() -> SemanticRuntimeError {
    SemanticRuntimeError::RuntimeUnavailable(
        "the bundled llama-server resource is missing from Kosh.app".into(),
    )
}

fn set_failure(inner: &EmbeddingRuntimeInner, error: &SemanticRuntimeError) {
    let _failure_recording = lock(&inner.failure_recording);
    match failure_disposition(error, inner.shutdown.load(Ordering::Acquire)) {
        FailureDisposition::Ignore => return,
        FailureDisposition::Record => {}
        FailureDisposition::RecordAndInvalidateVerification => {
            invalidate_verification_receipt(&inner.data_root);
        }
    }
    update_status(inner, |status| {
        status.phase = match error {
            SemanticRuntimeError::RuntimeUnavailable(_) => SemanticRuntimePhase::Unavailable,
            _ => SemanticRuntimePhase::Failed,
        };
        status.runtime_running = false;
        if matches!(error, SemanticRuntimeError::InvalidArtifact(_)) {
            status.verified = false;
        }
        status.message = Some(redacted_runtime_error(error));
    });
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailureDisposition {
    Ignore,
    Record,
    RecordAndInvalidateVerification,
}

fn failure_disposition(error: &SemanticRuntimeError, shutting_down: bool) -> FailureDisposition {
    if shutting_down {
        FailureDisposition::Ignore
    } else if matches!(error, SemanticRuntimeError::InvalidArtifact(_)) {
        // Runtime failures can be transient. Model, sidecar, and runtime-setting changes are
        // already detected by the receipt fingerprint on the next launch.
        FailureDisposition::RecordAndInvalidateVerification
    } else {
        FailureDisposition::Record
    }
}

fn ensure_lifecycle_lock(inner: &EmbeddingRuntimeInner) -> Result<(), SemanticRuntimeError> {
    let mut lifecycle_lock = lock(&inner.lifecycle_lock);
    if lifecycle_lock.is_none() {
        *lifecycle_lock = Some(acquire_lifecycle_lock(
            &inner.data_root,
            LIFECYCLE_LOCK_WAIT,
        )?);
    }
    Ok(())
}

fn acquire_lifecycle_lock(data_root: &Path, wait: Duration) -> Result<File, SemanticRuntimeError> {
    fs::create_dir_all(data_root)?;
    let path = data_root.join(LIFECYCLE_LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                thread::sleep(LIFECYCLE_LOCK_POLL_DELAY);
            }
            Err(TryLockError::WouldBlock) => {
                return Err(SemanticRuntimeError::RuntimeUnavailable(format!(
                    "another Kosh instance is still stopping semantic search; timed out waiting for {}",
                    path.display()
                )));
            }
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

fn release_lifecycle_lock(inner: &EmbeddingRuntimeInner) {
    if let Some(file) = lock(&inner.lifecycle_lock).take() {
        if let Err(error) = file.unlock() {
            log::warn!("could not release semantic-search lifecycle lock: {error}");
        }
    }
}

fn update_status(inner: &EmbeddingRuntimeInner, update: impl FnOnce(&mut SemanticRuntimeStatus)) {
    update(&mut lock(&inner.status));
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn interruptible_sleep(shutdown: &AtomicBool, duration: Duration) {
    let deadline = Instant::now() + duration;
    while !shutdown.load(Ordering::Relaxed) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
}

fn sync_directory(path: &Path) -> Result<(), SemanticRuntimeError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read as _, Write as _},
        net::TcpStream,
        sync::mpsc,
    };

    use super::*;

    fn test_runtime_settings() -> LlamaRuntimeSettings {
        LlamaRuntimeSettings {
            device: Some("test-device".into()),
            gpu_layers: "all".into(),
            pooling: "last".into(),
            embedding_normalization: LLAMA_EMBEDDING_NORMALIZATION.into(),
            parallel_slots: LLAMA_PARALLEL_SLOTS.into(),
        }
    }

    fn test_receipt(
        settings: &LlamaRuntimeSettings,
        model_path: &Path,
        sidecar_path: &Path,
    ) -> Result<VerificationReceipt, SemanticRuntimeError> {
        build_verification_receipt_for_inputs(
            settings,
            "manifest",
            "golden",
            "sidecar-pin",
            model_path,
            sidecar_path,
        )
    }

    fn test_contract(model_bytes: &[u8], download_url: String) -> RuntimeContract {
        let model_sha256 = hex(&Sha256::digest(model_bytes));
        let manifest = TextEmbeddingManifest {
            manifest_version: 1,
            id: "test-index".into(),
            created_at: 0,
            index_key: "test".into(),
            model_name: "test/model".into(),
            model_revision: "test-revision".into(),
            model_file_sha256: model_sha256.clone(),
            dimension: 3,
            distance_metric: "COSINE".into(),
            normalized: true,
            index_schema_version: 1,
            config: embedding::TextEmbeddingConfig {
                schema_version: 1,
                model_file: "test-model.gguf".into(),
                model_file_size: model_bytes.len() as u64,
                quantization: "TEST".into(),
                pooling: "last".into(),
                normalization: "L2".into(),
                query_prefix: "Query: ".into(),
                document_prefix: "Document: ".into(),
                document_construction_version: 1,
            },
        };
        let manifest_json = serde_json::to_string(&manifest).expect("test manifest JSON");
        let golden_json = serde_json::json!({
            "fixtureVersion": 1,
            "modelFileSha256": model_sha256,
            "generatedWith": {
                "runtime": "test-sidecar",
                "revision": "test",
                "build": 1,
                "device": "CPU"
            },
            "tolerance": {
                "minimumCosineSimilarity": 0.9999,
                "maximumAbsoluteDifference": 0.0001
            },
            "cases": [
                {
                    "name": "query",
                    "input": "Query: fixture query",
                    "embedding": [1.0, 0.0, 0.0]
                },
                {
                    "name": "document",
                    "input": "Document: fixture document",
                    "embedding": [0.0, 1.0, 0.0]
                }
            ]
        })
        .to_string();
        RuntimeContract {
            manifest,
            manifest_json,
            golden_json,
            download_url,
        }
    }

    fn test_configuration(
        data_root: &Path,
        sidecar_path: Option<PathBuf>,
        model_override: Option<PathBuf>,
        contract: RuntimeContract,
        idle_timeout: Duration,
    ) -> RuntimeConfiguration {
        RuntimeConfiguration {
            contract,
            data_root: data_root.to_owned(),
            model_override,
            sidecar_path,
            sidecar_expectation: None,
            sidecar_resolution_error: None,
            runtime_settings: LlamaRuntimeSettings {
                device: None,
                gpu_layers: "0".into(),
                pooling: "last".into(),
                embedding_normalization: LLAMA_EMBEDDING_NORMALIZATION.into(),
                parallel_slots: LLAMA_PARALLEL_SLOTS.into(),
            },
            idle_timeout,
            startup_timeout: Duration::from_secs(2),
            sidecar_environment: BTreeMap::new(),
        }
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        let header_end = loop {
            let read = stream.read(&mut buffer).expect("request bytes");
            assert!(read > 0, "request ended before headers");
            bytes.extend_from_slice(&buffer[..read]);
            if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().expect("content length"))
                })
            })
            .unwrap_or(0);
        while bytes.len() < header_end + content_length {
            let read = stream.read(&mut buffer).expect("request body");
            assert!(read > 0, "request ended before body");
            bytes.extend_from_slice(&buffer[..read]);
        }
        String::from_utf8_lossy(&bytes[header_end..header_end + content_length]).into_owned()
    }

    fn start_embedding_server(
        incompatible: bool,
        request_count: usize,
    ) -> (String, mpsc::Receiver<Vec<String>>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("embedding fixture listener");
        let address = listener.local_addr().expect("embedding fixture address");
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().expect("embedding fixture request");
                let body = read_request(&mut stream);
                let vector = if incompatible {
                    [0.0, 0.0, 1.0]
                } else if body.contains("Query:") {
                    [1.0, 0.0, 0.0]
                } else {
                    [0.0, 1.0, 0.0]
                };
                requests.push(body);
                let response_body =
                    serde_json::json!({"data": [{"embedding": vector}]}).to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                )
                .expect("embedding fixture response");
            }
            sender.send(requests).expect("embedding fixture transcript");
        });
        (format!("http://{address}"), receiver, worker)
    }

    fn start_download_server(
        expected_range: Option<&'static str>,
        response_bytes: &'static [u8],
    ) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("download fixture listener");
        let address = listener.local_addr().expect("download fixture address");
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("download fixture request");
            let mut request = [0_u8; 4096];
            let read = stream.read(&mut request).expect("download fixture headers");
            let request = String::from_utf8_lossy(&request[..read]);
            if let Some(expected_range) = expected_range {
                assert!(
                    request
                        .lines()
                        .any(|line| line.eq_ignore_ascii_case(expected_range)),
                    "{request}"
                );
            }
            let status = if expected_range.is_some() {
                "206 Partial Content"
            } else {
                "200 OK"
            };
            let content_range = expected_range
                .map(|_| "Content-Range: bytes 4-9/10\r\n")
                .unwrap_or_default();
            write!(
                stream,
                "HTTP/1.1 {status}\r\n{content_range}Content-Length: {}\r\nConnection: close\r\n\r\n",
                response_bytes.len()
            )
            .expect("download fixture response headers");
            stream
                .write_all(response_bytes)
                .expect("download fixture response body");
        });
        (format!("http://{address}/model.gguf"), worker)
    }

    fn start_stalled_download_server(delay: Duration) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("stalled fixture listener");
        let address = listener.local_addr().expect("stalled fixture address");
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("stalled fixture request");
            let mut request = [0_u8; 4096];
            let _read = stream.read(&mut request).expect("stalled fixture headers");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\n")
                .expect("stalled fixture response headers");
            stream.flush().expect("stalled fixture header flush");
            thread::sleep(delay);
        });
        (format!("http://{address}/model.gguf"), worker)
    }

    #[cfg(unix)]
    fn install_running_fixture(runtime: &EmbeddingRuntime, endpoint: String, model_path: &Path) {
        use std::os::unix::process::CommandExt;

        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]).process_group(0);
        let child = command.spawn().expect("fixture sidecar process");
        let sidecar_path = runtime
            .inner
            .sidecar_path
            .as_deref()
            .expect("fixture sidecar path");
        *lock(&runtime.inner.runtime) = SidecarRuntime {
            child: Some(child),
            endpoint: Some(endpoint),
            model_path: Some(model_path.to_owned()),
            model_fingerprint: Some(
                verification_file_fingerprint(model_path).expect("fixture model fingerprint"),
            ),
            sidecar_fingerprint: Some(
                verification_file_fingerprint(sidecar_path).expect("fixture sidecar fingerprint"),
            ),
            last_used: Some(Instant::now()),
            log_threads: Vec::new(),
        };
    }

    #[test]
    fn missing_and_partial_models_are_observable_without_network_access() {
        let missing = tempfile::tempdir().expect("missing model directory");
        let sidecar = missing.path().join("llama-server");
        fs::write(&sidecar, b"sidecar").expect("sidecar fixture");
        let runtime = EmbeddingRuntime::start(test_configuration(
            missing.path(),
            Some(sidecar.clone()),
            None,
            test_contract(b"0123456789", "http://127.0.0.1:1/model".into()),
            Duration::from_secs(60),
        ));
        assert_eq!(runtime.status().phase, SemanticRuntimePhase::NotDownloaded);
        assert_eq!(runtime.status().downloaded_bytes, 0);
        runtime.shutdown();

        let partial = tempfile::tempdir().expect("partial model directory");
        let model_directory = partial.path().join("models");
        fs::create_dir_all(&model_directory).expect("model directory");
        fs::write(model_directory.join("test-model.gguf.part"), b"0123").expect("partial model");
        let runtime = EmbeddingRuntime::start(test_configuration(
            partial.path(),
            Some(sidecar),
            None,
            test_contract(b"0123456789", "http://127.0.0.1:1/model".into()),
            Duration::from_secs(60),
        ));
        let status = runtime.status();
        assert_eq!(status.phase, SemanticRuntimePhase::NotDownloaded);
        assert_eq!(status.downloaded_bytes, 4);
        assert_eq!(status.model_disk_usage_bytes, 4);
    }

    #[test]
    fn managed_download_resumes_and_atomically_installs_a_verified_model() {
        let directory = tempfile::tempdir().expect("download model directory");
        let sidecar = directory.path().join("llama-server");
        fs::write(&sidecar, b"sidecar").expect("sidecar fixture");
        let model_directory = directory.path().join("models");
        fs::create_dir_all(&model_directory).expect("model directory");
        fs::write(model_directory.join("test-model.gguf.part"), b"0123").expect("partial model");
        let (url, server) = start_download_server(Some("Range: bytes=4-"), b"456789");
        let runtime = EmbeddingRuntime::start(test_configuration(
            directory.path(),
            Some(sidecar),
            None,
            test_contract(b"0123456789", url),
            Duration::from_secs(60),
        ));

        let artifact = prepare_managed_artifact(&runtime.inner).expect("resumed verified download");
        server.join().expect("download fixture server");

        assert_eq!(artifact, model_directory.join("test-model.gguf"));
        assert_eq!(fs::read(&artifact).expect("installed model"), b"0123456789");
        assert!(!model_directory.join("test-model.gguf.part").exists());
    }

    #[test]
    fn blocking_http_client_bounds_each_stalled_response_read() {
        let (url, server) = start_stalled_download_server(Duration::from_millis(250));
        let client = build_http_client(Duration::from_secs(1), Duration::from_millis(50));
        let mut response = client.get(url).send().expect("stalled fixture response");
        let error = response
            .read(&mut [0_u8; 1])
            .expect_err("stalled body read must time out");

        assert!(
            error
                .get_ref()
                .and_then(|source| source.downcast_ref::<reqwest::Error>())
                .is_some_and(reqwest::Error::is_timeout),
            "stalled body read returned a non-timeout error: {error:?}"
        );
        server.join().expect("stalled fixture server");
    }

    #[test]
    fn corrupt_managed_models_are_replaced_only_after_a_verified_download() {
        let directory = tempfile::tempdir().expect("corrupt model directory");
        let sidecar = directory.path().join("llama-server");
        fs::write(&sidecar, b"sidecar").expect("sidecar fixture");
        let model_directory = directory.path().join("models");
        fs::create_dir_all(&model_directory).expect("model directory");
        let model_path = model_directory.join("test-model.gguf");
        fs::write(&model_path, b"abcdefghij").expect("corrupt model");
        let (url, server) = start_download_server(None, b"0123456789");
        let runtime = EmbeddingRuntime::start(test_configuration(
            directory.path(),
            Some(sidecar),
            None,
            test_contract(b"0123456789", url),
            Duration::from_secs(60),
        ));

        assert!(matches!(
            verify_artifact(&model_path, &runtime.inner.contract.manifest),
            Err(SemanticRuntimeError::InvalidArtifact(_))
        ));
        prepare_managed_artifact(&runtime.inner).expect("replacement download");
        server.join().expect("download fixture server");
        assert_eq!(fs::read(model_path).expect("repaired model"), b"0123456789");
    }

    #[cfg(unix)]
    #[test]
    fn verified_runtime_uses_manifest_prefixes_and_stops_when_idle() {
        let directory = tempfile::tempdir().expect("verified runtime directory");
        let model_path = directory.path().join("model.gguf");
        let sidecar_path = directory.path().join("llama-server");
        fs::write(&model_path, b"0123456789").expect("model fixture");
        fs::write(&sidecar_path, b"sidecar").expect("sidecar fixture");
        let (endpoint, transcript, server) = start_embedding_server(false, 4);
        let runtime = EmbeddingRuntime::start(test_configuration(
            directory.path(),
            Some(sidecar_path),
            Some(model_path.clone()),
            test_contract(b"0123456789", "http://127.0.0.1:1/model".into()),
            Duration::from_millis(250),
        ));
        install_running_fixture(&runtime, endpoint, &model_path);

        let prepared = runtime.prepare().expect("golden-compatible runtime");
        assert_eq!(prepared.phase, SemanticRuntimePhase::Ready);
        assert!(prepared.verified);
        assert!(prepared.runtime_running);
        assert_eq!(
            runtime.embed_query("real query").expect("query vector"),
            vec![1.0, 0.0, 0.0]
        );
        assert_eq!(
            runtime
                .embed_document("real document")
                .expect("document vector"),
            vec![0.0, 1.0, 0.0]
        );
        server.join().expect("embedding fixture server");
        let requests = transcript.recv().expect("embedding transcript");
        assert!(requests[0].contains("Query: fixture query"));
        assert!(requests[1].contains("Document: fixture document"));
        assert!(requests[2].contains("Query: real query"));
        assert!(requests[3].contains("Document: real document"));

        let deadline = Instant::now() + Duration::from_secs(2);
        while runtime.status().runtime_running && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(25));
        }
        let idle = runtime.status();
        assert_eq!(idle.phase, SemanticRuntimePhase::Ready);
        assert!(idle.verified);
        assert!(!idle.runtime_running);
    }

    #[cfg(unix)]
    #[test]
    fn cached_preparation_preserves_the_runtime_owned_live_sidecar() {
        use std::os::unix::{fs::PermissionsExt, process::CommandExt};

        let directory = tempfile::tempdir().expect("cached runtime directory");
        let model_path = directory.path().join("model.gguf");
        let sidecar_path = directory.path().join("llama-server");
        fs::write(&model_path, b"0123456789").expect("model fixture");
        fs::write(&sidecar_path, b"#!/bin/sh\nsleep 30 &\nwait\n").expect("sidecar fixture");
        let mut permissions = sidecar_path
            .metadata()
            .expect("sidecar metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&sidecar_path, permissions).expect("executable sidecar");
        let runtime = EmbeddingRuntime::start(test_configuration(
            directory.path(),
            Some(sidecar_path.clone()),
            Some(model_path.clone()),
            test_contract(b"0123456789", "http://127.0.0.1:1/model".into()),
            Duration::from_secs(60),
        ));
        let receipt = build_verification_receipt(&runtime.inner, &model_path, &sidecar_path)
            .expect("verification receipt");
        write_verification_receipt(directory.path(), &receipt).expect("receipt write");

        let mut command = Command::new(&sidecar_path);
        command.arg("--model").arg(&model_path).process_group(0);
        let child = command.spawn().expect("owned sidecar process");
        let pid = child.id();
        write_pidfile(directory.path(), pid, &sidecar_path, &model_path).expect("sidecar PID file");
        *lock(&runtime.inner.runtime) = SidecarRuntime {
            child: Some(child),
            endpoint: Some("http://127.0.0.1:1".into()),
            model_path: Some(model_path.clone()),
            model_fingerprint: Some(receipt.model),
            sidecar_fingerprint: Some(receipt.sidecar),
            last_used: Some(Instant::now()),
            log_threads: Vec::new(),
        };

        let prepared = runtime.prepare().expect("cached preparation");

        assert!(prepared.runtime_running);
        assert_eq!(
            lock(&runtime.inner.runtime).child.as_ref().map(Child::id),
            Some(pid)
        );
        assert!(process_exists(pid as i32));
    }

    #[cfg(unix)]
    #[test]
    fn startup_sweeps_a_path_validated_orphan_without_a_receipt() {
        use std::os::unix::{fs::PermissionsExt, process::CommandExt};

        let directory = tempfile::tempdir().expect("orphaned runtime directory");
        let model_path = directory.path().join("model.gguf");
        let sidecar_path = directory.path().join("llama-server");
        fs::write(&model_path, b"0123456789").expect("model fixture");
        fs::write(&sidecar_path, b"#!/bin/sh\nsleep 30 &\nwait\n").expect("sidecar fixture");
        let mut permissions = sidecar_path
            .metadata()
            .expect("sidecar metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&sidecar_path, permissions).expect("executable sidecar");
        let mut command = Command::new(&sidecar_path);
        command
            .arg("--model")
            .arg(&model_path)
            .arg("--embedding")
            .process_group(0);
        let mut orphan = command.spawn().expect("orphan sidecar process");
        let pid = orphan.id();
        write_pidfile(directory.path(), pid, &sidecar_path, &model_path).expect("sidecar PID file");

        let runtime = EmbeddingRuntime::start(test_configuration(
            directory.path(),
            Some(sidecar_path),
            Some(model_path),
            test_contract(b"0123456789", "http://127.0.0.1:1/model".into()),
            Duration::from_secs(60),
        ));
        let exited = orphan.try_wait().expect("orphan process status");
        if exited.is_none() {
            terminate_process_group(&mut orphan);
        }

        assert!(exited.is_some());
        assert!(!pidfile_path(directory.path()).exists());
        assert_eq!(
            runtime.status().phase,
            SemanticRuntimePhase::VerificationRequired
        );
    }

    #[cfg(unix)]
    #[test]
    fn idle_restart_rejects_artifacts_changed_after_verification() {
        let directory = tempfile::tempdir().expect("mutated runtime directory");
        let model_path = directory.path().join("model.gguf");
        let sidecar_path = directory.path().join("llama-server");
        fs::write(&model_path, b"0123456789").expect("model fixture");
        fs::write(&sidecar_path, b"sidecar").expect("sidecar fixture");
        let (endpoint, transcript, server) = start_embedding_server(false, 2);
        let runtime = EmbeddingRuntime::start(test_configuration(
            directory.path(),
            Some(sidecar_path),
            Some(model_path.clone()),
            test_contract(b"0123456789", "http://127.0.0.1:1/model".into()),
            Duration::from_millis(250),
        ));
        install_running_fixture(&runtime, endpoint, &model_path);

        runtime.prepare().expect("golden-compatible runtime");
        server.join().expect("embedding fixture server");
        assert_eq!(transcript.recv().expect("embedding transcript").len(), 2);

        let deadline = Instant::now() + Duration::from_secs(2);
        while runtime.status().runtime_running && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(25));
        }
        assert!(!runtime.status().runtime_running);

        fs::write(&model_path, b"abcdefghij").expect("mutated model");
        assert!(matches!(
            runtime.embed_query("must not start"),
            Err(SemanticRuntimeError::InvalidArtifact(_))
        ));
        let status = runtime.status();
        assert_eq!(status.phase, SemanticRuntimePhase::Failed);
        assert!(!status.verified);
        assert!(!status.runtime_running);
        assert!(read_verification_receipt(directory.path())
            .expect("verification receipt state")
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn live_runtime_rejects_artifacts_changed_after_verification() {
        let directory = tempfile::tempdir().expect("live mutated runtime directory");
        let model_path = directory.path().join("model.gguf");
        let sidecar_path = directory.path().join("llama-server");
        fs::write(&model_path, b"0123456789").expect("model fixture");
        fs::write(&sidecar_path, b"sidecar").expect("sidecar fixture");
        let (endpoint, transcript, server) = start_embedding_server(false, 2);
        let runtime = EmbeddingRuntime::start(test_configuration(
            directory.path(),
            Some(sidecar_path),
            Some(model_path.clone()),
            test_contract(b"0123456789", "http://127.0.0.1:1/model".into()),
            Duration::from_secs(60),
        ));
        install_running_fixture(&runtime, endpoint, &model_path);

        runtime.prepare().expect("golden-compatible runtime");
        server.join().expect("embedding fixture server");
        assert_eq!(transcript.recv().expect("embedding transcript").len(), 2);
        assert!(runtime.status().runtime_running);

        fs::write(&model_path, b"abcdefghij").expect("mutated model");
        assert!(matches!(
            runtime.embed_document("must not reuse the sidecar"),
            Err(SemanticRuntimeError::InvalidArtifact(_))
        ));
        let status = runtime.status();
        assert_eq!(status.phase, SemanticRuntimePhase::Failed);
        assert!(!status.verified);
        assert!(!status.runtime_running);
        assert!(read_verification_receipt(directory.path())
            .expect("verification receipt state")
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn fresh_verification_restarts_a_sidecar_whose_artifact_changed() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("replaced sidecar runtime directory");
        let model_path = directory.path().join("model.gguf");
        let sidecar_path = directory.path().join("llama-server");
        fs::write(&model_path, b"0123456789").expect("model fixture");
        fs::write(&sidecar_path, b"original sidecar").expect("sidecar fixture");
        let (endpoint, transcript, server) = start_embedding_server(false, 2);
        let runtime = EmbeddingRuntime::start(test_configuration(
            directory.path(),
            Some(sidecar_path.clone()),
            Some(model_path.clone()),
            test_contract(b"0123456789", "http://127.0.0.1:1/model".into()),
            Duration::from_secs(60),
        ));
        install_running_fixture(&runtime, endpoint, &model_path);

        runtime.prepare().expect("initial golden verification");
        server.join().expect("embedding fixture server");
        assert_eq!(transcript.recv().expect("embedding transcript").len(), 2);
        fs::write(
            &sidecar_path,
            b"#!/bin/sh\necho replacement-sidecar-ran >&2\nexit 9\n",
        )
        .expect("replacement sidecar");
        let mut permissions = sidecar_path
            .metadata()
            .expect("replacement metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&sidecar_path, permissions).expect("executable replacement");

        assert!(matches!(
            runtime.retry(),
            Err(SemanticRuntimeError::Runtime(_))
        ));
        assert!(runtime
            .logs()
            .expect("replacement logs")
            .text
            .contains("replacement-sidecar-ran"));
        let status = runtime.status();
        assert_eq!(status.phase, SemanticRuntimePhase::Failed);
        assert!(!status.verified);
        assert!(!status.runtime_running);
    }

    #[cfg(unix)]
    #[test]
    fn preparation_fails_when_the_verification_receipt_cannot_persist() {
        let directory = tempfile::tempdir().expect("unwritable receipt runtime directory");
        let model_path = directory.path().join("model.gguf");
        let sidecar_path = directory.path().join("llama-server");
        fs::write(&model_path, b"0123456789").expect("model fixture");
        fs::write(&sidecar_path, b"sidecar").expect("sidecar fixture");
        fs::create_dir(verification_receipt_path(directory.path()).with_extension("json.tmp"))
            .expect("blocked receipt temporary path");
        let (endpoint, transcript, server) = start_embedding_server(false, 2);
        let runtime = EmbeddingRuntime::start(test_configuration(
            directory.path(),
            Some(sidecar_path),
            Some(model_path.clone()),
            test_contract(b"0123456789", "http://127.0.0.1:1/model".into()),
            Duration::from_secs(60),
        ));
        install_running_fixture(&runtime, endpoint, &model_path);

        assert!(matches!(
            runtime.prepare(),
            Err(SemanticRuntimeError::Io(_))
        ));
        server.join().expect("embedding fixture server");
        assert_eq!(transcript.recv().expect("embedding transcript").len(), 2);
        let status = runtime.status();
        assert_eq!(status.phase, SemanticRuntimePhase::Failed);
        assert!(!status.verified);
        assert!(!status.runtime_running);
        assert!(read_verification_receipt(directory.path())
            .expect("verification receipt state")
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn incompatible_golden_vectors_fail_closed_to_lexical_only() {
        let directory = tempfile::tempdir().expect("incompatible runtime directory");
        let model_path = directory.path().join("model.gguf");
        let sidecar_path = directory.path().join("llama-server");
        fs::write(&model_path, b"0123456789").expect("model fixture");
        fs::write(&sidecar_path, b"sidecar").expect("sidecar fixture");
        let (endpoint, transcript, server) = start_embedding_server(true, 1);
        let runtime = EmbeddingRuntime::start(test_configuration(
            directory.path(),
            Some(sidecar_path),
            Some(model_path.clone()),
            test_contract(b"0123456789", "http://127.0.0.1:1/model".into()),
            Duration::from_secs(60),
        ));
        install_running_fixture(&runtime, endpoint, &model_path);

        assert!(matches!(
            runtime.prepare(),
            Err(SemanticRuntimeError::InvalidArtifact(_))
        ));
        server.join().expect("embedding fixture server");
        assert_eq!(transcript.recv().expect("embedding transcript").len(), 1);
        let status = runtime.status();
        assert_eq!(status.phase, SemanticRuntimePhase::Failed);
        assert!(!status.runtime_running);
        assert!(runtime.embed_query("fallback").is_err());

        let (endpoint, transcript, server) = start_embedding_server(false, 2);
        install_running_fixture(&runtime, endpoint, &model_path);
        let retried = runtime.retry().expect("compatible retry");
        server.join().expect("retry fixture server");
        assert_eq!(transcript.recv().expect("retry transcript").len(), 2);
        assert_eq!(retried.phase, SemanticRuntimePhase::Ready);
        assert!(retried.verified);
    }

    #[cfg(unix)]
    #[test]
    fn failed_golden_retry_invalidates_verification_across_restart() {
        let directory = tempfile::tempdir().expect("retry invalidation runtime directory");
        let model_path = directory.path().join("model.gguf");
        let sidecar_path = directory.path().join("llama-server");
        fs::write(&model_path, b"0123456789").expect("model fixture");
        fs::write(&sidecar_path, b"sidecar").expect("sidecar fixture");
        let contract = || test_contract(b"0123456789", "http://127.0.0.1:1/model".into());
        let (endpoint, transcript, server) = start_embedding_server(false, 2);
        let runtime = EmbeddingRuntime::start(test_configuration(
            directory.path(),
            Some(sidecar_path.clone()),
            Some(model_path.clone()),
            contract(),
            Duration::from_secs(60),
        ));
        install_running_fixture(&runtime, endpoint, &model_path);

        runtime.prepare().expect("initial golden verification");
        server.join().expect("initial embedding fixture server");
        assert_eq!(
            transcript
                .recv()
                .expect("initial embedding transcript")
                .len(),
            2
        );
        assert!(read_verification_receipt(directory.path())
            .expect("initial verification receipt")
            .is_some());

        stop_sidecar(&runtime.inner);
        let (endpoint, transcript, server) = start_embedding_server(true, 1);
        install_running_fixture(&runtime, endpoint, &model_path);
        assert!(matches!(
            runtime.retry(),
            Err(SemanticRuntimeError::InvalidArtifact(_))
        ));
        server
            .join()
            .expect("incompatible embedding fixture server");
        assert_eq!(
            transcript
                .recv()
                .expect("incompatible embedding transcript")
                .len(),
            1
        );
        assert!(read_verification_receipt(directory.path())
            .expect("invalidated verification receipt")
            .is_none());

        runtime.shutdown();
        drop(runtime);
        let restarted = EmbeddingRuntime::start(test_configuration(
            directory.path(),
            Some(sidecar_path),
            Some(model_path),
            contract(),
            Duration::from_secs(60),
        ));
        let status = restarted.status();
        assert_eq!(status.phase, SemanticRuntimePhase::VerificationRequired);
        assert!(!status.verified);
        assert!(!status.runtime_running);
    }

    #[cfg(unix)]
    #[test]
    fn failed_sidecar_start_is_reported_with_bounded_logs() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("failed runtime directory");
        let model_path = directory.path().join("model.gguf");
        let sidecar_path = directory.path().join("llama-server");
        fs::write(&model_path, b"0123456789").expect("model fixture");
        fs::write(
            &sidecar_path,
            b"#!/bin/sh\necho fixture-sidecar-failed >&2\nexit 7\n",
        )
        .expect("failed sidecar fixture");
        let mut permissions = sidecar_path
            .metadata()
            .expect("sidecar metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&sidecar_path, permissions).expect("executable sidecar");
        let runtime = EmbeddingRuntime::start(test_configuration(
            directory.path(),
            Some(sidecar_path),
            Some(model_path),
            test_contract(b"0123456789", "http://127.0.0.1:1/model".into()),
            Duration::from_secs(60),
        ));

        assert!(matches!(
            runtime.prepare(),
            Err(SemanticRuntimeError::Runtime(_))
        ));
        let status = runtime.status();
        assert_eq!(status.phase, SemanticRuntimePhase::Failed);
        assert!(!status.runtime_running);
        let logs = runtime.logs().expect("sidecar logs");
        assert!(logs.text.contains("fixture-sidecar-failed"));
        assert!(!logs.truncated);
    }

    #[test]
    fn repair_removes_only_managed_derived_artifacts_before_retrying() {
        let directory = tempfile::tempdir().expect("repair runtime directory");
        let sidecar_path = directory.path().join("llama-server");
        fs::write(&sidecar_path, b"sidecar").expect("sidecar fixture");
        let model_directory = directory.path().join("models");
        fs::create_dir_all(&model_directory).expect("model directory");
        let model_path = model_directory.join("test-model.gguf");
        let partial_path = model_directory.join("test-model.gguf.part");
        fs::write(&model_path, b"abcdefghij").expect("corrupt managed model");
        fs::write(&partial_path, b"partial").expect("partial managed model");
        let runtime = EmbeddingRuntime::start(test_configuration(
            directory.path(),
            Some(sidecar_path),
            None,
            test_contract(b"0123456789", "http://127.0.0.1:1/model".into()),
            Duration::from_secs(60),
        ));

        assert!(runtime.repair().is_err());
        assert!(!model_path.exists());
        assert!(!partial_path.exists());
        assert_eq!(runtime.status().phase, SemanticRuntimePhase::Failed);

        let override_directory = tempfile::tempdir().expect("override runtime directory");
        let override_model = override_directory.path().join("model.gguf");
        let override_sidecar = override_directory.path().join("llama-server");
        fs::write(&override_model, b"0123456789").expect("override model");
        fs::write(&override_sidecar, b"sidecar").expect("override sidecar");
        let override_runtime = EmbeddingRuntime::start(test_configuration(
            override_directory.path(),
            Some(override_sidecar),
            Some(override_model.clone()),
            test_contract(b"0123456789", "http://127.0.0.1:1/model".into()),
            Duration::from_secs(60),
        ));
        assert!(matches!(
            override_runtime.repair(),
            Err(SemanticRuntimeError::InvalidArtifact(_))
        ));
        assert!(override_model.exists());
    }

    #[test]
    fn bundled_sidecar_is_bound_to_the_staged_release_manifest() {
        let resources = tempfile::tempdir().expect("temporary resources");
        let binary_directory = resources.path().join("bin");
        let release_directory = resources.path().join("release");
        fs::create_dir(&binary_directory).expect("binary resource directory");
        fs::create_dir(&release_directory).expect("release resource directory");
        let sidecar = binary_directory.join("llama-server");
        let sidecar_bytes = b"sidecar";
        fs::write(&sidecar, sidecar_bytes).expect("sidecar fixture");
        let sidecar_sha256 = hex(&Sha256::digest(sidecar_bytes));
        fs::write(
            release_directory.join("llama-server.json"),
            serde_json::json!({
                "binary": {
                    "bundlePath": BUNDLED_SIDECAR_PATH,
                    "sha256": sidecar_sha256,
                    "size": sidecar_bytes.len(),
                    "versionOutputByArchitecture": {
                        "arm64": "fixture",
                        "x86_64": "fixture"
                    }
                },
                "verification": {
                    "modelBundled": false
                }
            })
            .to_string(),
        )
        .expect("release manifest fixture");

        let resolved = resolve_sidecar_artifact(Some(resources.path()))
            .expect("bundled sidecar resolution")
            .expect("bundled sidecar");
        assert_eq!(resolved.path, sidecar);
        assert_eq!(
            resolved.expectation,
            Some(BundledSidecarExpectation {
                sha256: sidecar_sha256,
                size: sidecar_bytes.len() as u64,
            })
        );

        let data_root = tempfile::tempdir().expect("runtime data root");
        let runtime = EmbeddingRuntime::new(data_root.path(), Some(resources.path()));
        verify_bundled_sidecar_artifact(&runtime.inner, &sidecar)
            .expect("matching bundled sidecar");
        fs::write(&sidecar, b"sidocar").expect("tampered sidecar fixture");
        assert!(matches!(
            verify_bundled_sidecar_artifact(&runtime.inner, &sidecar),
            Err(SemanticRuntimeError::InvalidArtifact(message))
                if message.contains("SHA-256")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn bundled_sidecar_hash_mismatch_is_rejected_before_process_spawn() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("runtime directory");
        let model_path = directory.path().join("model.gguf");
        let sidecar_path = directory.path().join("llama-server");
        let marker_path = directory.path().join("sidecar-started");
        fs::write(&model_path, b"0123456789").expect("model fixture");
        fs::write(
            &sidecar_path,
            b"#!/bin/sh\nprintf started > \"$KOSH_MARKER\"\nexit 1\n",
        )
        .expect("sidecar fixture");
        let mut permissions = sidecar_path
            .metadata()
            .expect("sidecar metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&sidecar_path, permissions).expect("executable sidecar");
        let mut configuration = test_configuration(
            directory.path(),
            Some(sidecar_path.clone()),
            Some(model_path),
            test_contract(b"0123456789", "http://127.0.0.1:1/model".into()),
            Duration::from_secs(60),
        );
        configuration.sidecar_expectation = Some(BundledSidecarExpectation {
            sha256: "0".repeat(64),
            size: sidecar_path.metadata().expect("sidecar metadata").len(),
        });
        configuration.sidecar_environment.insert(
            "KOSH_MARKER".into(),
            marker_path.to_string_lossy().into_owned(),
        );
        let runtime = EmbeddingRuntime::start(configuration);

        assert!(matches!(
            runtime.prepare(),
            Err(SemanticRuntimeError::InvalidArtifact(message))
                if message.contains("SHA-256")
        ));
        assert!(!marker_path.exists());
        assert!(!runtime.status().runtime_running);
    }

    #[test]
    fn invalid_bundled_release_manifest_only_disables_semantic_search() {
        let resources = tempfile::tempdir().expect("temporary resources");
        let binary_directory = resources.path().join("bin");
        fs::create_dir(&binary_directory).expect("binary resource directory");
        fs::write(binary_directory.join("llama-server"), b"sidecar").expect("sidecar fixture");
        let data_root = tempfile::tempdir().expect("runtime data root");

        let runtime = EmbeddingRuntime::new(data_root.path(), Some(resources.path()));
        let status = runtime.status();

        assert_eq!(status.phase, SemanticRuntimePhase::Unavailable);
        assert!(status
            .message
            .is_some_and(|message| message.contains("release manifest")));
        assert!(!data_root.path().join("models").exists());
    }

    #[test]
    fn missing_resource_directory_keeps_semantic_search_optional() {
        let data_root = tempfile::tempdir().expect("runtime data root");

        let runtime = EmbeddingRuntime::new(data_root.path(), None);
        let status = runtime.status();

        assert_eq!(status.phase, SemanticRuntimePhase::Unavailable);
        assert!(!status.runtime_running);
        assert!(!status.verified);
        assert!(!data_root.path().join("models").exists());
    }

    #[test]
    fn bounded_sidecar_log_rotates_without_exceeding_its_budget() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut log =
            BoundedSidecarLog::open_with_limits(directory.path(), 8, 2).expect("bounded log");

        log.write_all(b"abcdefgh").expect("first log file");
        log.write_all(b"ijklmnop").expect("second log file");
        log.write_all(b"qrstuvwx").expect("third log file");
        log.write_all(b"yz").expect("rotated active log");
        drop(log);

        assert_eq!(
            fs::read(sidecar_log_path(directory.path(), None)).expect("active log"),
            b"yz"
        );
        assert_eq!(
            fs::read(sidecar_log_path(directory.path(), Some(1))).expect("first archive"),
            b"qrstuvwx"
        );
        assert_eq!(
            fs::read(sidecar_log_path(directory.path(), Some(2))).expect("second archive"),
            b"ijklmnop"
        );
        assert!(!sidecar_log_path(directory.path(), Some(3)).exists());
        for entry in fs::read_dir(directory.path()).expect("log directory") {
            assert!(
                entry
                    .expect("log entry")
                    .metadata()
                    .expect("metadata")
                    .len()
                    <= 8
            );
        }
    }

    #[test]
    fn bounded_sidecar_log_trims_existing_files_and_removes_excess_archives() {
        let directory = tempfile::tempdir().expect("temporary directory");
        fs::write(sidecar_log_path(directory.path(), None), b"0123456789ab")
            .expect("existing active log");
        fs::write(sidecar_log_path(directory.path(), Some(1)), b"abcdefghijkl")
            .expect("existing archive");
        fs::write(
            sidecar_log_path(directory.path(), Some(3)),
            b"excess archive",
        )
        .expect("excess archive");

        drop(
            BoundedSidecarLog::open_with_limits(directory.path(), 8, 2)
                .expect("bounded existing log"),
        );

        assert_eq!(
            fs::read(sidecar_log_path(directory.path(), None)).expect("new active log"),
            b""
        );
        assert_eq!(
            fs::read(sidecar_log_path(directory.path(), Some(1))).expect("trimmed active archive"),
            b"456789ab"
        );
        assert_eq!(
            fs::read(sidecar_log_path(directory.path(), Some(2))).expect("trimmed older archive"),
            b"efghijkl"
        );
        assert!(!sidecar_log_path(directory.path(), Some(3)).exists());
    }

    #[test]
    fn redacted_http_errors_do_not_expose_request_urls() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("temporary local port");
        let address = listener.local_addr().expect("temporary local address");
        drop(listener);
        let error = Client::builder()
            .connect_timeout(Duration::from_secs(1))
            .build()
            .expect("HTTP client")
            .get(format!(
                "http://{address}/private/path?api_key=not-for-logs"
            ))
            .send()
            .expect_err("closed local port should reject the request");
        let message = redacted_runtime_error(&SemanticRuntimeError::Http(error));

        assert_eq!(message, "semantic runtime HTTP connection failed");
        assert!(!message.contains("private"));
        assert!(!message.contains("api_key"));
        assert!(!message.contains("http://"));
    }

    #[test]
    fn content_range_must_resume_at_requested_offset() {
        let valid = reqwest::header::HeaderValue::from_static("bytes 100-199/200");
        validate_content_range(Some(&valid), 100).expect("valid range");
        let invalid = reqwest::header::HeaderValue::from_static("bytes 0-199/200");
        assert!(matches!(
            validate_content_range(Some(&invalid), 100),
            Err(SemanticRuntimeError::InvalidArtifact(_))
        ));
    }

    #[test]
    fn verification_receipt_round_trips_atomically_and_tolerates_corruption() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let model_path = directory.path().join("model.gguf");
        let sidecar_path = directory.path().join("llama-server");
        fs::write(&model_path, b"model").expect("model fixture");
        fs::write(&sidecar_path, b"sidecar").expect("sidecar fixture");
        let receipt = test_receipt(&test_runtime_settings(), &model_path, &sidecar_path)
            .expect("verification receipt");

        assert!(!verification_receipt_matches(directory.path(), &receipt));
        write_verification_receipt(directory.path(), &receipt).expect("receipt write");
        assert!(verification_receipt_matches(directory.path(), &receipt));
        assert_eq!(
            read_verification_receipt(directory.path()).expect("receipt read"),
            Some(receipt.clone())
        );
        assert!(!verification_receipt_path(directory.path())
            .with_extension("json.tmp")
            .exists());

        fs::write(verification_receipt_path(directory.path()), b"{broken")
            .expect("corrupt receipt");
        assert!(!verification_receipt_matches(directory.path(), &receipt));
        write_verification_receipt(directory.path(), &receipt).expect("receipt replacement");
        assert!(verification_receipt_matches(directory.path(), &receipt));

        invalidate_verification_receipt(directory.path());
        assert_eq!(
            read_verification_receipt(directory.path()).expect("receipt absence"),
            None
        );
    }

    #[test]
    fn verification_receipt_invalidates_for_artifact_and_contract_changes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let model_path = directory.path().join("model.gguf");
        let sidecar_path = directory.path().join("llama-server");
        fs::write(&model_path, b"model").expect("model fixture");
        fs::write(&sidecar_path, b"sidecar").expect("sidecar fixture");
        let settings = test_runtime_settings();
        let original =
            test_receipt(&settings, &model_path, &sidecar_path).expect("verification receipt");
        write_verification_receipt(directory.path(), &original).expect("receipt write");

        fs::write(&model_path, b"changed model").expect("changed model");
        let changed_model =
            test_receipt(&settings, &model_path, &sidecar_path).expect("changed-model receipt");
        assert!(!verification_receipt_matches(
            directory.path(),
            &changed_model
        ));
        write_verification_receipt(directory.path(), &changed_model)
            .expect("changed-model receipt write");

        fs::write(&sidecar_path, b"changed sidecar").expect("changed sidecar");
        let changed_sidecar =
            test_receipt(&settings, &model_path, &sidecar_path).expect("changed-sidecar receipt");
        assert!(!verification_receipt_matches(
            directory.path(),
            &changed_sidecar
        ));
        write_verification_receipt(directory.path(), &changed_sidecar)
            .expect("changed-sidecar receipt write");

        let mut changed_runtime = changed_sidecar.clone();
        changed_runtime.runtime.gpu_layers = "0".into();
        assert!(!verification_receipt_matches(
            directory.path(),
            &changed_runtime
        ));

        let mut changed_receipt_version = changed_sidecar.clone();
        changed_receipt_version.receipt_version += 1;
        assert!(!verification_receipt_matches(
            directory.path(),
            &changed_receipt_version
        ));

        let mut changed_manifest = changed_sidecar.clone();
        changed_manifest.manifest_sha256 = "changed".into();
        assert!(!verification_receipt_matches(
            directory.path(),
            &changed_manifest
        ));

        let mut changed_golden_fixtures = changed_sidecar.clone();
        changed_golden_fixtures.golden_fixtures_sha256 = "changed".into();
        assert!(!verification_receipt_matches(
            directory.path(),
            &changed_golden_fixtures
        ));

        let mut changed_sidecar_pin = changed_sidecar.clone();
        changed_sidecar_pin.sidecar_pin_sha256 = "changed".into();
        assert!(!verification_receipt_matches(
            directory.path(),
            &changed_sidecar_pin
        ));

        let mut changed_bundled_sidecar = changed_sidecar;
        changed_bundled_sidecar.bundled_sidecar = Some(BundledSidecarExpectation {
            sha256: "0".repeat(64),
            size: 1,
        });
        assert!(!verification_receipt_matches(
            directory.path(),
            &changed_bundled_sidecar
        ));
    }

    #[test]
    fn failure_disposition_only_invalidates_receipts_for_invalid_artifacts() {
        assert_eq!(
            failure_disposition(
                &SemanticRuntimeError::Runtime("sidecar stopped".into()),
                false
            ),
            FailureDisposition::Record
        );
        assert_eq!(
            failure_disposition(
                &SemanticRuntimeError::InvalidArtifact("model changed".into()),
                false
            ),
            FailureDisposition::RecordAndInvalidateVerification
        );
        assert_eq!(
            failure_disposition(
                &SemanticRuntimeError::RuntimeUnavailable("missing".into()),
                false
            ),
            FailureDisposition::Record
        );
        assert_eq!(
            failure_disposition(
                &SemanticRuntimeError::Runtime("sidecar stopped".into()),
                true
            ),
            FailureDisposition::Ignore
        );
    }

    #[test]
    fn pidfile_is_only_removed_by_its_owner() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let sidecar_path = directory.path().join("llama-server");
        let model_path = directory.path().join("model.gguf");
        write_pidfile(directory.path(), 101, &sidecar_path, &model_path).expect("pidfile write");

        remove_pidfile_if_owned(directory.path(), 202);
        assert!(pidfile_path(directory.path()).exists());

        remove_pidfile_if_owned(directory.path(), 101);
        assert!(!pidfile_path(directory.path()).exists());
    }

    #[test]
    fn lifecycle_lock_excludes_overlapping_search_services() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let first =
            acquire_lifecycle_lock(directory.path(), Duration::ZERO).expect("first lifecycle lock");
        let second = OpenOptions::new()
            .read(true)
            .write(true)
            .open(directory.path().join(LIFECYCLE_LOCK_FILE))
            .expect("second lifecycle handle");

        assert!(matches!(second.try_lock(), Err(TryLockError::WouldBlock)));
        first.unlock().expect("first lifecycle unlock");
        second.try_lock().expect("second lifecycle lock");
        second.unlock().expect("second lifecycle unlock");
    }
}

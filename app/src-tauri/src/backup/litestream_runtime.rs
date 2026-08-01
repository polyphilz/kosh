use std::{
    ffi::CString,
    fs,
    io::{Read, Write},
    net::Shutdown,
    os::fd::{AsRawFd, OwnedFd},
    os::unix::ffi::OsStrExt,
    os::unix::net::UnixStream,
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(test)]
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::database::{DatabaseClient, OffsiteBackupConfig};

use super::{
    credentials::{CredentialError, CredentialStore, MacOsKeychainCredentialStore},
    domain::CheckpointErrorCode,
    litestream::{
        write_aws_shared_credentials, CommandLitestreamControl, ImmutableLitestreamBinary,
        LitestreamConfig, LitestreamError, LitestreamRuntimePaths, LitestreamTxid, SyncResult,
        SystemCommandExecutor, VerifiedLitestreamBinary, AWS_EC2_METADATA_DISABLED_ENV,
        AWS_SHARED_CREDENTIALS_FILE_ENV, AWS_SHARED_CREDENTIALS_FILE_FD,
    },
    object_store::{ObjectStoreErrorCode, R2ObjectStore},
    owner::{claim_remote_owner_cancellable, RemoteOwnerError},
    writer_identity::{
        MacOsInstallationWriterIdentity, WriterIdentityError, WriterIdentityProvider,
    },
};

const SUPERVISOR_POLL_INTERVAL: Duration = Duration::from_secs(1);
const CONFIG_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const REMOTE_STATUS_INTERVAL: Duration = Duration::from_secs(30);
const RESTART_BASE_DELAY: Duration = Duration::from_secs(1);
const RESTART_MAX_DELAY: Duration = Duration::from_secs(5 * 60);
const CONTROL_REMOTE_TIMEOUT_SECONDS: u64 = 30;
const CHECKPOINT_LOCAL_COMMAND_TIMEOUT: Duration = Duration::from_secs(4);
const CHECKPOINT_LOCAL_SYNC_TIMEOUT: Duration = Duration::from_secs(5);
const CHECKPOINT_REMOTE_COMMAND_TIMEOUT: Duration = Duration::from_secs(34);
const CHECKPOINT_REMOTE_SYNC_TIMEOUT: Duration = Duration::from_secs(35);
const CHECKPOINT_HANDLE_COMPLETION_MARGIN: Duration = Duration::from_secs(1);
// Health confirmation must never consume Litestream's full remote timeout on
// the supervisor thread; application shutdown may wait behind this probe.
const STATUS_SYNC_TIMEOUT: Duration = Duration::from_secs(2);
const DAEMON_STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const DAEMON_LAUNCH_CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
const DAEMON_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(35);
const STALE_KILL_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);
const OWNER_CLAIM_POLL_INTERVAL: Duration = Duration::from_millis(50);
const PID_RECORD_FORMAT_VERSION: u32 = 2;
const MAX_PID_RECORD_BYTES: u64 = 16 * 1024;
const MAX_LITESTREAM_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum RelationalBackupPhase {
    #[default]
    Off,
    Starting,
    Running,
    Degraded,
    WaitingForCredentials,
    Unavailable,
    Blocked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum RelationalBackupErrorCode {
    CredentialsMissing,
    KeychainUnavailable,
    BinaryUnavailable,
    ConfigurationInvalid,
    LaunchFailed,
    ControlUnavailable,
    ProcessExited,
    RemoteSyncFailed,
    RemoteOwnerConflict,
    RemoteOwnerInvalid,
    WriterIdentityUnavailable,
    WorkerUnavailable,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RelationalBackupStatus {
    pub(crate) phase: RelationalBackupPhase,
    pub(crate) latest_local_txid: Option<String>,
    pub(crate) latest_remote_txid: Option<String>,
    pub(crate) last_remote_confirmed_at_ms: Option<i64>,
    pub(crate) restart_count: u32,
    pub(crate) last_error_code: Option<RelationalBackupErrorCode>,
}

enum SupervisorSignal {
    ReloadConfiguration(u64),
    Shutdown,
}

#[derive(Clone)]
pub(crate) struct LitestreamCheckpointHandle {
    shutdown: Arc<AtomicBool>,
    status: Arc<Mutex<RelationalBackupStatus>>,
    control: Arc<Mutex<Option<ActiveCheckpointControl>>>,
}

#[derive(Clone)]
struct ActiveCheckpointControl {
    config: OffsiteBackupConfig,
    control: Arc<dyn CheckpointControl>,
}

#[derive(Clone)]
pub(crate) struct BoundLitestreamCheckpointHandle {
    handle: LitestreamCheckpointHandle,
    config: OffsiteBackupConfig,
}

impl LitestreamCheckpointHandle {
    pub(crate) fn bind(&self, config: &OffsiteBackupConfig) -> BoundLitestreamCheckpointHandle {
        BoundLitestreamCheckpointHandle {
            handle: self.clone(),
            config: config.clone(),
        }
    }

    fn request_sync(
        &self,
        config: &OffsiteBackupConfig,
        local_only: bool,
        timeout: Duration,
    ) -> Result<SyncResult, CheckpointErrorCode> {
        let timeout_code = if local_only {
            CheckpointErrorCode::FenceTimeout
        } else {
            CheckpointErrorCode::NetworkTimeout
        };
        if self.shutdown.load(Ordering::Acquire) {
            return Err(CheckpointErrorCode::WorkerUnavailable);
        }
        let operation_timeout = timeout
            .checked_sub(CHECKPOINT_HANDLE_COMPLETION_MARGIN)
            .filter(|duration| !duration.is_zero())
            .ok_or(timeout_code)?;
        let control = self
            .control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or(CheckpointErrorCode::LitestreamUnavailable)?;
        if control.config != *config {
            return Err(CheckpointErrorCode::LitestreamUnavailable);
        }
        let expected_control = Arc::clone(&control.control);
        let operation_control = Arc::clone(&control.control);
        let (reply, receiver) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name(if local_only {
                "kosh-litestream-checkpoint-local".into()
            } else {
                "kosh-litestream-checkpoint-remote".into()
            })
            .spawn(move || {
                let result = if local_only {
                    operation_control.sync_local(operation_timeout)
                } else {
                    operation_control.sync_remote(operation_timeout)
                }
                .map_err(|failure| checkpoint_error(failure.code));
                let _ = reply.send(result);
            })
            .map_err(|_| CheckpointErrorCode::WorkerUnavailable)?;
        let result = receiver
            .recv_timeout(timeout)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => timeout_code,
                mpsc::RecvTimeoutError::Disconnected => CheckpointErrorCode::WorkerUnavailable,
            })??;
        let control_is_current = self
            .control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|current| {
                current.config == *config && Arc::ptr_eq(&current.control, &expected_control)
            });
        if !control_is_current {
            return Err(CheckpointErrorCode::LitestreamUnavailable);
        }
        let mut status = lock_status(&self.status);
        status.latest_local_txid = Some(result.txid.to_string());
        if !local_only {
            status.phase = RelationalBackupPhase::Running;
            status.latest_remote_txid = result.replica_txid.map(|txid| txid.to_string());
            status.last_remote_confirmed_at_ms = system_now_ms();
            status.last_error_code = None;
        }
        Ok(result)
    }
}

impl BoundLitestreamCheckpointHandle {
    pub(crate) fn sync_local(&self) -> Result<LitestreamTxid, CheckpointErrorCode> {
        self.sync_local_with_timeout(CHECKPOINT_LOCAL_SYNC_TIMEOUT)
    }

    pub(crate) fn sync_remote(&self) -> Result<SyncResult, CheckpointErrorCode> {
        self.handle
            .request_sync(&self.config, false, CHECKPOINT_REMOTE_SYNC_TIMEOUT)
    }

    pub(crate) fn sync_local_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<LitestreamTxid, CheckpointErrorCode> {
        self.handle
            .request_sync(
                &self.config,
                true,
                timeout.min(CHECKPOINT_LOCAL_SYNC_TIMEOUT),
            )
            .map(|sync| sync.txid)
    }
}

#[derive(Default)]
struct StartCancellation {
    reload_generation: AtomicU64,
    active: Mutex<Option<Arc<AtomicBool>>>,
}

impl StartCancellation {
    fn request_reload(&self) -> u64 {
        let generation = self.reload_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.cancel_active();
        generation
    }

    fn generation(&self) -> u64 {
        self.reload_generation.load(Ordering::Acquire)
    }

    fn install(&self, expected_generation: u64, shutdown: &AtomicBool) -> Option<Arc<AtomicBool>> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if shutdown.load(Ordering::Acquire) || self.generation() != expected_generation {
            return None;
        }
        let cancellation = Arc::new(AtomicBool::new(false));
        *active = Some(Arc::clone(&cancellation));
        Some(cancellation)
    }

    fn finish(&self, cancellation: &Arc<AtomicBool>) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, cancellation))
        {
            *active = None;
        }
    }

    fn cancel_active(&self) {
        if let Some(active) = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
        {
            active.store(true, Ordering::Release);
        }
    }
}

pub(crate) struct LitestreamRuntimeService {
    sender: mpsc::Sender<SupervisorSignal>,
    shutdown: Arc<AtomicBool>,
    start_cancellation: Arc<StartCancellation>,
    status: Arc<Mutex<RelationalBackupStatus>>,
    checkpoint_control: Arc<Mutex<Option<ActiveCheckpointControl>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl LitestreamRuntimeService {
    pub(crate) fn start(
        database: DatabaseClient,
        data_root: PathBuf,
        database_path: PathBuf,
        resource_dir: Option<PathBuf>,
    ) -> Self {
        let writer_identity = MacOsInstallationWriterIdentity::new(data_root.clone());
        Self::start_with_parts(
            database,
            Arc::new(SystemRuntimeFactory {
                data_root,
                database_path,
                resource_dir,
                credentials: MacOsKeychainCredentialStore,
                writer_identity,
            }),
            WorkerSchedule::production(),
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn disabled() -> Self {
        let (sender, receiver) = mpsc::channel();
        drop(receiver);
        Self {
            sender,
            shutdown: Arc::new(AtomicBool::new(true)),
            start_cancellation: Arc::new(StartCancellation::default()),
            status: Arc::new(Mutex::new(RelationalBackupStatus::default())),
            checkpoint_control: Arc::new(Mutex::new(None)),
            worker: Mutex::new(None),
        }
    }

    fn start_with_parts(
        database: DatabaseClient,
        factory: Arc<dyn RuntimeFactory>,
        schedule: WorkerSchedule,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let start_cancellation = Arc::new(StartCancellation::default());
        let status = Arc::new(Mutex::new(RelationalBackupStatus::default()));
        let checkpoint_control = Arc::new(Mutex::new(None));
        let worker_shutdown = Arc::clone(&shutdown);
        let worker_start_cancellation = Arc::clone(&start_cancellation);
        let worker_status = Arc::clone(&status);
        let worker_checkpoint_control = Arc::clone(&checkpoint_control);
        let spawned = thread::Builder::new()
            .name("kosh-litestream-supervisor".into())
            .spawn(move || {
                supervisor_worker(
                    database,
                    factory,
                    receiver,
                    SupervisorShared {
                        shutdown: worker_shutdown,
                        start_cancellation: worker_start_cancellation,
                        status: worker_status,
                        checkpoint_control: worker_checkpoint_control,
                    },
                    schedule,
                );
            });
        let worker = match spawned {
            Ok(worker) => Some(worker),
            Err(_) => {
                *lock_status(&status) = RelationalBackupStatus {
                    phase: RelationalBackupPhase::Unavailable,
                    last_error_code: Some(RelationalBackupErrorCode::WorkerUnavailable),
                    ..RelationalBackupStatus::default()
                };
                None
            }
        };
        let service = Self {
            sender,
            shutdown,
            start_cancellation,
            status,
            checkpoint_control,
            worker: Mutex::new(worker),
        };
        service.reload_configuration();
        service
    }

    pub(crate) fn status(&self) -> RelationalBackupStatus {
        lock_status(&self.status).clone()
    }

    pub(crate) fn checkpoint_handle(&self) -> LitestreamCheckpointHandle {
        LitestreamCheckpointHandle {
            shutdown: Arc::clone(&self.shutdown),
            status: Arc::clone(&self.status),
            control: Arc::clone(&self.checkpoint_control),
        }
    }

    pub(crate) fn reload_configuration(&self) {
        if self.shutdown.load(Ordering::Acquire) {
            return;
        }
        let generation = self.start_cancellation.request_reload();
        self.signal(SupervisorSignal::ReloadConfiguration(generation));
    }

    pub(crate) fn shutdown(&self) {
        if self.shutdown.swap(true, Ordering::SeqCst) {
            return;
        }
        self.start_cancellation.cancel_active();
        let _ = self.sender.send(SupervisorSignal::Shutdown);
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            if worker.join().is_err() {
                log::error!("Litestream supervisor panicked during shutdown");
            }
        }
    }

    fn signal(&self, signal: SupervisorSignal) {
        if self.shutdown.load(Ordering::Acquire) {
            return;
        }
        if self.sender.send(signal).is_err() {
            let mut status = lock_status(&self.status);
            status.phase = RelationalBackupPhase::Unavailable;
            status.last_error_code = Some(RelationalBackupErrorCode::WorkerUnavailable);
        }
    }
}

impl Drop for LitestreamRuntimeService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Clone, Copy)]
struct WorkerSchedule {
    supervisor_poll_interval: Duration,
    config_refresh_interval: Duration,
    remote_status_interval: Duration,
    restart_policy: RestartPolicy,
}

impl WorkerSchedule {
    const fn production() -> Self {
        Self {
            supervisor_poll_interval: SUPERVISOR_POLL_INTERVAL,
            config_refresh_interval: CONFIG_REFRESH_INTERVAL,
            remote_status_interval: REMOTE_STATUS_INTERVAL,
            restart_policy: RestartPolicy {
                base: RESTART_BASE_DELAY,
                maximum: RESTART_MAX_DELAY,
            },
        }
    }
}

#[derive(Clone, Copy)]
struct RestartPolicy {
    base: Duration,
    maximum: Duration,
}

impl RestartPolicy {
    fn delay(self, failure_count: u32) -> Duration {
        let exponent = failure_count.saturating_sub(1).min(20);
        let multiplier = 1_u32.checked_shl(exponent).unwrap_or(u32::MAX);
        self.base.saturating_mul(multiplier).min(self.maximum)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RuntimeFailure {
    code: RelationalBackupErrorCode,
    retryable: bool,
}

impl RuntimeFailure {
    const fn new(code: RelationalBackupErrorCode, retryable: bool) -> Self {
        Self { code, retryable }
    }
}

trait RuntimeFactory: Send + Sync {
    fn sweep_stale(&self, _shutdown: Arc<AtomicBool>) -> Result<(), RuntimeFailure> {
        Ok(())
    }

    fn start(
        &self,
        config: &OffsiteBackupConfig,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Box<dyn ManagedLitestream>, RuntimeFailure>;
}

trait ManagedLitestream: Send {
    fn has_exited(&mut self) -> Result<bool, RuntimeFailure>;
    fn checkpoint_control(&self) -> Arc<dyn CheckpointControl>;
    fn sync_remote(&self, timeout: Duration) -> Result<SyncResult, RuntimeFailure>;
    fn shutdown(&mut self);
    fn abort(&mut self) {
        self.shutdown();
    }
}

trait CheckpointControl: Send + Sync {
    fn sync_local(&self, timeout: Duration) -> Result<SyncResult, RuntimeFailure>;
    fn sync_remote(&self, timeout: Duration) -> Result<SyncResult, RuntimeFailure>;
}

struct SupervisorShared {
    shutdown: Arc<AtomicBool>,
    start_cancellation: Arc<StartCancellation>,
    status: Arc<Mutex<RelationalBackupStatus>>,
    checkpoint_control: Arc<Mutex<Option<ActiveCheckpointControl>>>,
}

fn supervisor_worker(
    database: DatabaseClient,
    factory: Arc<dyn RuntimeFactory>,
    receiver: mpsc::Receiver<SupervisorSignal>,
    shared: SupervisorShared,
    schedule: WorkerSchedule,
) {
    let SupervisorShared {
        shutdown,
        start_cancellation,
        status,
        checkpoint_control,
    } = shared;
    let mut current_config: Option<OffsiteBackupConfig> = None;
    let mut daemon: Option<Box<dyn ManagedLitestream>> = None;
    let mut blocked_revision: Option<i64> = None;
    let mut restart_count = 0_u32;
    let mut next_start = Instant::now();
    let mut next_config_refresh = Instant::now();
    let mut next_remote_status = Instant::now();
    let mut stale_sweep_pending = true;
    let mut stale_sweep_failures = 0_u32;
    let mut next_stale_sweep = Instant::now();
    let mut force_reload = true;
    let mut pending_reload_generation = None;
    let mut applied_reload_generation = 0;

    loop {
        let signal = match receiver.recv_timeout(schedule.supervisor_poll_interval) {
            Ok(signal) => Some(signal),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if shutdown.load(Ordering::Acquire)
            || matches!(signal.as_ref(), Some(SupervisorSignal::Shutdown))
        {
            break;
        }
        match signal {
            Some(SupervisorSignal::ReloadConfiguration(generation)) => {
                force_reload = true;
                pending_reload_generation = Some(generation);
                blocked_revision = None;
                restart_count = 0;
                next_start = Instant::now();
            }
            Some(SupervisorSignal::Shutdown) | None => {}
        }

        let now = Instant::now();
        if stale_sweep_pending {
            if now < next_stale_sweep {
                continue;
            }
            match factory.sweep_stale(Arc::clone(&shutdown)) {
                Ok(()) => {
                    stale_sweep_pending = false;
                    stale_sweep_failures = 0;
                }
                Err(failure) => {
                    stale_sweep_failures = stale_sweep_failures.saturating_add(1);
                    next_stale_sweep = now + schedule.restart_policy.delay(stale_sweep_failures);
                    update_failure_status(&status, failure, stale_sweep_failures);
                    log::warn!(
                        "could not safely sweep a stale Litestream runtime: {:?}",
                        failure.code
                    );
                    continue;
                }
            }
        }
        if force_reload || now >= next_config_refresh {
            let loading_reload_generation =
                pending_reload_generation.unwrap_or(applied_reload_generation);
            match database.load_enabled_offsite_backup_config() {
                Ok(config) => {
                    let previous_revision = current_config.as_ref().map(|value| value.revision);
                    let next_revision = config.as_ref().map(|value| value.revision);
                    if force_reload || previous_revision != next_revision {
                        shutdown_daemon(&mut daemon, &checkpoint_control);
                        current_config = config;
                        blocked_revision = None;
                        restart_count = 0;
                        next_start = Instant::now();
                        next_remote_status = Instant::now();
                        *lock_status(&status) = if current_config.is_some() {
                            RelationalBackupStatus {
                                phase: RelationalBackupPhase::Starting,
                                ..RelationalBackupStatus::default()
                            }
                        } else {
                            RelationalBackupStatus::default()
                        };
                    }
                    applied_reload_generation = loading_reload_generation;
                    pending_reload_generation = None;
                    force_reload = false;
                    next_config_refresh = now + schedule.config_refresh_interval;
                }
                Err(_) => {
                    force_reload = false;
                    update_failure_status(
                        &status,
                        RuntimeFailure::new(RelationalBackupErrorCode::WorkerUnavailable, true),
                        restart_count,
                    );
                    next_config_refresh = now + schedule.config_refresh_interval;
                }
            }
        }

        let Some(config) = current_config.as_ref() else {
            continue;
        };

        if let Some(running) = daemon.as_mut() {
            match running.has_exited() {
                Ok(false) => {}
                Ok(true) | Err(_) => {
                    shutdown_daemon(&mut daemon, &checkpoint_control);
                    schedule_restart(
                        &status,
                        RuntimeFailure::new(RelationalBackupErrorCode::ProcessExited, true),
                        &mut restart_count,
                        &mut next_start,
                        schedule.restart_policy,
                    );
                    continue;
                }
            }
        }

        if daemon.is_none()
            && blocked_revision != Some(config.revision)
            && Instant::now() >= next_start
        {
            let Some(start_cancel) =
                start_cancellation.install(applied_reload_generation, &shutdown)
            else {
                continue;
            };
            lock_status(&status).phase = RelationalBackupPhase::Starting;
            let start_result = factory.start(config, Arc::clone(&start_cancel));
            start_cancellation.finish(&start_cancel);
            if start_cancel.load(Ordering::Acquire) || shutdown.load(Ordering::Acquire) {
                if let Ok(mut started) = start_result {
                    started.abort();
                }
                continue;
            }
            match start_result {
                Ok(started) => {
                    let control = ActiveCheckpointControl {
                        config: config.clone(),
                        control: started.checkpoint_control(),
                    };
                    *checkpoint_control
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(control);
                    daemon = Some(started);
                    next_remote_status = Instant::now();
                }
                Err(failure) if failure.retryable => {
                    schedule_restart(
                        &status,
                        failure,
                        &mut restart_count,
                        &mut next_start,
                        schedule.restart_policy,
                    );
                    continue;
                }
                Err(failure) => {
                    blocked_revision = Some(config.revision);
                    update_failure_status(&status, failure, restart_count);
                    continue;
                }
            }
        }

        if let Some(running) = daemon.as_ref() {
            if Instant::now() >= next_remote_status {
                match running.sync_remote(STATUS_SYNC_TIMEOUT) {
                    Ok(sync) => {
                        restart_count = 0;
                        let mut current = lock_status(&status);
                        current.phase = RelationalBackupPhase::Running;
                        current.latest_local_txid = Some(sync.txid.to_string());
                        current.latest_remote_txid =
                            sync.replica_txid.map(|value| value.to_string());
                        current.last_remote_confirmed_at_ms = system_now_ms();
                        current.restart_count = 0;
                        current.last_error_code = None;
                    }
                    Err(failure) if failure.retryable => {
                        update_failure_status(&status, failure, restart_count);
                    }
                    Err(failure) => {
                        shutdown_daemon(&mut daemon, &checkpoint_control);
                        blocked_revision = Some(config.revision);
                        update_failure_status(&status, failure, restart_count);
                    }
                }
                next_remote_status = Instant::now() + schedule.remote_status_interval;
            }
        }
    }

    shutdown_daemon(&mut daemon, &checkpoint_control);
}

fn schedule_restart(
    status: &Mutex<RelationalBackupStatus>,
    failure: RuntimeFailure,
    restart_count: &mut u32,
    next_start: &mut Instant,
    policy: RestartPolicy,
) {
    *restart_count = restart_count.saturating_add(1);
    *next_start = Instant::now() + policy.delay(*restart_count);
    update_failure_status(status, failure, *restart_count);
}

fn update_failure_status(
    status: &Mutex<RelationalBackupStatus>,
    failure: RuntimeFailure,
    restart_count: u32,
) {
    let mut current = lock_status(status);
    current.phase = match failure.code {
        RelationalBackupErrorCode::CredentialsMissing => {
            RelationalBackupPhase::WaitingForCredentials
        }
        RelationalBackupErrorCode::KeychainUnavailable
        | RelationalBackupErrorCode::BinaryUnavailable
        | RelationalBackupErrorCode::WriterIdentityUnavailable
        | RelationalBackupErrorCode::WorkerUnavailable => RelationalBackupPhase::Unavailable,
        _ if failure.retryable => RelationalBackupPhase::Degraded,
        _ => RelationalBackupPhase::Blocked,
    };
    current.restart_count = restart_count;
    current.last_error_code = Some(failure.code);
}

fn shutdown_daemon(
    daemon: &mut Option<Box<dyn ManagedLitestream>>,
    checkpoint_control: &Mutex<Option<ActiveCheckpointControl>>,
) {
    checkpoint_control
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(mut daemon) = daemon.take() {
        daemon.shutdown();
    }
}

fn checkpoint_error(code: RelationalBackupErrorCode) -> CheckpointErrorCode {
    match code {
        RelationalBackupErrorCode::CredentialsMissing => CheckpointErrorCode::CredentialsMissing,
        RelationalBackupErrorCode::KeychainUnavailable => CheckpointErrorCode::KeychainUnavailable,
        RelationalBackupErrorCode::ConfigurationInvalid => {
            CheckpointErrorCode::InvalidConfiguration
        }
        RelationalBackupErrorCode::RemoteOwnerConflict => CheckpointErrorCode::OwnerConflict,
        RelationalBackupErrorCode::RemoteOwnerInvalid => CheckpointErrorCode::OwnerInvalid,
        RelationalBackupErrorCode::WorkerUnavailable => CheckpointErrorCode::WorkerUnavailable,
        RelationalBackupErrorCode::BinaryUnavailable
        | RelationalBackupErrorCode::LaunchFailed
        | RelationalBackupErrorCode::ControlUnavailable
        | RelationalBackupErrorCode::ProcessExited => CheckpointErrorCode::LitestreamUnavailable,
        RelationalBackupErrorCode::RemoteSyncFailed => CheckpointErrorCode::Network,
        RelationalBackupErrorCode::WriterIdentityUnavailable => {
            CheckpointErrorCode::InvalidConfiguration
        }
    }
}

fn lock_status(
    status: &Mutex<RelationalBackupStatus>,
) -> std::sync::MutexGuard<'_, RelationalBackupStatus> {
    status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn system_now_ms() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

struct SystemRuntimeFactory<C, I> {
    data_root: PathBuf,
    database_path: PathBuf,
    resource_dir: Option<PathBuf>,
    credentials: C,
    writer_identity: I,
}

impl<C: CredentialStore, I: WriterIdentityProvider> RuntimeFactory for SystemRuntimeFactory<C, I> {
    fn sweep_stale(&self, shutdown: Arc<AtomicBool>) -> Result<(), RuntimeFailure> {
        let runtime =
            LitestreamRuntimePaths::new(&self.data_root).map_err(map_litestream_start_error)?;
        if !stale_runtime_residue_exists(&runtime)? {
            return Ok(());
        }
        runtime.prepare().map_err(map_litestream_start_error)?;
        let ownership = acquire_runtime_ownership(&runtime)?;
        let trusted_cleanup_sha256s = VerifiedLitestreamBinary::trusted_cleanup_sha256s()
            .map_err(map_litestream_start_error)?;
        sweep_stale_runtime(
            &ownership,
            &runtime,
            &trusted_cleanup_sha256s,
            &self.database_path,
            &shutdown,
        )
    }

    fn start(
        &self,
        config: &OffsiteBackupConfig,
        shutdown: Arc<AtomicBool>,
    ) -> Result<Box<dyn ManagedLitestream>, RuntimeFailure> {
        if shutdown.load(Ordering::Acquire) {
            return Err(cancelled_start());
        }
        let runtime =
            LitestreamRuntimePaths::new(&self.data_root).map_err(map_litestream_start_error)?;
        runtime.prepare().map_err(map_litestream_start_error)?;
        let ownership = acquire_runtime_ownership(&runtime)?;
        let trusted_cleanup_sha256s = VerifiedLitestreamBinary::trusted_cleanup_sha256s()
            .map_err(map_litestream_start_error)?;
        sweep_stale_runtime(
            &ownership,
            &runtime,
            &trusted_cleanup_sha256s,
            &self.database_path,
            &shutdown,
        )?;
        let resource_dir = self.resource_dir.as_deref().ok_or_else(|| {
            RuntimeFailure::new(RelationalBackupErrorCode::BinaryUnavailable, false)
        })?;
        let binary =
            VerifiedLitestreamBinary::resolve(resource_dir).map_err(map_litestream_start_error)?;
        let binary = binary
            .stage_immutable(&runtime)
            .map_err(map_litestream_start_error)?;

        let credentials = self
            .credentials
            .load(&config.backup_set_id)
            .map_err(map_credential_start_error)?;
        let writer_id = self
            .writer_identity
            .load()
            .map_err(map_writer_identity_start_error)?;
        let keyspace = config.target.keyspace(&config.backup_set_id);
        let store = R2ObjectStore::new(config.target.clone(), keyspace.clone(), &credentials)
            .map_err(|error| map_remote_owner_error(RemoteOwnerError::Store(error)))?;
        claim_remote_owner_interruptibly(
            store,
            keyspace.clone(),
            config.backup_set_id.clone(),
            config.replica_epoch_id.clone(),
            writer_id,
            Arc::clone(&shutdown),
        )?;
        if shutdown.load(Ordering::Acquire) {
            return Err(cancelled_start());
        }
        let replica_path = keyspace.litestream(&config.replica_epoch_id);
        let endpoint = config.target.endpoint();
        let rendered = LitestreamConfig {
            database_path: &self.database_path,
            runtime: &runtime,
            bucket: config.target.bucket.as_str(),
            replica_path: replica_path.as_str(),
            endpoint: &endpoint,
        }
        .render()
        .map_err(map_litestream_start_error)?;
        runtime
            .write_config(&rendered)
            .map_err(map_litestream_start_error)?;
        let config_sha256 = sha256_hex(rendered.as_bytes());
        let daemon = SystemManagedLitestream::launch(
            binary,
            runtime,
            ownership,
            self.database_path.clone(),
            config.backup_set_id.as_str().to_owned(),
            config.replica_epoch_id.as_str().to_owned(),
            config_sha256,
            credentials,
            &shutdown,
        )?;
        Ok(Box::new(daemon))
    }
}

fn claim_remote_owner_interruptibly(
    store: R2ObjectStore,
    keyspace: super::domain::R2Keyspace,
    backup_set_id: super::domain::BackupSetId,
    replica_epoch_id: super::domain::ReplicaEpochId,
    writer_id: super::domain::BackupWriterId,
    shutdown: Arc<AtomicBool>,
) -> Result<(), RuntimeFailure> {
    run_start_operation_interruptibly(shutdown, "kosh-remote-owner-claim", move |cancelled| {
        claim_remote_owner_cancellable(
            &store,
            &keyspace,
            &backup_set_id,
            &replica_epoch_id,
            &writer_id,
            &cancelled,
        )
        .map_err(map_remote_owner_error)
    })
}

fn run_start_operation_interruptibly<T: Send + 'static>(
    shutdown: Arc<AtomicBool>,
    worker_name: &'static str,
    operation: impl FnOnce(Arc<AtomicBool>) -> Result<T, RuntimeFailure> + Send + 'static,
) -> Result<T, RuntimeFailure> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker_shutdown = Arc::clone(&shutdown);
    thread::Builder::new()
        .name(worker_name.into())
        .spawn(move || {
            let _ = sender.send(operation(worker_shutdown));
        })
        .map_err(|_| RuntimeFailure::new(RelationalBackupErrorCode::WorkerUnavailable, true))?;

    loop {
        if shutdown.load(Ordering::Acquire) {
            return Err(cancelled_start());
        }
        match receiver.recv_timeout(OWNER_CLAIM_POLL_INTERVAL) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(RuntimeFailure::new(
                    RelationalBackupErrorCode::WorkerUnavailable,
                    true,
                ));
            }
        }
    }
}

fn cancelled_start() -> RuntimeFailure {
    RuntimeFailure::new(RelationalBackupErrorCode::RemoteSyncFailed, true)
}

fn stale_runtime_residue_exists(runtime: &LitestreamRuntimePaths) -> Result<bool, RuntimeFailure> {
    for path in [runtime.pid(), runtime.socket()] {
        match fs::symlink_metadata(path) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(RuntimeFailure::new(
                    RelationalBackupErrorCode::ControlUnavailable,
                    true,
                ));
            }
        }
    }
    Ok(false)
}

fn map_credential_start_error(error: CredentialError) -> RuntimeFailure {
    match error {
        CredentialError::Missing => {
            RuntimeFailure::new(RelationalBackupErrorCode::CredentialsMissing, true)
        }
        CredentialError::Unavailable => {
            RuntimeFailure::new(RelationalBackupErrorCode::KeychainUnavailable, true)
        }
        CredentialError::InvalidCredential(_)
        | CredentialError::UnsupportedPayloadVersion
        | CredentialError::CorruptPayload => {
            RuntimeFailure::new(RelationalBackupErrorCode::ConfigurationInvalid, false)
        }
    }
}

fn map_writer_identity_start_error(error: WriterIdentityError) -> RuntimeFailure {
    match error {
        WriterIdentityError::Unavailable => {
            RuntimeFailure::new(RelationalBackupErrorCode::WriterIdentityUnavailable, true)
        }
        WriterIdentityError::Invalid => {
            RuntimeFailure::new(RelationalBackupErrorCode::ConfigurationInvalid, false)
        }
    }
}

fn map_remote_owner_error(error: RemoteOwnerError) -> RuntimeFailure {
    match error {
        RemoteOwnerError::Cancelled => cancelled_start(),
        RemoteOwnerError::Conflict => {
            RuntimeFailure::new(RelationalBackupErrorCode::RemoteOwnerConflict, false)
        }
        RemoteOwnerError::Invalid => {
            RuntimeFailure::new(RelationalBackupErrorCode::RemoteOwnerInvalid, false)
        }
        RemoteOwnerError::Store(error) => match error.code {
            ObjectStoreErrorCode::InvalidConfiguration
            | ObjectStoreErrorCode::KeyOutsidePrefix
            | ObjectStoreErrorCode::DeletionNotAuthorized
            | ObjectStoreErrorCode::MediaWriteRequiresVerifiedCreate
            | ObjectStoreErrorCode::ContentHashMismatch
            | ObjectStoreErrorCode::ObjectTooLarge => {
                RuntimeFailure::new(RelationalBackupErrorCode::ConfigurationInvalid, false)
            }
            ObjectStoreErrorCode::ResponseTooLarge | ObjectStoreErrorCode::InvalidResponse => {
                RuntimeFailure::new(RelationalBackupErrorCode::RemoteOwnerInvalid, false)
            }
            ObjectStoreErrorCode::Network
            | ObjectStoreErrorCode::Timeout
            | ObjectStoreErrorCode::AuthenticationRejected
            | ObjectStoreErrorCode::AuthorizationRejected
            | ObjectStoreErrorCode::NotFound
            | ObjectStoreErrorCode::Conflict
            | ObjectStoreErrorCode::PreconditionFailed
            | ObjectStoreErrorCode::RateLimited
            | ObjectStoreErrorCode::ServiceUnavailable => {
                RuntimeFailure::new(RelationalBackupErrorCode::RemoteSyncFailed, true)
            }
        },
    }
}

fn map_litestream_start_error(error: LitestreamError) -> RuntimeFailure {
    match error {
        LitestreamError::PrepareRuntime(_)
        | LitestreamError::WriteConfig(_)
        | LitestreamError::StageRestoreConfig(_)
        | LitestreamError::PrepareRestoreDestination(_)
        | LitestreamError::PublishRestoreDestination(_)
        | LitestreamError::Execute(_) => {
            RuntimeFailure::new(RelationalBackupErrorCode::ControlUnavailable, true)
        }
        LitestreamError::CommandFailed { .. } => {
            RuntimeFailure::new(RelationalBackupErrorCode::RemoteSyncFailed, true)
        }
        LitestreamError::InvalidConfigField(_)
        | LitestreamError::InvalidRestoreConfig
        | LitestreamError::RelativeDatabasePath
        | LitestreamError::NonUtf8RuntimePath
        | LitestreamError::ControlSocketPathTooLong => {
            RuntimeFailure::new(RelationalBackupErrorCode::ConfigurationInvalid, false)
        }
        LitestreamError::InvalidJson(_)
        | LitestreamError::InvalidTxid
        | LitestreamError::InvalidSyncContract
        | LitestreamError::InvalidRestoreContract
        | LitestreamError::InvalidRestoreDestination
        | LitestreamError::UnexpectedDatabasePath
        | LitestreamError::OversizedControlResponse => {
            RuntimeFailure::new(RelationalBackupErrorCode::RemoteSyncFailed, false)
        }
        LitestreamError::InvalidEmbeddedManifest(_)
        | LitestreamError::MissingReleaseManifest(_)
        | LitestreamError::InvalidReleaseManifest(_)
        | LitestreamError::ReleaseManifestMismatch
        | LitestreamError::MissingBinary(_)
        | LitestreamError::BinaryNotRegular
        | LitestreamError::BinaryNotExecutable
        | LitestreamError::BinarySizeMismatch
        | LitestreamError::BinaryChecksumMismatch
        | LitestreamError::StageBinary(_)
        | LitestreamError::InvalidStagedBinary
        | LitestreamError::ProcessCodeSignatureMismatch
        | LitestreamError::ProcessIdentityUnavailable(_)
        | LitestreamError::UnsafeProtocolPin
        | LitestreamError::RestoreDestinationExists
        | LitestreamError::RestorePlanTooLarge => {
            RuntimeFailure::new(RelationalBackupErrorCode::BinaryUnavailable, false)
        }
    }
}

fn map_control_error(error: LitestreamError) -> RuntimeFailure {
    if matches!(
        &error,
        LitestreamError::Execute(source) if source.kind() == std::io::ErrorKind::TimedOut
    ) {
        RuntimeFailure::new(RelationalBackupErrorCode::RemoteSyncFailed, true)
    } else {
        map_litestream_start_error(error)
    }
}

trait RuntimeChild {
    fn id(&self) -> u32;
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>>;
    fn kill(&mut self) -> std::io::Result<()>;
    fn wait(&mut self) -> std::io::Result<ExitStatus>;
}

impl RuntimeChild for Child {
    fn id(&self) -> u32 {
        Child::id(self)
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        Child::try_wait(self)
    }

    fn kill(&mut self) -> std::io::Result<()> {
        Child::kill(self)
    }

    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        Child::wait(self)
    }
}

struct TracedLitestreamChild {
    pid: libc::pid_t,
    reaped: bool,
    detached: bool,
}

impl TracedLitestreamChild {
    fn resume(&mut self) -> std::io::Result<()> {
        if self.detached {
            return Ok(());
        }
        // The child called PT_TRACE_ME before exec, so the kernel holds the
        // selected image at its exec trap until this tracing parent detaches.
        // Job-control signals from another same-user process cannot bypass
        // this stop.
        let result = unsafe {
            libc::ptrace(
                libc::PT_DETACH,
                self.pid,
                std::ptr::without_provenance_mut(1),
                0,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
        self.detached = true;
        Ok(())
    }
}

impl RuntimeChild for TracedLitestreamChild {
    fn id(&self) -> u32 {
        u32::try_from(self.pid).expect("fork returned a positive PID")
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        if self.reaped {
            return Err(std::io::Error::other("Litestream child was already reaped"));
        }
        let mut status = 0;
        // SAFETY: `status` is writable and `self.pid` belongs to this process.
        let result = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
        if result == 0 {
            return Ok(None);
        }
        if result < 0 {
            return Err(std::io::Error::last_os_error());
        }
        self.reaped = true;
        Ok(Some(ExitStatus::from_raw(status)))
    }

    fn kill(&mut self) -> std::io::Result<()> {
        if self.reaped {
            return Ok(());
        }
        // SAFETY: `self.pid` is the live child returned by fork.
        let result = unsafe { libc::kill(self.pid, libc::SIGKILL) };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        if self.reaped {
            return Err(std::io::Error::other("Litestream child was already reaped"));
        }
        let mut status = 0;
        loop {
            // SAFETY: `status` is writable and `self.pid` belongs to this process.
            let result = unsafe { libc::waitpid(self.pid, &mut status, 0) };
            if result > 0 {
                self.reaped = true;
                return Ok(ExitStatus::from_raw(status));
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

impl Drop for TracedLitestreamChild {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.kill();
            let _ = self.wait();
        }
    }
}

fn spawn_litestream_traced(
    binary: &Path,
    config: &Path,
    credential_reader: OwnedFd,
) -> std::io::Result<TracedLitestreamChild> {
    let binary = CString::new(binary.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("Litestream path contains NUL"))?;
    let config = CString::new(config.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("Litestream config path contains NUL"))?;
    let arguments = [
        binary.clone(),
        CString::new("replicate").expect("literal has no NUL"),
        CString::new("-config").expect("literal has no NUL"),
        config,
    ];
    let environment = [
        CString::new(format!(
            "{AWS_SHARED_CREDENTIALS_FILE_ENV}={AWS_SHARED_CREDENTIALS_FILE_FD}"
        ))
        .expect("credential environment has no NUL"),
        CString::new(format!("{AWS_EC2_METADATA_DISABLED_ENV}=true"))
            .expect("metadata environment has no NUL"),
    ];
    let argument_pointers = arguments
        .iter()
        .map(|value| value.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect::<Vec<_>>();
    let environment_pointers = environment
        .iter()
        .map(|value| value.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect::<Vec<_>>();
    let null_device = fs::OpenOptions::new().write(true).open("/dev/null")?;
    // SAFETY: sysconf reads the process descriptor limit and has no retained
    // pointers. The value is captured before fork so the child only loops and
    // closes descriptors with async-signal-safe calls.
    let max_descriptor = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
    if max_descriptor < 3 || max_descriptor > i64::from(i32::MAX) {
        return Err(std::io::Error::other(
            "process descriptor limit is unavailable",
        ));
    }
    let max_descriptor = i32::try_from(max_descriptor)
        .map_err(|_| std::io::Error::other("process descriptor limit is too large"))?;

    // SAFETY: the child branch calls only async-signal-safe libc operations
    // against values fully allocated before fork, then immediately execs or
    // calls _exit. It never returns into Rust or runs a destructor.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if pid == 0 {
        unsafe {
            let mut setup_ok = libc::setpgid(0, 0) == 0
                && libc::dup2(credential_reader.as_raw_fd(), libc::STDIN_FILENO) >= 0
                && libc::dup2(null_device.as_raw_fd(), libc::STDOUT_FILENO) >= 0
                && libc::dup2(null_device.as_raw_fd(), libc::STDERR_FILENO) >= 0;
            for descriptor in 3..max_descriptor {
                libc::close(descriptor);
            }
            setup_ok = setup_ok && libc::ptrace(libc::PT_TRACE_ME, 0, std::ptr::null_mut(), 0) == 0;
            if setup_ok {
                libc::execve(
                    binary.as_ptr(),
                    argument_pointers.as_ptr(),
                    environment_pointers.as_ptr(),
                );
            }
            libc::_exit(127);
        }
    }

    let mut status = 0;
    loop {
        // SAFETY: `status` is writable and `pid` is this process's child.
        let result = unsafe { libc::waitpid(pid, &mut status, 0) };
        if result > 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            kill_and_reap_untracked_child(pid);
            return Err(error);
        }
    }
    if !libc::WIFSTOPPED(status) || libc::WSTOPSIG(status) != libc::SIGTRAP {
        if libc::WIFSTOPPED(status) {
            kill_and_reap_untracked_child(pid);
        }
        return Err(std::io::Error::other(
            "Litestream image did not stop at the traced exec boundary",
        ));
    }
    Ok(TracedLitestreamChild {
        pid,
        reaped: false,
        detached: false,
    })
}

fn kill_and_reap_untracked_child(pid: libc::pid_t) {
    // SAFETY: `pid` is the child returned by this launch attempt. SIGKILL is
    // idempotent here, and waitpid is retried only for interruption.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
        let mut status = 0;
        loop {
            let result = libc::waitpid(pid, &mut status, 0);
            if result >= 0
                || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                break;
            }
        }
    }
}

struct SystemManagedLitestream {
    child: Option<TracedLitestreamChild>,
    runtime: LitestreamRuntimePaths,
    ownership: LitestreamRuntimeOwnership,
    control: Arc<SystemCheckpointControl>,
}

struct SystemCheckpointControl {
    database_path: PathBuf,
    control: CommandLitestreamControl<SystemCommandExecutor>,
    operation_gate: Mutex<()>,
}

impl SystemManagedLitestream {
    #[allow(clippy::too_many_arguments)]
    fn launch(
        binary: ImmutableLitestreamBinary,
        runtime: LitestreamRuntimePaths,
        ownership: LitestreamRuntimeOwnership,
        database_path: PathBuf,
        backup_set_id: String,
        replica_epoch_id: String,
        config_sha256: String,
        credentials: super::credentials::R2Credentials,
        shutdown: &AtomicBool,
    ) -> Result<Self, RuntimeFailure> {
        binary
            .reverify_before_spawn()
            .map_err(map_litestream_start_error)?;
        let binary_path = binary.path().to_owned();
        let (credential_reader, mut credential_writer) = UnixStream::pair()
            .map_err(|_| RuntimeFailure::new(RelationalBackupErrorCode::LaunchFailed, true))?;
        let credential_reader: OwnedFd = credential_reader.into();
        let mut child = spawn_litestream_traced(&binary_path, runtime.config(), credential_reader)
            .map_err(|_| RuntimeFailure::new(RelationalBackupErrorCode::LaunchFailed, true))?;
        let pid = child.id();
        if let Err(error) = binary.verify_running_process(pid) {
            kill_process_group(&mut child);
            cleanup_owned_runtime(&ownership, &runtime, pid, false);
            return Err(map_litestream_start_error(error));
        }
        let identity = LaunchIdentity {
            backup_set_id,
            replica_epoch_id,
            config_sha256,
            executable_sha256: binary.sha256().to_owned(),
        };
        if write_pid_record_then_release_credentials(
            &runtime,
            pid,
            &binary_path,
            &database_path,
            &identity,
            &mut credential_writer,
            &credentials,
        )
        .and_then(|()| credential_writer.shutdown(Shutdown::Write))
        .is_err()
        {
            kill_process_group(&mut child);
            cleanup_owned_runtime(&ownership, &runtime, pid, false);
            return Err(RuntimeFailure::new(
                RelationalBackupErrorCode::ControlUnavailable,
                true,
            ));
        }
        drop(credential_writer);
        if child.resume().is_err() {
            kill_process_group(&mut child);
            cleanup_owned_runtime(&ownership, &runtime, pid, false);
            return Err(RuntimeFailure::new(
                RelationalBackupErrorCode::LaunchFailed,
                true,
            ));
        }
        if let Err(failure) = wait_for_control_socket(&mut child, &runtime, shutdown) {
            terminate_process_group(&mut child, DAEMON_LAUNCH_CLEANUP_TIMEOUT);
            cleanup_owned_runtime(&ownership, &runtime, pid, true);
            return Err(failure);
        }
        let control = Arc::new(SystemCheckpointControl {
            database_path,
            control: CommandLitestreamControl::new(
                binary_path,
                runtime.socket().to_owned(),
                CONTROL_REMOTE_TIMEOUT_SECONDS,
                SystemCommandExecutor,
            ),
            operation_gate: Mutex::new(()),
        });
        Ok(Self {
            child: Some(child),
            runtime,
            ownership,
            control,
        })
    }

    fn cleanup(&self, pid: u32) {
        cleanup_owned_runtime(&self.ownership, &self.runtime, pid, true);
    }
}

impl ManagedLitestream for SystemManagedLitestream {
    fn has_exited(&mut self) -> Result<bool, RuntimeFailure> {
        let Some(child) = self.child.as_mut() else {
            return Ok(true);
        };
        match child.try_wait() {
            Ok(None) => Ok(false),
            Ok(Some(_)) => {
                let pid = child.id();
                self.child.take();
                self.cleanup(pid);
                Ok(true)
            }
            Err(_) => Err(RuntimeFailure::new(
                RelationalBackupErrorCode::ProcessExited,
                true,
            )),
        }
    }

    fn checkpoint_control(&self) -> Arc<dyn CheckpointControl> {
        self.control.clone()
    }

    fn sync_remote(&self, timeout: Duration) -> Result<SyncResult, RuntimeFailure> {
        CheckpointControl::sync_remote(self.control.as_ref(), timeout)
    }

    fn shutdown(&mut self) {
        if let Some(mut child) = self.child.take() {
            let pid = child.id();
            terminate_process_group(&mut child, DAEMON_SHUTDOWN_TIMEOUT);
            self.cleanup(pid);
        }
    }

    fn abort(&mut self) {
        if let Some(mut child) = self.child.take() {
            let pid = child.id();
            kill_process_group(&mut child);
            self.cleanup(pid);
        }
    }
}

impl CheckpointControl for SystemCheckpointControl {
    fn sync_local(&self, timeout: Duration) -> Result<SyncResult, RuntimeFailure> {
        let _operation = self
            .operation_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.control
            .sync_local_with_timeout(
                &self.database_path,
                timeout.min(CHECKPOINT_LOCAL_COMMAND_TIMEOUT),
            )
            .map_err(map_control_error)
    }

    fn sync_remote(&self, timeout: Duration) -> Result<SyncResult, RuntimeFailure> {
        let _operation = self
            .operation_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.control
            .sync_remote_with_timeout(
                &self.database_path,
                timeout.min(CHECKPOINT_REMOTE_COMMAND_TIMEOUT),
            )
            .map_err(map_control_error)
    }
}

impl Drop for SystemManagedLitestream {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn wait_for_control_socket(
    child: &mut impl RuntimeChild,
    runtime: &LitestreamRuntimePaths,
    shutdown: &AtomicBool,
) -> Result<(), RuntimeFailure> {
    let deadline = Instant::now() + DAEMON_STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        if shutdown.load(Ordering::Acquire) {
            return Err(cancelled_start());
        }
        match child.try_wait() {
            Ok(Some(_)) => {
                return Err(RuntimeFailure::new(
                    RelationalBackupErrorCode::LaunchFailed,
                    true,
                ));
            }
            Ok(None) => {}
            Err(_) => {
                return Err(RuntimeFailure::new(
                    RelationalBackupErrorCode::LaunchFailed,
                    true,
                ));
            }
        }
        match control_socket_is_private(runtime.socket()) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(()) => {
                return Err(RuntimeFailure::new(
                    RelationalBackupErrorCode::ControlUnavailable,
                    false,
                ));
            }
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    Err(RuntimeFailure::new(
        RelationalBackupErrorCode::ControlUnavailable,
        true,
    ))
}

#[cfg(unix)]
fn control_socket_is_private(path: &Path) -> Result<bool, ()> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() {
                return Err(());
            }
            if metadata.permissions().mode() & 0o777 != 0o600 {
                return Err(());
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(()),
    }
}

#[cfg(not(unix))]
fn control_socket_is_private(path: &Path) -> Result<bool, ()> {
    Ok(path.exists())
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LitestreamPidRecord {
    format_version: u32,
    pid: u32,
    executable: String,
    executable_sha256: String,
    config: String,
    socket: String,
    database: String,
    backup_set_id: String,
    replica_epoch_id: String,
    config_sha256: String,
}

struct LaunchIdentity {
    backup_set_id: String,
    replica_epoch_id: String,
    config_sha256: String,
    executable_sha256: String,
}

fn write_pid_record_then_release_credentials(
    runtime: &LitestreamRuntimePaths,
    pid: u32,
    binary: &Path,
    database_path: &Path,
    identity: &LaunchIdentity,
    credential_writer: &mut impl Write,
    credentials: &super::credentials::R2Credentials,
) -> std::io::Result<()> {
    write_pid_record(runtime, pid, binary, database_path, identity)?;
    write_aws_shared_credentials(credential_writer, credentials)
}

fn write_pid_record(
    runtime: &LitestreamRuntimePaths,
    pid: u32,
    binary: &Path,
    database_path: &Path,
    identity: &LaunchIdentity,
) -> std::io::Result<()> {
    let record = LitestreamPidRecord {
        format_version: PID_RECORD_FORMAT_VERSION,
        pid,
        executable: utf8_path(binary)?,
        executable_sha256: identity.executable_sha256.clone(),
        config: utf8_path(runtime.config())?,
        socket: utf8_path(runtime.socket())?,
        database: utf8_path(database_path)?,
        backup_set_id: identity.backup_set_id.clone(),
        replica_epoch_id: identity.replica_epoch_id.clone(),
        config_sha256: identity.config_sha256.clone(),
    };
    let bytes = serde_json::to_vec(&record).map_err(std::io::Error::other)?;
    if bytes.len() as u64 > MAX_PID_RECORD_BYTES {
        return Err(std::io::Error::other("Litestream PID record is oversized"));
    }
    write_private_atomic(runtime.pid(), &bytes)
}

fn read_pid_record(
    runtime: &LitestreamRuntimePaths,
) -> std::io::Result<Option<LitestreamPidRecord>> {
    let bytes = match read_private_bounded_regular_file(runtime.pid(), MAX_PID_RECORD_BYTES) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let record: LitestreamPidRecord =
        serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
    if record.format_version != PID_RECORD_FORMAT_VERSION
        || !is_canonical_sha256(&record.executable_sha256)
        || !is_canonical_sha256(&record.config_sha256)
    {
        return Err(std::io::Error::other("unsupported Litestream PID record"));
    }
    Ok(Some(record))
}

fn sweep_stale_runtime(
    _ownership: &LitestreamRuntimeOwnership,
    runtime: &LitestreamRuntimePaths,
    trusted_binary_sha256s: &[String],
    expected_database_path: &Path,
    shutdown: &AtomicBool,
) -> Result<(), RuntimeFailure> {
    let record = read_pid_record(runtime)
        .map_err(|_| RuntimeFailure::new(RelationalBackupErrorCode::ControlUnavailable, false))?;
    let Some(record) = record else {
        return match fs::symlink_metadata(runtime.socket()) {
            Ok(_) => Err(RuntimeFailure::new(
                RelationalBackupErrorCode::ControlUnavailable,
                false,
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(RuntimeFailure::new(
                RelationalBackupErrorCode::ControlUnavailable,
                true,
            )),
        };
    };
    let expected_database = utf8_path(expected_database_path)
        .map_err(|_| RuntimeFailure::new(RelationalBackupErrorCode::ConfigurationInvalid, false))?;
    if !trusted_binary_sha256s.contains(&record.executable_sha256)
        || record.config != runtime.config().to_string_lossy()
        || record.socket != runtime.socket().to_string_lossy()
        || record.database != expected_database
    {
        return Err(RuntimeFailure::new(
            RelationalBackupErrorCode::ControlUnavailable,
            false,
        ));
    }
    let config = read_private_bounded_config(runtime.config())
        .map_err(|_| RuntimeFailure::new(RelationalBackupErrorCode::ControlUnavailable, false))?;
    if sha256_hex(&config) != record.config_sha256 {
        return Err(RuntimeFailure::new(
            RelationalBackupErrorCode::ControlUnavailable,
            false,
        ));
    }
    if !process_exists(record.pid) {
        remove_socket_if_present(runtime).map_err(|_| {
            RuntimeFailure::new(RelationalBackupErrorCode::ControlUnavailable, true)
        })?;
        remove_pid_record_if_owned(runtime, record.pid);
        return Ok(());
    }
    if !process_matches_record(&record) {
        return Err(RuntimeFailure::new(
            RelationalBackupErrorCode::ControlUnavailable,
            false,
        ));
    }
    terminate_stale_process_group(&record, shutdown);
    if process_exists(record.pid) {
        return Err(RuntimeFailure::new(
            RelationalBackupErrorCode::ControlUnavailable,
            false,
        ));
    }
    remove_socket_if_present(runtime)
        .map_err(|_| RuntimeFailure::new(RelationalBackupErrorCode::ControlUnavailable, true))?;
    remove_pid_record_if_owned(runtime, record.pid);
    Ok(())
}

fn read_private_bounded_config(path: &Path) -> std::io::Result<Vec<u8>> {
    read_private_bounded_regular_file(path, MAX_LITESTREAM_CONFIG_BYTES)
}

fn read_private_bounded_regular_file(path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
    #[cfg(not(unix))]
    {
        let path_metadata = fs::symlink_metadata(path)?;
        if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
            return Err(std::io::Error::other("invalid private runtime file"));
        }
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Err(std::io::Error::other("invalid private runtime file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(std::io::Error::other("private runtime file is not private"));
        }
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(max_bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(std::io::Error::other("private runtime file is oversized"));
    }
    Ok(bytes)
}

struct LitestreamRuntimeOwnership {
    _file: fs::File,
}

fn acquire_runtime_ownership(
    runtime: &LitestreamRuntimePaths,
) -> Result<LitestreamRuntimeOwnership, RuntimeFailure> {
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options
        .open(runtime.ownership_lock())
        .map_err(|_| RuntimeFailure::new(RelationalBackupErrorCode::ControlUnavailable, true))?;
    let metadata = file
        .metadata()
        .map_err(|_| RuntimeFailure::new(RelationalBackupErrorCode::ControlUnavailable, true))?;
    if !metadata.is_file() {
        return Err(RuntimeFailure::new(
            RelationalBackupErrorCode::ControlUnavailable,
            false,
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| {
                RuntimeFailure::new(RelationalBackupErrorCode::ControlUnavailable, true)
            })?;
    }
    match file.try_lock() {
        Ok(()) => Ok(LitestreamRuntimeOwnership { _file: file }),
        Err(fs::TryLockError::WouldBlock) => Err(RuntimeFailure::new(
            RelationalBackupErrorCode::ControlUnavailable,
            true,
        )),
        Err(fs::TryLockError::Error(_)) => Err(RuntimeFailure::new(
            RelationalBackupErrorCode::ControlUnavailable,
            true,
        )),
    }
}

fn cleanup_owned_runtime(
    _ownership: &LitestreamRuntimeOwnership,
    runtime: &LitestreamRuntimePaths,
    expected_pid: u32,
    remove_socket: bool,
) {
    if remove_socket && remove_socket_if_present(runtime).is_err() {
        return;
    }
    remove_pid_record_if_owned(runtime, expected_pid);
}

fn process_matches_record(record: &LitestreamPidRecord) -> bool {
    let Ok(output) = Command::new("ps")
        .args(["-ww", "-p", &record.pid.to_string(), "-o", "command="])
        .output()
    else {
        return false;
    };
    let command = String::from_utf8_lossy(&output.stdout);
    output.status.success()
        && process_group_matches(record.pid)
        && command.contains(&record.executable)
        && command.contains("replicate")
        && command.contains("-config")
        && command.contains(&record.config)
}

#[cfg(unix)]
fn process_group_matches(pid: u32) -> bool {
    let Ok(output) = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pgid="])
        .output()
    else {
        return false;
    };
    output.status.success()
        && String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u32>()
            == Ok(pid)
}

#[cfg(not(unix))]
fn process_group_matches(_pid: u32) -> bool {
    true
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: signal zero only checks process existence.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn process_exists(pid: u32) -> bool {
    Command::new("ps")
        .args(["-p", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(unix)]
fn terminate_process_group(child: &mut impl RuntimeChild, timeout: Duration) {
    if child.try_wait().ok().flatten().is_some() {
        let _ = child.wait();
        return;
    }
    let Ok(pid) = i32::try_from(child.id()) else {
        let _ = child.kill();
        let _ = child.wait();
        return;
    };
    // SAFETY: launch created a process group whose ID is the child PID.
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            let _ = child.wait();
            return;
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    // SAFETY: this is still the child-owned process group.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    let _ = child.wait();
}

#[cfg(not(unix))]
fn terminate_process_group(child: &mut impl RuntimeChild, _timeout: Duration) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn kill_process_group(child: &mut impl RuntimeChild) {
    if child.try_wait().ok().flatten().is_some() {
        let _ = child.wait();
        return;
    }
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: launch created a process group whose ID is the child PID.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    } else {
        let _ = child.kill();
    }
    let _ = child.wait();
}

#[cfg(not(unix))]
fn kill_process_group(child: &mut impl RuntimeChild) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn terminate_stale_process_group(record: &LitestreamPidRecord, shutdown: &AtomicBool) {
    let Ok(pid) = i32::try_from(record.pid) else {
        return;
    };
    // SAFETY: the caller matched the PID record to the running process.
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    let deadline = Instant::now() + DAEMON_SHUTDOWN_TIMEOUT;
    while Instant::now() < deadline {
        if !process_exists(record.pid) {
            return;
        }
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    if !process_matches_record(record) {
        return;
    }
    // SAFETY: the verified process group did not exit after SIGTERM.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    let deadline = Instant::now() + STALE_KILL_WAIT_TIMEOUT;
    while Instant::now() < deadline {
        if !process_exists(record.pid) {
            return;
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
}

#[cfg(not(unix))]
fn terminate_stale_process_group(_record: &LitestreamPidRecord, _shutdown: &AtomicBool) {}

fn remove_pid_record_if_owned(runtime: &LitestreamRuntimePaths, expected_pid: u32) {
    let Ok(Some(record)) = read_pid_record(runtime) else {
        return;
    };
    if record.pid == expected_pid {
        let _ = fs::remove_file(runtime.pid());
    }
}

fn remove_socket_if_present(runtime: &LitestreamRuntimePaths) -> std::io::Result<()> {
    match fs::remove_file(runtime.socket()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension("tmp");
    reject_symlink_or_non_file(path)?;
    remove_regular_temporary(&temporary)?;
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&temporary)?;
    use std::io::Write;
    file.write_all(bytes)?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    drop(file);
    fs::rename(temporary, path)?;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("private file has no parent directory"))?;
    fs::File::open(parent)?.sync_all()
}

fn reject_symlink_or_non_file(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => Ok(()),
        Ok(_) => Err(std::io::Error::other(
            "private runtime file is not a regular file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_regular_temporary(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => {
            fs::remove_file(path)
        }
        Ok(_) => Err(std::io::Error::other(
            "private runtime temporary is not a regular file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn utf8_path(path: &Path) -> std::io::Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other("path is not valid UTF-8"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use tempfile::TempDir;

    use super::*;
    use crate::{
        backup::{
            domain::{
                BackupSetId, R2AccountId, R2BucketName, R2Jurisdiction, R2Target, ReplicaEpochId,
            },
            litestream::LitestreamTxid,
        },
        database::{Database, DatabasePaths, SaveOffsiteBackupConfigInput},
    };

    const ACCOUNT_ID: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn daemon_image_is_trapped_before_selected_code_and_stray_continue_cannot_release_it() {
        use std::os::unix::fs::PermissionsExt;

        let root = TempDir::new().expect("temporary runtime");
        let config = root.path().join("ls.yml");
        let selected_code_marker = root.path().join("selected-code-ran");
        let inherited_credential_copy = root.path().join("inherited-credential-copy");
        let selected_executable = root.path().join("selected-executable");
        fs::write(&config, b"").expect("config");
        fs::write(
            &selected_executable,
            format!(
                "#!/bin/sh\n/bin/cat > \"{}\"\n/usr/bin/touch \"{}\"\n",
                inherited_credential_copy.display(),
                selected_code_marker.display()
            ),
        )
        .expect("selected executable");
        fs::set_permissions(&selected_executable, fs::Permissions::from_mode(0o700))
            .expect("selected executable permissions");
        let (credential_reader, mut credential_writer) =
            UnixStream::pair().expect("credential stream");
        let mut child =
            spawn_litestream_traced(&selected_executable, &config, credential_reader.into())
                .expect("traced spawn");
        credential_writer
            .write_all(b"fake-r2-credential")
            .expect("credential payload");
        credential_writer
            .shutdown(Shutdown::Write)
            .expect("credential EOF");

        // A pathname-racing same-user process can send job-control signals,
        // but only the tracing parent can release the exec trap.
        assert_eq!(
            unsafe { libc::kill(i32::try_from(child.id()).expect("PID"), libc::SIGCONT) },
            0
        );
        thread::sleep(Duration::from_millis(50));
        assert!(child.try_wait().expect("child status").is_none());
        assert!(!selected_code_marker.exists());
        assert!(!inherited_credential_copy.exists());

        drop(credential_writer);
        child.resume().expect("resume verified image");
        assert!(child.wait().expect("child exit").success());
        assert!(selected_code_marker.exists());
        assert_eq!(
            fs::read(&inherited_credential_copy).expect("inherited credential copy"),
            b"fake-r2-credential"
        );
    }

    enum StartPlan {
        Failure(RuntimeFailure),
        Daemon {
            exited: Arc<AtomicBool>,
            sync: Result<SyncResult, RuntimeFailure>,
        },
    }

    struct FakeRuntimeFactory {
        plans: Mutex<VecDeque<StartPlan>>,
        sweep_plans: Mutex<VecDeque<Result<(), RuntimeFailure>>>,
        starts: AtomicUsize,
        shutdowns: Arc<AtomicUsize>,
        sweeps: AtomicUsize,
    }

    impl FakeRuntimeFactory {
        fn new(plans: impl IntoIterator<Item = StartPlan>) -> Arc<Self> {
            Arc::new(Self {
                plans: Mutex::new(plans.into_iter().collect()),
                sweep_plans: Mutex::new(VecDeque::new()),
                starts: AtomicUsize::new(0),
                shutdowns: Arc::new(AtomicUsize::new(0)),
                sweeps: AtomicUsize::new(0),
            })
        }

        fn queue_sweeps(&self, plans: impl IntoIterator<Item = Result<(), RuntimeFailure>>) {
            self.sweep_plans
                .lock()
                .expect("fake sweep plans")
                .extend(plans);
        }
    }

    impl RuntimeFactory for FakeRuntimeFactory {
        fn sweep_stale(&self, _shutdown: Arc<AtomicBool>) -> Result<(), RuntimeFailure> {
            self.sweeps.fetch_add(1, Ordering::SeqCst);
            self.sweep_plans
                .lock()
                .expect("fake sweep plans")
                .pop_front()
                .unwrap_or(Ok(()))
        }

        fn start(
            &self,
            _config: &OffsiteBackupConfig,
            _shutdown: Arc<AtomicBool>,
        ) -> Result<Box<dyn ManagedLitestream>, RuntimeFailure> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            match self
                .plans
                .lock()
                .expect("fake start plans")
                .pop_front()
                .expect("fake start plan")
            {
                StartPlan::Failure(failure) => Err(failure),
                StartPlan::Daemon { exited, sync } => Ok(Box::new(FakeManagedLitestream {
                    exited,
                    sync,
                    shutdowns: Arc::clone(&self.shutdowns),
                })),
            }
        }
    }

    struct FakeManagedLitestream {
        exited: Arc<AtomicBool>,
        sync: Result<SyncResult, RuntimeFailure>,
        shutdowns: Arc<AtomicUsize>,
    }

    impl ManagedLitestream for FakeManagedLitestream {
        fn has_exited(&mut self) -> Result<bool, RuntimeFailure> {
            Ok(self.exited.load(Ordering::SeqCst))
        }

        fn checkpoint_control(&self) -> Arc<dyn CheckpointControl> {
            Arc::new(FakeCheckpointControl {
                sync: self.sync.clone(),
            })
        }

        fn sync_remote(&self, _timeout: Duration) -> Result<SyncResult, RuntimeFailure> {
            self.sync.clone()
        }

        fn shutdown(&mut self) {
            self.shutdowns.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct FakeCheckpointControl {
        sync: Result<SyncResult, RuntimeFailure>,
    }

    impl CheckpointControl for FakeCheckpointControl {
        fn sync_local(&self, _timeout: Duration) -> Result<SyncResult, RuntimeFailure> {
            self.sync.clone()
        }

        fn sync_remote(&self, _timeout: Duration) -> Result<SyncResult, RuntimeFailure> {
            self.sync.clone()
        }
    }

    struct BlockingStartFactory {
        started: AtomicBool,
        returned: AtomicBool,
        syncs: Arc<AtomicUsize>,
        graceful_shutdowns: Arc<AtomicUsize>,
        aborts: Arc<AtomicUsize>,
    }

    impl BlockingStartFactory {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                started: AtomicBool::new(false),
                returned: AtomicBool::new(false),
                syncs: Arc::new(AtomicUsize::new(0)),
                graceful_shutdowns: Arc::new(AtomicUsize::new(0)),
                aborts: Arc::new(AtomicUsize::new(0)),
            })
        }
    }

    impl RuntimeFactory for BlockingStartFactory {
        fn start(
            &self,
            _config: &OffsiteBackupConfig,
            cancellation: Arc<AtomicBool>,
        ) -> Result<Box<dyn ManagedLitestream>, RuntimeFailure> {
            self.started.store(true, Ordering::Release);
            while !cancellation.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(2));
            }
            self.returned.store(true, Ordering::Release);
            Ok(Box::new(ReturnedAfterCancellation {
                syncs: Arc::clone(&self.syncs),
                graceful_shutdowns: Arc::clone(&self.graceful_shutdowns),
                aborts: Arc::clone(&self.aborts),
            }))
        }
    }

    struct ReturnedAfterCancellation {
        syncs: Arc<AtomicUsize>,
        graceful_shutdowns: Arc<AtomicUsize>,
        aborts: Arc<AtomicUsize>,
    }

    impl ManagedLitestream for ReturnedAfterCancellation {
        fn has_exited(&mut self) -> Result<bool, RuntimeFailure> {
            Ok(false)
        }

        fn checkpoint_control(&self) -> Arc<dyn CheckpointControl> {
            Arc::new(CountingCheckpointControl {
                syncs: Arc::clone(&self.syncs),
            })
        }

        fn sync_remote(&self, _timeout: Duration) -> Result<SyncResult, RuntimeFailure> {
            self.syncs.fetch_add(1, Ordering::SeqCst);
            Ok(remote_sync())
        }

        fn shutdown(&mut self) {
            self.graceful_shutdowns.fetch_add(1, Ordering::SeqCst);
        }

        fn abort(&mut self) {
            self.aborts.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct CountingCheckpointControl {
        syncs: Arc<AtomicUsize>,
    }

    impl CheckpointControl for CountingCheckpointControl {
        fn sync_local(&self, _timeout: Duration) -> Result<SyncResult, RuntimeFailure> {
            self.syncs.fetch_add(1, Ordering::SeqCst);
            Ok(remote_sync())
        }

        fn sync_remote(&self, _timeout: Duration) -> Result<SyncResult, RuntimeFailure> {
            self.syncs.fetch_add(1, Ordering::SeqCst);
            Ok(remote_sync())
        }
    }

    fn schedule() -> WorkerSchedule {
        WorkerSchedule {
            supervisor_poll_interval: Duration::from_millis(5),
            config_refresh_interval: Duration::from_secs(60),
            remote_status_interval: Duration::from_millis(5),
            restart_policy: RestartPolicy {
                base: Duration::from_millis(5),
                maximum: Duration::from_millis(20),
            },
        }
    }

    fn remote_sync() -> SyncResult {
        SyncResult {
            database_path: PathBuf::from("/tmp/kosh.sqlite3"),
            txid: LitestreamTxid::from_local(7),
            replica_txid: Some(LitestreamTxid::from_local(7)),
            duration_ms: 1,
        }
    }

    fn target() -> R2Target {
        R2Target {
            account_id: R2AccountId::parse(ACCOUNT_ID).expect("account"),
            jurisdiction: R2Jurisdiction::Default,
            bucket: R2BucketName::parse("kosh-local").expect("bucket"),
        }
    }

    fn configured_database(enabled: bool) -> (TempDir, Database) {
        let root = tempfile::tempdir().expect("temporary Litestream database");
        let database =
            Database::initialize(DatabasePaths::new(root.path())).expect("Litestream database");
        database
            .client()
            .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
                expected_revision: 0,
                backup_set_id: BackupSetId::new(),
                replica_epoch_id: ReplicaEpochId::new(),
                enabled,
                target: target(),
                now_ms: 10,
            })
            .expect("backup config");
        (root, database)
    }

    fn wait_until(timeout: Duration, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if condition() {
                return;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert!(condition(), "condition did not become true before timeout");
    }

    #[test]
    fn disabled_configuration_never_starts_litestream() {
        let (_root, database) = configured_database(false);
        let factory = FakeRuntimeFactory::new([]);
        let service = LitestreamRuntimeService::start_with_parts(
            database.client(),
            factory.clone(),
            schedule(),
        );
        wait_until(Duration::from_secs(1), || {
            factory.sweeps.load(Ordering::SeqCst) == 1
        });
        thread::sleep(Duration::from_millis(25));

        assert_eq!(factory.starts.load(Ordering::SeqCst), 0);
        assert_eq!(service.status(), RelationalBackupStatus::default());
        assert!(database.client().diagnostics().is_ok());
    }

    #[test]
    fn disabled_configuration_retries_and_reports_stale_sweep_failure_until_recovery() {
        let (_root, database) = configured_database(false);
        let factory = FakeRuntimeFactory::new([]);
        factory.queue_sweeps([
            Err(RuntimeFailure::new(
                RelationalBackupErrorCode::ControlUnavailable,
                false,
            )),
            Ok(()),
        ]);
        let service = LitestreamRuntimeService::start_with_parts(
            database.client(),
            factory.clone(),
            WorkerSchedule {
                restart_policy: RestartPolicy {
                    base: Duration::from_millis(50),
                    maximum: Duration::from_millis(50),
                },
                ..schedule()
            },
        );
        wait_until(Duration::from_secs(1), || {
            service.status().phase == RelationalBackupPhase::Blocked
        });

        assert_eq!(factory.sweeps.load(Ordering::SeqCst), 1);
        assert_eq!(
            service.status().last_error_code,
            Some(RelationalBackupErrorCode::ControlUnavailable)
        );
        assert_eq!(factory.starts.load(Ordering::SeqCst), 0);
        assert!(database.client().diagnostics().is_ok());

        wait_until(Duration::from_secs(1), || {
            factory.sweeps.load(Ordering::SeqCst) == 2
                && service.status().phase == RelationalBackupPhase::Off
        });
        assert_eq!(factory.starts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn enabled_configuration_reaches_running_with_canonical_bounded_status() {
        let (_root, database) = configured_database(true);
        let factory = FakeRuntimeFactory::new([StartPlan::Daemon {
            exited: Arc::new(AtomicBool::new(false)),
            sync: Ok(remote_sync()),
        }]);
        let service = LitestreamRuntimeService::start_with_parts(
            database.client(),
            factory.clone(),
            schedule(),
        );
        wait_until(Duration::from_secs(1), || {
            service.status().phase == RelationalBackupPhase::Running
        });

        let status = service.status();
        assert_eq!(
            status.latest_local_txid.as_deref(),
            Some("0000000000000007")
        );
        assert_eq!(status.latest_remote_txid, status.latest_local_txid);
        assert!(status.last_remote_confirmed_at_ms.is_some());
        assert_eq!(status.restart_count, 0);
        assert_eq!(status.last_error_code, None);
        assert_eq!(factory.starts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn checkpoint_handle_uses_only_the_current_daemon_control_and_closes_on_shutdown() {
        let (_root, database) = configured_database(true);
        let config = database
            .client()
            .load_enabled_offsite_backup_config()
            .expect("load config")
            .expect("enabled config");
        let factory = FakeRuntimeFactory::new([StartPlan::Daemon {
            exited: Arc::new(AtomicBool::new(false)),
            sync: Ok(remote_sync()),
        }]);
        let service =
            LitestreamRuntimeService::start_with_parts(database.client(), factory, schedule());
        let unbound_handle = service.checkpoint_handle();
        let handle = unbound_handle.bind(&config);
        wait_until(Duration::from_secs(1), || {
            service.status().phase == RelationalBackupPhase::Running
        });

        let mut mismatched_config = config.clone();
        mismatched_config.revision += 1;
        assert_eq!(
            unbound_handle
                .bind(&mismatched_config)
                .sync_remote()
                .expect_err("mismatched daemon lineage"),
            CheckpointErrorCode::LitestreamUnavailable
        );
        assert_eq!(
            handle
                .sync_local_with_timeout(Duration::from_millis(1_500))
                .expect("local checkpoint sync"),
            LitestreamTxid::from_local(7)
        );
        assert_eq!(
            handle.sync_remote().expect("remote checkpoint sync"),
            remote_sync()
        );
        service.shutdown();
        assert_eq!(
            handle
                .sync_local_with_timeout(Duration::from_millis(20))
                .expect_err("closed handle"),
            CheckpointErrorCode::WorkerUnavailable
        );
    }

    #[test]
    fn checkpoint_handle_enforces_an_outer_fence_deadline() {
        struct SlowControl;

        impl CheckpointControl for SlowControl {
            fn sync_local(&self, _timeout: Duration) -> Result<SyncResult, RuntimeFailure> {
                thread::sleep(Duration::from_millis(100));
                Ok(remote_sync())
            }

            fn sync_remote(&self, _timeout: Duration) -> Result<SyncResult, RuntimeFailure> {
                thread::sleep(Duration::from_millis(100));
                Ok(remote_sync())
            }
        }

        let (_root, database) = configured_database(true);
        let config = database
            .client()
            .load_enabled_offsite_backup_config()
            .expect("load config")
            .expect("enabled config");
        let handle = LitestreamCheckpointHandle {
            shutdown: Arc::new(AtomicBool::new(false)),
            status: Arc::new(Mutex::new(RelationalBackupStatus::default())),
            control: Arc::new(Mutex::new(Some(ActiveCheckpointControl {
                config: config.clone(),
                control: Arc::new(SlowControl),
            }))),
        }
        .bind(&config);
        assert_eq!(
            handle
                .sync_local_with_timeout(Duration::from_millis(20))
                .expect_err("fence deadline"),
            CheckpointErrorCode::FenceTimeout
        );
    }

    #[test]
    fn crashed_children_restart_without_blocking_database_writes() {
        let (_root, database) = configured_database(true);
        let first_exited = Arc::new(AtomicBool::new(false));
        let factory = FakeRuntimeFactory::new([
            StartPlan::Daemon {
                exited: Arc::clone(&first_exited),
                sync: Ok(remote_sync()),
            },
            StartPlan::Daemon {
                exited: Arc::new(AtomicBool::new(false)),
                sync: Ok(remote_sync()),
            },
        ]);
        let service = LitestreamRuntimeService::start_with_parts(
            database.client(),
            factory.clone(),
            schedule(),
        );
        wait_until(Duration::from_secs(1), || {
            service.status().phase == RelationalBackupPhase::Running
        });
        first_exited.store(true, Ordering::SeqCst);
        wait_until(Duration::from_secs(1), || {
            factory.starts.load(Ordering::SeqCst) >= 2
                && service.status().phase == RelationalBackupPhase::Running
        });

        assert!(database.client().diagnostics().is_ok());
        assert!(factory.shutdowns.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn transient_start_failure_is_bounded_and_recovers() {
        let (_root, database) = configured_database(true);
        let factory = FakeRuntimeFactory::new([
            StartPlan::Failure(RuntimeFailure::new(
                RelationalBackupErrorCode::KeychainUnavailable,
                true,
            )),
            StartPlan::Daemon {
                exited: Arc::new(AtomicBool::new(false)),
                sync: Ok(remote_sync()),
            },
        ]);
        let service = LitestreamRuntimeService::start_with_parts(
            database.client(),
            factory.clone(),
            schedule(),
        );
        wait_until(Duration::from_secs(1), || {
            factory.starts.load(Ordering::SeqCst) >= 2
                && service.status().phase == RelationalBackupPhase::Running
        });

        assert_eq!(service.status().last_error_code, None);
        assert!(database.client().diagnostics().is_ok());
    }

    #[test]
    fn remote_sync_failure_degrades_backup_without_stopping_local_work() {
        let (_root, database) = configured_database(true);
        let factory = FakeRuntimeFactory::new([StartPlan::Daemon {
            exited: Arc::new(AtomicBool::new(false)),
            sync: Err(RuntimeFailure::new(
                RelationalBackupErrorCode::RemoteSyncFailed,
                true,
            )),
        }]);
        let service =
            LitestreamRuntimeService::start_with_parts(database.client(), factory, schedule());
        wait_until(Duration::from_secs(1), || {
            service.status().phase == RelationalBackupPhase::Degraded
        });

        assert_eq!(
            service.status().last_error_code,
            Some(RelationalBackupErrorCode::RemoteSyncFailed)
        );
        assert!(database.client().diagnostics().is_ok());
    }

    #[test]
    fn fatal_start_failure_blocks_only_that_configuration_revision() {
        let (_root, database) = configured_database(true);
        let factory = FakeRuntimeFactory::new([StartPlan::Failure(RuntimeFailure::new(
            RelationalBackupErrorCode::ConfigurationInvalid,
            false,
        ))]);
        let service = LitestreamRuntimeService::start_with_parts(
            database.client(),
            factory.clone(),
            schedule(),
        );
        wait_until(Duration::from_secs(1), || {
            service.status().phase == RelationalBackupPhase::Blocked
        });
        thread::sleep(Duration::from_millis(30));

        assert_eq!(factory.starts.load(Ordering::SeqCst), 1);
        assert_eq!(
            service.status().last_error_code,
            Some(RelationalBackupErrorCode::ConfigurationInvalid)
        );
        assert!(database.client().diagnostics().is_ok());
    }

    #[test]
    fn disabling_a_running_configuration_gracefully_stops_the_child() {
        let (_root, database) = configured_database(true);
        let factory = FakeRuntimeFactory::new([StartPlan::Daemon {
            exited: Arc::new(AtomicBool::new(false)),
            sync: Ok(remote_sync()),
        }]);
        let service = LitestreamRuntimeService::start_with_parts(
            database.client(),
            factory.clone(),
            schedule(),
        );
        wait_until(Duration::from_secs(1), || {
            service.status().phase == RelationalBackupPhase::Running
        });
        let current = database
            .client()
            .load_offsite_backup_config()
            .expect("load config")
            .expect("stored config");
        database
            .client()
            .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
                expected_revision: current.revision,
                backup_set_id: current.backup_set_id,
                replica_epoch_id: current.replica_epoch_id,
                enabled: false,
                target: current.target,
                now_ms: 20,
            })
            .expect("disable config");
        service.reload_configuration();
        wait_until(Duration::from_secs(1), || {
            service.status().phase == RelationalBackupPhase::Off
        });

        assert_eq!(factory.shutdowns.load(Ordering::SeqCst), 1);
        assert!(database.client().diagnostics().is_ok());
    }

    #[test]
    fn disabling_during_start_cancels_and_aborts_the_stale_generation_before_sync() {
        let (_root, database) = configured_database(true);
        let factory = BlockingStartFactory::new();
        let service = LitestreamRuntimeService::start_with_parts(
            database.client(),
            factory.clone(),
            schedule(),
        );
        wait_until(Duration::from_secs(1), || {
            factory.started.load(Ordering::Acquire)
        });
        let current = database
            .client()
            .load_offsite_backup_config()
            .expect("load config")
            .expect("stored config");
        database
            .client()
            .save_offsite_backup_config(SaveOffsiteBackupConfigInput {
                expected_revision: current.revision,
                backup_set_id: current.backup_set_id,
                replica_epoch_id: current.replica_epoch_id,
                enabled: false,
                target: current.target,
                now_ms: 20,
            })
            .expect("disable config");

        service.reload_configuration();
        wait_until(Duration::from_secs(1), || {
            factory.returned.load(Ordering::Acquire)
                && factory.aborts.load(Ordering::SeqCst) == 1
                && service.status().phase == RelationalBackupPhase::Off
        });

        assert_eq!(factory.syncs.load(Ordering::SeqCst), 0);
        assert_eq!(factory.graceful_shutdowns.load(Ordering::SeqCst), 0);
        assert!(database.client().diagnostics().is_ok());
    }

    #[test]
    fn service_shutdown_invokes_the_daemons_graceful_final_sync_path_once() {
        let (_root, database) = configured_database(true);
        let factory = FakeRuntimeFactory::new([StartPlan::Daemon {
            exited: Arc::new(AtomicBool::new(false)),
            sync: Ok(remote_sync()),
        }]);
        let service = LitestreamRuntimeService::start_with_parts(
            database.client(),
            factory.clone(),
            schedule(),
        );
        wait_until(Duration::from_secs(1), || {
            service.status().phase == RelationalBackupPhase::Running
        });

        service.shutdown();
        service.shutdown();
        assert_eq!(factory.shutdowns.load(Ordering::SeqCst), 1);
        assert!(database.client().diagnostics().is_ok());
    }

    #[test]
    fn shutdown_interrupts_a_stalled_start_operation_without_waiting_for_its_timeout() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let (completed_tx, completed_rx) = mpsc::sync_channel(1);
        let worker_shutdown = Arc::clone(&shutdown);
        let worker_started = Arc::clone(&started);
        let worker_release = Arc::clone(&release);
        let outer = thread::spawn(move || {
            let result = run_start_operation_interruptibly(
                worker_shutdown,
                "kosh-test-stalled-owner-request",
                move |_| {
                    worker_started.store(true, Ordering::Release);
                    while !worker_release.load(Ordering::Acquire) {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Ok(())
                },
            );
            completed_tx.send(result).expect("completion receiver");
        });
        wait_until(Duration::from_secs(1), || started.load(Ordering::Acquire));

        let quit_started = Instant::now();
        shutdown.store(true, Ordering::Release);
        assert_eq!(
            completed_rx
                .recv_timeout(Duration::from_millis(500))
                .expect("interruptible operation result"),
            Err(cancelled_start())
        );
        assert!(
            quit_started.elapsed() < Duration::from_millis(500),
            "shutdown waited for the stalled owner request"
        );

        release.store(true, Ordering::Release);
        outer.join().expect("interruptible operation caller");
    }

    #[test]
    fn restart_backoff_is_exponential_and_capped() {
        let policy = RestartPolicy {
            base: Duration::from_secs(1),
            maximum: Duration::from_secs(5),
        };
        assert_eq!(policy.delay(1), Duration::from_secs(1));
        assert_eq!(policy.delay(2), Duration::from_secs(2));
        assert_eq!(policy.delay(3), Duration::from_secs(4));
        assert_eq!(policy.delay(100), Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn child_exit_during_startup_is_retryable() {
        let root = tempfile::tempdir().expect("temporary runtime root");
        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");
        let mut child = Command::new("/usr/bin/true")
            .spawn()
            .expect("short-lived child");
        let shutdown = AtomicBool::new(false);

        let failure = wait_for_control_socket(&mut child, &runtime, &shutdown)
            .expect_err("startup must fail");

        assert_eq!(
            failure,
            RuntimeFailure::new(RelationalBackupErrorCode::LaunchFailed, true)
        );
    }

    #[cfg(unix)]
    #[test]
    fn socket_readiness_is_interruptible_during_shutdown() {
        use std::os::unix::process::CommandExt;

        let root = tempfile::tempdir().expect("temporary runtime root");
        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");
        let mut command = Command::new("/bin/sleep");
        command.arg("30").process_group(0);
        let mut child = command.spawn().expect("stalled launcher child");
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(75));
            worker_shutdown.store(true, Ordering::Release);
        });

        let quit_started = Instant::now();
        let failure = wait_for_control_socket(&mut child, &runtime, &shutdown)
            .expect_err("shutdown must interrupt socket readiness");
        terminate_process_group(&mut child, DAEMON_LAUNCH_CLEANUP_TIMEOUT);
        canceller.join().expect("shutdown signal");

        assert_eq!(failure, cancelled_start());
        assert!(
            quit_started.elapsed() < Duration::from_millis(500),
            "shutdown waited for the socket startup timeout"
        );
        assert!(child.try_wait().expect("child status").is_some());
    }

    #[test]
    fn credential_failures_map_without_exposing_secret_material() {
        assert_eq!(
            map_credential_start_error(CredentialError::Missing),
            RuntimeFailure::new(RelationalBackupErrorCode::CredentialsMissing, true)
        );
        assert_eq!(
            map_credential_start_error(CredentialError::CorruptPayload),
            RuntimeFailure::new(RelationalBackupErrorCode::ConfigurationInvalid, false)
        );
        let json = serde_json::to_string(&RelationalBackupStatus {
            phase: RelationalBackupPhase::WaitingForCredentials,
            last_error_code: Some(RelationalBackupErrorCode::CredentialsMissing),
            ..RelationalBackupStatus::default()
        })
        .expect("bounded status");
        assert!(!json.contains(ACCOUNT_ID));
        assert_eq!(
            map_writer_identity_start_error(WriterIdentityError::Unavailable),
            RuntimeFailure::new(RelationalBackupErrorCode::WriterIdentityUnavailable, true)
        );
        assert_eq!(
            map_writer_identity_start_error(WriterIdentityError::Invalid),
            RuntimeFailure::new(RelationalBackupErrorCode::ConfigurationInvalid, false)
        );
    }

    #[test]
    fn remote_owner_failures_block_conflicts_and_retry_transport_outages() {
        assert_eq!(
            map_remote_owner_error(RemoteOwnerError::Cancelled),
            cancelled_start()
        );
        assert_eq!(
            map_remote_owner_error(RemoteOwnerError::Conflict),
            RuntimeFailure::new(RelationalBackupErrorCode::RemoteOwnerConflict, false)
        );
        assert_eq!(
            map_remote_owner_error(RemoteOwnerError::Invalid),
            RuntimeFailure::new(RelationalBackupErrorCode::RemoteOwnerInvalid, false)
        );
        assert_eq!(
            map_remote_owner_error(RemoteOwnerError::Store(
                crate::backup::object_store::ObjectStoreError::new(ObjectStoreErrorCode::Timeout,),
            )),
            RuntimeFailure::new(RelationalBackupErrorCode::RemoteSyncFailed, true)
        );
    }

    #[test]
    fn credentials_are_released_only_after_durable_ownership_record() {
        struct OwnershipProbe<'a> {
            runtime: &'a LitestreamRuntimePaths,
            expected_pid: u32,
            released: Vec<u8>,
        }

        impl Write for OwnershipProbe<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                let record = read_pid_record(self.runtime)?.ok_or_else(|| {
                    std::io::Error::other("credentials preceded the ownership record")
                })?;
                if record.pid != self.expected_pid {
                    return Err(std::io::Error::other(
                        "credentials observed the wrong ownership record",
                    ));
                }
                self.released.extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let root = tempfile::tempdir().expect("temporary credential-gate root");
        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");
        runtime.write_config("safe config").expect("write config");
        let binary = root.path().join("litestream");
        let database = root.path().join("kosh.sqlite3");
        fs::write(&binary, b"binary").expect("binary fixture");
        fs::write(&database, b"database").expect("database fixture");
        let identity = LaunchIdentity {
            backup_set_id: BackupSetId::new().to_string(),
            replica_epoch_id: ReplicaEpochId::new().to_string(),
            config_sha256: sha256_hex(b"safe config"),
            executable_sha256: sha256_hex(b"binary"),
        };
        let credentials = super::super::credentials::R2Credentials::new(
            "0123456789abcdef0123456789abcdef",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .expect("credentials");
        let mut credential_gate = OwnershipProbe {
            runtime: &runtime,
            expected_pid: u32::MAX,
            released: Vec::new(),
        };

        write_pid_record_then_release_credentials(
            &runtime,
            u32::MAX,
            &binary,
            &database,
            &identity,
            &mut credential_gate,
            &credentials,
        )
        .expect("durable ownership and credential release");

        assert_eq!(
            String::from_utf8(credential_gate.released).expect("credential profile"),
            "[default]\n\
             aws_access_key_id = 0123456789abcdef0123456789abcdef\n\
             aws_secret_access_key = \
             0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n"
        );
        assert_eq!(
            read_pid_record(&runtime)
                .expect("PID record")
                .expect("owned PID record")
                .config_sha256,
            identity.config_sha256
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn actual_litestream_starts_from_the_verified_image_and_credential_pipe() {
        use std::os::unix::fs::PermissionsExt;

        let staged_resources = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/release");
        if !staged_resources.join("bin/litestream").is_file() {
            return;
        }
        let root = tempfile::tempdir().expect("direct launch root");
        let resource_dir = root.path().join("packaged-resources");
        fs::create_dir_all(resource_dir.join("bin")).expect("packaged binary directory");
        fs::create_dir_all(resource_dir.join("release")).expect("packaged manifest directory");
        fs::hard_link(
            staged_resources.join("bin/litestream"),
            resource_dir.join("bin/litestream"),
        )
        .expect("packaged Litestream link");
        let source_manifest_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/sidecars/litestream-v1.json");
        let mut release_manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(source_manifest_path).expect("source Litestream manifest"),
        )
        .expect("source Litestream JSON");
        let universal = release_manifest["binary"]["universal"].clone();
        release_manifest["stagedBinary"] = serde_json::json!({
            "sha256": universal["sha256"],
            "size": universal["size"],
        });
        release_manifest["verification"]["architectureChecks"] = serde_json::json!([]);
        fs::write(
            resource_dir.join("release/litestream.json"),
            serde_json::to_vec(&release_manifest).expect("release Litestream JSON"),
        )
        .expect("packaged Litestream manifest");
        let database_path = root.path().join("kosh.sqlite3");
        let database =
            rusqlite::Connection::open(&database_path).expect("Litestream SQLite fixture");
        database
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE evidence (value TEXT NOT NULL);
                 INSERT INTO evidence (value) VALUES ('credential-gated');",
            )
            .expect("Litestream SQLite contents");
        drop(database);

        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");
        let rendered = LitestreamConfig {
            database_path: &database_path,
            runtime: &runtime,
            bucket: "kosh-credential-gate",
            replica_path: "kosh/credential-gate/litestream",
            endpoint: "https://127.0.0.1:1",
        }
        .render()
        .expect("Litestream config");
        runtime.write_config(&rendered).expect("write config");
        let binary = VerifiedLitestreamBinary::resolve(&resource_dir)
            .expect("verified Litestream")
            .stage_immutable(&runtime)
            .expect("immutable Litestream");
        let immutable_path = binary.path().to_owned();
        let ownership = acquire_runtime_ownership(&runtime).expect("runtime ownership");
        let credentials = super::super::credentials::R2Credentials::new(
            "0123456789abcdef0123456789abcdef",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .expect("credentials");
        let shutdown = AtomicBool::new(false);
        let mut daemon = SystemManagedLitestream::launch(
            binary,
            runtime.clone(),
            ownership,
            database_path,
            BackupSetId::new().to_string(),
            ReplicaEpochId::new().to_string(),
            sha256_hex(rendered.as_bytes()),
            credentials,
            &shutdown,
        )
        .expect("credential-gated Litestream launch");

        assert!(control_socket_is_private(runtime.socket()).expect("private control socket"));
        daemon.shutdown();
        assert!(!runtime.pid().exists());

        let immutable_directory = immutable_path.parent().expect("immutable directory");
        let directory = fs::File::open(immutable_directory).expect("immutable directory");
        super::super::litestream::clear_user_immutable(&directory)
            .expect("thaw immutable directory");
        fs::set_permissions(immutable_directory, fs::Permissions::from_mode(0o700))
            .expect("thaw directory permissions");
        let file = fs::File::open(&immutable_path).expect("immutable binary");
        super::super::litestream::clear_user_immutable(&file).expect("thaw immutable binary");
        fs::set_permissions(immutable_path, fs::Permissions::from_mode(0o700))
            .expect("thaw binary permissions");
    }

    #[cfg(unix)]
    #[test]
    fn pid_records_are_private_and_dead_owned_runtime_is_swept() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("temporary runtime root");
        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");
        runtime.write_config("safe config").expect("write config");
        let binary = root.path().join("litestream");
        let database = root.path().join("kosh.sqlite3");
        fs::write(&binary, b"binary").expect("binary fixture");
        fs::write(&database, b"database").expect("database fixture");
        write_pid_record(
            &runtime,
            u32::MAX,
            &binary,
            &database,
            &LaunchIdentity {
                backup_set_id: BackupSetId::new().to_string(),
                replica_epoch_id: ReplicaEpochId::new().to_string(),
                config_sha256: sha256_hex(b"safe config"),
                executable_sha256: sha256_hex(b"binary"),
            },
        )
        .expect("PID record");
        assert_eq!(
            fs::metadata(runtime.pid())
                .expect("PID metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let ownership = acquire_runtime_ownership(&runtime).expect("runtime ownership");
        sweep_stale_runtime(
            &ownership,
            &runtime,
            &[sha256_hex(b"binary")],
            &database,
            &AtomicBool::new(false),
        )
        .expect("sweep dead owned runtime");
        assert!(!runtime.pid().exists());
    }

    #[cfg(unix)]
    #[test]
    fn stale_sweep_does_not_require_current_binary_availability() {
        use std::os::unix::{fs::PermissionsExt, process::CommandExt};

        let root = tempfile::tempdir().expect("temporary runtime root");
        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");
        runtime.write_config("safe config").expect("write config");
        let binary = root.path().join("previous-litestream");
        let database = root.path().join("kosh.sqlite3");
        let script = b"#!/bin/sh\nwhile :; do /bin/sleep 1; done\n";
        fs::write(&binary, script).expect("previous binary fixture");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))
            .expect("executable previous binary");
        fs::write(&database, b"database").expect("database fixture");
        let mut command = Command::new(&binary);
        command
            .arg("replicate")
            .arg("-config")
            .arg(runtime.config())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        let child = command.spawn().expect("previous Litestream child");
        let pid = child.id();
        let trusted_cleanup_sha256s =
            VerifiedLitestreamBinary::trusted_cleanup_sha256s().expect("embedded cleanup registry");
        write_pid_record(
            &runtime,
            pid,
            &binary,
            &database,
            &LaunchIdentity {
                backup_set_id: BackupSetId::new().to_string(),
                replica_epoch_id: ReplicaEpochId::new().to_string(),
                config_sha256: sha256_hex(b"safe config"),
                executable_sha256: trusted_cleanup_sha256s[0].clone(),
            },
        )
        .expect("PID record");
        let record = read_pid_record(&runtime)
            .expect("read PID record")
            .expect("previous PID record");
        wait_until(Duration::from_secs(1), || process_matches_record(&record));
        let (reaped_tx, reaped_rx) = mpsc::sync_channel(1);
        let waiter = thread::spawn(move || {
            reaped_tx
                .send(child.wait_with_output().map(|output| output.status))
                .expect("reaped status receiver");
        });
        let factory = SystemRuntimeFactory {
            data_root: root.path().to_owned(),
            database_path: database.clone(),
            resource_dir: None,
            credentials: MacOsKeychainCredentialStore,
            writer_identity: MacOsInstallationWriterIdentity::new(root.path().to_owned()),
        };

        factory
            .sweep_stale(Arc::new(AtomicBool::new(false)))
            .expect("cleanup from embedded registry");

        reaped_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("reaped previous child")
            .expect("wait for previous child");
        waiter.join().expect("previous child waiter");
        assert!(!runtime.pid().exists());
        assert!(!process_exists(pid));
    }

    #[cfg(unix)]
    #[test]
    fn stale_config_reads_reject_special_symlinked_and_oversized_files_without_opening_them() {
        use std::{
            ffi::CString,
            os::unix::{
                ffi::OsStrExt,
                fs::{symlink, PermissionsExt},
            },
        };

        let root = tempfile::tempdir().expect("temporary runtime root");
        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.write_config("safe config").expect("write config");
        assert_eq!(
            read_private_bounded_config(runtime.config()).expect("private bounded config"),
            b"safe config"
        );

        fs::write(
            runtime.config(),
            vec![b'x'; MAX_LITESTREAM_CONFIG_BYTES as usize + 1],
        )
        .expect("oversized regular file");
        fs::set_permissions(runtime.config(), fs::Permissions::from_mode(0o600))
            .expect("private oversized file");
        assert!(read_private_bounded_config(runtime.config()).is_err());

        fs::remove_file(runtime.config()).expect("remove oversized config");
        symlink("/dev/null", runtime.config()).expect("symlink to device");
        assert!(read_private_bounded_config(runtime.config()).is_err());

        fs::remove_file(runtime.config()).expect("remove symlink");
        let fifo = CString::new(runtime.config().as_os_str().as_bytes()).expect("FIFO path");
        // SAFETY: `fifo` is a live, NUL-terminated path and `mkfifo` does not
        // retain its pointer after returning.
        let result = unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) };
        assert_eq!(result, 0, "create FIFO fixture");
        assert!(
            read_private_bounded_config(runtime.config()).is_err(),
            "the FIFO must be rejected from metadata before any blocking open"
        );
    }

    #[cfg(unix)]
    #[test]
    fn pid_record_reads_reject_special_symlinked_and_oversized_files_without_blocking() {
        use std::{
            ffi::CString,
            os::unix::{
                ffi::OsStrExt,
                fs::{symlink, PermissionsExt},
            },
        };

        let root = tempfile::tempdir().expect("temporary runtime root");
        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");

        fs::write(runtime.pid(), vec![b'x'; MAX_PID_RECORD_BYTES as usize + 1])
            .expect("oversized PID record");
        fs::set_permissions(runtime.pid(), fs::Permissions::from_mode(0o600))
            .expect("private oversized PID record");
        assert!(read_pid_record(&runtime).is_err());

        fs::remove_file(runtime.pid()).expect("remove oversized PID record");
        symlink("/dev/null", runtime.pid()).expect("PID symlink to device");
        assert!(read_pid_record(&runtime).is_err());

        fs::remove_file(runtime.pid()).expect("remove PID symlink");
        let fifo = CString::new(runtime.pid().as_os_str().as_bytes()).expect("FIFO path");
        // SAFETY: `fifo` is a live, NUL-terminated path and `mkfifo` does not
        // retain its pointer after returning.
        let result = unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) };
        assert_eq!(result, 0, "create PID FIFO fixture");
        assert!(
            read_pid_record(&runtime).is_err(),
            "the nonblocking descriptor must reject the FIFO"
        );
    }

    #[cfg(unix)]
    #[test]
    fn runtime_ownership_serializes_cleanup_before_a_replacement_generation() {
        use std::os::{fd::AsRawFd, unix::fs::PermissionsExt};

        let root = tempfile::tempdir().expect("temporary runtime root");
        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");
        runtime.write_config("safe config").expect("write config");
        let binary = root.path().join("litestream");
        let database = root.path().join("kosh.sqlite3");
        fs::write(&binary, b"binary").expect("binary fixture");
        fs::write(&database, b"database").expect("database fixture");
        let identity = || LaunchIdentity {
            backup_set_id: BackupSetId::new().to_string(),
            replica_epoch_id: ReplicaEpochId::new().to_string(),
            config_sha256: sha256_hex(b"safe config"),
            executable_sha256: sha256_hex(b"binary"),
        };
        let exiting_pid = u32::MAX - 1;
        let replacement_pid = u32::MAX;
        let exiting = acquire_runtime_ownership(&runtime).expect("exiting generation ownership");
        // SAFETY: F_GETFD only reads flags from this live, owned descriptor.
        let descriptor_flags = unsafe { libc::fcntl(exiting._file.as_raw_fd(), libc::F_GETFD) };
        assert!(descriptor_flags >= 0);
        assert_ne!(descriptor_flags & libc::FD_CLOEXEC, 0);
        assert_eq!(
            fs::metadata(runtime.ownership_lock())
                .expect("ownership metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        write_pid_record(&runtime, exiting_pid, &binary, &database, &identity())
            .expect("exiting PID record");
        fs::write(runtime.socket(), b"exiting socket").expect("exiting socket");

        assert_eq!(
            acquire_runtime_ownership(&runtime).err(),
            Some(RuntimeFailure::new(
                RelationalBackupErrorCode::ControlUnavailable,
                true,
            ))
        );
        cleanup_owned_runtime(&exiting, &runtime, exiting_pid, true);
        assert!(!runtime.pid().exists());
        assert!(!runtime.socket().exists());
        assert_eq!(
            acquire_runtime_ownership(&runtime).err(),
            Some(RuntimeFailure::new(
                RelationalBackupErrorCode::ControlUnavailable,
                true,
            )),
            "cleanup retains ownership until both generation artifacts are gone"
        );

        drop(exiting);
        let replacement =
            acquire_runtime_ownership(&runtime).expect("replacement generation ownership");
        write_pid_record(&runtime, replacement_pid, &binary, &database, &identity())
            .expect("replacement PID record");
        fs::write(runtime.socket(), b"replacement socket").expect("replacement socket");

        assert_eq!(
            read_pid_record(&runtime)
                .expect("replacement record")
                .expect("replacement ownership")
                .pid,
            replacement_pid
        );
        assert_eq!(
            fs::read(runtime.socket()).expect("replacement socket"),
            b"replacement socket"
        );
        cleanup_owned_runtime(&replacement, &runtime, replacement_pid, true);
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_retains_the_pid_record_until_the_socket_is_removed() {
        let root = tempfile::tempdir().expect("temporary runtime root");
        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");
        runtime.write_config("safe config").expect("write config");
        let binary = root.path().join("litestream");
        let database = root.path().join("kosh.sqlite3");
        fs::write(&binary, b"binary").expect("binary fixture");
        fs::write(&database, b"database").expect("database fixture");
        let pid = u32::MAX;
        write_pid_record(
            &runtime,
            pid,
            &binary,
            &database,
            &LaunchIdentity {
                backup_set_id: BackupSetId::new().to_string(),
                replica_epoch_id: ReplicaEpochId::new().to_string(),
                config_sha256: sha256_hex(b"safe config"),
                executable_sha256: sha256_hex(b"binary"),
            },
        )
        .expect("PID record");
        fs::create_dir(runtime.socket()).expect("unremovable socket fixture");
        let ownership = acquire_runtime_ownership(&runtime).expect("runtime ownership");

        cleanup_owned_runtime(&ownership, &runtime, pid, true);

        assert!(
            runtime.pid().exists(),
            "the ownership record must survive a failed socket unlink"
        );
        fs::remove_dir(runtime.socket()).expect("remove socket fixture");
        cleanup_owned_runtime(&ownership, &runtime, pid, true);
        assert!(!runtime.pid().exists());
    }

    #[cfg(unix)]
    #[test]
    fn runtime_ownership_rejects_a_symlinked_lock_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temporary runtime root");
        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");
        let target = root.path().join("must-not-change");
        fs::write(&target, b"retained").expect("lock symlink target");
        symlink(&target, runtime.ownership_lock()).expect("ownership lock symlink");

        assert_eq!(
            acquire_runtime_ownership(&runtime).err(),
            Some(RuntimeFailure::new(
                RelationalBackupErrorCode::ControlUnavailable,
                true,
            ))
        );
        assert_eq!(fs::read(&target).expect("unchanged target"), b"retained");
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_stale_runtime_is_retryable_instead_of_reported_absent() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("temporary runtime root");
        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");
        fs::set_permissions(runtime.directory(), fs::Permissions::from_mode(0o000))
            .expect("make runtime unreadable");

        let result = stale_runtime_residue_exists(&runtime);

        fs::set_permissions(runtime.directory(), fs::Permissions::from_mode(0o700))
            .expect("restore runtime permissions");
        assert_eq!(
            result,
            Err(RuntimeFailure::new(
                RelationalBackupErrorCode::ControlUnavailable,
                true,
            ))
        );
    }

    #[cfg(unix)]
    #[test]
    fn pid_record_writes_reject_symlinked_temporaries_without_touching_targets() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temporary runtime root");
        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");
        runtime.write_config("safe config").expect("write config");
        let binary = root.path().join("litestream");
        let database = root.path().join("kosh.sqlite3");
        fs::write(&binary, b"binary").expect("binary fixture");
        fs::write(&database, b"database").expect("database fixture");
        let target = root.path().join("must-not-change");
        fs::write(&target, b"retained").expect("symlink target");
        let temporary = runtime.pid().with_extension("tmp");
        symlink(&target, &temporary).expect("PID temporary symlink");

        assert!(write_pid_record(
            &runtime,
            u32::MAX,
            &binary,
            &database,
            &LaunchIdentity {
                backup_set_id: BackupSetId::new().to_string(),
                replica_epoch_id: ReplicaEpochId::new().to_string(),
                config_sha256: sha256_hex(b"safe config"),
                executable_sha256: sha256_hex(b"binary"),
            },
        )
        .is_err());
        assert_eq!(fs::read(&target).expect("unchanged target"), b"retained");
        assert!(fs::symlink_metadata(&temporary)
            .expect("rejected symlink")
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn stale_ownership_matching_reads_untruncated_command_lines() {
        use std::os::unix::process::CommandExt;

        let root = tempfile::tempdir().expect("temporary long command root");
        let long_directory = root.path().join("x".repeat(180));
        fs::create_dir(&long_directory).expect("long command directory");
        let config = long_directory.join("ls.yml");
        let mut child = Command::new("/bin/sh");
        child
            .arg("-c")
            .arg("while :; do /bin/sleep 1; done")
            .arg("/bin/sh")
            .arg("replicate")
            .arg("-config")
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        let mut child = child.spawn().expect("long-command child");
        let record = LitestreamPidRecord {
            format_version: PID_RECORD_FORMAT_VERSION,
            pid: child.id(),
            executable: "/bin/sh".into(),
            executable_sha256: sha256_hex(b"/bin/sh fixture"),
            config: config.to_string_lossy().into_owned(),
            socket: long_directory
                .join("ls.sock")
                .to_string_lossy()
                .into_owned(),
            database: long_directory
                .join("kosh.sqlite3")
                .to_string_lossy()
                .into_owned(),
            backup_set_id: BackupSetId::new().to_string(),
            replica_epoch_id: ReplicaEpochId::new().to_string(),
            config_sha256: sha256_hex(b"safe config"),
        };

        let matched = process_matches_record(&record);
        terminate_process_group(&mut child, Duration::from_millis(100));

        assert!(
            matched,
            "wide process inspection must preserve the final config argument"
        );
    }

    #[cfg(unix)]
    #[test]
    fn stale_daemon_is_reaped_after_its_verified_binary_upgrades_and_relocates() {
        use std::os::unix::{fs::PermissionsExt, process::CommandExt};

        let root = tempfile::tempdir().expect("temporary relocated app root");
        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");
        runtime.write_config("safe config").expect("write config");
        let old_app = root.path().join("Downloads").join("Kosh.app");
        let old_binary = old_app.join("Contents/Resources/bin/litestream");
        fs::create_dir_all(old_binary.parent().expect("old binary parent"))
            .expect("old bundle resources");
        let script = b"#!/bin/sh\nwhile :; do /bin/sleep 1; done\n";
        fs::write(&old_binary, script).expect("old Litestream fixture");
        fs::set_permissions(&old_binary, fs::Permissions::from_mode(0o700))
            .expect("executable Litestream fixture");
        let database = root.path().join("kosh.sqlite3");
        fs::write(&database, b"database").expect("database fixture");
        let mut command = Command::new(&old_binary);
        command
            .arg("replicate")
            .arg("-config")
            .arg(runtime.config())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        let child = command.spawn().expect("orphaned Litestream fixture");
        let pid = child.id();
        write_pid_record(
            &runtime,
            pid,
            &old_binary,
            &database,
            &LaunchIdentity {
                backup_set_id: BackupSetId::new().to_string(),
                replica_epoch_id: ReplicaEpochId::new().to_string(),
                config_sha256: sha256_hex(b"safe config"),
                executable_sha256: sha256_hex(script),
            },
        )
        .expect("PID record");
        let identity_deadline = Instant::now() + Duration::from_secs(1);
        while !process_matches_record(
            &read_pid_record(&runtime)
                .expect("read PID record")
                .expect("recorded Litestream fixture"),
        ) {
            assert!(
                Instant::now() < identity_deadline,
                "spawned Litestream fixture must expose its recorded process identity"
            );
            thread::sleep(PROCESS_POLL_INTERVAL);
        }
        let (reaped_tx, reaped_rx) = mpsc::sync_channel(1);
        let waiter = thread::spawn(move || {
            reaped_tx
                .send(child.wait_with_output().map(|output| output.status))
                .expect("reaped status receiver");
        });

        let new_app = root.path().join("Applications").join("Kosh.app");
        fs::create_dir_all(new_app.parent().expect("new app parent"))
            .expect("Applications directory");
        fs::rename(&old_app, &new_app).expect("relocate app bundle");
        let ownership = acquire_runtime_ownership(&runtime).expect("runtime ownership");

        assert_eq!(
            sweep_stale_runtime(
                &ownership,
                &runtime,
                &[sha256_hex(b"different binary")],
                &database,
                &AtomicBool::new(false),
            ),
            Err(RuntimeFailure::new(
                RelationalBackupErrorCode::ControlUnavailable,
                false,
            )),
            "a different pinned binary must not authorize process termination"
        );
        assert!(process_exists(pid));
        sweep_stale_runtime(
            &ownership,
            &runtime,
            &[sha256_hex(b"new binary"), sha256_hex(script)],
            &database,
            &AtomicBool::new(false),
        )
        .expect("reap relocated previously pinned daemon");

        assert!(!runtime.pid().exists());
        reaped_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("reaped child status")
            .expect("wait for child");
        waiter.join().expect("child waiter");
        assert!(!process_exists(pid));
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_escalates_stale_daemon_cleanup_without_waiting_for_graceful_timeout() {
        use std::os::unix::{fs::PermissionsExt, process::CommandExt};

        let root = tempfile::tempdir().expect("temporary stale daemon root");
        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");
        runtime.write_config("safe config").expect("write config");
        let binary = root.path().join("litestream");
        let ready = root.path().join("ready");
        let script =
            b"#!/bin/sh\ntrap '' TERM\ntouch \"$KOSH_TEST_READY\"\nwhile :; do /bin/sleep 1; done\n";
        fs::write(&binary, script).expect("stale Litestream fixture");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))
            .expect("executable stale fixture");
        let database = root.path().join("kosh.sqlite3");
        fs::write(&database, b"database").expect("database fixture");
        let mut command = Command::new(&binary);
        command
            .arg("replicate")
            .arg("-config")
            .arg(runtime.config())
            .env("KOSH_TEST_READY", &ready)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        let child = command.spawn().expect("stale Litestream child");
        let pid = child.id();
        write_pid_record(
            &runtime,
            pid,
            &binary,
            &database,
            &LaunchIdentity {
                backup_set_id: BackupSetId::new().to_string(),
                replica_epoch_id: ReplicaEpochId::new().to_string(),
                config_sha256: sha256_hex(b"safe config"),
                executable_sha256: sha256_hex(script),
            },
        )
        .expect("PID record");
        wait_until(Duration::from_secs(1), || ready.exists());
        let record = read_pid_record(&runtime)
            .expect("read PID record")
            .expect("stale PID record");
        wait_until(Duration::from_secs(1), || process_matches_record(&record));

        let (reaped_tx, reaped_rx) = mpsc::sync_channel(1);
        let waiter = thread::spawn(move || {
            reaped_tx
                .send(child.wait_with_output().map(|output| output.status))
                .expect("reaped status receiver");
        });
        let ownership = acquire_runtime_ownership(&runtime).expect("runtime ownership");
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let worker_runtime = runtime.clone();
        let worker_database = database.clone();
        let (completed_tx, completed_rx) = mpsc::sync_channel(1);
        let sweeper = thread::spawn(move || {
            completed_tx
                .send(sweep_stale_runtime(
                    &ownership,
                    &worker_runtime,
                    &[sha256_hex(script)],
                    &worker_database,
                    &worker_shutdown,
                ))
                .expect("stale sweep result receiver");
        });
        thread::sleep(Duration::from_millis(75));

        let quit_started = Instant::now();
        shutdown.store(true, Ordering::Release);
        completed_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("shutdown-aware stale sweep")
            .expect("stale daemon cleanup");
        assert!(
            quit_started.elapsed() < Duration::from_secs(1),
            "shutdown waited for the stale daemon graceful timeout"
        );

        sweeper.join().expect("stale sweeper");
        reaped_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("reaped stale child")
            .expect("wait for stale child");
        waiter.join().expect("stale child waiter");
        assert!(!runtime.pid().exists());
        assert!(!process_exists(pid));
    }

    #[test]
    fn unowned_socket_is_never_deleted_during_stale_sweep() {
        let root = tempfile::tempdir().expect("temporary runtime root");
        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");
        fs::write(runtime.socket(), b"not owned").expect("unowned socket");
        let ownership = acquire_runtime_ownership(&runtime).expect("runtime ownership");

        assert_eq!(
            sweep_stale_runtime(
                &ownership,
                &runtime,
                &[sha256_hex(b"binary")],
                &root.path().join("kosh.sqlite3"),
                &AtomicBool::new(false),
            ),
            Err(RuntimeFailure::new(
                RelationalBackupErrorCode::ControlUnavailable,
                false,
            ))
        );
        assert!(runtime.socket().exists());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_unowned_socket_symlinks_also_fail_closed() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temporary runtime root");
        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");
        symlink(root.path().join("missing-target"), runtime.socket())
            .expect("dangling socket symlink");
        let ownership = acquire_runtime_ownership(&runtime).expect("runtime ownership");

        assert_eq!(
            sweep_stale_runtime(
                &ownership,
                &runtime,
                &[sha256_hex(b"binary")],
                &root.path().join("kosh.sqlite3"),
                &AtomicBool::new(false),
            ),
            Err(RuntimeFailure::new(
                RelationalBackupErrorCode::ControlUnavailable,
                false,
            ))
        );
        assert!(fs::symlink_metadata(runtime.socket())
            .expect("socket symlink retained")
            .file_type()
            .is_symlink());
    }
}

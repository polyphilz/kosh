use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::database::{DatabaseClient, OffsiteBackupConfig};

use super::{
    credentials::{CredentialError, CredentialStore, MacOsKeychainCredentialStore},
    litestream::{
        configure_credentials_environment, CommandLitestreamControl, LitestreamConfig,
        LitestreamError, LitestreamRuntimePaths, SyncResult, SystemCommandExecutor,
        VerifiedLitestreamBinary,
    },
};

const SUPERVISOR_POLL_INTERVAL: Duration = Duration::from_secs(1);
const CONFIG_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const REMOTE_STATUS_INTERVAL: Duration = Duration::from_secs(30);
const RESTART_BASE_DELAY: Duration = Duration::from_secs(1);
const RESTART_MAX_DELAY: Duration = Duration::from_secs(5 * 60);
const CONTROL_REMOTE_TIMEOUT_SECONDS: u64 = 30;
// Health confirmation must never consume Litestream's full remote timeout on
// the supervisor thread; application shutdown may wait behind this probe.
const STATUS_SYNC_TIMEOUT: Duration = Duration::from_secs(2);
const DAEMON_STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const DAEMON_LAUNCH_CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
const DAEMON_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(35);
const STALE_KILL_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);
const PID_RECORD_FORMAT_VERSION: u32 = 1;
const MAX_PID_RECORD_BYTES: u64 = 16 * 1024;
const LITESTREAM_LAUNCHER_ARG: &str = "--kosh-litestream-launcher";
const LITESTREAM_ACTIVATION_TOKEN: &[u8] = b"kosh-litestream-activate-v1\n";
const LITESTREAM_LAUNCHER_USAGE_EXIT: i32 = 64;
const LITESTREAM_LAUNCHER_IO_EXIT: i32 = 74;

pub(crate) fn run_launcher_if_requested() -> Option<i32> {
    let request = match parse_launcher_request(std::env::args_os())? {
        Ok(request) => request,
        Err(()) => return Some(LITESTREAM_LAUNCHER_USAGE_EXIT),
    };
    if await_activation(&mut std::io::stdin()).is_err() {
        return Some(LITESTREAM_LAUNCHER_IO_EXIT);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = Command::new(request.binary)
            .arg("replicate")
            .arg("-config")
            .arg(request.config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .exec();
        let _ = error;
        Some(LITESTREAM_LAUNCHER_IO_EXIT)
    }
    #[cfg(not(unix))]
    {
        let _ = request;
        Some(LITESTREAM_LAUNCHER_IO_EXIT)
    }
}

struct LauncherRequest {
    binary: PathBuf,
    config: PathBuf,
}

fn parse_launcher_request(
    arguments: impl IntoIterator<Item = OsString>,
) -> Option<Result<LauncherRequest, ()>> {
    let mut arguments = arguments.into_iter();
    let _executable = arguments.next();
    let operation = arguments.next()?;
    if operation != OsStr::new(LITESTREAM_LAUNCHER_ARG) {
        return None;
    }
    let Some(binary) = arguments.next().map(PathBuf::from) else {
        return Some(Err(()));
    };
    let Some(replicate) = arguments.next() else {
        return Some(Err(()));
    };
    let Some(config_flag) = arguments.next() else {
        return Some(Err(()));
    };
    let Some(config) = arguments.next().map(PathBuf::from) else {
        return Some(Err(()));
    };
    if arguments.next().is_some()
        || replicate != OsStr::new("replicate")
        || config_flag != OsStr::new("-config")
        || !binary.is_absolute()
        || !config.is_absolute()
    {
        return Some(Err(()));
    }
    Some(Ok(LauncherRequest { binary, config }))
}

fn await_activation(reader: &mut impl Read) -> std::io::Result<()> {
    let mut activation = [0_u8; LITESTREAM_ACTIVATION_TOKEN.len()];
    reader.read_exact(&mut activation)?;
    if activation != LITESTREAM_ACTIVATION_TOKEN {
        return Err(std::io::Error::other("invalid Litestream activation token"));
    }
    Ok(())
}

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
    ReloadConfiguration,
    Shutdown,
}

pub(crate) struct LitestreamRuntimeService {
    sender: mpsc::Sender<SupervisorSignal>,
    shutdown: Arc<AtomicBool>,
    status: Arc<Mutex<RelationalBackupStatus>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl LitestreamRuntimeService {
    pub(crate) fn start(
        database: DatabaseClient,
        data_root: PathBuf,
        database_path: PathBuf,
        resource_dir: Option<PathBuf>,
    ) -> Self {
        Self::start_with_parts(
            database,
            Arc::new(SystemRuntimeFactory {
                data_root,
                database_path,
                resource_dir,
                credentials: MacOsKeychainCredentialStore,
            }),
            WorkerSchedule::production(),
        )
    }

    pub(crate) fn disabled() -> Self {
        let (sender, receiver) = mpsc::channel();
        drop(receiver);
        Self {
            sender,
            shutdown: Arc::new(AtomicBool::new(true)),
            status: Arc::new(Mutex::new(RelationalBackupStatus::default())),
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
        let status = Arc::new(Mutex::new(RelationalBackupStatus::default()));
        let worker_shutdown = Arc::clone(&shutdown);
        let worker_status = Arc::clone(&status);
        let spawned = thread::Builder::new()
            .name("kosh-litestream-supervisor".into())
            .spawn(move || {
                supervisor_worker(
                    database,
                    factory,
                    receiver,
                    worker_shutdown,
                    worker_status,
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
            status,
            worker: Mutex::new(worker),
        };
        service.reload_configuration();
        service
    }

    pub(crate) fn status(&self) -> RelationalBackupStatus {
        lock_status(&self.status).clone()
    }

    pub(crate) fn reload_configuration(&self) {
        self.signal(SupervisorSignal::ReloadConfiguration);
    }

    pub(crate) fn shutdown(&self) {
        if self.shutdown.swap(true, Ordering::SeqCst) {
            return;
        }
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
    fn sweep_stale(&self) -> Result<(), RuntimeFailure> {
        Ok(())
    }

    fn start(
        &self,
        config: &OffsiteBackupConfig,
    ) -> Result<Box<dyn ManagedLitestream>, RuntimeFailure>;
}

trait ManagedLitestream: Send {
    fn has_exited(&mut self) -> Result<bool, RuntimeFailure>;
    fn sync_remote(&self, timeout: Duration) -> Result<SyncResult, RuntimeFailure>;
    fn shutdown(&mut self);
}

fn supervisor_worker(
    database: DatabaseClient,
    factory: Arc<dyn RuntimeFactory>,
    receiver: mpsc::Receiver<SupervisorSignal>,
    shutdown: Arc<AtomicBool>,
    status: Arc<Mutex<RelationalBackupStatus>>,
    schedule: WorkerSchedule,
) {
    let mut current_config: Option<OffsiteBackupConfig> = None;
    let mut daemon: Option<Box<dyn ManagedLitestream>> = None;
    let mut blocked_revision: Option<i64> = None;
    let mut restart_count = 0_u32;
    let mut next_start = Instant::now();
    let mut next_config_refresh = Instant::now();
    let mut next_remote_status = Instant::now();
    let mut force_reload = true;

    if let Err(failure) = factory.sweep_stale() {
        update_failure_status(&status, failure, restart_count);
        log::warn!(
            "could not safely sweep a stale Litestream runtime: {:?}",
            failure.code
        );
    }

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
            Some(SupervisorSignal::ReloadConfiguration) => {
                force_reload = true;
                blocked_revision = None;
                restart_count = 0;
                next_start = Instant::now();
            }
            Some(SupervisorSignal::Shutdown) | None => {}
        }

        let now = Instant::now();
        if force_reload || now >= next_config_refresh {
            match database.load_enabled_offsite_backup_config() {
                Ok(config) => {
                    let previous_revision = current_config.as_ref().map(|value| value.revision);
                    let next_revision = config.as_ref().map(|value| value.revision);
                    if force_reload || previous_revision != next_revision {
                        shutdown_daemon(&mut daemon);
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
                    shutdown_daemon(&mut daemon);
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
            lock_status(&status).phase = RelationalBackupPhase::Starting;
            match factory.start(config) {
                Ok(started) => {
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
                        shutdown_daemon(&mut daemon);
                        blocked_revision = Some(config.revision);
                        update_failure_status(&status, failure, restart_count);
                    }
                }
                next_remote_status = Instant::now() + schedule.remote_status_interval;
            }
        }
    }

    shutdown_daemon(&mut daemon);
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
        | RelationalBackupErrorCode::WorkerUnavailable => RelationalBackupPhase::Unavailable,
        _ if failure.retryable => RelationalBackupPhase::Degraded,
        _ => RelationalBackupPhase::Blocked,
    };
    current.restart_count = restart_count;
    current.last_error_code = Some(failure.code);
}

fn shutdown_daemon(daemon: &mut Option<Box<dyn ManagedLitestream>>) {
    if let Some(mut daemon) = daemon.take() {
        daemon.shutdown();
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

struct SystemRuntimeFactory<C> {
    data_root: PathBuf,
    database_path: PathBuf,
    resource_dir: Option<PathBuf>,
    credentials: C,
}

impl<C: CredentialStore> RuntimeFactory for SystemRuntimeFactory<C> {
    fn sweep_stale(&self) -> Result<(), RuntimeFailure> {
        let runtime =
            LitestreamRuntimePaths::new(&self.data_root).map_err(map_litestream_start_error)?;
        if fs::symlink_metadata(runtime.pid()).is_err()
            && fs::symlink_metadata(runtime.socket()).is_err()
        {
            return Ok(());
        }
        let resource_dir = self.resource_dir.as_deref().ok_or_else(|| {
            RuntimeFailure::new(RelationalBackupErrorCode::BinaryUnavailable, false)
        })?;
        let binary =
            VerifiedLitestreamBinary::resolve(resource_dir).map_err(map_litestream_start_error)?;
        runtime.prepare().map_err(map_litestream_start_error)?;
        sweep_stale_runtime(&runtime, binary.path(), &self.database_path)
    }

    fn start(
        &self,
        config: &OffsiteBackupConfig,
    ) -> Result<Box<dyn ManagedLitestream>, RuntimeFailure> {
        let resource_dir = self.resource_dir.as_deref().ok_or_else(|| {
            RuntimeFailure::new(RelationalBackupErrorCode::BinaryUnavailable, false)
        })?;
        let binary =
            VerifiedLitestreamBinary::resolve(resource_dir).map_err(map_litestream_start_error)?;
        let runtime =
            LitestreamRuntimePaths::new(&self.data_root).map_err(map_litestream_start_error)?;
        runtime.prepare().map_err(map_litestream_start_error)?;
        sweep_stale_runtime(&runtime, binary.path(), &self.database_path)?;

        let credentials = self
            .credentials
            .load(&config.backup_set_id)
            .map_err(map_credential_start_error)?;
        let keyspace = config.target.keyspace(&config.backup_set_id);
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
            binary.path().to_owned(),
            runtime,
            self.database_path.clone(),
            config.backup_set_id.as_str().to_owned(),
            config.replica_epoch_id.as_str().to_owned(),
            config_sha256,
            credentials,
        )?;
        Ok(Box::new(daemon))
    }
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

fn map_litestream_start_error(error: LitestreamError) -> RuntimeFailure {
    match error {
        LitestreamError::PrepareRuntime(_)
        | LitestreamError::WriteConfig(_)
        | LitestreamError::Execute(_) => {
            RuntimeFailure::new(RelationalBackupErrorCode::ControlUnavailable, true)
        }
        LitestreamError::CommandFailed { .. } => {
            RuntimeFailure::new(RelationalBackupErrorCode::RemoteSyncFailed, true)
        }
        LitestreamError::InvalidConfigField(_)
        | LitestreamError::RelativeDatabasePath
        | LitestreamError::NonUtf8RuntimePath
        | LitestreamError::ControlSocketPathTooLong => {
            RuntimeFailure::new(RelationalBackupErrorCode::ConfigurationInvalid, false)
        }
        LitestreamError::InvalidJson(_)
        | LitestreamError::InvalidTxid
        | LitestreamError::InvalidSyncContract
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
        | LitestreamError::UnsafeProtocolPin
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

struct SystemManagedLitestream {
    child: Option<Child>,
    runtime: LitestreamRuntimePaths,
    database_path: PathBuf,
    control: CommandLitestreamControl<SystemCommandExecutor>,
}

impl SystemManagedLitestream {
    #[allow(clippy::too_many_arguments)]
    fn launch(
        binary: PathBuf,
        runtime: LitestreamRuntimePaths,
        database_path: PathBuf,
        backup_set_id: String,
        replica_epoch_id: String,
        config_sha256: String,
        credentials: super::credentials::R2Credentials,
    ) -> Result<Self, RuntimeFailure> {
        let launcher = std::env::current_exe()
            .map_err(|_| RuntimeFailure::new(RelationalBackupErrorCode::LaunchFailed, true))?;
        let mut command = Command::new(&launcher);
        command
            .arg(LITESTREAM_LAUNCHER_ARG)
            .arg(&binary)
            .arg("replicate")
            .arg("-config")
            .arg(runtime.config())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_credentials_environment(&mut command, &credentials);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .map_err(|_| RuntimeFailure::new(RelationalBackupErrorCode::LaunchFailed, true))?;
        let pid = child.id();
        let identity = LaunchIdentity {
            backup_set_id,
            replica_epoch_id,
            config_sha256,
        };
        let ownership = child
            .stdin
            .as_mut()
            .ok_or_else(|| RuntimeFailure::new(RelationalBackupErrorCode::LaunchFailed, true));
        if ownership
            .and_then(|activation| {
                write_pid_record_then_activate(
                    &runtime,
                    pid,
                    &binary,
                    &database_path,
                    &identity,
                    activation,
                )
                .map_err(|_| {
                    RuntimeFailure::new(RelationalBackupErrorCode::ControlUnavailable, true)
                })
            })
            .is_err()
        {
            terminate_process_group(&mut child, DAEMON_LAUNCH_CLEANUP_TIMEOUT);
            remove_pid_record_if_owned(&runtime, pid);
            return Err(RuntimeFailure::new(
                RelationalBackupErrorCode::ControlUnavailable,
                true,
            ));
        }
        child.stdin.take();
        if let Err(failure) = wait_for_control_socket(&mut child, &runtime) {
            terminate_process_group(&mut child, DAEMON_LAUNCH_CLEANUP_TIMEOUT);
            remove_pid_record_if_owned(&runtime, pid);
            remove_socket_if_present(&runtime);
            return Err(failure);
        }
        let control = CommandLitestreamControl::new(
            binary,
            runtime.socket().to_owned(),
            CONTROL_REMOTE_TIMEOUT_SECONDS,
            SystemCommandExecutor,
        );
        Ok(Self {
            child: Some(child),
            runtime,
            database_path,
            control,
        })
    }

    fn cleanup(&self, pid: u32) {
        remove_pid_record_if_owned(&self.runtime, pid);
        remove_socket_if_present(&self.runtime);
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

    fn sync_remote(&self, timeout: Duration) -> Result<SyncResult, RuntimeFailure> {
        self.control
            .sync_remote_with_timeout(&self.database_path, timeout)
            .map_err(map_control_error)
    }

    fn shutdown(&mut self) {
        if let Some(mut child) = self.child.take() {
            let pid = child.id();
            terminate_process_group(&mut child, DAEMON_SHUTDOWN_TIMEOUT);
            self.cleanup(pid);
        }
    }
}

impl Drop for SystemManagedLitestream {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn wait_for_control_socket(
    child: &mut Child,
    runtime: &LitestreamRuntimePaths,
) -> Result<(), RuntimeFailure> {
    let deadline = Instant::now() + DAEMON_STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => {
                return Err(RuntimeFailure::new(
                    RelationalBackupErrorCode::LaunchFailed,
                    false,
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
}

fn write_pid_record_then_activate(
    runtime: &LitestreamRuntimePaths,
    pid: u32,
    binary: &Path,
    database_path: &Path,
    identity: &LaunchIdentity,
    activation: &mut impl Write,
) -> std::io::Result<()> {
    write_pid_record(runtime, pid, binary, database_path, identity)?;
    activation.write_all(LITESTREAM_ACTIVATION_TOKEN)?;
    activation.flush()
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
    let metadata = match fs::symlink_metadata(runtime.pid()) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_PID_RECORD_BYTES
    {
        return Err(std::io::Error::other("invalid Litestream PID record"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(std::io::Error::other(
                "Litestream PID record is not private",
            ));
        }
    }
    let bytes = fs::read(runtime.pid())?;
    let record: LitestreamPidRecord =
        serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
    if record.format_version != PID_RECORD_FORMAT_VERSION {
        return Err(std::io::Error::other("unsupported Litestream PID record"));
    }
    Ok(Some(record))
}

fn sweep_stale_runtime(
    runtime: &LitestreamRuntimePaths,
    expected_binary: &Path,
    expected_database_path: &Path,
) -> Result<(), RuntimeFailure> {
    let record = read_pid_record(runtime)
        .map_err(|_| RuntimeFailure::new(RelationalBackupErrorCode::ControlUnavailable, false))?;
    let Some(record) = record else {
        if runtime.socket().exists() {
            return Err(RuntimeFailure::new(
                RelationalBackupErrorCode::ControlUnavailable,
                false,
            ));
        }
        return Ok(());
    };
    let expected_binary = utf8_path(expected_binary)
        .map_err(|_| RuntimeFailure::new(RelationalBackupErrorCode::ConfigurationInvalid, false))?;
    let expected_database = utf8_path(expected_database_path)
        .map_err(|_| RuntimeFailure::new(RelationalBackupErrorCode::ConfigurationInvalid, false))?;
    if record.executable != expected_binary
        || record.config != runtime.config().to_string_lossy()
        || record.socket != runtime.socket().to_string_lossy()
        || record.database != expected_database
    {
        return Err(RuntimeFailure::new(
            RelationalBackupErrorCode::ControlUnavailable,
            false,
        ));
    }
    let config = fs::read(runtime.config())
        .map_err(|_| RuntimeFailure::new(RelationalBackupErrorCode::ControlUnavailable, false))?;
    if sha256_hex(&config) != record.config_sha256 {
        return Err(RuntimeFailure::new(
            RelationalBackupErrorCode::ControlUnavailable,
            false,
        ));
    }
    if !process_exists(record.pid) {
        remove_pid_record_if_owned(runtime, record.pid);
        remove_socket_if_present(runtime);
        return Ok(());
    }
    if !process_matches_record(&record) {
        return Err(RuntimeFailure::new(
            RelationalBackupErrorCode::ControlUnavailable,
            false,
        ));
    }
    terminate_stale_process_group(&record);
    if process_exists(record.pid) {
        return Err(RuntimeFailure::new(
            RelationalBackupErrorCode::ControlUnavailable,
            false,
        ));
    }
    remove_pid_record_if_owned(runtime, record.pid);
    remove_socket_if_present(runtime);
    Ok(())
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
fn terminate_process_group(child: &mut Child, timeout: Duration) {
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
fn terminate_process_group(child: &mut Child, _timeout: Duration) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn terminate_stale_process_group(record: &LitestreamPidRecord) {
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
fn terminate_stale_process_group(_record: &LitestreamPidRecord) {}

fn remove_pid_record_if_owned(runtime: &LitestreamRuntimePaths, expected_pid: u32) {
    let Ok(Some(record)) = read_pid_record(runtime) else {
        return;
    };
    if record.pid == expected_pid {
        let _ = fs::remove_file(runtime.pid());
    }
}

fn remove_socket_if_present(runtime: &LitestreamRuntimePaths) {
    match fs::remove_file(runtime.socket()) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension("tmp");
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
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

fn utf8_path(path: &Path) -> std::io::Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other("path is not valid UTF-8"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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

    enum StartPlan {
        Failure(RuntimeFailure),
        Daemon {
            exited: Arc<AtomicBool>,
            sync: Result<SyncResult, RuntimeFailure>,
        },
    }

    struct FakeRuntimeFactory {
        plans: Mutex<VecDeque<StartPlan>>,
        starts: AtomicUsize,
        shutdowns: Arc<AtomicUsize>,
        sweeps: AtomicUsize,
        sweep_failure: Option<RuntimeFailure>,
    }

    impl FakeRuntimeFactory {
        fn new(plans: impl IntoIterator<Item = StartPlan>) -> Arc<Self> {
            Arc::new(Self {
                plans: Mutex::new(plans.into_iter().collect()),
                starts: AtomicUsize::new(0),
                shutdowns: Arc::new(AtomicUsize::new(0)),
                sweeps: AtomicUsize::new(0),
                sweep_failure: None,
            })
        }
    }

    impl RuntimeFactory for FakeRuntimeFactory {
        fn sweep_stale(&self) -> Result<(), RuntimeFailure> {
            self.sweeps.fetch_add(1, Ordering::SeqCst);
            self.sweep_failure.map_or(Ok(()), Err)
        }

        fn start(
            &self,
            _config: &OffsiteBackupConfig,
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

        fn sync_remote(&self, _timeout: Duration) -> Result<SyncResult, RuntimeFailure> {
            self.sync.clone()
        }

        fn shutdown(&mut self) {
            self.shutdowns.fetch_add(1, Ordering::SeqCst);
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
    }

    #[test]
    fn launcher_protocol_requires_exact_arguments_and_activation_token() {
        let request = parse_launcher_request([
            OsString::from("/Applications/Kosh.app/Contents/MacOS/kosh"),
            OsString::from(LITESTREAM_LAUNCHER_ARG),
            OsString::from("/Applications/Kosh.app/Contents/Resources/litestream"),
            OsString::from("replicate"),
            OsString::from("-config"),
            OsString::from("/tmp/kosh/run/backup/ls.yml"),
        ])
        .expect("launcher operation")
        .expect("launcher request");
        assert_eq!(
            request.binary,
            PathBuf::from("/Applications/Kosh.app/Contents/Resources/litestream")
        );
        assert_eq!(request.config, PathBuf::from("/tmp/kosh/run/backup/ls.yml"));
        assert!(matches!(
            parse_launcher_request([
                OsString::from("/Applications/Kosh.app/Contents/MacOS/kosh"),
                OsString::from(LITESTREAM_LAUNCHER_ARG),
                OsString::from("relative-litestream"),
                OsString::from("replicate"),
                OsString::from("-config"),
                OsString::from("/tmp/kosh/run/backup/ls.yml"),
            ]),
            Some(Err(()))
        ));
        assert!(await_activation(&mut std::io::Cursor::new(LITESTREAM_ACTIVATION_TOKEN)).is_ok());
        assert!(await_activation(&mut std::io::Cursor::new(b"truncated")).is_err());
        assert!(
            await_activation(&mut std::io::Cursor::new(b"kosh-litestream-activate-v2\n")).is_err()
        );
    }

    #[test]
    fn child_activation_is_emitted_only_after_durable_ownership_record() {
        struct OwnershipProbe<'a> {
            runtime: &'a LitestreamRuntimePaths,
            expected_pid: u32,
            activated: Vec<u8>,
        }

        impl Write for OwnershipProbe<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                let record = read_pid_record(self.runtime)?.ok_or_else(|| {
                    std::io::Error::other("activation preceded the ownership record")
                })?;
                if record.pid != self.expected_pid {
                    return Err(std::io::Error::other(
                        "activation observed the wrong ownership record",
                    ));
                }
                self.activated.extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let root = tempfile::tempdir().expect("temporary activation root");
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
        };
        let mut activation = OwnershipProbe {
            runtime: &runtime,
            expected_pid: u32::MAX,
            activated: Vec::new(),
        };

        write_pid_record_then_activate(
            &runtime,
            u32::MAX,
            &binary,
            &database,
            &identity,
            &mut activation,
        )
        .expect("durable ownership and activation");

        assert_eq!(activation.activated, LITESTREAM_ACTIVATION_TOKEN);
        assert_eq!(
            read_pid_record(&runtime)
                .expect("PID record")
                .expect("owned PID record")
                .config_sha256,
            identity.config_sha256
        );
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

        sweep_stale_runtime(&runtime, &binary, &database).expect("sweep dead owned runtime");
        assert!(!runtime.pid().exists());
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

    #[test]
    fn unowned_socket_is_never_deleted_during_stale_sweep() {
        let root = tempfile::tempdir().expect("temporary runtime root");
        let runtime = LitestreamRuntimePaths::new(root.path()).expect("runtime paths");
        runtime.prepare().expect("prepare runtime");
        fs::write(runtime.socket(), b"not owned").expect("unowned socket");

        assert_eq!(
            sweep_stale_runtime(
                &runtime,
                &root.path().join("litestream"),
                &root.path().join("kosh.sqlite3"),
            ),
            Err(RuntimeFailure::new(
                RelationalBackupErrorCode::ControlUnavailable,
                false,
            ))
        );
        assert!(runtime.socket().exists());
    }
}

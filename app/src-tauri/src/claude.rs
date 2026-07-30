use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc, Arc, Mutex, Weak,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

#[cfg(unix)]
use std::os::unix::{fs::PermissionsExt, process::CommandExt};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, State};

use crate::{
    database::{AppendResearchEventWrite, CreateResearchRunWrite, DatabaseClient},
    embedding_runtime::EmbeddingRuntime,
    research::{
        ClaudeMcpBridge, EphemeralResearchMcpServer, GroundedResearchAnswer,
        ResearchCitationRegistry, ResearchLimits, ResearchQueryEmbedder, ResearchRun,
    },
    runtime::Clock,
    runtime::RuntimeState,
};

pub const RESEARCH_PROCESS_EVENT: &str = "kosh://research-process";

const MAX_PROMPT_BYTES: usize = 64 * 1024;
const MAX_STDOUT_LINE_BYTES: usize = 256 * 1024;
const MAX_STDOUT_MESSAGES_PER_POLL: usize = 256;
const MAX_STALE_WORK_DIRECTORIES_PER_START: usize = 8;
const MAX_STDERR_BYTES: usize = 32 * 1024;
const MAX_STREAM_EVENT_COUNT: usize = 16_384;
const MAX_STREAM_BYTES: usize = 16 * 1024 * 1024;
const MAX_VISIBLE_EVENT_COUNT: usize = 4_096;
const MAX_VISIBLE_EVENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_VISIBLE_TEXT_BYTES: usize = 1024 * 1024;
const MAX_ERROR_BYTES: usize = 2_048;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);
const TERMINATION_GRACE: Duration = Duration::from_millis(500);
const SETUP_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_RUN_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MIN_RUN_TIMEOUT_SECS: u64 = 5;
const MAX_RUN_TIMEOUT_SECS: u64 = 2 * 60 * 60;
const WORK_DIRECTORY_NAME: &str = "research-processes";
const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

const DISALLOWED_BUILTIN_TOOLS: &str = concat!(
    "WebSearch,WebFetch,Bash,Read,Edit,Write,Glob,Grep,NotebookEdit,",
    "Task,TaskOutput,KillShell,AskUserQuestion,EnterPlanMode,ExitPlanMode"
);

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCliDefaults {
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClaudeSetupPhase {
    Ready,
    Missing,
    Unauthenticated,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSetupStatus {
    pub phase: ClaudeSetupPhase,
    pub binary_path: Option<String>,
    pub version: Option<String>,
    pub defaults: ClaudeCliDefaults,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartResearchProcessInput {
    pub run_id: String,
    pub prompt: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BeginResearchProcessInput {
    pub prompt: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartResearchProcessOutput {
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaced_run_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResearchToolActivityPhase {
    Started,
    Finished,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResearchProcessOutcome {
    Succeeded,
    Failed,
    Canceled,
    Replaced,
    TimedOut,
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchProcessEvent {
    pub run_id: String,
    pub sequence: u32,
    #[serde(flatten)]
    pub detail: ResearchProcessEventDetail,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResearchProcessEventDetail {
    Started,
    Metadata {
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// Untrusted CLI text for inert plaintext preview only. It must pass
    /// through the grounded-output protocol before any citation or link UI.
    UntrustedTextDelta {
        text: String,
    },
    ToolActivity {
        tool: String,
        phase: ResearchToolActivityPhase,
    },
    /// Untrusted complete CLI output. Chunk 22's grounded-output boundary
    /// consumes this value; clients must not treat its strings as citations.
    UntrustedFinalOutput {
        text: String,
    },
    /// Complete output whose citation markers and evidence were resolved only
    /// through the per-run Kosh registry after Claude finished.
    GroundedFinalOutput {
        answer: GroundedResearchAnswer,
    },
    Finished {
        outcome: ResearchProcessOutcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        stderr_truncated: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClaudeProcessErrorCode {
    InvalidInput,
    CliMissing,
    CliUnavailable,
    ShuttingDown,
    LaunchFailed,
    ResearchUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct ClaudeProcessError {
    pub code: ClaudeProcessErrorCode,
    pub message: String,
}

impl ClaudeProcessError {
    fn new(code: ClaudeProcessErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: truncate_utf8(&message.into(), MAX_ERROR_BYTES),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new(ClaudeProcessErrorCode::InvalidInput, message)
    }
}

impl ResearchQueryEmbedder for EmbeddingRuntime {
    fn embed_query(&self, query: &str) -> Result<Vec<f32>, String> {
        EmbeddingRuntime::embed_query(self, query).map_err(|error| error.public_message())
    }
}

trait ProcessEventSink: Send + Sync {
    fn emit(&self, event: ResearchProcessEvent) -> Result<(), ClaudeProcessError>;
}

struct TauriProcessEventSink<R: tauri::Runtime> {
    app: AppHandle<R>,
}

struct DurableProcessEventSink {
    database: DatabaseClient,
    clock: Arc<dyn Clock>,
    downstream: Arc<dyn ProcessEventSink>,
}

impl<R: tauri::Runtime> ProcessEventSink for TauriProcessEventSink<R> {
    fn emit(&self, event: ResearchProcessEvent) -> Result<(), ClaudeProcessError> {
        if let Err(error) = self.app.emit(RESEARCH_PROCESS_EVENT, event) {
            log::warn!("could not emit a research process event: {error}");
        }
        Ok(())
    }
}

impl ProcessEventSink for DurableProcessEventSink {
    fn emit(&self, event: ResearchProcessEvent) -> Result<(), ClaudeProcessError> {
        let payload = serde_json::to_value(&event).map_err(|error| {
            ClaudeProcessError::new(
                ClaudeProcessErrorCode::ResearchUnavailable,
                format!("Kosh could not encode durable research history: {error}"),
            )
        })?;
        self.database
            .append_research_event(AppendResearchEventWrite {
                run_id: event.run_id.clone(),
                sequence: event.sequence,
                kind: event_kind(&event.detail).into(),
                payload,
                now_ms: self.clock.now_ms(),
            })
            .map_err(|error| {
                ClaudeProcessError::new(
                    ClaudeProcessErrorCode::ResearchUnavailable,
                    format!("Kosh could not save durable research history: {error}"),
                )
            })?;
        if let Err(error) = self.downstream.emit(event) {
            log::warn!("could not deliver a persisted research event: {error}");
        }
        Ok(())
    }
}

fn event_kind(detail: &ResearchProcessEventDetail) -> &'static str {
    match detail {
        ResearchProcessEventDetail::Started => "STARTED",
        ResearchProcessEventDetail::Metadata { .. } => "METADATA",
        ResearchProcessEventDetail::UntrustedTextDelta { .. } => "UNTRUSTED_TEXT_DELTA",
        ResearchProcessEventDetail::ToolActivity { .. } => "TOOL_ACTIVITY",
        ResearchProcessEventDetail::UntrustedFinalOutput { .. } => "UNTRUSTED_FINAL_OUTPUT",
        ResearchProcessEventDetail::GroundedFinalOutput { .. } => "GROUNDED_FINAL_OUTPUT",
        ResearchProcessEventDetail::Finished { .. } => "FINISHED",
    }
}

#[derive(Clone)]
pub(crate) struct ClaudeProcessManager {
    inner: Arc<ManagerInner>,
}

struct ManagerInner {
    binary: Option<PathBuf>,
    discover_binary: bool,
    work_root: PathBuf,
    work_recovery_cutoff: SystemTime,
    work_recovery_started: AtomicBool,
    default_timeout: Duration,
    active: Mutex<Option<ActiveProcess>>,
    owned_processes: Mutex<HashMap<String, ActiveProcess>>,
    shutting_down: AtomicBool,
}

#[derive(Clone)]
struct ActiveProcess {
    run_id: String,
    generation: String,
    process_id: u32,
    termination: Arc<AtomicU8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum TerminationReason {
    Running = 0,
    Canceled = 1,
    Replaced = 2,
    TimedOut = 3,
    Shutdown = 4,
    Faulted = 5,
    Completed = 6,
}

impl TerminationReason {
    fn from_atomic(value: u8) -> Self {
        match value {
            1 => Self::Canceled,
            2 => Self::Replaced,
            3 => Self::TimedOut,
            4 => Self::Shutdown,
            5 => Self::Faulted,
            6 => Self::Completed,
            _ => Self::Running,
        }
    }

    fn outcome(self) -> Option<ResearchProcessOutcome> {
        match self {
            Self::Running => None,
            Self::Canceled => Some(ResearchProcessOutcome::Canceled),
            Self::Replaced => Some(ResearchProcessOutcome::Replaced),
            Self::TimedOut => Some(ResearchProcessOutcome::TimedOut),
            Self::Shutdown => Some(ResearchProcessOutcome::Shutdown),
            Self::Faulted | Self::Completed => None,
        }
    }
}

struct CliInvocation {
    arguments: Vec<String>,
    environment: Vec<(String, String)>,
    sensitive_values: Vec<String>,
    citation_registry: Option<ResearchCitationRegistry>,
    keepalive: Box<dyn Send>,
}

#[derive(Debug)]
struct ValidatedStart {
    run_id: String,
    prompt: String,
    timeout: Duration,
}

struct MonitoredProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr: ChildStderr,
    request: ValidatedStart,
    generation: String,
    termination: Arc<AtomicU8>,
    emitter: Arc<RunEmitter>,
    manager: Weak<ManagerInner>,
    citation_registry: Option<ResearchCitationRegistry>,
    _keepalive: Box<dyn Send>,
    _work_directory: OwnedWorkDirectory,
}

impl ClaudeProcessManager {
    pub(crate) fn production(data_dir: &Path) -> Self {
        Self::new_with_discovery(
            discover_claude_binary(),
            data_dir.join(WORK_DIRECTORY_NAME),
            DEFAULT_RUN_TIMEOUT,
            true,
        )
    }

    #[cfg(test)]
    fn new(binary: Option<PathBuf>, work_root: PathBuf, default_timeout: Duration) -> Self {
        Self::new_with_discovery(binary, work_root, default_timeout, false)
    }

    fn new_with_discovery(
        binary: Option<PathBuf>,
        work_root: PathBuf,
        default_timeout: Duration,
        discover_binary: bool,
    ) -> Self {
        Self {
            inner: Arc::new(ManagerInner {
                binary,
                discover_binary,
                work_root,
                work_recovery_cutoff: SystemTime::now(),
                work_recovery_started: AtomicBool::new(false),
                default_timeout,
                active: Mutex::new(None),
                owned_processes: Mutex::new(HashMap::new()),
                shutting_down: AtomicBool::new(false),
            }),
        }
    }

    pub(crate) fn recover_work_directories_async(&self) {
        if let Err(error) = self.start_work_directory_recovery() {
            log::warn!("could not start stale research workspace recovery: {error}");
        }
    }

    fn start_work_directory_recovery(&self) -> std::io::Result<thread::JoinHandle<()>> {
        if self
            .inner
            .work_recovery_started
            .swap(true, Ordering::AcqRel)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "stale research workspace recovery already started",
            ));
        }
        let root = self.inner.work_root.clone();
        let cutoff = self.inner.work_recovery_cutoff;
        match thread::Builder::new()
            .name("kosh-research-recovery".into())
            .spawn(move || {
                if let Err(error) =
                    recover_work_directories(&root, cutoff, MAX_STALE_WORK_DIRECTORIES_PER_START)
                {
                    log::warn!("could not recover old research work directories: {error}");
                }
            }) {
            Ok(handle) => Ok(handle),
            Err(error) => {
                self.inner
                    .work_recovery_started
                    .store(false, Ordering::Release);
                Err(error)
            }
        }
    }

    pub(crate) fn setup_status(&self) -> ClaudeSetupStatus {
        let defaults = read_cli_defaults();
        let Some(binary) = self.resolved_binary() else {
            return ClaudeSetupStatus {
                phase: ClaudeSetupPhase::Missing,
                binary_path: None,
                version: None,
                defaults,
                message: "Install Claude Code, then sign in with `claude auth login`.".into(),
            };
        };
        let binary_path = Some(binary.to_string_lossy().into_owned());
        let version = match run_probe(&binary, &["--version"], SETUP_PROBE_TIMEOUT) {
            Ok(output) if output.status.success() => nonempty_bounded_text(&output.stdout, 256),
            Ok(output) => {
                return ClaudeSetupStatus {
                    phase: ClaudeSetupPhase::Unavailable,
                    binary_path,
                    version: None,
                    defaults,
                    message: probe_failure_message(
                        "Claude Code could not report its version",
                        &output,
                    ),
                };
            }
            Err(error) => {
                return ClaudeSetupStatus {
                    phase: ClaudeSetupPhase::Unavailable,
                    binary_path,
                    version: None,
                    defaults,
                    message: error,
                };
            }
        };
        let auth = match run_probe(&binary, &["auth", "status", "--json"], SETUP_PROBE_TIMEOUT) {
            Ok(output) => output,
            Err(error) => {
                return ClaudeSetupStatus {
                    phase: ClaudeSetupPhase::Unavailable,
                    binary_path,
                    version,
                    defaults,
                    message: error,
                };
            }
        };
        let logged_in = auth.status.success()
            && serde_json::from_slice::<Value>(&auth.stdout)
                .ok()
                .and_then(|value| value.get("loggedIn").and_then(Value::as_bool))
                == Some(true);
        if !logged_in {
            return ClaudeSetupStatus {
                phase: ClaudeSetupPhase::Unauthenticated,
                binary_path,
                version,
                defaults,
                message: "Claude Code is installed but not signed in. Run `claude auth login`."
                    .into(),
            };
        }
        ClaudeSetupStatus {
            phase: ClaudeSetupPhase::Ready,
            binary_path,
            version,
            defaults,
            message: "Claude Code is ready for Kosh research.".into(),
        }
    }

    fn start(
        &self,
        input: StartResearchProcessInput,
        bridge: ClaudeMcpBridge,
        server: EphemeralResearchMcpServer,
        sink: Arc<dyn ProcessEventSink>,
    ) -> Result<StartResearchProcessOutput, ClaudeProcessError> {
        let (environment_name, environment_value) = bridge.environment();
        let citation_registry = server.citation_registry();
        let invocation = CliInvocation {
            arguments: claude_arguments(&bridge, input.model.as_deref(), input.effort.as_deref())?,
            environment: vec![(environment_name.into(), environment_value.into())],
            sensitive_values: vec![environment_value.into()],
            citation_registry: Some(citation_registry),
            keepalive: Box::new(server),
        };
        self.start_with_invocation(input, invocation, sink)
    }

    fn start_with_invocation(
        &self,
        input: StartResearchProcessInput,
        invocation: CliInvocation,
        sink: Arc<dyn ProcessEventSink>,
    ) -> Result<StartResearchProcessOutput, ClaudeProcessError> {
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err(ClaudeProcessError::new(
                ClaudeProcessErrorCode::ShuttingDown,
                "Kosh is shutting down and cannot start another research process",
            ));
        }
        let mut request = validate_start(input, self.inner.default_timeout)?;
        if invocation.citation_registry.is_some() {
            request.prompt = crate::research::grounded_research_prompt(&request.prompt);
        }
        let binary = self.resolved_binary().ok_or_else(|| {
            ClaudeProcessError::new(
                ClaudeProcessErrorCode::CliMissing,
                "Claude Code is not installed. Install it, then run `claude auth login`.",
            )
        })?;
        let work_directory = OwnedWorkDirectory::create(&self.inner.work_root)?;
        let mut command = Command::new(&binary);
        command
            .args(&invocation.arguments)
            .current_dir(work_directory.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in &invocation.environment {
            command.env(name, value);
        }
        #[cfg(unix)]
        command.process_group(0);

        let mut child = command.spawn().map_err(|error| {
            ClaudeProcessError::new(
                ClaudeProcessErrorCode::LaunchFailed,
                format!("Claude Code could not be launched: {error}"),
            )
        })?;
        let process_id = child.id();
        let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (stdin, stdout, stderr) = match pipes {
            (Some(stdin), Some(stdout), Some(stderr)) => (stdin, stdout, stderr),
            _ => {
                force_kill_process_group(process_id);
                let _ = child.wait();
                return Err(ClaudeProcessError::new(
                    ClaudeProcessErrorCode::LaunchFailed,
                    "Claude Code did not provide all required process pipes",
                ));
            }
        };

        let generation = uuid::Uuid::now_v7().to_string();
        let termination = Arc::new(AtomicU8::new(TerminationReason::Running as u8));
        let active = ActiveProcess {
            run_id: request.run_id.clone(),
            generation: generation.clone(),
            process_id,
            termination: Arc::clone(&termination),
        };
        let replaced = {
            let mut active_slot = lock(&self.inner.active);
            if self.inner.shutting_down.load(Ordering::Acquire) {
                drop(active_slot);
                force_kill_process_group(process_id);
                let _ = child.wait();
                return Err(ClaudeProcessError::new(
                    ClaudeProcessErrorCode::ShuttingDown,
                    "Kosh is shutting down and cannot start another research process",
                ));
            }
            if active_slot
                .as_ref()
                .is_some_and(|active| active.run_id == request.run_id)
            {
                drop(active_slot);
                force_kill_process_group(process_id);
                let _ = child.wait();
                return Err(ClaudeProcessError::invalid(
                    "a research process for this runId is already active",
                ));
            }
            lock(&self.inner.owned_processes).insert(generation.clone(), active.clone());
            let replaced = active_slot.replace(active);
            replaced.and_then(|process| {
                terminate_active(&process, TerminationReason::Replaced).then_some(process.run_id)
            })
        };

        let emitter = Arc::new(RunEmitter::new(
            request.run_id.clone(),
            sink,
            invocation.sensitive_values,
        ));
        if let Err(error) = emitter.emit(ResearchProcessEventDetail::Started) {
            {
                let mut slot = lock(&self.inner.active);
                slot.take_if(|active| active.generation == generation);
            }
            lock(&self.inner.owned_processes).remove(&generation);
            force_kill_process_group(process_id);
            let _ = child.wait();
            return Err(error);
        }
        let weak_manager = Arc::downgrade(&self.inner);
        let run_id = request.run_id.clone();
        let monitor_generation = generation.clone();
        let monitor_emitter = Arc::clone(&emitter);
        let child_slot = Arc::new(Mutex::new(Some(child)));
        let monitor_child_slot = Arc::clone(&child_slot);
        thread::Builder::new()
            .name(format!("kosh-research-{}", short_identifier(&run_id)))
            .spawn(move || {
                let child = lock(&monitor_child_slot)
                    .take()
                    .expect("the research child is owned by its monitor");
                monitor_process(MonitoredProcess {
                    child,
                    stdin,
                    stdout,
                    stderr,
                    request,
                    generation: monitor_generation,
                    termination,
                    emitter: monitor_emitter,
                    manager: weak_manager,
                    citation_registry: invocation.citation_registry,
                    _keepalive: invocation.keepalive,
                    _work_directory: work_directory,
                });
            })
            .map_err(|error| {
                let active = {
                    let mut slot = lock(&self.inner.active);
                    slot.take_if(|active| active.generation == generation)
                };
                if let Some(active) = active {
                    terminate_active(&active, TerminationReason::Canceled);
                }
                lock(&self.inner.owned_processes).remove(&generation);
                if let Some(mut child) = lock(&child_slot).take() {
                    force_kill_process_group(child.id());
                    let _ = child.wait();
                }
                let error = ClaudeProcessError::new(
                    ClaudeProcessErrorCode::LaunchFailed,
                    format!("Kosh could not monitor Claude Code: {error}"),
                );
                emitter.finish(
                    ResearchProcessOutcome::Failed,
                    Some(error.message.clone()),
                    false,
                );
                error
            })?;

        Ok(StartResearchProcessOutput {
            run_id,
            replaced_run_id: replaced,
        })
    }

    pub(crate) fn cancel(&self, run_id: &str) -> Result<bool, ClaudeProcessError> {
        validate_run_id(run_id)?;
        let active = lock(&self.inner.active)
            .as_ref()
            .filter(|active| active.run_id == run_id)
            .cloned();
        if let Some(active) = active {
            Ok(terminate_active(&active, TerminationReason::Canceled))
        } else {
            Ok(false)
        }
    }

    pub(crate) fn shutdown(&self) {
        if self.inner.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        let active = lock(&self.inner.active);
        let processes = lock(&self.inner.owned_processes)
            .values()
            .cloned()
            .collect::<Vec<_>>();
        drop(active);
        for process in processes {
            force_owned_process_for_shutdown(&process);
        }
    }

    fn resolved_binary(&self) -> Option<PathBuf> {
        self.inner
            .binary
            .as_ref()
            .filter(|path| is_executable_file(path))
            .cloned()
            .or_else(|| {
                self.inner
                    .discover_binary
                    .then(discover_claude_binary)
                    .flatten()
            })
    }
}

impl Drop for ManagerInner {
    fn drop(&mut self) {
        self.shutting_down.store(true, Ordering::Release);
        for process in lock(&self.owned_processes).values() {
            force_owned_process_for_shutdown(process);
        }
    }
}

#[tauri::command]
pub(crate) async fn claude_setup_status(
    state: State<'_, RuntimeState>,
) -> Result<ClaudeSetupStatus, ClaudeProcessError> {
    let manager = state.claude_processes().clone();
    tauri::async_runtime::spawn_blocking(move || manager.setup_status())
        .await
        .map_err(|_| {
            ClaudeProcessError::new(
                ClaudeProcessErrorCode::CliUnavailable,
                "Kosh could not complete the Claude Code setup check",
            )
        })
}

#[tauri::command]
pub(crate) fn claude_cli_defaults() -> Result<ClaudeCliDefaults, ClaudeProcessError> {
    Ok(read_cli_defaults())
}

#[tauri::command]
pub(crate) fn start_research_process<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, RuntimeState>,
    input: BeginResearchProcessInput,
) -> Result<StartResearchProcessOutput, ClaudeProcessError> {
    let run_id = state
        .next_ids(1)
        .into_iter()
        .next()
        .expect("requested research run ID");
    start_durable_research_process(app, &state, input, run_id, None)
}

#[tauri::command]
pub(crate) fn rerun_research_process<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, RuntimeState>,
    run_id: String,
) -> Result<StartResearchProcessOutput, ClaudeProcessError> {
    let previous = state
        .database_client()
        .load_research_run(run_id.clone())
        .map_err(|error| {
            ClaudeProcessError::new(
                ClaudeProcessErrorCode::ResearchUnavailable,
                format!("Kosh could not load the research run to retry: {error}"),
            )
        })?;
    let next_run_id = state
        .next_ids(1)
        .into_iter()
        .next()
        .expect("requested research run ID");
    start_durable_research_process(
        app,
        &state,
        BeginResearchProcessInput {
            prompt: previous.summary.query,
            model: previous.summary.requested_model,
            effort: previous.summary.requested_effort,
            timeout_seconds: None,
        },
        next_run_id,
        Some(run_id),
    )
}

fn start_durable_research_process<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: &RuntimeState,
    input: BeginResearchProcessInput,
    run_id: String,
    rerun_of_id: Option<String>,
) -> Result<StartResearchProcessOutput, ClaudeProcessError> {
    let process_input = StartResearchProcessInput {
        run_id: run_id.clone(),
        prompt: input.prompt,
        model: input.model,
        effort: input.effort,
        timeout_seconds: input.timeout_seconds,
    };
    validate_start(process_input.clone(), DEFAULT_RUN_TIMEOUT)?;
    let database = state.database_client();
    database
        .create_research_run(CreateResearchRunWrite {
            id: run_id.clone(),
            rerun_of_id,
            query: process_input.prompt.clone(),
            requested_model: process_input.model.clone(),
            requested_effort: process_input.effort.clone(),
            now_ms: state.now_ms(),
        })
        .map_err(|error| {
            ClaudeProcessError::new(
                ClaudeProcessErrorCode::ResearchUnavailable,
                format!("Kosh could not create durable research history: {error}"),
            )
        })?;
    let result = start_research_process_runtime(app, state, process_input);
    if let Err(error) = &result {
        if let Err(persist_error) =
            database.fail_research_run_start(run_id, error.message.clone(), state.now_ms())
        {
            log::warn!("could not save research launch failure: {persist_error}");
        }
    }
    result
}

fn start_research_process_runtime<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: &RuntimeState,
    input: StartResearchProcessInput,
) -> Result<StartResearchProcessOutput, ClaudeProcessError> {
    let embedder: Arc<dyn ResearchQueryEmbedder> = state.embedding_runtime();
    let run = ResearchRun::open(
        state.database_paths(),
        Some(embedder),
        ResearchLimits::default(),
    )
    .map_err(|error| {
        ClaudeProcessError::new(
            ClaudeProcessErrorCode::ResearchUnavailable,
            format!("Kosh could not open its read-only research library: {error}"),
        )
    })?;
    let server = EphemeralResearchMcpServer::start(run).map_err(|error| {
        ClaudeProcessError::new(
            ClaudeProcessErrorCode::ResearchUnavailable,
            format!("Kosh could not start its read-only research tools: {error}"),
        )
    })?;
    let bridge = server.bridge().map_err(|error| {
        ClaudeProcessError::new(
            ClaudeProcessErrorCode::ResearchUnavailable,
            format!("Kosh could not configure its read-only research tools: {error}"),
        )
    })?;
    state.claude_processes().start(
        input,
        bridge,
        server,
        Arc::new(DurableProcessEventSink {
            database: state.database_client(),
            clock: state.clock(),
            downstream: Arc::new(TauriProcessEventSink { app }),
        }),
    )
}

#[tauri::command]
pub(crate) fn cancel_research_process(
    state: State<'_, RuntimeState>,
    run_id: String,
) -> Result<bool, ClaudeProcessError> {
    state.claude_processes().cancel(&run_id)
}

fn validate_start(
    input: StartResearchProcessInput,
    default_timeout: Duration,
) -> Result<ValidatedStart, ClaudeProcessError> {
    validate_run_id(&input.run_id)?;
    if input.prompt.trim().is_empty() {
        return Err(ClaudeProcessError::invalid(
            "the research prompt must not be empty",
        ));
    }
    if input.prompt.len() > MAX_PROMPT_BYTES {
        return Err(ClaudeProcessError::invalid(format!(
            "the research prompt must not exceed {MAX_PROMPT_BYTES} bytes"
        )));
    }
    if let Some(model) = input.model.as_deref() {
        validate_model(model)?;
    }
    if let Some(effort) = input.effort.as_deref() {
        validate_effort(effort)?;
    }
    let timeout = match input.timeout_seconds {
        Some(seconds @ MIN_RUN_TIMEOUT_SECS..=MAX_RUN_TIMEOUT_SECS) => Duration::from_secs(seconds),
        Some(_) => {
            return Err(ClaudeProcessError::invalid(format!(
                "timeoutSeconds must be between {MIN_RUN_TIMEOUT_SECS} and {MAX_RUN_TIMEOUT_SECS}"
            )));
        }
        None => default_timeout,
    };
    Ok(ValidatedStart {
        run_id: input.run_id,
        prompt: input.prompt,
        timeout,
    })
}

fn validate_run_id(run_id: &str) -> Result<(), ClaudeProcessError> {
    let parsed = uuid::Uuid::parse_str(run_id)
        .map_err(|_| ClaudeProcessError::invalid("runId must be a UUID"))?;
    if parsed.get_version_num() != 7 || parsed.hyphenated().to_string() != run_id {
        return Err(ClaudeProcessError::invalid(
            "runId must be a canonical UUIDv7",
        ));
    }
    Ok(())
}

fn validate_model(model: &str) -> Result<(), ClaudeProcessError> {
    let valid = !model.is_empty()
        && model.len() <= 128
        && !model.starts_with('-')
        && model
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-._[]".contains(character));
    if valid {
        Ok(())
    } else {
        Err(ClaudeProcessError::invalid(
            "the Claude model name is invalid",
        ))
    }
}

fn validate_effort(effort: &str) -> Result<(), ClaudeProcessError> {
    if EFFORT_LEVELS.contains(&effort) {
        Ok(())
    } else {
        Err(ClaudeProcessError::invalid(
            "effort must be low, medium, high, xhigh, or max",
        ))
    }
}

fn claude_arguments(
    bridge: &ClaudeMcpBridge,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<Vec<String>, ClaudeProcessError> {
    if let Some(model) = model {
        validate_model(model)?;
    }
    if let Some(effort) = effort {
        validate_effort(effort)?;
    }
    let mut arguments = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--include-partial-messages".into(),
        "--no-session-persistence".into(),
        "--permission-mode".into(),
        "dontAsk".into(),
        "--disable-slash-commands".into(),
        "--setting-sources".into(),
        String::new(),
        "--no-chrome".into(),
        "--prompt-suggestions".into(),
        "false".into(),
        "--disallowed-tools".into(),
        DISALLOWED_BUILTIN_TOOLS.into(),
    ];
    arguments.extend(bridge.claude_cli_arguments());
    if let Some(model) = model {
        arguments.extend(["--model".into(), model.into()]);
    }
    if let Some(effort) = effort {
        arguments.extend(["--effort".into(), effort.into()]);
    }
    Ok(arguments)
}

fn monitor_process(process: MonitoredProcess) {
    let MonitoredProcess {
        mut child,
        mut stdin,
        stdout,
        stderr,
        request,
        generation,
        termination,
        emitter,
        manager,
        citation_registry,
        _keepalive,
        _work_directory,
    } = process;
    let _process_guard = ProcessGroupGuard(child.id());
    let prompt = request.prompt;
    let stdin_thread = thread::Builder::new()
        .name("kosh-research-stdin".into())
        .spawn(move || {
            let result = stdin.write_all(prompt.as_bytes());
            drop(stdin);
            result
        });

    let (stdout_sender, stdout_receiver) = mpsc::sync_channel(64);
    let stdout_thread = thread::Builder::new()
        .name("kosh-research-stdout".into())
        .spawn(move || read_stdout(stdout, stdout_sender));

    let stderr_tail = Arc::new(Mutex::new(BoundedTail::new(MAX_STDERR_BYTES)));
    let stderr_for_thread = Arc::clone(&stderr_tail);
    let stderr_thread = thread::Builder::new()
        .name("kosh-research-stderr".into())
        .spawn(move || drain_stderr(stderr, stderr_for_thread));

    let start = Instant::now();
    let mut parser = StreamParser::new(Arc::clone(&emitter), citation_registry);
    let mut process_status = None;
    let mut process_error = None;
    let mut termination_started = None;

    loop {
        for _ in 0..MAX_STDOUT_MESSAGES_PER_POLL {
            let Ok(message) = stdout_receiver.try_recv() else {
                break;
            };
            match message {
                StdoutMessage::Line(line) => {
                    if process_error.is_some()
                        || TerminationReason::from_atomic(termination.load(Ordering::Acquire))
                            != TerminationReason::Running
                    {
                        continue;
                    }
                    if let Err(error) = parser.parse_line(&line) {
                        process_error.get_or_insert(error.message);
                        request_termination(&termination, TerminationReason::Faulted, child.id());
                    }
                }
                StdoutMessage::Error(error) => {
                    process_error.get_or_insert(error);
                    request_termination(&termination, TerminationReason::Faulted, child.id());
                }
                StdoutMessage::Eof => {}
            }
        }

        if process_status.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let _ = termination.compare_exchange(
                        TerminationReason::Running as u8,
                        TerminationReason::Completed as u8,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                    process_status = Some(status);
                }
                Ok(None) => {}
                Err(error) => {
                    process_error.get_or_insert_with(|| {
                        format!("Kosh could not monitor Claude Code: {error}")
                    });
                    request_termination(&termination, TerminationReason::Faulted, child.id());
                }
            }
        }

        if process_status.is_none() && start.elapsed() >= request.timeout {
            request_termination(&termination, TerminationReason::TimedOut, child.id());
        }
        let reason = TerminationReason::from_atomic(termination.load(Ordering::Acquire));
        if reason != TerminationReason::Running && process_status.is_none() {
            let termination_at = termination_started.get_or_insert_with(Instant::now);
            if termination_at.elapsed() >= TERMINATION_GRACE {
                force_kill_process_group(child.id());
            }
        }

        if process_status.is_some() {
            break;
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    }

    // A CLI process can exit while leaving a descendant holding its pipes.
    // Tear down the whole isolated process group before joining the readers.
    force_kill_process_group(child.id());
    let _ = stdin_thread.and_then(|thread| {
        thread
            .join()
            .map_err(|_| std::io::Error::other("prompt writer panicked"))?
    });
    while let Ok(message) = stdout_receiver.recv() {
        match message {
            StdoutMessage::Line(line) => {
                let reason = TerminationReason::from_atomic(termination.load(Ordering::Acquire));
                if process_error.is_some()
                    || !matches!(
                        reason,
                        TerminationReason::Running | TerminationReason::Completed
                    )
                {
                    continue;
                }
                if let Err(error) = parser.parse_line(&line) {
                    process_error.get_or_insert(error.message);
                }
            }
            StdoutMessage::Error(error) => {
                process_error.get_or_insert(error);
            }
            StdoutMessage::Eof => break,
        }
    }
    if let Ok(thread) = stdout_thread {
        let _ = thread.join();
    }
    if let Ok(thread) = stderr_thread {
        let _ = thread.join();
    }

    if let Some(manager) = manager.upgrade() {
        let mut active = lock(&manager.active);
        if active
            .as_ref()
            .is_some_and(|active| active.generation == generation)
        {
            *active = None;
        }
        lock(&manager.owned_processes).remove(&generation);
    }

    let stderr = lock(&stderr_tail);
    let stderr_text = emitter.redact(&stderr.text());
    let stderr_truncated = stderr.truncated;
    let reason = TerminationReason::from_atomic(termination.load(Ordering::Acquire));
    let (outcome, error) = finish_outcome(
        reason,
        process_status,
        process_error.or(parser.failure_message),
        parser.saw_success_result,
        &stderr_text,
    );
    emitter.finish(outcome, error, stderr_truncated);
}

fn finish_outcome(
    termination: TerminationReason,
    status: Option<ExitStatus>,
    process_error: Option<String>,
    saw_success_result: bool,
    stderr: &str,
) -> (ResearchProcessOutcome, Option<String>) {
    if let Some(outcome) = termination.outcome() {
        return (outcome, None);
    }
    if let Some(error) = process_error {
        return (ResearchProcessOutcome::Failed, Some(error));
    }
    if status.is_some_and(|status| status.success()) && saw_success_result {
        return (ResearchProcessOutcome::Succeeded, None);
    }
    let error = if !stderr.trim().is_empty() {
        stderr.trim().to_owned()
    } else if status.is_some_and(|status| status.success()) {
        "Claude Code exited without a final result.".into()
    } else {
        "Claude Code exited with an error.".into()
    };
    (ResearchProcessOutcome::Failed, Some(error))
}

enum StdoutMessage {
    Line(String),
    Error(String),
    Eof,
}

fn read_stdout(mut stdout: impl Read, sender: mpsc::SyncSender<StdoutMessage>) {
    let mut buffer = [0_u8; 8 * 1024];
    let mut line = Vec::new();
    loop {
        match stdout.read(&mut buffer) {
            Ok(0) => {
                if !line.is_empty() && send_stdout_line(&sender, &line).is_err() {
                    return;
                }
                let _ = sender.send(StdoutMessage::Eof);
                return;
            }
            Ok(read) => {
                for byte in &buffer[..read] {
                    if *byte == b'\n' {
                        if !line.is_empty() && send_stdout_line(&sender, &line).is_err() {
                            return;
                        }
                        line.clear();
                    } else {
                        line.push(*byte);
                        if line.len() > MAX_STDOUT_LINE_BYTES {
                            let _ = sender.send(StdoutMessage::Error(
                                "Claude Code emitted an oversized stream event.".into(),
                            ));
                            return;
                        }
                    }
                }
            }
            Err(error) => {
                let _ = sender.send(StdoutMessage::Error(format!(
                    "Kosh could not read Claude Code output: {error}"
                )));
                return;
            }
        }
    }
}

fn send_stdout_line(sender: &mpsc::SyncSender<StdoutMessage>, bytes: &[u8]) -> Result<(), ()> {
    let line = match std::str::from_utf8(bytes) {
        Ok(line) => line.trim_end_matches('\r'),
        Err(_) => {
            let _ = sender.send(StdoutMessage::Error(
                "Claude Code emitted non-UTF-8 stream output.".into(),
            ));
            return Err(());
        }
    };
    sender
        .send(StdoutMessage::Line(line.to_owned()))
        .map_err(|_| ())
}

fn drain_stderr(mut stderr: impl Read, tail: Arc<Mutex<BoundedTail>>) {
    let mut buffer = [0_u8; 4 * 1024];
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => lock(&tail).push(&buffer[..read]),
        }
    }
}

struct BoundedTail {
    bytes: Vec<u8>,
    capacity: usize,
    truncated: bool,
}

impl BoundedTail {
    fn new(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
            capacity,
            truncated: false,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        if bytes.len() >= self.capacity {
            self.bytes.clear();
            self.bytes
                .extend_from_slice(&bytes[bytes.len() - self.capacity..]);
            self.truncated = true;
            return;
        }
        let overflow = self
            .bytes
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(self.capacity);
        if overflow > 0 {
            self.bytes.drain(..overflow);
            self.truncated = true;
        }
        self.bytes.extend_from_slice(bytes);
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

struct StreamParser {
    emitter: Arc<RunEmitter>,
    citation_registry: Option<ResearchCitationRegistry>,
    active_tools: HashMap<String, String>,
    saw_success_result: bool,
    failure_message: Option<String>,
    stream_event_count: usize,
    stream_bytes: usize,
}

impl StreamParser {
    fn new(emitter: Arc<RunEmitter>, citation_registry: Option<ResearchCitationRegistry>) -> Self {
        Self {
            emitter,
            citation_registry,
            active_tools: HashMap::new(),
            saw_success_result: false,
            failure_message: None,
            stream_event_count: 0,
            stream_bytes: 0,
        }
    }

    fn parse_line(&mut self, line: &str) -> Result<(), ClaudeProcessError> {
        if line.trim().is_empty() {
            return Ok(());
        }
        self.stream_event_count = self.stream_event_count.saturating_add(1);
        self.stream_bytes = self.stream_bytes.saturating_add(line.len());
        if self.stream_event_count > MAX_STREAM_EVENT_COUNT || self.stream_bytes > MAX_STREAM_BYTES
        {
            return Err(stream_error(
                "Claude Code exceeded Kosh's research stream limit.",
            ));
        }
        let value: Value = serde_json::from_str(line)
            .map_err(|_| stream_error("Claude Code emitted malformed stream JSON."))?;
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| stream_error("Claude Code emitted a stream event without a type."))?;
        match event_type {
            "system" if value.get("subtype").and_then(Value::as_str) == Some("init") => {
                let model = value
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                self.emitter
                    .emit(ResearchProcessEventDetail::Metadata { model })?;
            }
            "stream_event" => self.parse_partial_event(value.get("event").ok_or_else(|| {
                stream_error("Claude Code emitted an incomplete stream event.")
            })?)?,
            "result" => {
                let success = value.get("subtype").and_then(Value::as_str) == Some("success")
                    && value.get("is_error").and_then(Value::as_bool) != Some(true);
                if success {
                    let text = value.get("result").and_then(Value::as_str).ok_or_else(|| {
                        stream_error("Claude Code emitted a successful result without output.")
                    })?;
                    if text.len() > MAX_VISIBLE_TEXT_BYTES {
                        return Err(stream_error(
                            "Claude Code final output exceeded Kosh's limit.",
                        ));
                    }
                    self.emitter.discard_pending_text();
                    if let Some(registry) = &self.citation_registry {
                        let redacted = self.emitter.redact(text);
                        let answer = registry.ground_output(&redacted).map_err(|_| {
                            stream_error("Kosh could not ground Claude Code's final answer.")
                        })?;
                        self.emitter
                            .emit(ResearchProcessEventDetail::GroundedFinalOutput { answer })?;
                    } else {
                        self.emitter
                            .emit(ResearchProcessEventDetail::UntrustedFinalOutput {
                                text: text.to_owned(),
                            })?;
                    }
                    self.saw_success_result = true;
                } else {
                    self.failure_message = value
                        .get("result")
                        .and_then(Value::as_str)
                        .filter(|message| !message.trim().is_empty())
                        .map(|message| truncate_utf8(message, MAX_ERROR_BYTES));
                }
            }
            // Full assistant/user messages duplicate partial events or contain
            // MCP payloads. Kosh emits only text deltas and compact tool activity.
            "user" => self.parse_tool_results(&value)?,
            "assistant" | "rate_limit_event" | "prompt_suggestion" => {}
            _ => {}
        }
        Ok(())
    }

    fn parse_partial_event(&mut self, event: &Value) -> Result<(), ClaudeProcessError> {
        match event.get("type").and_then(Value::as_str) {
            Some("content_block_start") => {
                let block = event.get("content_block").unwrap_or(&Value::Null);
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    let identifier = block
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|identifier| !identifier.is_empty() && identifier.len() <= 256)
                        .ok_or_else(|| {
                            stream_error("Claude Code emitted a tool event without a valid ID.")
                        })?
                        .to_owned();
                    let tool = block
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            stream_error("Claude Code emitted a tool event without a name.")
                        })?
                        .to_owned();
                    if !is_authorized_tool(&tool) {
                        return Err(stream_error(
                            "Claude Code attempted to use an unauthorized tool.",
                        ));
                    }
                    self.active_tools.insert(identifier, tool.clone());
                    self.emitter
                        .emit(ResearchProcessEventDetail::ToolActivity {
                            tool,
                            phase: ResearchToolActivityPhase::Started,
                        })?;
                }
            }
            Some("content_block_delta") => {
                let delta = event.get("delta").unwrap_or(&Value::Null);
                if delta.get("type").and_then(Value::as_str) == Some("text_delta") {
                    let text = delta.get("text").and_then(Value::as_str).ok_or_else(|| {
                        stream_error("Claude Code emitted an invalid text delta.")
                    })?;
                    self.emitter.emit_text_delta(text)?;
                }
            }
            Some("content_block_stop") => {}
            Some("message_start" | "message_delta" | "message_stop") | None => {}
            Some(_) => {}
        }
        Ok(())
    }

    fn parse_tool_results(&mut self, event: &Value) -> Result<(), ClaudeProcessError> {
        let Some(content) = event
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
        else {
            return Ok(());
        };
        for block in content {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(identifier) = block.get("tool_use_id").and_then(Value::as_str) else {
                continue;
            };
            if let Some(tool) = self.active_tools.remove(identifier) {
                self.emitter
                    .emit(ResearchProcessEventDetail::ToolActivity {
                        tool,
                        phase: ResearchToolActivityPhase::Finished,
                    })?;
            }
        }
        Ok(())
    }
}

fn is_authorized_tool(tool: &str) -> bool {
    crate::research::RESEARCH_TOOL_NAMES
        .iter()
        .any(|name| tool == format!("mcp__kosh__{name}"))
}

fn stream_error(message: impl Into<String>) -> ClaudeProcessError {
    ClaudeProcessError::new(ClaudeProcessErrorCode::CliUnavailable, message)
}

struct RunEmitter {
    run_id: String,
    sink: Arc<dyn ProcessEventSink>,
    state: Mutex<EmitterState>,
    text_redactor: Mutex<StreamingRedactor>,
    sensitive_values: Vec<String>,
}

struct EmitterState {
    sequence: u32,
    event_count: usize,
    event_bytes: usize,
    visible_text_bytes: usize,
    finished: bool,
}

impl RunEmitter {
    fn new(run_id: String, sink: Arc<dyn ProcessEventSink>, sensitive_values: Vec<String>) -> Self {
        let text_redactor = StreamingRedactor::new(&sensitive_values);
        Self {
            run_id,
            sink,
            state: Mutex::new(EmitterState {
                sequence: 0,
                event_count: 0,
                event_bytes: 0,
                visible_text_bytes: 0,
                finished: false,
            }),
            text_redactor: Mutex::new(text_redactor),
            sensitive_values,
        }
    }

    fn emit_text_delta(&self, text: &str) -> Result<(), ClaudeProcessError> {
        let output = lock(&self.text_redactor).push(text);
        if let Some(text) = output.filter(|text| !text.is_empty()) {
            self.emit(ResearchProcessEventDetail::UntrustedTextDelta { text })?;
        }
        Ok(())
    }

    fn discard_pending_text(&self) {
        lock(&self.text_redactor).discard();
    }

    fn emit(&self, mut detail: ResearchProcessEventDetail) -> Result<(), ClaudeProcessError> {
        self.redact_detail(&mut detail);
        let mut state = lock(&self.state);
        if state.finished {
            return Ok(());
        }
        let visible_text_bytes = visible_text_bytes(&detail);
        if state.visible_text_bytes.saturating_add(visible_text_bytes) > MAX_VISIBLE_TEXT_BYTES {
            return Err(ClaudeProcessError::new(
                ClaudeProcessErrorCode::CliUnavailable,
                "Claude Code output exceeded Kosh's visible text limit",
            ));
        }
        let sequence = state.sequence.saturating_add(1);
        let event = ResearchProcessEvent {
            run_id: self.run_id.clone(),
            sequence,
            detail,
        };
        let bytes = serde_json::to_vec(&event)
            .map_err(|_| {
                ClaudeProcessError::new(
                    ClaudeProcessErrorCode::CliUnavailable,
                    "Kosh could not encode a research process event",
                )
            })?
            .len();
        if state.event_count >= MAX_VISIBLE_EVENT_COUNT
            || state.event_bytes.saturating_add(bytes) > MAX_VISIBLE_EVENT_BYTES
        {
            return Err(ClaudeProcessError::new(
                ClaudeProcessErrorCode::CliUnavailable,
                "Claude Code emitted too many research events",
            ));
        }
        self.sink.emit(event)?;
        state.sequence = sequence;
        state.event_count += 1;
        state.event_bytes += bytes;
        state.visible_text_bytes += visible_text_bytes;
        Ok(())
    }

    fn finish(
        &self,
        outcome: ResearchProcessOutcome,
        error: Option<String>,
        stderr_truncated: bool,
    ) {
        let mut state = lock(&self.state);
        if state.finished {
            return;
        }
        state.finished = true;
        state.sequence = state.sequence.saturating_add(1);
        let event = ResearchProcessEvent {
            run_id: self.run_id.clone(),
            sequence: state.sequence,
            detail: ResearchProcessEventDetail::Finished {
                outcome,
                error: error.map(|error| truncate_utf8(&self.redact(&error), MAX_ERROR_BYTES)),
                stderr_truncated,
            },
        };
        drop(state);
        if let Err(error) = self.sink.emit(event) {
            log::error!("could not persist the terminal research event: {error}");
        }
    }

    fn redact_detail(&self, detail: &mut ResearchProcessEventDetail) {
        match detail {
            ResearchProcessEventDetail::Metadata { model } => {
                if let Some(model) = model {
                    *model = truncate_utf8(&self.redact(model), 128);
                }
            }
            ResearchProcessEventDetail::ToolActivity { tool, .. } => {
                *tool = self.redact(tool);
            }
            ResearchProcessEventDetail::UntrustedFinalOutput { text } => {
                *text = self.redact(text);
            }
            ResearchProcessEventDetail::GroundedFinalOutput { .. } => {}
            ResearchProcessEventDetail::UntrustedTextDelta { .. }
            | ResearchProcessEventDetail::Started
            | ResearchProcessEventDetail::Finished { .. } => {}
        }
    }

    fn redact(&self, text: &str) -> String {
        redact_values(text, &self.sensitive_values)
    }
}

struct StreamingRedactor {
    pending: String,
    secrets: Vec<String>,
}

impl StreamingRedactor {
    fn new(secrets: &[String]) -> Self {
        Self {
            pending: String::new(),
            secrets: secrets
                .iter()
                .filter(|secret| !secret.is_empty())
                .cloned()
                .collect(),
        }
    }

    fn push(&mut self, text: &str) -> Option<String> {
        self.pending.push_str(text);
        self.pending = redact_values(&self.pending, &self.secrets);
        let retained_bytes = self
            .secrets
            .iter()
            .map(|secret| {
                secret
                    .char_indices()
                    .skip(1)
                    .map(|(index, _)| index)
                    .filter(|index| self.pending.ends_with(&secret[..*index]))
                    .max()
                    .unwrap_or(0)
            })
            .max()
            .unwrap_or(0);
        let split = self.pending.len().saturating_sub(retained_bytes);
        let emitted = self.pending[..split].to_owned();
        self.pending.drain(..split);
        (!emitted.is_empty()).then_some(emitted)
    }

    fn discard(&mut self) {
        self.pending.clear();
    }
}

fn redact_values(text: &str, secrets: &[String]) -> String {
    secrets
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(text.to_owned(), |text, secret| {
            text.replace(secret, "[REDACTED]")
        })
}

fn visible_text_bytes(detail: &ResearchProcessEventDetail) -> usize {
    match detail {
        ResearchProcessEventDetail::UntrustedTextDelta { text }
        | ResearchProcessEventDetail::UntrustedFinalOutput { text } => text.len(),
        ResearchProcessEventDetail::GroundedFinalOutput { answer } => answer.markdown.len(),
        _ => 0,
    }
}

fn request_termination(termination: &AtomicU8, reason: TerminationReason, process_id: u32) -> bool {
    let requested = termination
        .compare_exchange(
            TerminationReason::Running as u8,
            reason as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok();
    if requested {
        terminate_process_group(process_id);
    }
    requested
}

fn terminate_active(active: &ActiveProcess, reason: TerminationReason) -> bool {
    request_termination(&active.termination, reason, active.process_id)
}

fn force_owned_process_for_shutdown(process: &ActiveProcess) {
    let _ = terminate_active(process, TerminationReason::Shutdown);
    // Completed describes the direct child, not the whole process group. A
    // descendant can still own inherited pipes until the monitor tears it down.
    force_kill_process_group(process.process_id);
}

#[cfg(unix)]
fn terminate_process_group(process_id: u32) {
    if let Ok(process_id) = i32::try_from(process_id) {
        // SAFETY: A negative PID addresses only the child-created process
        // group. The ID comes directly from the child Kosh just spawned.
        unsafe {
            libc::kill(-process_id, libc::SIGTERM);
        }
    }
}

#[cfg(not(unix))]
fn terminate_process_group(_process_id: u32) {}

#[cfg(unix)]
fn force_kill_process_group(process_id: u32) {
    if let Ok(process_id) = i32::try_from(process_id) {
        // SAFETY: See terminate_process_group. SIGKILL is used only after the
        // bounded graceful-termination interval expires.
        unsafe {
            libc::kill(-process_id, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn force_kill_process_group(_process_id: u32) {}

struct ProcessGroupGuard(u32);

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        force_kill_process_group(self.0);
    }
}

struct OwnedWorkDirectory {
    path: PathBuf,
}

impl OwnedWorkDirectory {
    fn create(root: &Path) -> Result<Self, ClaudeProcessError> {
        fs::create_dir_all(root).map_err(|error| {
            ClaudeProcessError::new(
                ClaudeProcessErrorCode::LaunchFailed,
                format!("Kosh could not create its research workspace: {error}"),
            )
        })?;
        let path = root.join(format!("run-{}", uuid::Uuid::now_v7()));
        fs::create_dir(&path).map_err(|error| {
            ClaudeProcessError::new(
                ClaudeProcessErrorCode::LaunchFailed,
                format!("Kosh could not isolate the research process: {error}"),
            )
        })?;
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(|error| {
            ClaudeProcessError::new(
                ClaudeProcessErrorCode::LaunchFailed,
                format!("Kosh could not secure its research workspace: {error}"),
            )
        })?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for OwnedWorkDirectory {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "could not remove research work directory {}: {error}",
                    self.path.display()
                );
            }
        }
    }
}

fn recover_work_directories(
    root: &Path,
    created_before: SystemTime,
    limit: usize,
) -> std::io::Result<usize> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut recovered = 0;
    for entry in entries {
        if recovered >= limit {
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                log::warn!("could not inspect a stale research workspace entry: {error}");
                continue;
            }
        };
        let path = entry.path();
        let Some(identifier) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("run-"))
        else {
            continue;
        };
        let owned_name = uuid::Uuid::parse_str(identifier).is_ok_and(|parsed| {
            parsed.get_version_num() == 7 && parsed.hyphenated().to_string() == identifier
        });
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                log::warn!(
                    "could not inspect stale research workspace type {}: {error}",
                    path.display()
                );
                continue;
            }
        };
        if !owned_name || !file_type.is_dir() {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                log::warn!(
                    "could not inspect stale research work directory {}: {error}",
                    path.display()
                );
                continue;
            }
        };
        let stale = metadata
            .modified()
            .ok()
            .is_some_and(|modified| modified < created_before);
        if !stale {
            continue;
        }
        if let Err(error) = fs::remove_dir_all(&path) {
            log::warn!(
                "could not remove stale research work directory {}: {error}",
                path.display()
            );
            continue;
        }
        recovered += 1;
    }
    Ok(recovered)
}

struct ProbeOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_probe(binary: &Path, arguments: &[&str], timeout: Duration) -> Result<ProbeOutput, String> {
    let mut command = Command::new(binary);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("Claude Code could not be launched: {error}"))?;
    let pipes = (child.stdout.take(), child.stderr.take());
    let (stdout, stderr) = match pipes {
        (Some(stdout), Some(stderr)) => (stdout, stderr),
        _ => {
            stop_probe_process_group(&mut child);
            return Err("Claude Code setup check did not provide its output pipes.".into());
        }
    };
    let stdout_thread = match thread::Builder::new()
        .name("kosh-claude-probe-stdout".into())
        .spawn(move || read_bounded_prefix(stdout, MAX_STDERR_BYTES))
    {
        Ok(thread) => thread,
        Err(error) => {
            stop_probe_process_group(&mut child);
            return Err(format!(
                "Kosh could not monitor the Claude Code setup check: {error}"
            ));
        }
    };
    let stderr_thread = match thread::Builder::new()
        .name("kosh-claude-probe-stderr".into())
        .spawn(move || read_bounded_prefix(stderr, MAX_STDERR_BYTES))
    {
        Ok(thread) => thread,
        Err(error) => {
            stop_probe_process_group(&mut child);
            let _ = stdout_thread.join();
            return Err(format!(
                "Kosh could not monitor the Claude Code setup check: {error}"
            ));
        }
    };
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < timeout => thread::sleep(PROCESS_POLL_INTERVAL),
            Ok(None) => {
                stop_probe_process_group(&mut child);
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return Err("Claude Code did not respond to its setup check.".into());
            }
            Err(error) => {
                stop_probe_process_group(&mut child);
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return Err(format!("Claude Code setup check failed: {error}"));
            }
        }
    };
    // A setup command can exit after spawning a descendant that inherited its
    // pipes. Terminate the owned group before joining readers on every path.
    force_kill_process_group(child.id());
    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    Ok(ProbeOutput {
        status,
        stdout,
        stderr,
    })
}

fn stop_probe_process_group(child: &mut Child) {
    force_kill_process_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

fn read_bounded_prefix(mut reader: impl Read, capacity: usize) -> Vec<u8> {
    let mut stored = Vec::with_capacity(capacity);
    let mut buffer = [0_u8; 4 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => return stored,
            Ok(read) if stored.len() < capacity => {
                let remaining = capacity - stored.len();
                stored.extend_from_slice(&buffer[..read.min(remaining)]);
            }
            Ok(_) => {}
        }
    }
}

fn probe_failure_message(prefix: &str, output: &ProbeOutput) -> String {
    let detail = nonempty_bounded_text(&output.stderr, 512)
        .or_else(|| nonempty_bounded_text(&output.stdout, 512));
    detail.map_or_else(|| prefix.to_owned(), |detail| format!("{prefix}: {detail}"))
}

fn nonempty_bounded_text(bytes: &[u8], limit: usize) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    (!text.is_empty()).then(|| truncate_utf8(text, limit))
}

fn discover_claude_binary() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path = std::env::var_os("PATH");
    discover_claude_binary_from(
        home.as_deref(),
        path.as_deref(),
        &[
            PathBuf::from("/opt/homebrew/bin/claude"),
            PathBuf::from("/usr/local/bin/claude"),
        ],
        std::env::var_os("KOSH_CLAUDE_DISABLED").is_some_and(|value| value == "1"),
    )
}

fn discover_claude_binary_from(
    home: Option<&Path>,
    path: Option<&std::ffi::OsStr>,
    system_candidates: &[PathBuf],
    explicitly_disabled: bool,
) -> Option<PathBuf> {
    if explicitly_disabled {
        return None;
    }
    let mut candidates = Vec::new();
    if let Some(home) = home {
        candidates.push(home.join(".local/bin/claude"));
        candidates.push(home.join(".claude/local/claude"));
    }
    candidates.extend(system_candidates.iter().cloned());
    if let Some(path) = path {
        candidates.extend(std::env::split_paths(path).map(|directory| directory.join("claude")));
    }
    candidates.into_iter().find(|path| is_executable_file(path))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn read_cli_defaults() -> ClaudeCliDefaults {
    let configuration = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude")));
    configuration
        .as_deref()
        .map(|directory| read_cli_defaults_from(&directory.join("settings.json")))
        .unwrap_or_default()
}

fn read_cli_defaults_from(path: &Path) -> ClaudeCliDefaults {
    let Ok(bytes) = fs::read(path) else {
        return ClaudeCliDefaults::default();
    };
    if bytes.len() > 1024 * 1024 {
        return ClaudeCliDefaults::default();
    }
    let Ok(settings) = serde_json::from_slice::<Value>(&bytes) else {
        return ClaudeCliDefaults::default();
    };
    let model = settings
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| validate_model(model).is_ok())
        .map(str::to_owned);
    let effort = settings
        .get("effortLevel")
        .and_then(Value::as_str)
        .filter(|effort| validate_effort(effort).is_ok())
        .map(str::to_owned);
    ClaudeCliDefaults { model, effort }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn short_identifier(identifier: &str) -> &str {
    identifier.get(..8).unwrap_or(identifier)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        collections::HashMap,
        fs,
        os::unix::fs::PermissionsExt,
        sync::mpsc,
        time::{Duration, Instant},
    };

    use tempfile::TempDir;

    use crate::database::{
        tidbits::CreateTidbitWrite, CreateResearchRunWrite, Database, DatabasePaths, SourceDraft,
        TidbitDraft,
    };

    use super::*;

    #[derive(Clone)]
    struct ChannelSink {
        sender: mpsc::Sender<ResearchProcessEvent>,
    }

    struct FixedClock(i64);

    impl Clock for FixedClock {
        fn now_ms(&self) -> i64 {
            self.0
        }
    }

    impl ProcessEventSink for ChannelSink {
        fn emit(&self, event: ResearchProcessEvent) -> Result<(), ClaudeProcessError> {
            let _ = self.sender.send(event);
            Ok(())
        }
    }

    fn write_fake_cli(root: &Path, body: &str) -> PathBuf {
        let path = root.join("claude");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake Claude CLI");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .expect("make fake Claude CLI executable");
        path
    }

    #[test]
    fn gui_discovery_finds_home_install_without_a_login_shell_path() {
        let root = tempfile::tempdir().expect("temporary home");
        let binary_directory = root.path().join(".local/bin");
        fs::create_dir_all(&binary_directory).expect("home-local binary directory");
        let binary = write_fake_cli(&binary_directory, "exit 0");

        assert_eq!(
            discover_claude_binary_from(Some(root.path()), Some("".as_ref()), &[], false),
            Some(binary)
        );
    }

    #[test]
    fn gui_discovery_falls_back_to_standard_install_locations_before_path() {
        let root = tempfile::tempdir().expect("temporary install roots");
        let system_directory = root.path().join("opt/homebrew/bin");
        let path_directory = root.path().join("login-shell/bin");
        fs::create_dir_all(&system_directory).expect("standard binary directory");
        fs::create_dir_all(&path_directory).expect("login-shell binary directory");
        let system_binary = write_fake_cli(&system_directory, "exit 0");
        let _path_binary = write_fake_cli(&path_directory, "exit 0");

        assert_eq!(
            discover_claude_binary_from(
                None,
                Some(path_directory.as_os_str()),
                std::slice::from_ref(&system_binary),
                false,
            ),
            Some(system_binary)
        );
    }

    #[test]
    fn gui_discovery_honors_explicit_disable_even_when_claude_is_installed() {
        let root = tempfile::tempdir().expect("temporary install root");
        let binary = write_fake_cli(root.path(), "exit 0");

        assert_eq!(
            discover_claude_binary_from(
                None,
                Some(root.path().as_os_str()),
                std::slice::from_ref(&binary),
                true,
            ),
            None
        );
    }

    fn test_manager(binary: PathBuf, root: &TempDir, timeout: Duration) -> ClaudeProcessManager {
        ClaudeProcessManager::new(Some(binary), root.path().join("work"), timeout)
    }

    fn request(prompt: &str) -> StartResearchProcessInput {
        StartResearchProcessInput {
            run_id: uuid::Uuid::now_v7().to_string(),
            prompt: prompt.into(),
            model: None,
            effort: None,
            timeout_seconds: None,
        }
    }

    fn invocation() -> CliInvocation {
        CliInvocation {
            arguments: Vec::new(),
            environment: Vec::new(),
            sensitive_values: Vec::new(),
            citation_registry: None,
            keepalive: Box::new(()),
        }
    }

    fn grounded_invocation(root: &TempDir) -> (CliInvocation, String) {
        let database = Database::initialize(DatabasePaths::new(root.path().join("library")))
            .expect("grounded research database");
        database
            .client()
            .create_tidbit(CreateTidbitWrite {
                input: TidbitDraft {
                    title: Some("Grounded process".into()),
                    body_markdown: "grounded_process_evidence is exact and retained.".into(),
                    sources: vec![SourceDraft {
                        label: Some("Process fixture".into()),
                        url: Some("https://example.com/process-fixture".into()),
                    }],
                },
                now_ms: 1,
                tidbit_id: uuid::Uuid::now_v7().to_string(),
                revision_id: uuid::Uuid::now_v7().to_string(),
                source_ids: vec![uuid::Uuid::now_v7().to_string()],
            })
            .expect("grounded research tidbit");
        let mut run = ResearchRun::from_read_only_connection(
            database
                .open_main_read_only()
                .expect("grounded read-only connection"),
            None,
            ResearchLimits::default(),
        )
        .expect("grounded research run");
        let result = run
            .call_tool(
                crate::research::EXACT_SEARCH_TOOL,
                serde_json::json!({"query": "grounded_process_evidence"}),
            )
            .expect("issue a citation handle");
        let handle = result["items"][0]["citationHandle"]
            .as_str()
            .expect("citation handle")
            .to_owned();
        let server = EphemeralResearchMcpServer::start(run).expect("grounded MCP server");
        let registry = server.citation_registry();
        (
            CliInvocation {
                arguments: Vec::new(),
                environment: Vec::new(),
                sensitive_values: Vec::new(),
                citation_registry: Some(registry),
                keepalive: Box::new((server, database)),
            },
            handle,
        )
    }

    fn start_fake(
        manager: &ClaudeProcessManager,
        input: StartResearchProcessInput,
        sender: &mpsc::Sender<ResearchProcessEvent>,
    ) -> StartResearchProcessOutput {
        manager
            .start_with_invocation(
                input,
                invocation(),
                Arc::new(ChannelSink {
                    sender: sender.clone(),
                }),
            )
            .expect("start fake Claude")
    }

    fn receive_terminals(
        receiver: &mpsc::Receiver<ResearchProcessEvent>,
        expected: usize,
    ) -> (
        Vec<ResearchProcessEvent>,
        HashMap<String, ResearchProcessOutcome>,
    ) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        let mut terminals = HashMap::new();
        while terminals.len() < expected {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let event = receiver
                .recv_timeout(remaining)
                .expect("research process terminal event");
            if let ResearchProcessEventDetail::Finished { outcome, .. } = event.detail {
                terminals.insert(event.run_id.clone(), outcome);
            }
            events.push(event);
        }
        (events, terminals)
    }

    fn finished_error(events: &[ResearchProcessEvent], run_id: &str) -> (Option<String>, bool) {
        events
            .iter()
            .find_map(|event| {
                if event.run_id != run_id {
                    return None;
                }
                let ResearchProcessEventDetail::Finished {
                    error,
                    stderr_truncated,
                    ..
                } = &event.detail
                else {
                    return None;
                };
                Some((error.clone(), *stderr_truncated))
            })
            .expect("finished event")
    }

    #[test]
    fn validates_ids_models_effort_and_timeouts() {
        let mut input = request("Find evidence");
        input.run_id = "550e8400-e29b-41d4-a716-446655440000".into();
        assert_eq!(
            validate_start(input, DEFAULT_RUN_TIMEOUT)
                .expect_err("UUIDv4 rejected")
                .code,
            ClaudeProcessErrorCode::InvalidInput
        );
        assert!(validate_model("sonnet").is_ok());
        assert!(validate_model("claude-fable-5[1m]").is_ok());
        assert!(validate_model("--dangerously-skip-permissions").is_err());
        assert!(validate_effort("xhigh").is_ok());
        assert!(validate_effort("maximum").is_err());

        let mut input = request("Find evidence");
        input.timeout_seconds = Some(1);
        assert!(validate_start(input, DEFAULT_RUN_TIMEOUT).is_err());
    }

    #[test]
    fn reads_only_valid_cli_defaults() {
        let root = tempfile::tempdir().expect("settings root");
        let settings = root.path().join("settings.json");
        fs::write(
            &settings,
            r#"{"model":"sonnet","effortLevel":"high","permissions":{"allow":["Bash"]}}"#,
        )
        .expect("write settings");
        assert_eq!(
            read_cli_defaults_from(&settings),
            ClaudeCliDefaults {
                model: Some("sonnet".into()),
                effort: Some("high".into()),
            }
        );
        fs::write(&settings, r#"{"model":"--bad","effortLevel":"unlimited"}"#)
            .expect("write invalid settings");
        assert_eq!(
            read_cli_defaults_from(&settings),
            ClaudeCliDefaults::default()
        );
    }

    #[test]
    fn setup_status_distinguishes_missing_unauthenticated_and_ready() {
        let root = tempfile::tempdir().expect("fake CLI root");
        let missing = ClaudeProcessManager::new(
            None,
            root.path().join("missing-work"),
            Duration::from_secs(1),
        );
        assert_eq!(missing.setup_status().phase, ClaudeSetupPhase::Missing);

        let binary = write_fake_cli(
            root.path(),
            r#"
if [ "$1" = "--version" ]; then
  printf '%s\n' '9.9.9 (Claude Code)'
elif [ "$1" = "auth" ]; then
  printf '%s\n' '{"loggedIn":false}'
fi
"#,
        );
        let manager = test_manager(binary.clone(), &root, Duration::from_secs(1));
        assert_eq!(
            manager.setup_status().phase,
            ClaudeSetupPhase::Unauthenticated
        );

        write_fake_cli(
            root.path(),
            r#"
if [ "$1" = "--version" ]; then
  printf '%s\n' '9.9.9 (Claude Code)'
elif [ "$1" = "auth" ]; then
  printf '%s\n' '{"loggedIn":true,"email":"must-not-leak@example.com"}'
fi
"#,
        );
        let status = manager.setup_status();
        assert_eq!(status.phase, ClaudeSetupPhase::Ready);
        assert_eq!(status.version.as_deref(), Some("9.9.9 (Claude Code)"));
        assert!(!status.message.contains("must-not-leak"));
    }

    #[test]
    fn setup_probe_cleans_descendants_before_joining_inherited_pipes() {
        let root = tempfile::tempdir().expect("fake CLI root");
        let binary = write_fake_cli(
            root.path(),
            r#"
sleep 30 &
exit 0
"#,
        );
        let started = Instant::now();
        let output = run_probe(&binary, &[], Duration::from_secs(2)).expect("successful probe");
        assert!(output.status.success());
        assert!(started.elapsed() < Duration::from_secs(5));

        write_fake_cli(
            root.path(),
            r#"
sleep 30 &
wait
"#,
        );
        let started = Instant::now();
        assert!(run_probe(&binary, &[], Duration::from_millis(500)).is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn stale_workspace_recovery_is_deferred_bounded_and_owned() {
        let root = tempfile::tempdir().expect("workspace root");
        let work_root = root.path().join("work");
        fs::create_dir(&work_root).expect("create work root");
        let deferred = work_root.join(format!("run-{}", uuid::Uuid::now_v7()));
        fs::create_dir(&deferred).expect("create deferred workspace");
        fs::write(deferred.join("artifact"), b"stale").expect("write stale artifact");
        let unrelated = work_root.join("run-not-a-kosh-uuid");
        fs::create_dir(&unrelated).expect("create unrelated directory");
        #[cfg(unix)]
        let external = {
            let external = root.path().join("external");
            fs::create_dir(&external).expect("create external directory");
            fs::write(external.join("keep"), b"unowned").expect("write external artifact");
            let link = work_root.join(format!("run-{}", uuid::Uuid::now_v7()));
            std::os::unix::fs::symlink(&external, &link).expect("link unowned directory");
            (external, link)
        };

        let manager = ClaudeProcessManager::new(None, work_root.clone(), Duration::from_secs(1));
        assert!(deferred.exists());
        manager
            .start_work_directory_recovery()
            .expect("start deferred recovery")
            .join()
            .expect("join deferred recovery");
        assert!(manager.start_work_directory_recovery().is_err());
        assert!(unrelated.exists());
        if deferred.exists() {
            fs::remove_dir_all(&deferred).expect("remove deferred fixture");
        }

        let stale = (0..3)
            .map(|_| {
                let path = work_root.join(format!("run-{}", uuid::Uuid::now_v7()));
                fs::create_dir(&path).expect("create stale workspace");
                fs::write(path.join("artifact"), b"stale").expect("write stale artifact");
                path
            })
            .collect::<Vec<_>>();
        let later_than_every_entry = SystemTime::now() + Duration::from_secs(1);
        assert_eq!(
            recover_work_directories(&work_root, later_than_every_entry, 2)
                .expect("bounded recovery"),
            2
        );
        assert_eq!(stale.iter().filter(|path| path.exists()).count(), 1);
        assert!(unrelated.exists());
        #[cfg(unix)]
        {
            assert!(external.0.join("keep").exists());
            assert!(fs::symlink_metadata(&external.1).is_ok());
        }
    }

    #[test]
    fn streams_partial_json_and_completes_successfully() {
        let root = tempfile::tempdir().expect("fake CLI root");
        let binary = write_fake_cli(
            root.path(),
            r#"
cat >/dev/null
printf '%s\n' '{"type":"system","subtype":"init","model":"fake-sonnet"}'
printf '%s' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel'
sleep 0.02
printf '%s\n' 'lo"}}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"Hello"}'
"#,
        );
        let manager = test_manager(binary, &root, Duration::from_secs(2));
        let (sender, receiver) = mpsc::channel();
        let started = start_fake(&manager, request("answer"), &sender);
        let (events, terminals) = receive_terminals(&receiver, 1);

        assert_eq!(
            terminals.get(&started.run_id),
            Some(&ResearchProcessOutcome::Succeeded)
        );
        assert!(events.iter().any(|event| {
            matches!(
                &event.detail,
                ResearchProcessEventDetail::UntrustedTextDelta { text } if text == "Hello"
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                &event.detail,
                ResearchProcessEventDetail::UntrustedFinalOutput { text } if text == "Hello"
            )
        }));
        assert!(!manager
            .cancel(&started.run_id)
            .expect("completed run is not cancelable"));
    }

    #[test]
    fn production_boundary_wraps_prompt_and_emits_only_grounded_final_output() {
        let root = tempfile::tempdir().expect("fake grounded CLI root");
        let (invocation, handle) = grounded_invocation(&root);
        let result = serde_json::json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": format!(
                "The retained fact is supported by exact Kosh evidence [[cite:{handle}]]."
            ),
        });
        let binary = write_fake_cli(
            root.path(),
            &format!(
                r#"
prompt=$(cat)
case "$prompt" in
  *"You are Kosh Research."*"The user's request is the following JSON string:"*) ;;
  *) exit 9 ;;
esac
printf '%s\n' '{}'
"#,
                result
            ),
        );
        let manager = test_manager(binary, &root, Duration::from_secs(2));
        let (sender, receiver) = mpsc::channel();
        let input = request("Explain the retained fact.");
        let run_id = input.run_id.clone();
        manager
            .start_with_invocation(input, invocation, Arc::new(ChannelSink { sender }))
            .expect("start grounded fake Claude");
        let (events, terminals) = receive_terminals(&receiver, 1);

        assert_eq!(
            terminals.get(&run_id),
            Some(&ResearchProcessOutcome::Succeeded)
        );
        let answer = events
            .iter()
            .find_map(|event| match &event.detail {
                ResearchProcessEventDetail::GroundedFinalOutput { answer } => Some(answer),
                _ => None,
            })
            .expect("grounded final output");
        assert_eq!(answer.markdown.matches("【1】").count(), 1);
        assert_eq!(answer.citations.len(), 1);
        assert_eq!(
            answer.citations[0].evidence.excerpt,
            "grounded_process_evidence is exact and retained."
        );
        assert!(!events.iter().any(|event| matches!(
            event.detail,
            ResearchProcessEventDetail::UntrustedFinalOutput { .. }
        )));
    }

    #[test]
    fn grounded_fake_process_persists_complete_history_before_delivery() {
        let root = tempfile::tempdir().expect("fake durable CLI root");
        let (invocation, handle) = grounded_invocation(&root);
        let result = serde_json::json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": format!("Durable evidence [[cite:{handle}]]."),
        });
        let binary = write_fake_cli(
            root.path(),
            &format!("cat >/dev/null\nprintf '%s\\n' '{}'", result),
        );
        let manager = test_manager(binary, &root, Duration::from_secs(2));
        let history = Database::initialize(DatabasePaths::new(root.path().join("history")))
            .expect("history database");
        let input = request("Persist this answer.");
        history
            .client()
            .create_research_run(CreateResearchRunWrite {
                id: input.run_id.clone(),
                rerun_of_id: None,
                query: input.prompt.clone(),
                requested_model: None,
                requested_effort: None,
                now_ms: 10,
            })
            .expect("create durable run");
        let (sender, receiver) = mpsc::channel();
        manager
            .start_with_invocation(
                input.clone(),
                invocation,
                Arc::new(DurableProcessEventSink {
                    database: history.client(),
                    clock: Arc::new(FixedClock(20)),
                    downstream: Arc::new(ChannelSink { sender }),
                }),
            )
            .expect("start durable fake Claude");
        let (_, terminals) = receive_terminals(&receiver, 1);
        assert_eq!(
            terminals.get(&input.run_id),
            Some(&ResearchProcessOutcome::Succeeded)
        );
        let stored = history
            .client()
            .load_research_run(input.run_id)
            .expect("load durable process history");
        assert_eq!(
            serde_json::to_value(stored.summary.status).expect("serialize stored status"),
            serde_json::json!("COMPLETED")
        );
        assert!(stored.final_answer.is_some());
        assert!(stored.events.iter().any(|event| {
            event.get("kind").and_then(Value::as_str) == Some("GROUNDED_FINAL_OUTPUT")
        }));
        assert!(!stored.events.iter().any(|event| {
            event.get("kind").and_then(Value::as_str) == Some("UNTRUSTED_FINAL_OUTPUT")
        }));
    }

    #[test]
    fn drains_a_stdout_burst_larger_than_the_channel_before_joining() {
        let root = tempfile::tempdir().expect("fake CLI root");
        let binary = write_fake_cli(
            root.path(),
            r#"
cat >/dev/null
i=0
while [ "$i" -lt 200 ]; do
  printf '%s\n' '{"type":"rate_limit_event"}'
  i=$((i + 1))
done
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"done"}'
"#,
        );
        let manager = test_manager(binary, &root, Duration::from_secs(2));
        let (sender, receiver) = mpsc::channel();
        let started = start_fake(&manager, request("burst"), &sender);
        let (_, terminals) = receive_terminals(&receiver, 1);

        assert_eq!(
            terminals.get(&started.run_id),
            Some(&ResearchProcessOutcome::Succeeded)
        );
    }

    #[test]
    fn streams_only_compact_authorized_tool_activity() {
        let root = tempfile::tempdir().expect("fake CLI root");
        let binary = write_fake_cli(
            root.path(),
            r#"
cat >/dev/null
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"mcp__kosh__kosh_v1_exact_search"}}}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_stop","index":0}}'
printf '%s\n' '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"private evidence payload"}]}]}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"done"}'
"#,
        );
        let manager = test_manager(binary, &root, Duration::from_secs(2));
        let (sender, receiver) = mpsc::channel();
        let started = start_fake(&manager, request("tool activity"), &sender);
        let (events, terminals) = receive_terminals(&receiver, 1);
        let phases = events
            .iter()
            .filter_map(|event| match &event.detail {
                ResearchProcessEventDetail::ToolActivity { tool, phase } => {
                    Some((tool.clone(), *phase))
                }
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            terminals.get(&started.run_id),
            Some(&ResearchProcessOutcome::Succeeded)
        );
        assert_eq!(
            phases,
            vec![
                (
                    "mcp__kosh__kosh_v1_exact_search".into(),
                    ResearchToolActivityPhase::Started
                ),
                (
                    "mcp__kosh__kosh_v1_exact_search".into(),
                    ResearchToolActivityPhase::Finished
                ),
            ]
        );
        assert!(!serde_json::to_string(&events)
            .expect("serialize events")
            .contains("private evidence payload"));
    }

    #[test]
    fn redacts_run_secrets_from_visible_output() {
        let root = tempfile::tempdir().expect("fake CLI root");
        let binary = write_fake_cli(
            root.path(),
            r#"
cat >/dev/null
printf '%s\n' '{"type":"system","subtype":"init","model":"token-secret"}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"token-"}}}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"secret"}}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"token-secret"}'
"#,
        );
        let manager = test_manager(binary, &root, Duration::from_secs(2));
        let (sender, receiver) = mpsc::channel();
        let input = request("redact");
        let run_id = input.run_id.clone();
        manager
            .start_with_invocation(
                input,
                CliInvocation {
                    arguments: Vec::new(),
                    environment: Vec::new(),
                    sensitive_values: vec!["token-secret".into()],
                    citation_registry: None,
                    keepalive: Box::new(()),
                },
                Arc::new(ChannelSink { sender }),
            )
            .expect("start redaction fixture");
        let (events, terminals) = receive_terminals(&receiver, 1);
        let encoded = serde_json::to_string(&events).expect("serialize events");
        let streamed = events
            .iter()
            .filter_map(|event| match &event.detail {
                ResearchProcessEventDetail::UntrustedTextDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();

        assert_eq!(
            terminals.get(&run_id),
            Some(&ResearchProcessOutcome::Succeeded)
        );
        assert!(!encoded.contains("token-secret"));
        assert!(encoded.contains("[REDACTED]"));
        assert_eq!(streamed, "[REDACTED]");
        assert!(events.iter().any(|event| {
            matches!(
                &event.detail,
                ResearchProcessEventDetail::Metadata { model }
                    if model.as_deref() == Some("[REDACTED]")
            )
        }));
    }

    #[test]
    fn malformed_stream_json_fails_and_terminates_the_process_group() {
        let root = tempfile::tempdir().expect("fake CLI root");
        let binary = write_fake_cli(
            root.path(),
            r#"
printf '%s\n' '{not-json'
sleep 30
"#,
        );
        let manager = test_manager(binary, &root, Duration::from_secs(2));
        let (sender, receiver) = mpsc::channel();
        let started = start_fake(&manager, request("malformed"), &sender);
        let (events, terminals) = receive_terminals(&receiver, 1);

        assert_eq!(
            terminals.get(&started.run_id),
            Some(&ResearchProcessOutcome::Failed)
        );
        assert!(finished_error(&events, &started.run_id)
            .0
            .unwrap_or_default()
            .contains("malformed stream JSON"));
    }

    #[test]
    fn stderr_only_failure_is_bounded_and_reported() {
        let root = tempfile::tempdir().expect("fake CLI root");
        let binary = write_fake_cli(
            root.path(),
            r#"
i=0
while [ "$i" -lt 4000 ]; do
  printf 'backend-unavailable-%04d\n' "$i" >&2
  i=$((i + 1))
done
exit 9
"#,
        );
        let manager = test_manager(binary, &root, Duration::from_secs(2));
        let (sender, receiver) = mpsc::channel();
        let started = start_fake(&manager, request("stderr"), &sender);
        let (events, terminals) = receive_terminals(&receiver, 1);
        let (error, truncated) = finished_error(&events, &started.run_id);

        assert_eq!(
            terminals.get(&started.run_id),
            Some(&ResearchProcessOutcome::Failed)
        );
        assert!(truncated);
        assert!(error.unwrap_or_default().len() <= MAX_ERROR_BYTES);
    }

    #[test]
    fn immediate_cancel_emits_one_canceled_terminal() {
        let root = tempfile::tempdir().expect("fake CLI root");
        let binary = write_fake_cli(
            root.path(),
            r#"
trap 'exit 0' TERM
cat >/dev/null
sleep 30
"#,
        );
        let manager = test_manager(binary, &root, Duration::from_secs(2));
        let (sender, receiver) = mpsc::channel();
        let started = start_fake(&manager, request("cancel"), &sender);
        assert!(manager.cancel(&started.run_id).expect("cancel run"));
        let (mut events, terminals) = receive_terminals(&receiver, 1);
        thread::sleep(Duration::from_millis(50));
        events.extend(receiver.try_iter());

        assert_eq!(
            terminals.get(&started.run_id),
            Some(&ResearchProcessOutcome::Canceled)
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| {
                    event.run_id == started.run_id
                        && matches!(event.detail, ResearchProcessEventDetail::Finished { .. })
                })
                .count(),
            1
        );
    }

    #[test]
    fn continuous_stdout_cannot_starve_cancellation_force_kill() {
        let root = tempfile::tempdir().expect("fake CLI root");
        let binary = write_fake_cli(
            root.path(),
            r#"
trap '' TERM
cat >/dev/null
while :; do
  printf '%s\n' '{"type":"rate_limit_event"}'
done
"#,
        );
        let manager = test_manager(binary, &root, Duration::from_secs(2));
        let (sender, receiver) = mpsc::channel();
        let started = start_fake(&manager, request("continuous output"), &sender);
        assert!(manager.cancel(&started.run_id).expect("cancel run"));
        let (_, terminals) = receive_terminals(&receiver, 1);

        assert_eq!(
            terminals.get(&started.run_id),
            Some(&ResearchProcessOutcome::Canceled)
        );
    }

    #[test]
    fn starting_a_new_run_replaces_the_active_run() {
        let root = tempfile::tempdir().expect("fake CLI root");
        let binary = write_fake_cli(
            root.path(),
            r#"
prompt=$(cat)
if [ "$prompt" = "slow" ]; then
  trap 'exit 0' TERM
  sleep 30
else
  printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"done"}'
fi
"#,
        );
        let manager = test_manager(binary, &root, Duration::from_secs(2));
        let (sender, receiver) = mpsc::channel();
        let first = start_fake(&manager, request("slow"), &sender);
        let second = start_fake(&manager, request("fast"), &sender);
        let (_, terminals) = receive_terminals(&receiver, 2);

        assert_eq!(second.replaced_run_id, Some(first.run_id.clone()));
        assert_eq!(
            terminals.get(&first.run_id),
            Some(&ResearchProcessOutcome::Replaced)
        );
        assert_eq!(
            terminals.get(&second.run_id),
            Some(&ResearchProcessOutcome::Succeeded)
        );
    }

    #[test]
    fn reusing_an_active_run_id_is_rejected_without_ambiguous_events() {
        let root = tempfile::tempdir().expect("fake CLI root");
        let binary = write_fake_cli(
            root.path(),
            r#"
trap 'exit 0' TERM
cat >/dev/null
sleep 30
"#,
        );
        let manager = test_manager(binary, &root, Duration::from_secs(2));
        let (sender, receiver) = mpsc::channel();
        let first = start_fake(&manager, request("original"), &sender);
        let duplicate = StartResearchProcessInput {
            run_id: first.run_id.clone(),
            ..request("duplicate")
        };

        let error = manager
            .start_with_invocation(
                duplicate,
                invocation(),
                Arc::new(ChannelSink {
                    sender: sender.clone(),
                }),
            )
            .expect_err("active run ID must not be reused");
        assert_eq!(error.code, ClaudeProcessErrorCode::InvalidInput);
        assert!(error.message.contains("already active"));
        assert!(manager.cancel(&first.run_id).expect("cancel original run"));

        let (mut events, terminals) = receive_terminals(&receiver, 1);
        thread::sleep(Duration::from_millis(50));
        events.extend(receiver.try_iter());
        assert_eq!(
            terminals.get(&first.run_id),
            Some(&ResearchProcessOutcome::Canceled)
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| {
                    event.run_id == first.run_id
                        && matches!(event.detail, ResearchProcessEventDetail::Started)
                })
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| {
                    event.run_id == first.run_id
                        && matches!(event.detail, ResearchProcessEventDetail::Finished { .. })
                })
                .count(),
            1
        );
    }

    #[test]
    fn timeout_terminates_a_stuck_process() {
        let root = tempfile::tempdir().expect("fake CLI root");
        let binary = write_fake_cli(
            root.path(),
            r#"
trap 'exit 0' TERM
cat >/dev/null
sleep 30
"#,
        );
        let manager = test_manager(binary, &root, Duration::from_millis(80));
        let (sender, receiver) = mpsc::channel();
        let started = start_fake(&manager, request("timeout"), &sender);
        let (_, terminals) = receive_terminals(&receiver, 1);

        assert_eq!(
            terminals.get(&started.run_id),
            Some(&ResearchProcessOutcome::TimedOut)
        );
    }

    #[test]
    fn shutdown_terminates_the_active_process_and_rejects_new_work() {
        let root = tempfile::tempdir().expect("fake CLI root");
        let binary = write_fake_cli(
            root.path(),
            r#"
trap 'exit 0' TERM
cat >/dev/null
sleep 30
"#,
        );
        let manager = test_manager(binary, &root, Duration::from_secs(2));
        let (sender, receiver) = mpsc::channel();
        let started = start_fake(&manager, request("shutdown"), &sender);
        manager.shutdown();
        let (_, terminals) = receive_terminals(&receiver, 1);

        assert_eq!(
            terminals.get(&started.run_id),
            Some(&ResearchProcessOutcome::Shutdown)
        );
        assert_eq!(
            manager
                .start_with_invocation(
                    request("late"),
                    invocation(),
                    Arc::new(ChannelSink { sender })
                )
                .expect_err("shutdown rejects new work")
                .code,
            ClaudeProcessErrorCode::ShuttingDown
        );
    }

    #[test]
    fn shutdown_force_kills_a_process_already_in_cancellation() {
        let root = tempfile::tempdir().expect("fake CLI root");
        let binary = write_fake_cli(
            root.path(),
            r#"
trap '' TERM
cat >/dev/null
sleep 30
"#,
        );
        let manager = test_manager(binary, &root, Duration::from_secs(2));
        let (sender, receiver) = mpsc::channel();
        let started = start_fake(&manager, request("cancel then shutdown"), &sender);
        let process_id = lock(&manager.inner.active)
            .as_ref()
            .expect("active process")
            .process_id;
        assert!(manager.cancel(&started.run_id).expect("cancel run"));
        manager.shutdown();
        let (_, terminals) = receive_terminals(&receiver, 1);

        assert_eq!(
            terminals.get(&started.run_id),
            Some(&ResearchProcessOutcome::Canceled)
        );
        let process_id = i32::try_from(process_id).expect("test process ID");
        // SAFETY: The negative PID is the fake CLI's isolated process group.
        let result = unsafe { libc::kill(-process_id, 0) };
        assert_eq!(result, -1, "the canceled process group must be gone");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[test]
    fn shutdown_force_kills_an_owned_group_after_parent_completion_is_observed() {
        let root = tempfile::tempdir().expect("fake CLI root");
        let binary = write_fake_cli(
            root.path(),
            r#"
trap '' TERM
cat >/dev/null
sleep 30
"#,
        );
        let manager = test_manager(binary, &root, Duration::from_secs(2));
        let (sender, receiver) = mpsc::channel();
        let started = start_fake(&manager, request("completed parent race"), &sender);
        let active = lock(&manager.inner.active)
            .as_ref()
            .expect("active process")
            .clone();
        active
            .termination
            .store(TerminationReason::Completed as u8, Ordering::Release);
        manager.shutdown();
        let (_, terminals) = receive_terminals(&receiver, 1);

        assert_eq!(
            terminals.get(&started.run_id),
            Some(&ResearchProcessOutcome::Failed)
        );
        let process_id = i32::try_from(active.process_id).expect("test process ID");
        // SAFETY: The negative PID is the fake CLI's isolated process group.
        let result = unsafe { libc::kill(-process_id, 0) };
        assert_eq!(
            result, -1,
            "the completed parent's process group must be gone"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[test]
    fn unauthorized_tool_activity_fails_closed() {
        let root = tempfile::tempdir().expect("fake CLI root");
        let binary = write_fake_cli(
            root.path(),
            r#"
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_bad","name":"WebSearch"}}}'
sleep 30
"#,
        );
        let manager = test_manager(binary, &root, Duration::from_secs(2));
        let (sender, receiver) = mpsc::channel();
        let started = start_fake(&manager, request("tool"), &sender);
        let (events, terminals) = receive_terminals(&receiver, 1);

        assert_eq!(
            terminals.get(&started.run_id),
            Some(&ResearchProcessOutcome::Failed)
        );
        assert!(finished_error(&events, &started.run_id)
            .0
            .unwrap_or_default()
            .contains("unauthorized tool"));
    }
}

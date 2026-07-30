use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::{App, AppHandle, Listener, Manager};

use crate::{
    database::{
        CitationState, DatabaseClient, DatabaseDiagnostics, LexicalSearchMode, SearchExecutionMode,
        SearchPassagesInput, SemanticSearchReadiness,
    },
    runtime::RuntimeState,
};

const RECEIPT_ENV: &str = "KOSH_STARTUP_SMOKE_RECEIPT";
const HEAD_ENV: &str = "KOSH_STARTUP_SMOKE_HEAD";
const EXPECT_ENV: &str = "KOSH_STARTUP_SMOKE_EXPECT";
const READY_EVENT: &str = "kosh://startup-smoke-ready";
const READY_TIMEOUT: Duration = Duration::from_secs(30);
const DEVELOPMENT_FRONTEND_ORIGIN: &str = "http://127.0.0.1:1420";
const RELEASE_FRONTEND_ORIGIN: &str = "tauri://localhost";
const CANARY: &str = "koshstartupcanaryv1";
const CANARY_TITLE: &str = "Kosh progressive startup canary";
const CANARY_SOURCE_URL: &str = "https://example.invalid/kosh-progressive-operability";
const REQUIRED_SURFACES: [&str; 2] = ["main", "quick-add"];
static CAPTURE_REQUESTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum CanaryExpectation {
    Absent,
    Present,
    Ensure,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CanaryEvidence {
    tidbit_id: String,
    revision_id: String,
    passage_id: String,
    source_url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WebviewCanaryEvidence {
    citation_state: String,
    execution_mode: String,
    passage_id: String,
    resolved_passage_id: String,
    revision_id: String,
    result_count: usize,
    source_url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WebviewReady {
    surface: String,
    rendered: bool,
    capture_created: bool,
    document_ready_state: String,
    root_child_count: u32,
    frontend_origin: String,
    probe_data_dir: String,
    probe_request_id: String,
    canary: Option<WebviewCanaryEvidence>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartupSmokeReceipt {
    schema_version: u32,
    head_sha: String,
    expectation: CanaryExpectation,
    data_dir: String,
    process_id: u32,
    completed_at_ms: u128,
    windows: Vec<String>,
    webviews: Vec<WebviewReady>,
    diagnostics: DatabaseDiagnostics,
    canary_preexisting: bool,
    canary_created: bool,
    canary: CanaryEvidence,
}

#[derive(Debug)]
struct StartupSmokeRequest {
    receipt_path: PathBuf,
    head_sha: String,
    expectation: CanaryExpectation,
}

#[derive(Debug)]
struct PreparedCanary {
    preexisting: bool,
    evidence: Option<CanaryEvidence>,
}

pub(crate) fn canary_query_if_requested() -> Option<String> {
    std::env::var_os(RECEIPT_ENV)
        .filter(|value| !value.is_empty())
        .map(|_| CANARY.to_owned())
}

pub(crate) fn capture_requested_if_requested() -> bool {
    canary_query_if_requested().is_some() && CAPTURE_REQUESTED.load(Ordering::SeqCst)
}

pub(crate) fn run_if_requested(app: &App) -> io::Result<bool> {
    let Some(receipt_path) = std::env::var_os(RECEIPT_ENV).map(PathBuf::from) else {
        return Ok(false);
    };
    if receipt_path.as_os_str().is_empty() {
        return Err(invalid("the startup smoke receipt path is empty"));
    }

    let head_sha = required_environment_text(HEAD_ENV)?;
    if head_sha.len() != 40 || !head_sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid(
            "the startup smoke head must be a 40-character hexadecimal Git SHA",
        ));
    }
    let expectation = match required_environment_text(EXPECT_ENV)?.as_str() {
        "absent" => CanaryExpectation::Absent,
        "present" => CanaryExpectation::Present,
        "ensure" => CanaryExpectation::Ensure,
        _ => {
            return Err(invalid(
                "the startup smoke expectation must be absent, present, or ensure",
            ));
        }
    };
    let request = StartupSmokeRequest {
        receipt_path,
        head_sha,
        expectation,
    };
    let prepared = prepare_canary(app.handle(), expectation)?;

    let (ready_sender, ready_receiver) = mpsc::channel();
    app.listen(READY_EVENT, move |event| {
        let _ = ready_sender.send(event.payload().to_owned());
    });
    let app_handle = app.handle().clone();
    thread::Builder::new()
        .name("kosh-startup-smoke".into())
        .spawn(move || {
            let result = wait_for_webviews(&ready_receiver).and_then(|webviews| {
                complete_startup_smoke(&app_handle, request, prepared, webviews)
            });
            match result {
                Ok(()) => app_handle.exit(0),
                Err(error) => {
                    eprintln!("Kosh startup smoke failed: {error}");
                    app_handle.exit(1);
                }
            }
        })
        .map_err(|error| invalid(format!("could not start the startup smoke worker: {error}")))?;
    Ok(true)
}

fn complete_startup_smoke(
    app: &AppHandle,
    request: StartupSmokeRequest,
    prepared: PreparedCanary,
    mut webviews: Vec<WebviewReady>,
) -> io::Result<()> {
    let state = app.state::<RuntimeState>();
    let client = state.database_client();
    let live_evidence = find_canary(&client)?
        .ok_or_else(|| invalid("the startup smoke canary disappeared during frontend startup"))?;
    if prepared
        .evidence
        .as_ref()
        .is_some_and(|evidence| evidence != &live_evidence)
    {
        return Err(invalid(
            "the startup smoke canary changed during frontend startup",
        ));
    }
    let capture_surfaces = webviews
        .iter()
        .filter(|ready| ready.capture_created)
        .map(|ready| ready.surface.as_str())
        .collect::<Vec<_>>();
    if prepared.preexisting && !capture_surfaces.is_empty() {
        return Err(invalid(
            "a webview recreated the preexisting startup smoke canary",
        ));
    }
    if !prepared.preexisting && capture_surfaces != ["main"] {
        return Err(invalid(
            "the main webview did not create the fresh startup canary through capture IPC",
        ));
    }

    let diagnostics = client.diagnostics().map_err(database_error)?;
    if !diagnostics.main_foreign_keys
        || !diagnostics.media_foreign_keys
        || !diagnostics.main_journal_mode.eq_ignore_ascii_case("wal")
        || !diagnostics.media_journal_mode.eq_ignore_ascii_case("wal")
        || diagnostics.migration_heads.main.is_none()
        || diagnostics.migration_heads.media.is_none()
    {
        return Err(invalid(
            "the startup smoke database diagnostics are not durable and current",
        ));
    }

    let mut windows = app.webview_windows().into_keys().collect::<Vec<_>>();
    windows.sort();
    if windows != REQUIRED_SURFACES {
        return Err(invalid(format!(
            "the startup smoke expected main and quick-add windows, found {}",
            windows.join(", ")
        )));
    }

    let data_dir = fs::canonicalize(state.database_paths().root()).map_err(|error| {
        invalid(format!(
            "could not resolve the startup smoke data directory: {error}"
        ))
    })?;
    let data_dir_text = data_dir.to_string_lossy().into_owned();
    for ready in &webviews {
        let probe_data_dir = fs::canonicalize(&ready.probe_data_dir).map_err(|error| {
            invalid(format!(
                "the {} webview reported an invalid IPC data directory: {error}",
                ready.surface
            ))
        })?;
        if probe_data_dir != data_dir {
            return Err(invalid(format!(
                "the {} webview IPC probe resolved to a different data directory",
                ready.surface
            )));
        }
        let canary = ready.canary.as_ref().ok_or_else(|| {
            invalid(format!(
                "the {} webview emitted readiness without search and citation IPC evidence",
                ready.surface
            ))
        })?;
        if canary.execution_mode != "EXACT"
            || canary.citation_state != "CURRENT"
            || canary.result_count != 1
            || canary.passage_id != live_evidence.passage_id
            || canary.resolved_passage_id != live_evidence.passage_id
            || canary.revision_id != live_evidence.revision_id
            || canary.source_url != live_evidence.source_url
        {
            return Err(invalid(format!(
                "the {} webview search or citation IPC evidence did not resolve the startup canary",
                ready.surface
            )));
        }
    }
    webviews.sort_by(|left, right| left.surface.cmp(&right.surface));
    let completed_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| invalid(format!("the system clock is invalid: {error}")))?
        .as_millis();
    write_receipt(
        &request.receipt_path,
        &StartupSmokeReceipt {
            schema_version: 3,
            head_sha: request.head_sha,
            expectation: request.expectation,
            data_dir: data_dir_text,
            process_id: std::process::id(),
            completed_at_ms,
            windows,
            webviews,
            diagnostics,
            canary_preexisting: prepared.preexisting,
            canary_created: !prepared.preexisting,
            canary: live_evidence,
        },
    )
}

fn prepare_canary(app: &AppHandle, expectation: CanaryExpectation) -> io::Result<PreparedCanary> {
    let client = app.state::<RuntimeState>().database_client();
    let existing = find_canary(&client)?;
    match (expectation, existing.is_some()) {
        (CanaryExpectation::Absent, true) => {
            return Err(invalid(
                "the startup smoke canary unexpectedly existed before the fresh launch",
            ));
        }
        (CanaryExpectation::Present, false) => {
            return Err(invalid(
                "the startup smoke canary did not survive the previous launch",
            ));
        }
        (CanaryExpectation::Ensure, _) | (_, false) | (_, true) => {}
    }

    let capture_requested = existing.is_none()
        && matches!(
            expectation,
            CanaryExpectation::Absent | CanaryExpectation::Ensure
        );
    CAPTURE_REQUESTED.store(capture_requested, Ordering::SeqCst);
    Ok(PreparedCanary {
        preexisting: existing.is_some(),
        evidence: existing,
    })
}

fn wait_for_webviews(receiver: &mpsc::Receiver<String>) -> io::Result<Vec<WebviewReady>> {
    let deadline = Instant::now() + READY_TIMEOUT;
    let mut ready_by_surface = HashMap::new();
    while ready_by_surface.len() < REQUIRED_SURFACES.len() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| readiness_timeout(&ready_by_surface))?;
        let payload = receiver
            .recv_timeout(remaining)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => readiness_timeout(&ready_by_surface),
                mpsc::RecvTimeoutError::Disconnected => {
                    invalid("the startup smoke readiness channel disconnected")
                }
            })?;
        let ready: WebviewReady = serde_json::from_str(&payload).map_err(|error| {
            invalid(format!(
                "a webview emitted an invalid startup readiness payload: {error}"
            ))
        })?;
        if !REQUIRED_SURFACES.contains(&ready.surface.as_str()) {
            return Err(invalid(format!(
                "an unknown webview emitted startup readiness: {}",
                ready.surface
            )));
        }
        if !ready.rendered || ready.root_child_count == 0 {
            return Err(invalid(format!(
                "the {} webview emitted readiness without a rendered React root",
                ready.surface
            )));
        }
        if !matches!(
            ready.document_ready_state.as_str(),
            "interactive" | "complete"
        ) {
            return Err(invalid(format!(
                "the {} webview emitted readiness while the document was {}",
                ready.surface, ready.document_ready_state
            )));
        }
        if !valid_frontend_origin(&ready.frontend_origin) {
            return Err(invalid(format!(
                "the {} webview loaded from unexpected frontend origin {}",
                ready.surface, ready.frontend_origin
            )));
        }
        if ready.probe_data_dir.is_empty() || ready.probe_request_id.is_empty() {
            return Err(invalid(format!(
                "the {} webview emitted readiness without IPC probe evidence",
                ready.surface
            )));
        }
        ready_by_surface
            .entry(ready.surface.clone())
            .or_insert(ready);
    }

    let probe_ids = ready_by_surface
        .values()
        .map(|ready| ready.probe_request_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    if probe_ids.len() != REQUIRED_SURFACES.len() {
        return Err(invalid(
            "the webview readiness IPC probes did not have distinct request IDs",
        ));
    }
    Ok(ready_by_surface.into_values().collect())
}

fn valid_frontend_origin(origin: &str) -> bool {
    if cfg!(debug_assertions) {
        origin == DEVELOPMENT_FRONTEND_ORIGIN
    } else {
        origin == RELEASE_FRONTEND_ORIGIN
    }
}

fn readiness_timeout(ready: &HashMap<String, WebviewReady>) -> io::Error {
    let mut missing = REQUIRED_SURFACES
        .into_iter()
        .filter(|surface| !ready.contains_key(*surface))
        .collect::<Vec<_>>();
    missing.sort();
    invalid(format!(
        "timed out waiting for rendered webview readiness: {}",
        missing.join(", ")
    ))
}

fn find_canary(client: &DatabaseClient) -> io::Result<Option<CanaryEvidence>> {
    let response = client
        .search_passages_with_semantics(
            SearchPassagesInput {
                query: CANARY.into(),
                mode: LexicalSearchMode::Exact,
                limit: 10,
            },
            None,
            SemanticSearchReadiness::NotRequested,
        )
        .map_err(database_error)?;
    if response.execution_mode != SearchExecutionMode::Exact {
        return Err(invalid(
            "the startup smoke canary query did not execute in exact lexical mode",
        ));
    }

    let matches = response
        .results
        .into_iter()
        .filter(|result| result.citation.excerpt.contains(CANARY))
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err(invalid(
            "the startup smoke canary resolved to more than one passage",
        ));
    }
    let Some(result) = matches.into_iter().next() else {
        return Ok(None);
    };
    if result.citation.state != CitationState::Current {
        return Err(invalid("the startup smoke canary citation is not current"));
    }
    let tidbit = result
        .citation
        .tidbit
        .ok_or_else(|| invalid("the startup smoke citation has no authored tidbit"))?;
    let source_url = result
        .citation
        .sources
        .iter()
        .find_map(|source| source.url.as_deref())
        .filter(|url| *url == CANARY_SOURCE_URL)
        .ok_or_else(|| invalid("the startup smoke citation lost its source URL"))?;
    let loaded = client
        .load_tidbit(tidbit.id.clone())
        .map_err(database_error)?;
    if loaded.current_revision_id != tidbit.revision_id
        || loaded.body_markdown != CANARY
        || loaded.title.as_deref() != Some(CANARY_TITLE)
    {
        return Err(invalid(
            "the startup smoke citation did not resolve to the stored authored revision",
        ));
    }

    Ok(Some(CanaryEvidence {
        tidbit_id: tidbit.id,
        revision_id: tidbit.revision_id,
        passage_id: result.passage_id,
        source_url: source_url.into(),
    }))
}

fn write_receipt(path: &Path, receipt: &StartupSmokeReceipt) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| invalid("the startup smoke receipt must have a parent directory"))?;
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("the startup smoke receipt filename is invalid"))?;
    let temporary = parent.join(format!(".{file_name}.{}.tmp", uuid::Uuid::now_v7()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        serde_json::to_writer_pretty(&mut file, receipt)
            .map_err(|error| invalid(format!("could not serialize startup receipt: {error}")))?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn required_environment_text(name: &str) -> io::Result<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid(format!("{name} is required for startup smoke mode")))
}

fn database_error(error: crate::database::DatabaseError) -> io::Error {
    invalid(format!("startup smoke database operation failed: {error}"))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

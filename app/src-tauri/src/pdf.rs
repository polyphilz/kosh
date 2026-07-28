use std::{
    fs::{self, File, OpenOptions},
    io::{Cursor, Read, Write},
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{
    database::{
        media::{
            IngestAttachmentMetadata, IngestPdfWrite, PdfExtractionJob, PdfPageExtraction,
            PdfPageSource, StagedAttachment,
        },
        DatabaseClient, DatabaseError, PdfRecord, PdfStatusRecord,
    },
    runtime::RuntimeState,
};

const MAX_PDF_BYTES: usize = 32 * 1024 * 1024;
const MAX_PDF_PAGES: usize = 2_000;
const MAX_OCR_PAGES: usize = 128;
const MIN_NATIVE_TEXT_CHARS: usize = 24;
const MAX_RENDERED_PAGE_BYTES: usize = 48 * 1024 * 1024;
const PDF_RECONCILIATION_INTERVAL: Duration = Duration::from_secs(5 * 60);
const PDF_STALE_ATTEMPT_AGE: Duration = Duration::from_secs(5 * 60);
const PDF_DROP_EVENT: &str = "kosh://pdf-drop";
const MAX_PDF_OPEN_RECOVERY_FILES: usize = 256;
const PDF_INSPECTION_WORKER_ARG: &str = "--kosh-pdf-inspection-worker";
const PDF_EXTRACTION_WORKER_ARG: &str = "--kosh-pdf-extraction-worker";
const PDF_EXTRACTION_WORKER_TIMEOUT: Duration = Duration::from_secs(2 * 60);
const MAX_PDF_WORKER_OUTPUT_BYTES: u64 = 32 * 1024 * 1024;
#[cfg(unix)]
const PDF_WORKER_CPU_SECONDS: libc::rlim_t = 90;
const PDF_WORKER_MEMORY_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(
    tag = "operation",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase"
)]
enum PdfWorkerResponse {
    Inspection {
        result: std::result::Result<u32, String>,
    },
    Extraction {
        result: std::result::Result<Vec<PdfPageExtraction>, String>,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PdfDropNotice {
    pub selections: Vec<PdfDropSelection>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PdfDropSelection {
    pub selection_id: String,
    pub filename: String,
}

#[derive(Clone, Copy, Debug)]
enum PdfWorkerSignal {
    WorkAvailable,
}

pub(crate) struct PdfExtractionCoordinator {
    sender: Option<mpsc::SyncSender<PdfWorkerSignal>>,
}

impl PdfExtractionCoordinator {
    pub(crate) fn start(client: DatabaseClient) -> Result<Self, DatabaseError> {
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("kosh-pdf-extraction".into())
            .spawn(move || pdf_worker(client, receiver))?;
        let coordinator = Self {
            sender: Some(sender),
        };
        coordinator.wake();
        Ok(coordinator)
    }

    pub(crate) fn disabled() -> Self {
        Self { sender: None }
    }

    pub(crate) fn wake(&self) {
        let Some(sender) = &self.sender else {
            return;
        };
        match sender.try_send(PdfWorkerSignal::WorkAvailable) {
            Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
            Err(mpsc::TrySendError::Disconnected(_)) => {
                log::error!("PDF extraction worker is unavailable");
            }
        }
    }
}

#[tauri::command]
pub(crate) async fn select_pdf<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, RuntimeState>,
) -> Result<Option<String>, crate::database::commands::CommandError> {
    let Some(path) = select_pdf_file(&app).await? else {
        return Ok(None);
    };
    state
        .register_pdf_selection(path)
        .map(Some)
        .map_err(Into::into)
}

#[tauri::command]
pub(crate) async fn ingest_selected_pdf(
    state: State<'_, RuntimeState>,
    draft_id: String,
    selection_id: String,
) -> Result<PdfRecord, crate::database::commands::CommandError> {
    let path = state.take_pdf_selection(&selection_id)?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Document.pdf")
        .to_owned();
    let raw = tauri::async_runtime::spawn_blocking(move || read_bounded_pdf(&path))
        .await
        .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
        .map_err(crate::database::commands::CommandError::from)?;
    let client = state.database_client();
    let now_ms = state.now_ms();
    let limits = state.media_limits();
    let staging_directory = state.media_staging_directory();
    let mut ids = state.next_ids(4).into_iter();
    let stage_id = ids.next().expect("requested PDF staging ID");
    let attachment_id = ids.next().expect("requested PDF attachment ID");
    let ingest_lease_id = ids.next().expect("requested PDF lease ID");
    let extraction_id = ids.next().expect("requested PDF extraction ID");
    let record = tauri::async_runtime::spawn_blocking(move || {
        let page_count = inspect_pdf_isolated(&raw)?;
        let staged = StagedAttachment::from_reader(
            Cursor::new(raw),
            &staging_directory,
            &stage_id,
            limits.max_attachment_bytes,
        )?;
        client.ingest_pdf(IngestPdfWrite {
            attachment: staged.write(IngestAttachmentMetadata {
                attachment_id,
                ingest_lease_id,
                draft_id,
                display_filename: safe_pdf_filename(&filename),
                media_type: "application/pdf".into(),
                now_ms,
                limits,
            }),
            extraction_id,
            page_count: u32::try_from(page_count).expect("validated page count fits u32"),
        })
    })
    .await
    .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
    .map_err(crate::database::commands::CommandError::from)?;
    state.wake_pdf_extraction();
    Ok(record)
}

#[tauri::command]
pub(crate) async fn pdf_status(
    state: State<'_, RuntimeState>,
    attachment_id: String,
) -> Result<PdfStatusRecord, crate::database::commands::CommandError> {
    let client = state.database_client();
    tauri::async_runtime::spawn_blocking(move || client.load_pdf_status(attachment_id))
        .await
        .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
        .map_err(Into::into)
}

#[tauri::command]
pub(crate) async fn retry_pdf_extraction(
    state: State<'_, RuntimeState>,
    attachment_id: String,
) -> Result<PdfStatusRecord, crate::database::commands::CommandError> {
    let client = state.database_client();
    let now_ms = state.now_ms();
    let status = tauri::async_runtime::spawn_blocking(move || {
        client.retry_pdf_extraction(attachment_id, now_ms)
    })
    .await
    .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
    .map_err(crate::database::commands::CommandError::from)?;
    state.wake_pdf_extraction();
    Ok(status)
}

#[tauri::command]
pub(crate) async fn open_pdf_external<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, RuntimeState>,
    attachment_id: String,
) -> Result<(), crate::database::commands::CommandError> {
    let client = state.database_client();
    let now_ms = state.now_ms();
    let max_bytes = state.media_limits().max_protocol_response_bytes;
    let directory = state.pdf_open_directory();
    let path = tauri::async_runtime::spawn_blocking(move || {
        let payload = client.load_media_payload(attachment_id.clone(), now_ms, None, max_bytes)?;
        if payload.media_type != "application/pdf"
            || payload.range.start != 0
            || payload.bytes.len() as u64 != payload.total_byte_length
        {
            return Err(DatabaseError::InvalidInput(
                "only complete PDF attachments can be opened externally".into(),
            ));
        }
        fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{attachment_id}.pdf"));
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        file.write_all(&payload.bytes)?;
        file.sync_all()?;
        Ok::<_, DatabaseError>(path)
    })
    .await
    .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
    .map_err(crate::database::commands::CommandError::from)?;
    open_path_on_main_thread(&app, path).await
}

pub(crate) fn handle_pdf_drop<R: tauri::Runtime>(
    window: &tauri::Window<R>,
    event: &tauri::WindowEvent,
) {
    let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event else {
        return;
    };
    let Some(state) = window.try_state::<RuntimeState>() else {
        return;
    };
    let selections = paths
        .iter()
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        })
        .take(8)
        .filter_map(|path| {
            let selection_id = match state.register_pdf_selection(path.clone()) {
                Ok(id) => id,
                Err(error) => {
                    log::warn!("could not register a dropped PDF: {error}");
                    return None;
                }
            };
            Some(PdfDropSelection {
                selection_id,
                filename: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Dropped PDF")
                    .to_owned(),
            })
        })
        .collect::<Vec<_>>();
    if selections.is_empty() {
        return;
    }
    if let Err(error) = window.emit(PDF_DROP_EVENT, PdfDropNotice { selections }) {
        log::warn!("could not notify the editor about a native PDF drop: {error}");
    }
}

pub(crate) fn recover_pdf_open_directory(path: &Path) -> Result<usize, DatabaseError> {
    fs::create_dir_all(path)?;
    let mut removed = 0;
    for entry in fs::read_dir(path)? {
        if removed >= MAX_PDF_OPEN_RECOVERY_FILES {
            break;
        }
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let filename = entry.file_name();
        let Some(stem) = filename
            .to_str()
            .and_then(|filename| filename.strip_suffix(".pdf"))
        else {
            continue;
        };
        if uuid::Uuid::parse_str(stem)
            .ok()
            .is_none_or(|id| id.get_version_num() != 7 || id.hyphenated().to_string() != stem)
        {
            continue;
        }
        fs::remove_file(entry.path())?;
        removed += 1;
    }
    Ok(removed)
}

fn read_bounded_pdf(path: &Path) -> Result<Vec<u8>, DatabaseError> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(DatabaseError::InvalidInput(
            "the selected PDF is not a regular file".into(),
        ));
    }
    if metadata.len() == 0 || metadata.len() > MAX_PDF_BYTES as u64 {
        return Err(DatabaseError::InvalidInput(format!(
            "the selected PDF must contain between 1 and {MAX_PDF_BYTES} bytes"
        )));
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len())
            .map_err(|_| DatabaseError::InvalidInput("the PDF does not fit memory".into()))?,
    );
    File::open(path)?
        .take(MAX_PDF_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_PDF_BYTES {
        return Err(DatabaseError::InvalidInput(
            "the selected PDF changed while it was being read".into(),
        ));
    }
    Ok(bytes)
}

fn safe_pdf_filename(value: &str) -> String {
    let mut filename = value
        .chars()
        .filter(|character| !character.is_control() && *character != '/' && *character != '\\')
        .take(255)
        .collect::<String>();
    filename = filename.trim().trim_start_matches('.').to_owned();
    if filename.is_empty() {
        return "Document.pdf".into();
    }
    if !filename.to_ascii_lowercase().ends_with(".pdf") {
        filename = filename.chars().take(251).collect();
        filename.push_str(".pdf");
    }
    filename
}

fn pdf_worker(client: DatabaseClient, receiver: mpsc::Receiver<PdfWorkerSignal>) {
    let mut last_reconciliation = None;
    loop {
        let timeout = last_reconciliation
            .map(|instant: std::time::Instant| {
                PDF_RECONCILIATION_INTERVAL.saturating_sub(instant.elapsed())
            })
            .unwrap_or(Duration::ZERO);
        match receiver.recv_timeout(timeout) {
            Ok(PdfWorkerSignal::WorkAvailable) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        let now_ms = match system_now_ms() {
            Ok(value) => value,
            Err(error) => {
                log::error!("could not read the clock for PDF extraction: {error}");
                continue;
            }
        };
        let stale_before = now_ms
            .saturating_sub(i64::try_from(PDF_STALE_ATTEMPT_AGE.as_millis()).unwrap_or(i64::MAX));
        if let Err(error) = client.recover_interrupted_pdf_extraction(stale_before, now_ms) {
            log::error!("could not recover interrupted PDF extraction: {error}");
        }
        last_reconciliation = Some(std::time::Instant::now());
        drain_pdf_queue(&client);
    }
}

fn drain_pdf_queue(client: &DatabaseClient) {
    loop {
        let now_ms = match system_now_ms() {
            Ok(value) => value,
            Err(error) => {
                log::error!("could not read the PDF extraction clock: {error}");
                return;
            }
        };
        let job = match client.claim_next_pdf_extraction(now_ms) {
            Ok(Some(job)) => job,
            Ok(None) => return,
            Err(error) => {
                log::error!("could not claim the next PDF extraction job: {error}");
                return;
            }
        };
        let result = catch_unwind(AssertUnwindSafe(|| extract_pdf_pages_isolated(&job)))
            .unwrap_or_else(|_| Err("the PDF extractor panicked".into()));
        let completed_at_ms = match system_now_ms() {
            Ok(value) => value,
            Err(error) => {
                log::error!("could not read the PDF extraction completion clock: {error}");
                return;
            }
        };
        let attachment_id = job.attachment_id.clone();
        if let Err(error) = client.complete_pdf_extraction(job, result, completed_at_ms) {
            log::error!("could not persist extraction for PDF {attachment_id}: {error}");
            return;
        }
    }
}

fn extract_pdf_pages_isolated(
    job: &PdfExtractionJob,
) -> std::result::Result<Vec<PdfPageExtraction>, String> {
    let arguments = [
        PDF_EXTRACTION_WORKER_ARG.to_owned(),
        job.page_count.to_string(),
    ];
    match run_isolated_pdf_worker(&arguments, &job.pdf_bytes)? {
        PdfWorkerResponse::Extraction { result } => result,
        PdfWorkerResponse::Inspection { .. } => {
            Err("isolated PDF extractor returned an inspection response".into())
        }
    }
}

fn inspect_pdf_isolated(bytes: &[u8]) -> Result<usize, DatabaseError> {
    let arguments = [PDF_INSPECTION_WORKER_ARG.to_owned()];
    match run_isolated_pdf_worker(&arguments, bytes).map_err(DatabaseError::InvalidInput)? {
        PdfWorkerResponse::Inspection { result } => result
            .map(|page_count| page_count as usize)
            .map_err(DatabaseError::InvalidInput),
        PdfWorkerResponse::Extraction { .. } => Err(DatabaseError::InvalidInput(
            "isolated PDF inspector returned an extraction response".into(),
        )),
    }
}

fn run_isolated_pdf_worker(
    arguments: &[String],
    pdf_bytes: &[u8],
) -> std::result::Result<PdfWorkerResponse, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not locate the PDF helper: {error}"))?;
    let mut child = Command::new(executable)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not start the isolated PDF worker: {error}"))?;
    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| "isolated PDF worker stdin was unavailable".to_owned())
        .and_then(|mut stdin| {
            stdin
                .write_all(pdf_bytes)
                .map_err(|error| format!("could not send the PDF to its worker: {error}"))
        });
    if let Err(error) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "isolated PDF worker stdout was unavailable".to_owned())?;
    let output_reader = thread::Builder::new()
        .name("kosh-pdf-extractor-output".into())
        .spawn(move || {
            let mut output = Vec::new();
            stdout
                .take(MAX_PDF_WORKER_OUTPUT_BYTES + 1)
                .read_to_end(&mut output)
                .map(|_| output)
        })
        .map_err(|error| format!("could not read isolated PDF worker output: {error}"))?;

    let deadline = Instant::now() + PDF_EXTRACTION_WORKER_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                match pdf_worker_physical_footprint(child.id()) {
                    Ok(bytes) if bytes > PDF_WORKER_MEMORY_BYTES => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err("isolated PDF worker exceeded its memory limit".into());
                    }
                    Ok(_) => thread::sleep(Duration::from_millis(25)),
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(error);
                    }
                }
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("isolated PDF worker exceeded its time limit".into());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "could not monitor the isolated PDF worker: {error}"
                ));
            }
        }
    };
    let output = output_reader
        .join()
        .map_err(|_| "isolated PDF worker output reader panicked".to_owned())?
        .map_err(|error| format!("could not read isolated PDF worker output: {error}"))?;
    if output.len() as u64 > MAX_PDF_WORKER_OUTPUT_BYTES {
        return Err("isolated PDF worker exceeded its output limit".into());
    }
    if !status.success() {
        return Err(format!(
            "isolated PDF worker stopped safely with status {status}"
        ));
    }
    serde_json::from_slice::<PdfWorkerResponse>(&output)
        .map_err(|error| format!("isolated PDF worker returned invalid output: {error}"))
}

pub(crate) fn run_worker_if_requested() -> Option<i32> {
    let mut arguments = std::env::args();
    let _executable = arguments.next();
    let operation = arguments.next()?;
    if operation != PDF_INSPECTION_WORKER_ARG && operation != PDF_EXTRACTION_WORKER_ARG {
        return None;
    }
    let page_count = if operation == PDF_EXTRACTION_WORKER_ARG {
        arguments
            .next()
            .ok_or_else(|| "PDF extraction worker page count is missing".to_owned())
            .and_then(|value| {
                value
                    .parse::<u32>()
                    .map_err(|_| "PDF extraction worker page count is invalid".to_owned())
            })
            .and_then(|value| {
                if value == 0 || value as usize > MAX_PDF_PAGES {
                    Err("PDF extraction worker page count is out of bounds".into())
                } else {
                    Ok(value)
                }
            })
    } else {
        Ok(0)
    };
    let input = install_pdf_worker_resource_limits().and_then(|()| {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(MAX_PDF_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("could not read PDF worker input: {error}"))?;
        if bytes.len() > MAX_PDF_BYTES {
            return Err("PDF worker input exceeded its byte limit".into());
        }
        Ok(bytes)
    });
    let response = if operation == PDF_INSPECTION_WORKER_ARG {
        PdfWorkerResponse::Inspection {
            result: input.and_then(|bytes| {
                catch_unwind(AssertUnwindSafe(|| inspect_pdf(&bytes)))
                    .unwrap_or_else(|_| {
                        Err(DatabaseError::InvalidInput(
                            "the isolated PDF inspector panicked".into(),
                        ))
                    })
                    .and_then(|page_count| {
                        u32::try_from(page_count).map_err(|_| {
                            DatabaseError::InvalidInput(
                                "PDF page count exceeds the worker protocol".into(),
                            )
                        })
                    })
                    .map_err(|error| error.to_string())
            }),
        }
    } else {
        PdfWorkerResponse::Extraction {
            result: page_count.and_then(|page_count| {
                input.and_then(|bytes| {
                    catch_unwind(AssertUnwindSafe(|| {
                        extract_pdf_pages_from_bytes(&bytes, page_count)
                    }))
                    .unwrap_or_else(|_| Err("the isolated PDF extractor panicked".into()))
                })
            }),
        }
    };
    let serialized = match serde_json::to_vec(&response) {
        Ok(serialized) if serialized.len() as u64 <= MAX_PDF_WORKER_OUTPUT_BYTES => serialized,
        Ok(_) | Err(_) => return Some(74),
    };
    match std::io::stdout().write_all(&serialized) {
        Ok(()) => Some(0),
        Err(_) => Some(74),
    }
}

#[cfg(unix)]
fn install_pdf_worker_resource_limits() -> std::result::Result<(), String> {
    let cpu = libc::rlimit {
        rlim_cur: PDF_WORKER_CPU_SECONDS,
        rlim_max: PDF_WORKER_CPU_SECONDS,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_CPU, &cpu) } != 0 {
        return Err(format!(
            "could not install the PDF extractor CPU limit: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn pdf_worker_physical_footprint(pid: u32) -> std::result::Result<u64, String> {
    #[repr(C)]
    #[derive(Default)]
    struct RusageInfoV0 {
        uuid: [u8; 16],
        user_time: u64,
        system_time: u64,
        package_idle_wakeups: u64,
        interrupt_wakeups: u64,
        pageins: u64,
        wired_size: u64,
        resident_size: u64,
        physical_footprint: u64,
        process_start_absolute_time: u64,
        process_exit_absolute_time: u64,
    }

    unsafe extern "C" {
        fn proc_pid_rusage(
            pid: libc::c_int,
            flavor: libc::c_int,
            buffer: *mut libc::c_void,
        ) -> libc::c_int;
    }

    let mut usage = RusageInfoV0::default();
    let result =
        unsafe { proc_pid_rusage(pid as libc::c_int, 0, std::ptr::from_mut(&mut usage).cast()) };
    if result != 0 {
        return Err(format!(
            "could not monitor isolated PDF extraction memory: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(usage.physical_footprint)
}

#[cfg(not(target_os = "macos"))]
fn pdf_worker_physical_footprint(_pid: u32) -> std::result::Result<u64, String> {
    Ok(0)
}

#[cfg(not(unix))]
fn install_pdf_worker_resource_limits() -> std::result::Result<(), String> {
    Err("isolated PDF extraction requires Unix resource limits".into())
}

#[cfg(target_os = "macos")]
fn inspect_pdf(bytes: &[u8]) -> Result<usize, DatabaseError> {
    with_pdf_document(bytes, |document| {
        let encrypted = unsafe { document.isEncrypted() };
        let locked = unsafe { document.isLocked() };
        let page_count = unsafe { document.pageCount() };
        validate_pdf_document_state(encrypted, locked, page_count)
    })
}

#[cfg(not(target_os = "macos"))]
fn inspect_pdf(_bytes: &[u8]) -> Result<usize, DatabaseError> {
    Err(DatabaseError::InvalidInput(
        "native PDF ingestion is available only on macOS".into(),
    ))
}

fn validate_pdf_document_state(
    encrypted: bool,
    locked: bool,
    page_count: usize,
) -> Result<usize, DatabaseError> {
    if encrypted || locked {
        return Err(DatabaseError::InvalidInput(
            "encrypted or password-protected PDFs are not supported".into(),
        ));
    }
    if page_count == 0 || page_count > MAX_PDF_PAGES {
        return Err(DatabaseError::InvalidInput(format!(
            "PDFs must contain between 1 and {MAX_PDF_PAGES} pages"
        )));
    }
    Ok(page_count)
}

#[cfg(target_os = "macos")]
fn with_pdf_document<T>(
    bytes: &[u8],
    operation: impl FnOnce(&objc2_pdf_kit::PDFDocument) -> Result<T, DatabaseError>,
) -> Result<T, DatabaseError> {
    use objc2::{rc::autoreleasepool, AnyThread};
    use objc2_foundation::NSData;
    use objc2_pdf_kit::PDFDocument;

    if !bytes.starts_with(b"%PDF-") {
        return Err(DatabaseError::InvalidInput(
            "the selected file does not have a valid PDF signature".into(),
        ));
    }
    autoreleasepool(|_| {
        let data = NSData::with_bytes(bytes);
        let document = unsafe { PDFDocument::initWithData(PDFDocument::alloc(), &data) }
            .ok_or_else(|| DatabaseError::InvalidInput("the PDF is malformed".into()))?;
        operation(&document)
    })
}

#[cfg(all(test, target_os = "macos"))]
fn extract_pdf_pages(
    job: &PdfExtractionJob,
) -> std::result::Result<Vec<PdfPageExtraction>, String> {
    extract_pdf_pages_from_bytes(&job.pdf_bytes, job.page_count)
}

#[cfg(target_os = "macos")]
fn extract_pdf_pages_from_bytes(
    pdf_bytes: &[u8],
    page_count: u32,
) -> std::result::Result<Vec<PdfPageExtraction>, String> {
    use objc2_foundation::NSSize;
    use objc2_pdf_kit::PDFDisplayBox;

    with_pdf_document(pdf_bytes, |document| {
        if unsafe { document.isEncrypted() } || unsafe { document.isLocked() } {
            return Err(DatabaseError::InvalidInput(
                "the PDF became encrypted before extraction".into(),
            ));
        }
        if unsafe { document.pageCount() } != page_count as usize {
            return Err(DatabaseError::InvalidInput(
                "the PDF page count no longer matches ingestion metadata".into(),
            ));
        }
        let mut pages = Vec::with_capacity(page_count as usize);
        let mut ocr_pages = 0_usize;
        for index in 0..page_count as usize {
            let page_number = u32::try_from(index + 1).expect("bounded PDF page fits u32");
            let Some(page) = (unsafe { document.pageAtIndex(index) }) else {
                pages.push(PdfPageExtraction {
                    page_number,
                    result: Err("PDFKit could not load this page".into()),
                });
                continue;
            };
            let native = unsafe { page.string() }
                .map(|value| normalize_pdf_text(&value.to_string()))
                .unwrap_or_default();
            if native
                .chars()
                .filter(|character| !character.is_whitespace())
                .count()
                >= MIN_NATIVE_TEXT_CHARS
            {
                pages.push(PdfPageExtraction {
                    page_number,
                    result: Ok((PdfPageSource::NativeText, native)),
                });
                continue;
            }
            if ocr_pages >= MAX_OCR_PAGES {
                pages.push(PdfPageExtraction {
                    page_number,
                    result: finish_sparse_pdf_page(
                        native,
                        Err(format!(
                            "OCR was skipped after the {MAX_OCR_PAGES}-page safety limit"
                        )),
                    ),
                });
                continue;
            }
            ocr_pages += 1;
            let thumbnail = unsafe {
                page.thumbnailOfSize_forBox(NSSize::new(1_600.0, 2_200.0), PDFDisplayBox::MediaBox)
            };
            let Some(data) = thumbnail.TIFFRepresentation() else {
                pages.push(PdfPageExtraction {
                    page_number,
                    result: finish_sparse_pdf_page(
                        native,
                        Err("PDFKit could not render this page for OCR".into()),
                    ),
                });
                continue;
            };
            if data.len() > MAX_RENDERED_PAGE_BYTES {
                pages.push(PdfPageExtraction {
                    page_number,
                    result: finish_sparse_pdf_page(
                        native,
                        Err("rendered PDF page exceeded the OCR memory limit".into()),
                    ),
                });
                continue;
            }
            let ocr_result = crate::media::recognize_image_text(&data.to_vec())
                .map(|mut regions| {
                    regions.sort_by(|left, right| {
                        right
                            .y
                            .total_cmp(&left.y)
                            .then_with(|| left.x.total_cmp(&right.x))
                    });
                    normalize_pdf_text(
                        &regions
                            .into_iter()
                            .map(|region| region.text)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                })
                .and_then(|text| {
                    if text.is_empty() {
                        Err("Vision found no searchable text on this page".into())
                    } else {
                        Ok(text)
                    }
                });
            pages.push(PdfPageExtraction {
                page_number,
                result: finish_sparse_pdf_page(native, ocr_result),
            });
        }
        Ok(pages)
    })
    .map_err(|error| error.to_string())
}

#[cfg(not(target_os = "macos"))]
fn extract_pdf_pages_from_bytes(
    _pdf_bytes: &[u8],
    _page_count: u32,
) -> std::result::Result<Vec<PdfPageExtraction>, String> {
    Err("native PDF extraction is available only on macOS".into())
}

fn finish_sparse_pdf_page(
    native: String,
    ocr_result: std::result::Result<String, String>,
) -> std::result::Result<(PdfPageSource, String), String> {
    match ocr_result {
        Ok(ocr) => {
            let text = if native.is_empty() || ocr.contains(&native) {
                ocr
            } else if native.contains(&ocr) {
                native
            } else {
                format!("{native}\n{ocr}")
            };
            Ok((PdfPageSource::Ocr, text))
        }
        Err(error) if native.is_empty() => Err(error),
        Err(_) => Ok((PdfPageSource::NativeText, native)),
    }
}

fn normalize_pdf_text(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn system_now_ms() -> Result<i64, DatabaseError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| DatabaseError::InvalidInput("system clock is before Unix epoch".into()))?
        .as_millis();
    i64::try_from(millis)
        .map_err(|_| DatabaseError::InvalidInput("system clock is out of range".into()))
}

async fn select_pdf_file<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<Option<PathBuf>, crate::database::commands::CommandError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let _ = sender.send(select_pdf_file_on_main_thread());
    })
    .map_err(|error| {
        crate::database::commands::CommandError::from(DatabaseError::InvalidInput(format!(
            "could not open the PDF picker: {error}"
        )))
    })?;
    tauri::async_runtime::spawn_blocking(move || receiver.recv())
        .await
        .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
        .map_err(|_| {
            crate::database::commands::CommandError::from(DatabaseError::WriterUnavailable)
        })?
        .map_err(Into::into)
}

#[cfg(target_os = "macos")]
fn select_pdf_file_on_main_thread() -> Result<Option<PathBuf>, DatabaseError> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};

    let mtm = MainThreadMarker::new().ok_or_else(|| {
        DatabaseError::InvalidInput("the PDF picker was not opened on the main thread".into())
    })?;
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setAllowsMultipleSelection(false);
    panel.setCanChooseDirectories(false);
    panel.setCanChooseFiles(true);
    panel.setResolvesAliases(true);
    if panel.runModal() != NSModalResponseOK {
        return Ok(None);
    }
    Ok(panel
        .URL()
        .and_then(|url| url.path())
        .map(|path| PathBuf::from(path.to_string())))
}

#[cfg(not(target_os = "macos"))]
fn select_pdf_file_on_main_thread() -> Result<Option<PathBuf>, DatabaseError> {
    Err(DatabaseError::InvalidInput(
        "native PDF picking is available only on macOS".into(),
    ))
}

async fn open_path_on_main_thread<R: tauri::Runtime>(
    app: &AppHandle<R>,
    path: PathBuf,
) -> Result<(), crate::database::commands::CommandError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let _ = sender.send(open_path(path));
    })
    .map_err(|error| {
        crate::database::commands::CommandError::from(DatabaseError::InvalidInput(format!(
            "could not ask macOS to open the PDF: {error}"
        )))
    })?;
    tauri::async_runtime::spawn_blocking(move || receiver.recv())
        .await
        .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
        .map_err(|_| {
            crate::database::commands::CommandError::from(DatabaseError::WriterUnavailable)
        })?
        .map_err(Into::into)
}

#[cfg(target_os = "macos")]
fn open_path(path: PathBuf) -> Result<(), DatabaseError> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSURL};

    let _mtm = MainThreadMarker::new().ok_or_else(|| {
        DatabaseError::InvalidInput("the PDF was not opened on the main thread".into())
    })?;
    let path = NSString::from_str(&path.to_string_lossy());
    let url = NSURL::fileURLWithPath(&path);
    if NSWorkspace::sharedWorkspace().openURL(&url) {
        Ok(())
    } else {
        Err(DatabaseError::InvalidInput(
            "macOS could not open the PDF".into(),
        ))
    }
}

#[cfg(not(target_os = "macos"))]
fn open_path(_path: PathBuf) -> Result<(), DatabaseError> {
    Err(DatabaseError::InvalidInput(
        "opening PDFs externally is available only on macOS".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        finish_sparse_pdf_page, normalize_pdf_text, read_bounded_pdf, safe_pdf_filename,
        validate_pdf_document_state, MAX_PDF_BYTES,
    };
    use crate::database::media::PdfPageSource;

    #[test]
    fn normalization_is_stable_and_preserves_nonempty_lines() {
        assert_eq!(
            normalize_pdf_text("  First   line\r\n\r\n Second\tline "),
            "First line\nSecond line"
        );
    }

    #[test]
    fn sparse_native_text_survives_ocr_failure_and_augments_ocr_success() {
        assert_eq!(
            finish_sparse_pdf_page("p. 7".into(), Err("OCR safety limit".into()))
                .expect("native fallback"),
            (PdfPageSource::NativeText, "p. 7".into())
        );
        assert_eq!(
            finish_sparse_pdf_page("p. 7".into(), Ok("diagram label".into()))
                .expect("augmented OCR"),
            (PdfPageSource::Ocr, "p. 7\ndiagram label".into())
        );
        assert_eq!(
            finish_sparse_pdf_page(String::new(), Err("no text".into())),
            Err("no text".into())
        );
    }

    #[test]
    fn filenames_are_bounded_and_keep_a_pdf_extension() {
        assert_eq!(safe_pdf_filename("../../notes"), "notes.pdf");
        assert_eq!(safe_pdf_filename("chapter.PDF"), "chapter.PDF");
        let unicode = format!("{}notes", "🧠".repeat(251));
        let bounded = safe_pdf_filename(&unicode);
        assert_eq!(bounded.chars().count(), 255);
        assert!(bounded.ends_with(".pdf"));
    }

    #[test]
    fn encrypted_locked_empty_and_oversized_documents_are_rejected() {
        for (encrypted, locked, pages) in [
            (true, false, 1),
            (false, true, 1),
            (false, false, 0),
            (false, false, 2_001),
        ] {
            assert!(validate_pdf_document_state(encrypted, locked, pages).is_err());
        }
        assert_eq!(
            validate_pdf_document_state(false, false, 2_000).expect("maximum valid PDF"),
            2_000
        );
    }

    #[test]
    fn oversized_files_are_rejected_before_allocation() {
        let directory = tempfile::tempdir().expect("temporary PDF fixture directory");
        let path = directory.path().join("large.pdf");
        let file = std::fs::File::create(&path).expect("large PDF fixture");
        file.set_len(MAX_PDF_BYTES as u64 + 1)
            .expect("sparse PDF fixture");
        assert!(read_bounded_pdf(&path).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn malformed_pdf_signature_is_rejected_by_pdfkit() {
        assert!(super::inspect_pdf(b"%PDF-1.7\nnot a document").is_err());
        assert!(super::inspect_pdf(b"not a PDF").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn born_digital_and_scanned_page_fixtures_have_honest_outcomes() {
        use crate::database::media::{PdfExtractionJob, PdfPageSource};

        let born_digital = single_page_pdf(Some("Born digital exact evidence"));
        assert_eq!(
            super::inspect_pdf(&born_digital).expect("valid text PDF"),
            1
        );
        let pages = super::extract_pdf_pages(&PdfExtractionJob {
            extraction_id: "019f547b-6200-7000-8000-000000000a01".into(),
            attachment_id: "019f547b-6200-7000-8000-000000000a02".into(),
            extractor_version: "1".into(),
            content_hash: vec![0; 32],
            attempt_count: 1,
            page_count: 1,
            pdf_bytes: born_digital,
        })
        .expect("extract born-digital PDF");
        let (source, text) = pages[0].result.as_ref().expect("native page evidence");
        assert_eq!(*source, PdfPageSource::NativeText);
        assert!(text.contains("Born digital exact evidence"));

        let scanned = single_page_pdf(None);
        let pages = super::extract_pdf_pages(&PdfExtractionJob {
            extraction_id: "019f547b-6200-7000-8000-000000000a03".into(),
            attachment_id: "019f547b-6200-7000-8000-000000000a04".into(),
            extractor_version: "1".into(),
            content_hash: vec![0; 32],
            attempt_count: 1,
            page_count: 1,
            pdf_bytes: scanned,
        })
        .expect("extract blank scanned PDF");
        assert!(
            pages[0].result.is_err(),
            "a blank scanned page must not fabricate text evidence"
        );
    }

    #[cfg(target_os = "macos")]
    fn single_page_pdf(text: Option<&str>) -> Vec<u8> {
        let content = text
            .map(|text| format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET"))
            .unwrap_or_default();
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        ];
        let mut bytes = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        bytes.extend_from_slice(b"0000000000 65535 f \n");
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        bytes
    }
}

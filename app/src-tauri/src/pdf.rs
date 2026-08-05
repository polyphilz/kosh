use std::{
    fs::{self, File, OpenOptions},
    io::{Cursor, Read, Write},
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{mpsc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::{
    database::{
        media::{IngestAttachmentMetadata, IngestPdfWrite, StagedAttachment},
        DatabaseError, PdfRecord,
    },
    runtime::RuntimeState,
};

const MAX_PDF_BYTES: usize = 32 * 1024 * 1024;
const MAX_PDF_PAGES: usize = 2_000;
const MAX_PDF_OPEN_MATERIALIZATIONS: usize = 16;
const PDF_INSPECTION_WORKER_ARG: &str = "--kosh-pdf-inspection-worker";
const PDF_EXTRACTION_WORKER_TIMEOUT: Duration = Duration::from_secs(2 * 60);
const MAX_PDF_WORKER_OUTPUT_BYTES: u64 = 32 * 1024 * 1024;
#[cfg(unix)]
const PDF_WORKER_CPU_SECONDS: libc::rlim_t = 90;
const PDF_WORKER_MEMORY_BYTES: u64 = 512 * 1024 * 1024;
static PDF_OPEN_DIRECTORY_LOCK: Mutex<()> = Mutex::new(());

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
        .register_file_selection(path)
        .map(Some)
        .map_err(Into::into)
}

#[tauri::command]
pub(crate) async fn ingest_selected_pdf<R: tauri::Runtime>(
    window: tauri::Window<R>,
    state: State<'_, RuntimeState>,
    draft_id: String,
    selection_id: String,
) -> Result<PdfRecord, crate::database::commands::CommandError> {
    let path = state.take_file_selection(window.label(), &selection_id)?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Document.pdf")
        .to_owned();
    let raw = tauri::async_runtime::spawn_blocking(move || read_bounded_pdf(&path))
        .await
        .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
        .map_err(crate::database::commands::CommandError::from)?;
    ingest_pdf_bytes(&state, draft_id, &filename, raw).await
}

pub(crate) async fn ingest_pdf_bytes(
    state: &RuntimeState,
    draft_id: String,
    filename: &str,
    raw: Vec<u8>,
) -> Result<PdfRecord, crate::database::commands::CommandError> {
    let client = state.database_client();
    let now_ms = state.now_ms();
    let limits = state.media_limits();
    let staging_directory = state.media_staging_directory();
    let mut ids = state.next_ids(3).into_iter();
    let stage_id = ids.next().expect("requested PDF staging ID");
    let attachment_id = ids.next().expect("requested PDF attachment ID");
    let ingest_lease_id = ids.next().expect("requested PDF lease ID");
    let filename = filename.to_owned();
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
            page_count: u32::try_from(page_count).expect("validated page count fits u32"),
        })
    })
    .await
    .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
    .map_err(crate::database::commands::CommandError::from)?;
    state.wake_media_backup();
    Ok(record)
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
        materialize_pdf_for_external_open(&directory, &attachment_id, &payload.bytes)
    })
    .await
    .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
    .map_err(crate::database::commands::CommandError::from)?;
    open_path_on_main_thread(&app, path).await
}

pub(crate) fn recover_pdf_open_directory(path: &Path) -> Result<usize, DatabaseError> {
    let _guard = PDF_OPEN_DIRECTORY_LOCK
        .lock()
        .map_err(|_| DatabaseError::InvalidInput("PDF open directory lock was poisoned".into()))?;
    fs::create_dir_all(path)?;
    let mut removed = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let Some(filename) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !is_pdf_open_materialization(&filename) {
            continue;
        }
        fs::remove_file(entry.path())?;
        removed += 1;
    }
    Ok(removed)
}

fn materialize_pdf_for_external_open(
    directory: &Path,
    attachment_id: &str,
    bytes: &[u8],
) -> Result<PathBuf, DatabaseError> {
    let _guard = PDF_OPEN_DIRECTORY_LOCK
        .lock()
        .map_err(|_| DatabaseError::InvalidInput("PDF open directory lock was poisoned".into()))?;
    fs::create_dir_all(directory)?;
    let path = directory.join(format!("{attachment_id}.pdf"));
    let temporary_path = directory.join(format!("{attachment_id}.pdf.part"));
    remove_file_if_present(&path)?;
    remove_file_if_present(&temporary_path)?;
    prune_pdf_open_directory(directory, MAX_PDF_OPEN_MATERIALIZATIONS - 1)?;

    let write_result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary_path, &path)?;
        Ok::<_, DatabaseError>(())
    })();
    if let Err(error) = write_result {
        let _ = remove_file_if_present(&temporary_path);
        return Err(error);
    }
    Ok(path)
}

fn prune_pdf_open_directory(path: &Path, keep: usize) -> Result<(), DatabaseError> {
    let mut materializations = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let Some(filename) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if filename.ends_with(".pdf.part") && is_pdf_open_materialization(&filename) {
            fs::remove_file(entry.path())?;
            continue;
        }
        if !filename.ends_with(".pdf") || !is_pdf_open_materialization(&filename) {
            continue;
        }
        let modified = entry
            .metadata()?
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH);
        materializations.push((modified, filename, entry.path()));
    }
    materializations.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let remove_count = materializations.len().saturating_sub(keep);
    for (_, _, path) in materializations.into_iter().take(remove_count) {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn remove_file_if_present(path: &Path) -> Result<(), DatabaseError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn is_pdf_open_materialization(filename: &str) -> bool {
    let Some(stem) = filename
        .strip_suffix(".pdf.part")
        .or_else(|| filename.strip_suffix(".pdf"))
    else {
        return false;
    };
    uuid::Uuid::parse_str(stem)
        .ok()
        .is_some_and(|id| id.get_version_num() == 7 && id.hyphenated().to_string() == stem)
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

fn inspect_pdf_isolated(bytes: &[u8]) -> Result<usize, DatabaseError> {
    let arguments = [PDF_INSPECTION_WORKER_ARG.to_owned()];
    match run_isolated_pdf_worker(&arguments, bytes).map_err(DatabaseError::InvalidInput)? {
        PdfWorkerResponse::Inspection { result } => result
            .map(|page_count| page_count as usize)
            .map_err(DatabaseError::InvalidInput),
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
    if operation != PDF_INSPECTION_WORKER_ARG {
        return None;
    }
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
    let response = PdfWorkerResponse::Inspection {
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
            "could not monitor isolated PDF inspection memory: {}",
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
    Err("isolated PDF inspection requires Unix resource limits".into())
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
        materialize_pdf_for_external_open, read_bounded_pdf, recover_pdf_open_directory,
        safe_pdf_filename, validate_pdf_document_state, MAX_PDF_BYTES,
        MAX_PDF_OPEN_MATERIALIZATIONS,
    };

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

    #[test]
    fn external_pdf_materializations_remain_bounded() {
        let directory = tempfile::tempdir().expect("temporary PDF open directory");
        let mut latest = None;
        for index in 0..(MAX_PDF_OPEN_MATERIALIZATIONS + 8) {
            let attachment_id = format!("019f547b-6200-7000-8000-{index:012x}");
            latest = Some(
                materialize_pdf_for_external_open(
                    directory.path(),
                    &attachment_id,
                    b"%PDF-bounded",
                )
                .expect("materialize PDF"),
            );
        }
        let materializations = std::fs::read_dir(directory.path())
            .expect("list PDF open directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.ends_with(".pdf"))
            })
            .count();
        assert_eq!(materializations, MAX_PDF_OPEN_MATERIALIZATIONS);
        assert_eq!(
            std::fs::read(latest.expect("latest PDF path")).expect("read latest PDF"),
            b"%PDF-bounded"
        );
    }

    #[test]
    fn startup_recovery_drains_every_owned_pdf_materialization() {
        let directory = tempfile::tempdir().expect("temporary PDF open directory");
        for index in 0..300 {
            let attachment_id = format!("019f547b-6200-7000-8000-{index:012x}");
            std::fs::write(
                directory.path().join(format!("{attachment_id}.pdf")),
                b"%PDF",
            )
            .expect("write recovered PDF");
        }
        std::fs::write(
            directory
                .path()
                .join("019f547b-6200-7000-8000-00000000012c.pdf.part"),
            b"%PDF-interrupted",
        )
        .expect("write interrupted PDF");
        std::fs::write(directory.path().join("user-file.pdf"), b"keep")
            .expect("write unrelated file");

        assert_eq!(
            recover_pdf_open_directory(directory.path()).expect("recover PDF open directory"),
            301
        );
        let remaining = std::fs::read_dir(directory.path())
            .expect("list recovered PDF directory")
            .map(|entry| {
                entry
                    .expect("directory entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(remaining, vec!["user-file.pdf"]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn malformed_pdf_signature_is_rejected_by_pdfkit() {
        assert!(super::inspect_pdf(b"%PDF-1.7\nnot a document").is_err());
        assert!(super::inspect_pdf(b"not a PDF").is_err());
    }
}

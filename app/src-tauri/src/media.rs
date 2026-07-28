use std::{
    fs::{self, File},
    io::{Cursor, Read},
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use image::{
    imageops::FilterType, GenericImageView, ImageDecoder, ImageFormat, ImageReader, Limits,
};
use serde::Serialize;
use tauri::{http, AppHandle, Emitter, Manager, State};

use crate::{
    database::{
        media::{
            CanonicalImage, ImageOcrJob, ImageOcrRegion, IngestAttachmentMetadata,
            IngestImageWrite, MediaByteRange, StagedAttachment,
        },
        DatabaseClient, DatabaseError, ImageOcrDiagnostics, ImageRecord, ImageStatusRecord,
        MediaIntegrityReport, MediaLimits, MediaMaintenanceReport,
    },
    runtime::RuntimeState,
};

const MAX_STAGING_RECOVERY_FILES: usize = 1_024;
const MEDIA_PATH_PREFIX: &str = "/attachment/";
const MAX_SOURCE_IMAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_DECODED_IMAGE_DIMENSION: u32 = 32_768;
const MAX_DECODED_IMAGE_ALLOCATION: u64 = 256 * 1024 * 1024;
const MAX_CANONICAL_IMAGE_EDGE: u32 = 1_600;
const WEBP_QUALITY: f32 = 80.0;
const OCR_RECONCILIATION_INTERVAL: Duration = Duration::from_secs(5 * 60);
const OCR_STALE_ATTEMPT_AGE: Duration = Duration::from_secs(2 * 60);
const IMAGE_DROP_EVENT: &str = "kosh://image-drop";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImageDropNotice {
    pub drop_id: String,
    pub filenames: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
enum OcrWorkerSignal {
    WorkAvailable,
}

pub(crate) struct ImageOcrCoordinator {
    sender: Option<mpsc::SyncSender<OcrWorkerSignal>>,
}

impl ImageOcrCoordinator {
    pub(crate) fn start(client: DatabaseClient) -> Result<Self, DatabaseError> {
        let now_ms = system_now_ms()?;
        let recovery = client.recover_interrupted_image_ocr(now_ms, now_ms)?;
        log_ocr_recovery("launch", recovery);
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("kosh-image-ocr".into())
            .spawn(move || image_ocr_worker(client, receiver))?;
        let coordinator = Self {
            sender: Some(sender),
        };
        coordinator.wake();
        Ok(coordinator)
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn disabled() -> Self {
        Self { sender: None }
    }

    pub(crate) fn wake(&self) {
        let Some(sender) = &self.sender else {
            return;
        };
        match sender.try_send(OcrWorkerSignal::WorkAvailable) {
            Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
            Err(mpsc::TrySendError::Disconnected(_)) => {
                log::error!("image OCR worker is unavailable");
            }
        }
    }
}

pub(crate) fn recover_staging_directory(path: &Path) -> Result<usize, DatabaseError> {
    fs::create_dir_all(path)?;
    let mut removed = 0;
    for entry in fs::read_dir(path)? {
        if removed >= MAX_STAGING_RECOVERY_FILES {
            break;
        }
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let Some(filename) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(stage_id) = filename.strip_suffix(".part") else {
            continue;
        };
        if !is_uuid_v7(stage_id) {
            continue;
        }
        fs::remove_file(entry.path())?;
        removed += 1;
    }
    Ok(removed)
}

#[tauri::command]
pub(crate) fn media_limits(state: State<'_, RuntimeState>) -> MediaLimits {
    state.media_limits()
}

#[tauri::command]
pub(crate) async fn media_integrity_scan(
    state: State<'_, RuntimeState>,
) -> Result<MediaIntegrityReport, crate::database::commands::CommandError> {
    let client = state.database_client();
    let now_ms = state.now_ms();
    tauri::async_runtime::spawn_blocking(move || client.media_integrity_report(now_ms))
        .await
        .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
        .map_err(Into::into)
}

#[tauri::command]
pub(crate) async fn maintain_media(
    state: State<'_, RuntimeState>,
) -> Result<MediaMaintenanceReport, crate::database::commands::CommandError> {
    let client = state.database_client();
    let now_ms = state.now_ms();
    let limits = state.media_limits();
    tauri::async_runtime::spawn_blocking(move || client.maintain_media(now_ms, limits))
        .await
        .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
        .map_err(Into::into)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImageIngestFailure {
    filename: String,
    message: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImageDropIngestResult {
    images: Vec<ImageRecord>,
    failures: Vec<ImageIngestFailure>,
}

#[tauri::command]
pub(crate) async fn pick_image<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, RuntimeState>,
    draft_id: String,
) -> Result<Option<ImageRecord>, crate::database::commands::CommandError> {
    let Some(path) = select_image_file(&app).await? else {
        return Ok(None);
    };
    ingest_image_path(&state, draft_id, path).await.map(Some)
}

#[tauri::command]
pub(crate) async fn ingest_clipboard_image<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, RuntimeState>,
    draft_id: String,
) -> Result<ImageRecord, crate::database::commands::CommandError> {
    let raw = read_clipboard_image(&app).await?;
    ingest_image_bytes(&state, draft_id, "Pasted image", raw).await
}

#[tauri::command]
pub(crate) async fn ingest_dropped_images(
    state: State<'_, RuntimeState>,
    draft_id: String,
    drop_id: String,
) -> Result<ImageDropIngestResult, crate::database::commands::CommandError> {
    let paths = state.take_image_drop(&drop_id)?;
    let mut result = ImageDropIngestResult::default();
    for path in paths {
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Dropped image")
            .to_owned();
        match ingest_image_path(&state, draft_id.clone(), path).await {
            Ok(image) => result.images.push(image),
            Err(error) => result.failures.push(ImageIngestFailure {
                filename,
                message: error.public_message().to_owned(),
            }),
        }
    }
    Ok(result)
}

#[tauri::command]
pub(crate) async fn image_status(
    state: State<'_, RuntimeState>,
    attachment_id: String,
) -> Result<ImageStatusRecord, crate::database::commands::CommandError> {
    let client = state.database_client();
    tauri::async_runtime::spawn_blocking(move || client.load_image_status(attachment_id))
        .await
        .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
        .map_err(Into::into)
}

#[tauri::command]
pub(crate) async fn retry_image_ocr(
    state: State<'_, RuntimeState>,
    attachment_id: String,
) -> Result<ImageStatusRecord, crate::database::commands::CommandError> {
    let client = state.database_client();
    let now_ms = state.now_ms();
    let result =
        tauri::async_runtime::spawn_blocking(move || client.retry_image_ocr(attachment_id, now_ms))
            .await
            .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
            .map_err(crate::database::commands::CommandError::from)?;
    state.wake_image_ocr();
    Ok(result)
}

#[tauri::command]
pub(crate) async fn image_ocr_diagnostics(
    state: State<'_, RuntimeState>,
) -> Result<ImageOcrDiagnostics, crate::database::commands::CommandError> {
    let client = state.database_client();
    tauri::async_runtime::spawn_blocking(move || client.image_ocr_diagnostics())
        .await
        .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
        .map_err(Into::into)
}

pub(crate) fn handle_image_drop<R: tauri::Runtime>(
    window: &tauri::Window<R>,
    event: &tauri::WindowEvent,
) {
    let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event else {
        return;
    };
    let Some(state) = window.try_state::<RuntimeState>() else {
        return;
    };
    let Some(notice) = state.register_image_drop(paths) else {
        return;
    };
    if let Err(error) = window.emit(IMAGE_DROP_EVENT, notice) {
        log::warn!("could not notify the editor about a native image drop: {error}");
    }
}

async fn ingest_image_path(
    state: &RuntimeState,
    draft_id: String,
    path: PathBuf,
) -> Result<ImageRecord, crate::database::commands::CommandError> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Image")
        .to_owned();
    let raw = tauri::async_runtime::spawn_blocking(move || read_bounded_file(&path))
        .await
        .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
        .map_err(crate::database::commands::CommandError::from)?;
    ingest_image_bytes(state, draft_id, &filename, raw).await
}

async fn ingest_image_bytes(
    state: &RuntimeState,
    draft_id: String,
    filename: &str,
    raw: Vec<u8>,
) -> Result<ImageRecord, crate::database::commands::CommandError> {
    let client = state.database_client();
    let now_ms = state.now_ms();
    let limits = state.media_limits();
    let staging_directory = state.media_staging_directory();
    let mut ids = state.next_ids(4).into_iter();
    let stage_id = ids.next().expect("requested image staging ID");
    let attachment_id = ids.next().expect("requested image attachment ID");
    let ingest_lease_id = ids.next().expect("requested image lease ID");
    let extraction_id = ids.next().expect("requested image extraction ID");
    let filename = filename.to_owned();
    let record = tauri::async_runtime::spawn_blocking(move || {
        if raw.is_empty() || raw.len() > MAX_SOURCE_IMAGE_BYTES {
            return Err(DatabaseError::InvalidInput(format!(
                "the selected image must contain between 1 and {MAX_SOURCE_IMAGE_BYTES} bytes"
            )));
        }
        let processed = canonicalize_image(&raw)?;
        let staged = StagedAttachment::from_reader(
            Cursor::new(raw),
            &staging_directory,
            &stage_id,
            limits.max_attachment_bytes,
        )?;
        client.ingest_image(IngestImageWrite {
            attachment: staged.write(IngestAttachmentMetadata {
                attachment_id,
                ingest_lease_id,
                draft_id,
                display_filename: safe_image_filename(&filename, processed.format),
                media_type: image_media_type(processed.format).into(),
                now_ms,
                limits,
            }),
            extraction_id,
            preview: processed.preview,
        })
    })
    .await
    .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
    .map_err(crate::database::commands::CommandError::from)?;
    state.wake_image_ocr();
    Ok(record)
}

fn read_bounded_file(path: &Path) -> Result<Vec<u8>, DatabaseError> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(DatabaseError::InvalidInput(
            "the selected image is not a regular file".into(),
        ));
    }
    if metadata.len() == 0 || metadata.len() > MAX_SOURCE_IMAGE_BYTES as u64 {
        return Err(DatabaseError::InvalidInput(format!(
            "the selected image must contain between 1 and {MAX_SOURCE_IMAGE_BYTES} bytes"
        )));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).map_err(|_| {
        DatabaseError::InvalidInput("the selected image does not fit memory".into())
    })?);
    File::open(path)?
        .take(MAX_SOURCE_IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_SOURCE_IMAGE_BYTES {
        return Err(DatabaseError::InvalidInput(
            "the selected image changed while it was being read".into(),
        ));
    }
    Ok(bytes)
}

struct ProcessedImage {
    format: ImageFormat,
    preview: CanonicalImage,
}

fn canonicalize_image(bytes: &[u8]) -> Result<ProcessedImage, DatabaseError> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(invalid_image)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODED_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_DECODED_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED_IMAGE_ALLOCATION);
    reader.limits(limits);
    let format = reader
        .format()
        .filter(|format| {
            matches!(
                format,
                ImageFormat::Gif
                    | ImageFormat::Jpeg
                    | ImageFormat::Png
                    | ImageFormat::Tiff
                    | ImageFormat::WebP
            )
        })
        .ok_or_else(|| invalid_image("the image format is unsupported"))?;
    let mut decoder = reader.into_decoder().map_err(invalid_image)?;
    let orientation = decoder.orientation().map_err(invalid_image)?;
    drop(decoder);

    let mut decode_reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODED_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_DECODED_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED_IMAGE_ALLOCATION);
    decode_reader.limits(limits);
    let mut decoded = decode_reader.decode().map_err(invalid_image)?;
    decoded.apply_orientation(orientation);
    let (source_width, source_height) = decoded.dimensions();
    let resized = if source_width.max(source_height) > MAX_CANONICAL_IMAGE_EDGE {
        decoded.resize(
            MAX_CANONICAL_IMAGE_EDGE,
            MAX_CANONICAL_IMAGE_EDGE,
            FilterType::Lanczos3,
        )
    } else {
        decoded
    };
    let rgba = resized.to_rgba8();
    let (natural_width, natural_height) = rgba.dimensions();
    let encoded = webp::Encoder::from_rgba(rgba.as_raw(), natural_width, natural_height)
        .encode_simple(false, WEBP_QUALITY)
        .map_err(|error| invalid_image(format!("WebP encoding failed: {error:?}")))?;
    Ok(ProcessedImage {
        format,
        preview: CanonicalImage {
            bytes: encoded.to_vec(),
            natural_width,
            natural_height,
        },
    })
}

fn invalid_image(error: impl std::fmt::Display) -> DatabaseError {
    DatabaseError::InvalidInput(format!("could not process the image: {error}"))
}

fn image_media_type(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Gif => "image/gif",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Png => "image/png",
        ImageFormat::Tiff => "image/tiff",
        ImageFormat::WebP => "image/webp",
        _ => unreachable!("canonicalization rejects unsupported image formats"),
    }
}

fn image_extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Gif => "gif",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Png => "png",
        ImageFormat::Tiff => "tiff",
        ImageFormat::WebP => "webp",
        _ => unreachable!("canonicalization rejects unsupported image formats"),
    }
}

fn safe_image_filename(filename: &str, format: ImageFormat) -> String {
    let mut safe = filename
        .trim()
        .chars()
        .map(|character| {
            if character.is_control() || matches!(character, '/' | '\\' | ':') {
                '_'
            } else {
                character
            }
        })
        .take(255)
        .collect::<String>();
    if safe.is_empty() || matches!(safe.as_str(), "." | "..") {
        safe = format!("Image.{}", image_extension(format));
    } else if !safe.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && !stem.ends_with('.')
            && !extension.is_empty()
            && extension.len() <= 10
            && extension
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
    }) {
        let extension = image_extension(format);
        let max_stem_chars = 255_usize.saturating_sub(extension.len() + 1);
        safe = safe.chars().take(max_stem_chars).collect();
        safe.push('.');
        safe.push_str(extension);
    }
    safe
}

async fn select_image_file<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<Option<PathBuf>, crate::database::commands::CommandError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let _ = sender.send(select_image_file_on_main_thread());
    })
    .map_err(|error| {
        crate::database::commands::CommandError::from(DatabaseError::InvalidInput(format!(
            "could not open the image picker: {error}"
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
fn select_image_file_on_main_thread() -> Result<Option<PathBuf>, DatabaseError> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};

    let mtm = MainThreadMarker::new().ok_or_else(|| {
        DatabaseError::InvalidInput("the image picker was not opened on the main thread".into())
    })?;
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setAllowsMultipleSelection(false);
    panel.setCanChooseDirectories(false);
    panel.setCanChooseFiles(true);
    panel.setResolvesAliases(true);
    if panel.runModal() != NSModalResponseOK {
        return Ok(None);
    }
    let path = panel
        .URL()
        .and_then(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()));
    Ok(path)
}

#[cfg(not(target_os = "macos"))]
fn select_image_file_on_main_thread() -> Result<Option<PathBuf>, DatabaseError> {
    Err(DatabaseError::InvalidInput(
        "native image picking is available only on macOS".into(),
    ))
}

async fn read_clipboard_image<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<Vec<u8>, crate::database::commands::CommandError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let _ = sender.send(read_clipboard_image_on_main_thread());
    })
    .map_err(|error| {
        crate::database::commands::CommandError::from(DatabaseError::InvalidInput(format!(
            "could not access the clipboard: {error}"
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
fn read_clipboard_image_on_main_thread() -> Result<Vec<u8>, DatabaseError> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeTIFF};

    MainThreadMarker::new().ok_or_else(|| {
        DatabaseError::InvalidInput("the clipboard was not read on the macOS main thread".into())
    })?;
    let pasteboard = NSPasteboard::generalPasteboard();
    for pasteboard_type in [unsafe { NSPasteboardTypePNG }, unsafe {
        NSPasteboardTypeTIFF
    }] {
        if let Some(data) = pasteboard.dataForType(pasteboard_type) {
            let bytes = data.to_vec();
            if bytes.len() > MAX_SOURCE_IMAGE_BYTES {
                return Err(DatabaseError::InvalidInput(format!(
                    "the pasted image is larger than {MAX_SOURCE_IMAGE_BYTES} bytes"
                )));
            }
            if !bytes.is_empty() {
                return Ok(bytes);
            }
        }
    }
    Err(DatabaseError::InvalidInput(
        "the clipboard does not contain a supported image".into(),
    ))
}

#[cfg(not(target_os = "macos"))]
fn read_clipboard_image_on_main_thread() -> Result<Vec<u8>, DatabaseError> {
    Err(DatabaseError::InvalidInput(
        "clipboard image ingestion is available only on macOS".into(),
    ))
}

fn image_ocr_worker(client: DatabaseClient, receiver: mpsc::Receiver<OcrWorkerSignal>) {
    let mut next_reconciliation = Instant::now() + OCR_RECONCILIATION_INTERVAL;
    loop {
        let wait = next_reconciliation.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(wait) {
            Ok(OcrWorkerSignal::WorkAvailable) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if Instant::now() >= next_reconciliation {
            reconcile_image_ocr_queue(&client);
            next_reconciliation = Instant::now() + OCR_RECONCILIATION_INTERVAL;
        }
        drain_image_ocr_queue(&client, recognize_image_text);
    }
}

fn reconcile_image_ocr_queue(client: &DatabaseClient) {
    let result = system_now_ms().and_then(|now_ms| {
        let stale_age = i64::try_from(OCR_STALE_ATTEMPT_AGE.as_millis()).map_err(|_| {
            DatabaseError::InvalidInput("OCR stale-attempt duration overflow".into())
        })?;
        client.recover_interrupted_image_ocr(now_ms.saturating_sub(stale_age), now_ms)
    });
    match result {
        Ok(recovery) => log_ocr_recovery("periodic reconciliation", recovery),
        Err(error) => log::error!("could not reconcile the image OCR queue: {error}"),
    }
}

fn log_ocr_recovery(context: &str, recovery: crate::database::ImageOcrRecovery) {
    if recovery.requeued > 0 || recovery.terminally_failed > 0 {
        log::warn!(
            "image OCR {context} requeued {} interrupted attempt(s) and terminally failed {}",
            recovery.requeued,
            recovery.terminally_failed
        );
    }
}

fn drain_image_ocr_queue(
    client: &DatabaseClient,
    recognize: impl Fn(&[u8]) -> std::result::Result<Vec<ImageOcrRegion>, String>,
) {
    loop {
        let now_ms = match system_now_ms() {
            Ok(now_ms) => now_ms,
            Err(error) => {
                log::error!("could not read the clock for image OCR: {error}");
                return;
            }
        };
        let job = match client.claim_next_image_ocr(now_ms) {
            Ok(Some(job)) => job,
            Ok(None) => return,
            Err(error) => {
                log::error!("could not claim the next image OCR job: {error}");
                return;
            }
        };
        let result = recognize_without_panicking(&job, &recognize);
        let completed_at_ms = match system_now_ms() {
            Ok(now_ms) => now_ms,
            Err(error) => {
                log::error!("could not read the image OCR completion clock: {error}");
                return;
            }
        };
        let attachment_id = job.attachment_id.clone();
        if let Err(error) = client.complete_image_ocr(job, result, completed_at_ms) {
            log::error!("could not persist OCR for image {attachment_id}: {error}");
            return;
        }
    }
}

fn recognize_without_panicking(
    job: &ImageOcrJob,
    recognize: &impl Fn(&[u8]) -> std::result::Result<Vec<ImageOcrRegion>, String>,
) -> std::result::Result<Vec<ImageOcrRegion>, String> {
    catch_unwind(AssertUnwindSafe(|| recognize(&job.preview_bytes)))
        .unwrap_or_else(|_| Err("the image OCR recognizer panicked".into()))
}

#[cfg(target_os = "macos")]
fn recognize_image_text(bytes: &[u8]) -> std::result::Result<Vec<ImageOcrRegion>, String> {
    use objc2::{rc::autoreleasepool, runtime::AnyObject, AnyThread};
    use objc2_foundation::{NSArray, NSData, NSDictionary};
    use objc2_vision::{
        VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRequest,
        VNRequestTextRecognitionLevel,
    };

    autoreleasepool(|_| {
        let data = NSData::with_bytes(bytes);
        let options = NSDictionary::<VNImageOption, AnyObject>::init(NSDictionary::<
            VNImageOption,
            AnyObject,
        >::alloc());
        let handler = VNImageRequestHandler::initWithData_options(
            VNImageRequestHandler::alloc(),
            &data,
            &options,
        );
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(true);
        request.setAutomaticallyDetectsLanguage(true);
        let requests = NSArray::<VNRequest>::from_slice(&[&request]);
        handler
            .performRequests_error(&requests)
            .map_err(|error| error.localizedDescription().to_string())?;

        let mut regions = Vec::new();
        if let Some(results) = request.results() {
            for observation in results.iter() {
                let Some(candidate) = observation.topCandidates(1).firstObject() else {
                    continue;
                };
                let text = candidate.string().to_string();
                if text.trim().is_empty() {
                    continue;
                }
                let bounds = unsafe { observation.boundingBox() };
                regions.push(ImageOcrRegion {
                    text,
                    x: bounds.origin.x,
                    y: bounds.origin.y,
                    width: bounds.size.width,
                    height: bounds.size.height,
                });
            }
        }
        Ok(regions)
    })
}

#[cfg(not(target_os = "macos"))]
fn recognize_image_text(_bytes: &[u8]) -> std::result::Result<Vec<ImageOcrRegion>, String> {
    Err("Apple Vision OCR is available only on macOS".into())
}

fn system_now_ms() -> Result<i64, DatabaseError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| DatabaseError::InvalidInput("system clock is before Unix epoch".into()))?
        .as_millis();
    i64::try_from(millis)
        .map_err(|_| DatabaseError::InvalidInput("system clock is out of range".into()))
}

pub(crate) fn protocol_response<R: tauri::Runtime>(
    app: &AppHandle<R>,
    request: http::Request<Vec<u8>>,
) -> http::Response<Vec<u8>> {
    if request.method() != http::Method::GET {
        return empty_response(http::StatusCode::METHOD_NOT_ALLOWED);
    }
    if request.uri().host() != Some("localhost") || request.uri().query().is_some() {
        return empty_response(http::StatusCode::BAD_REQUEST);
    }
    let Some(attachment_id) = request.uri().path().strip_prefix(MEDIA_PATH_PREFIX) else {
        return empty_response(http::StatusCode::BAD_REQUEST);
    };
    if !is_uuid_v7(attachment_id) {
        return empty_response(http::StatusCode::BAD_REQUEST);
    }
    let requested_range = match request.headers().get(http::header::RANGE) {
        Some(value) => match value.to_str().ok().and_then(parse_range) {
            Some(range) => Some(range),
            None => return empty_response(http::StatusCode::RANGE_NOT_SATISFIABLE),
        },
        None => None,
    };
    let Some(state) = app.try_state::<RuntimeState>() else {
        return empty_response(http::StatusCode::SERVICE_UNAVAILABLE);
    };
    let result = state.database_client().load_media_payload(
        attachment_id.to_owned(),
        state.now_ms(),
        requested_range,
        state.media_limits().max_protocol_response_bytes,
    );
    match result {
        Ok(payload) => {
            let partial = requested_range.is_some();
            let mut response = http::Response::builder()
                .status(if partial {
                    http::StatusCode::PARTIAL_CONTENT
                } else {
                    http::StatusCode::OK
                })
                .header(http::header::CONTENT_TYPE, payload.media_type)
                .header(http::header::CONTENT_LENGTH, payload.bytes.len())
                .header(http::header::ACCEPT_RANGES, "bytes")
                .header(
                    http::header::CACHE_CONTROL,
                    if payload.revision_bound {
                        "private, max-age=31536000, immutable"
                    } else {
                        "private, no-store"
                    },
                )
                .header(http::header::ETAG, format!("\"{}\"", hex(&payload.sha256)))
                .header("X-Content-Type-Options", "nosniff")
                .header("Referrer-Policy", "no-referrer")
                .header(
                    "Content-Security-Policy",
                    "sandbox; default-src 'none'; base-uri 'none'; form-action 'none'",
                );
            if partial {
                response = response.header(
                    http::header::CONTENT_RANGE,
                    format!(
                        "bytes {}-{}/{}",
                        payload.range.start, payload.range.end_inclusive, payload.total_byte_length
                    ),
                );
            }
            response
                .body(payload.bytes)
                .unwrap_or_else(|_| empty_response(http::StatusCode::INTERNAL_SERVER_ERROR))
        }
        Err(DatabaseError::NotFound { .. }) => empty_response(http::StatusCode::NOT_FOUND),
        Err(DatabaseError::InvalidInput(message)) if message.contains("range") => {
            empty_response(http::StatusCode::RANGE_NOT_SATISFIABLE)
        }
        Err(DatabaseError::InvalidInput(_)) => empty_response(http::StatusCode::PAYLOAD_TOO_LARGE),
        Err(error) => {
            log::error!("authorized local media read failed: {error}");
            empty_response(http::StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

fn parse_range(value: &str) -> Option<MediaByteRange> {
    let range = value.strip_prefix("bytes=")?;
    if range.contains(',') {
        return None;
    }
    let (start, end) = range.split_once('-')?;
    if start.is_empty() || end.is_empty() {
        return None;
    }
    Some(MediaByteRange {
        start: start.parse().ok()?,
        end_inclusive: end.parse().ok()?,
    })
}

fn is_uuid_v7(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|parsed| {
        parsed.get_version_num() == 7 && parsed.hyphenated().to_string() == value
    })
}

fn empty_response(status: http::StatusCode) -> http::Response<Vec<u8>> {
    http::Response::builder()
        .status(status)
        .header("X-Content-Type-Options", "nosniff")
        .header("Referrer-Policy", "no-referrer")
        .body(Vec::new())
        .expect("valid empty media response")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::DynamicImage::new_rgba8(width, height);
        let mut bytes = Cursor::new(Vec::new());
        image
            .write_to(&mut bytes, ImageFormat::Png)
            .expect("encode PNG fixture");
        bytes.into_inner()
    }

    #[test]
    fn canonical_image_preview_is_bounded_decodable_webp() {
        let processed = canonicalize_image(&png(3_200, 800)).expect("canonical image");

        assert_eq!(processed.format, ImageFormat::Png);
        assert_eq!(processed.preview.natural_width, 1_600);
        assert_eq!(processed.preview.natural_height, 400);
        let decoded =
            image::load_from_memory_with_format(&processed.preview.bytes, ImageFormat::WebP)
                .expect("decode canonical WebP");
        assert_eq!(decoded.dimensions(), (1_600, 400));
    }

    #[test]
    fn canonical_image_rejects_invalid_and_excessive_dimensions() {
        assert!(matches!(
            canonicalize_image(b"not an image"),
            Err(DatabaseError::InvalidInput(_))
        ));
        assert!(matches!(
            canonicalize_image(&png(MAX_DECODED_IMAGE_DIMENSION + 1, 1)),
            Err(DatabaseError::InvalidInput(_))
        ));
    }

    #[test]
    fn image_filenames_are_sanitized_and_receive_canonical_extensions() {
        assert_eq!(
            safe_image_filename("../chapter:image", ImageFormat::Png),
            ".._chapter_image.png"
        );
        assert_eq!(safe_image_filename(" \n ", ImageFormat::WebP), "Image.webp");
    }

    #[test]
    fn range_parser_accepts_one_bounded_range_only() {
        assert_eq!(
            parse_range("bytes=10-19"),
            Some(MediaByteRange {
                start: 10,
                end_inclusive: 19
            })
        );
        assert_eq!(parse_range("bytes=10-"), None);
        assert_eq!(parse_range("bytes=-10"), None);
        assert_eq!(parse_range("bytes=0-1,4-5"), None);
    }

    #[test]
    fn staging_recovery_removes_only_internal_uuid_part_files() {
        let root = tempfile::tempdir().expect("staging recovery root");
        let stale = root
            .path()
            .join("019f547b-6200-7000-8000-000000000123.part");
        let unrelated = root.path().join("keep.txt");
        std::fs::write(&stale, b"partial").expect("stale part");
        std::fs::write(&unrelated, b"keep").expect("unrelated file");

        assert_eq!(
            recover_staging_directory(root.path()).expect("staging recovery"),
            1
        );
        assert!(!stale.exists());
        assert!(unrelated.exists());
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn protocol_rejects_paths_and_unknown_attachment_ids() {
        let data_root = crate::test_support::TestDataRoot::new();
        let app = crate::test_support::mock_app(&data_root, 100, std::iter::empty());
        let traversal = http::Request::builder()
            .method(http::Method::GET)
            .uri("kosh-media://localhost/attachment/../../etc/passwd")
            .body(Vec::new())
            .expect("traversal request");
        assert_eq!(
            protocol_response(app.handle(), traversal).status(),
            http::StatusCode::BAD_REQUEST
        );
        let unknown = http::Request::builder()
            .method(http::Method::GET)
            .uri("kosh-media://localhost/attachment/019f547b-6200-7000-8000-000000000999")
            .body(Vec::new())
            .expect("unknown attachment request");
        assert_eq!(
            protocol_response(app.handle(), unknown).status(),
            http::StatusCode::NOT_FOUND
        );
        let invalid = http::Request::builder()
            .method(http::Method::GET)
            .uri("kosh-media://localhost/attachment/not-an-id")
            .body(Vec::new())
            .expect("invalid attachment request");
        assert_eq!(
            protocol_response(app.handle(), invalid).status(),
            http::StatusCode::BAD_REQUEST
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn protocol_serves_only_authorized_bounded_draft_bytes_without_caching() {
        use crate::database::{
            drafts::SaveDraftWrite,
            media::{IngestAttachmentMetadata, StagedAttachment},
            MediaLimits, SaveDraftInput,
        };

        let data_root = crate::test_support::TestDataRoot::new();
        let app = crate::test_support::mock_app(&data_root, 100, std::iter::empty());
        let state = app.state::<RuntimeState>();
        let draft = state
            .database_client()
            .save_draft(SaveDraftWrite {
                input: SaveDraftInput {
                    context_key: "capture".into(),
                    tidbit_id: None,
                    base_revision_id: None,
                    title: None,
                    body_markdown: String::new(),
                    sources: Vec::new(),
                },
                now_ms: 90,
                draft_id: "019f547b-6200-7000-8000-000000000901".into(),
                media_limits: MediaLimits::default(),
            })
            .expect("protocol draft");
        let staged = StagedAttachment::from_reader(
            std::io::Cursor::new(b"protocol bytes"),
            &state.media_staging_directory(),
            "019f547b-6200-7000-8000-000000000904",
            MediaLimits::default().max_attachment_bytes,
        )
        .expect("protocol stage");
        let attachment = state
            .database_client()
            .ingest_attachment(staged.write(IngestAttachmentMetadata {
                attachment_id: "019f547b-6200-7000-8000-000000000902".into(),
                ingest_lease_id: "019f547b-6200-7000-8000-000000000903".into(),
                draft_id: draft.id,
                display_filename: "protocol.txt".into(),
                media_type: "text/plain".into(),
                now_ms: 91,
                limits: MediaLimits::default(),
            }))
            .expect("protocol attachment");
        let request = http::Request::builder()
            .method(http::Method::GET)
            .uri(format!(
                "kosh-media://localhost/attachment/{}",
                attachment.id
            ))
            .header(http::header::RANGE, "bytes=1-3")
            .body(Vec::new())
            .expect("authorized media request");
        let response = protocol_response(app.handle(), request);

        assert_eq!(response.status(), http::StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.body(), b"rot");
        assert_eq!(
            response.headers()[http::header::CONTENT_RANGE],
            "bytes 1-3/14"
        );
        assert_eq!(
            response.headers()[http::header::CACHE_CONTROL],
            "private, no-store"
        );
    }
}

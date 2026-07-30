use std::{
    fs::{self, File, OpenOptions},
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::{mpsc, Mutex},
    time::SystemTime,
};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{
    database::{
        media::{
            IngestAttachmentMetadata, IngestGenericAttachmentWrite, StagedAttachment,
            TextFileSegment, MAX_TEXT_FILE_PASSAGES,
        },
        DatabaseError, GenericAttachmentRecord, GenericAttachmentStatusRecord, ImageRecord,
        PdfRecord,
    },
    runtime::RuntimeState,
};

const FILE_DROP_EVENT: &str = "kosh://file-drop";
const MAX_FILES_PER_DROP: usize = 8;
const MAX_TEXT_EXTRACTION_BYTES: usize = 4 * 1024 * 1024;
const MAX_TEXT_PASSAGE_CHARS: usize = 1_000;
const MAX_OPEN_MATERIALIZATIONS: usize = 16;
const MAX_MATERIALIZED_FILENAME_BYTES: usize = 160;
static OPEN_DIRECTORY_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileDropNotice {
    pub selections: Vec<FileDropSelection>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileDropSelection {
    pub selection_id: String,
    pub filename: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "recordKind",
    content = "record",
    rename_all = "SCREAMING_SNAKE_CASE"
)]
pub(crate) enum SelectedAttachmentRecord {
    Image(ImageRecord),
    Pdf(PdfRecord),
    Generic(GenericAttachmentRecord),
}

#[tauri::command]
pub(crate) async fn select_attachment<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, RuntimeState>,
) -> Result<Option<String>, crate::database::commands::CommandError> {
    let Some(path) = select_attachment_file(&app).await? else {
        return Ok(None);
    };
    state
        .register_file_selection(path)
        .map(Some)
        .map_err(Into::into)
}

#[tauri::command]
pub(crate) async fn ingest_selected_attachment<R: tauri::Runtime>(
    window: tauri::Window<R>,
    state: State<'_, RuntimeState>,
    draft_id: String,
    selection_id: String,
) -> Result<SelectedAttachmentRecord, crate::database::commands::CommandError> {
    let path = state.take_file_selection(window.label(), &selection_id)?;
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Attachment".into());
    let max_bytes = state.media_limits().max_attachment_bytes;
    let raw =
        tauri::async_runtime::spawn_blocking(move || read_bounded_attachment(&path, max_bytes))
            .await
            .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
            .map_err(crate::database::commands::CommandError::from)?;

    if looks_like_pdf(&raw) {
        return crate::pdf::ingest_pdf_bytes(&state, draft_id, &filename, raw)
            .await
            .map(SelectedAttachmentRecord::Pdf);
    }
    if crate::media::is_supported_image(&raw) {
        return crate::media::ingest_image_bytes(&state, draft_id, &filename, raw)
            .await
            .map(SelectedAttachmentRecord::Image);
    }
    ingest_generic_attachment(&state, draft_id, &filename, raw)
        .await
        .map(SelectedAttachmentRecord::Generic)
}

#[tauri::command]
pub(crate) async fn attachment_status(
    state: State<'_, RuntimeState>,
    attachment_id: String,
) -> Result<GenericAttachmentStatusRecord, crate::database::commands::CommandError> {
    let client = state.database_client();
    tauri::async_runtime::spawn_blocking(move || {
        client.load_generic_attachment_status(attachment_id)
    })
    .await
    .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
    .map_err(Into::into)
}

#[tauri::command]
pub(crate) async fn open_attachment_external<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, RuntimeState>,
    attachment_id: String,
) -> Result<(), crate::database::commands::CommandError> {
    external_attachment_action(app, state, attachment_id, ExternalAction::Open).await
}

#[tauri::command]
pub(crate) async fn reveal_attachment_in_finder<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, RuntimeState>,
    attachment_id: String,
) -> Result<(), crate::database::commands::CommandError> {
    external_attachment_action(app, state, attachment_id, ExternalAction::Reveal).await
}

#[tauri::command]
pub(crate) async fn open_source_url<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, RuntimeState>,
    source_id: String,
) -> Result<(), crate::database::commands::CommandError> {
    let client = state.database_client();
    let source_url =
        tauri::async_runtime::spawn_blocking(move || client.load_source_url(source_id))
            .await
            .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
            .map_err(crate::database::commands::CommandError::from)?;
    run_source_url_on_main_thread(&app, source_url).await
}

#[tauri::command]
pub(crate) fn set_file_drop_consumer_active<R: tauri::Runtime>(
    window: tauri::Window<R>,
    state: State<'_, RuntimeState>,
    active: bool,
) {
    state.set_file_drop_consumer_active(window.label(), active);
}

#[tauri::command]
pub(crate) fn discard_file_drop_selections<R: tauri::Runtime>(
    window: tauri::Window<R>,
    state: State<'_, RuntimeState>,
    selection_ids: Vec<String>,
) -> Result<(), crate::database::commands::CommandError> {
    state
        .discard_file_drop_selections(window.label(), &selection_ids)
        .map_err(Into::into)
}

pub(crate) fn handle_file_drop<R: tauri::Runtime>(
    window: &tauri::Window<R>,
    event: &tauri::WindowEvent,
) {
    let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event else {
        return;
    };
    let Some(state) = window.try_state::<RuntimeState>() else {
        return;
    };
    if !state.file_drop_consumer_active(window.label()) {
        return;
    }
    let selections = paths
        .iter()
        .filter(|path| path.is_file())
        .take(MAX_FILES_PER_DROP)
        .filter_map(|path| {
            let selection_id =
                match state.register_dropped_file_selection(window.label(), path.clone()) {
                    Ok(id) => id,
                    Err(error) => {
                        log::warn!("could not register a dropped file: {error}");
                        return None;
                    }
                };
            Some(FileDropSelection {
                selection_id,
                filename: path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Dropped file".into()),
            })
        })
        .collect::<Vec<_>>();
    if selections.is_empty() {
        return;
    }
    let selection_ids = selections
        .iter()
        .map(|selection| selection.selection_id.clone())
        .collect::<Vec<_>>();
    if let Err(error) = emit_file_drop_notice(window, FileDropNotice { selections }) {
        if let Err(discard_error) =
            state.discard_file_drop_selections(window.label(), &selection_ids)
        {
            log::warn!("could not revoke an undelivered file drop: {discard_error}");
        }
        log::warn!("could not notify the editor about a native file drop: {error}");
    }
}

fn emit_file_drop_notice<R: tauri::Runtime>(
    window: &tauri::Window<R>,
    notice: FileDropNotice,
) -> tauri::Result<()> {
    window.emit_to(
        tauri::EventTarget::webview_window(window.label()),
        FILE_DROP_EVENT,
        notice,
    )
}

pub(crate) fn recover_attachment_open_directory(path: &Path) -> Result<usize, DatabaseError> {
    let _guard = OPEN_DIRECTORY_LOCK.lock().map_err(|_| {
        DatabaseError::InvalidInput("attachment open directory lock was poisoned".into())
    })?;
    fs::create_dir_all(path)?;
    let mut removed = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(is_open_materialization)
        {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

async fn ingest_generic_attachment(
    state: &RuntimeState,
    draft_id: String,
    filename: &str,
    raw: Vec<u8>,
) -> Result<GenericAttachmentRecord, crate::database::commands::CommandError> {
    let client = state.database_client();
    let now_ms = state.now_ms();
    let limits = state.media_limits();
    let staging_directory = state.media_staging_directory();
    let mut ids = state.next_ids(4).into_iter();
    let stage_id = ids.next().expect("requested attachment staging ID");
    let attachment_id = ids.next().expect("requested attachment ID");
    let ingest_lease_id = ids.next().expect("requested attachment lease ID");
    let extraction_id = ids.next().expect("requested attachment extraction ID");
    let display_filename = safe_attachment_filename(filename);
    let media_type = attachment_media_type(&display_filename).to_owned();

    let record = tauri::async_runtime::spawn_blocking(move || {
        let extraction = is_text_media_type(&media_type).then(|| extract_text(&raw));
        let staged = StagedAttachment::from_reader(
            Cursor::new(raw),
            &staging_directory,
            &stage_id,
            limits.max_attachment_bytes,
        )?;
        client.ingest_generic_attachment(IngestGenericAttachmentWrite {
            attachment: staged.write(IngestAttachmentMetadata {
                attachment_id,
                ingest_lease_id,
                draft_id,
                display_filename,
                media_type,
                now_ms,
                limits,
            }),
            extraction_id,
            extraction,
        })
    })
    .await
    .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
    .map_err(crate::database::commands::CommandError::from)?;
    state.wake_media_backup();
    Ok(record)
}

fn read_bounded_attachment(path: &Path, max_bytes: u64) -> Result<Vec<u8>, DatabaseError> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(DatabaseError::InvalidInput(
            "the selected attachment is not a regular file".into(),
        ));
    }
    if metadata.len() == 0 || metadata.len() > max_bytes {
        return Err(DatabaseError::InvalidInput(format!(
            "the selected attachment must contain between 1 and {max_bytes} bytes"
        )));
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        DatabaseError::InvalidInput("the selected attachment does not fit memory".into())
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    File::open(path)?
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(DatabaseError::InvalidInput(
            "the selected attachment changed while it was being read".into(),
        ));
    }
    Ok(bytes)
}

fn looks_like_pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}

fn extract_text(bytes: &[u8]) -> std::result::Result<Vec<TextFileSegment>, String> {
    if bytes.len() > MAX_TEXT_EXTRACTION_BYTES {
        return Err(format!(
            "Text extraction is limited to {MAX_TEXT_EXTRACTION_BYTES} bytes"
        ));
    }
    let decoded = decode_text(bytes)?;
    if decoded.contains('\0') {
        return Err("Text extraction stopped because the file contains NUL bytes".into());
    }
    split_text_passages(&decoded.replace("\r\n", "\n").replace('\r', "\n"))
}

fn decode_text(bytes: &[u8]) -> std::result::Result<String, String> {
    if let Some(body) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return std::str::from_utf8(body)
            .map(str::to_owned)
            .map_err(|_| "The text file is not valid UTF-8".into());
    }
    if let Some(body) = bytes.strip_prefix(&[0xff, 0xfe]) {
        return decode_utf16(body, true);
    }
    if let Some(body) = bytes.strip_prefix(&[0xfe, 0xff]) {
        return decode_utf16(body, false);
    }
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| "The text file is neither UTF-8 nor BOM-marked UTF-16".into())
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> std::result::Result<String, String> {
    if !bytes.len().is_multiple_of(2) {
        return Err("The UTF-16 text file has an incomplete code unit".into());
    }
    let units = bytes.chunks_exact(2).map(|pair| {
        let pair = [pair[0], pair[1]];
        if little_endian {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        }
    });
    char::decode_utf16(units)
        .collect::<std::result::Result<String, _>>()
        .map_err(|_| "The text file contains invalid UTF-16".into())
}

fn push_text_passage(
    passages: &mut Vec<TextFileSegment>,
    passage: TextFileSegment,
) -> std::result::Result<(), String> {
    if passages.len() >= MAX_TEXT_FILE_PASSAGES {
        return Err(format!(
            "Text extraction is limited to {MAX_TEXT_FILE_PASSAGES} passages"
        ));
    }
    passages.push(passage);
    Ok(())
}

fn split_text_passages(text: &str) -> std::result::Result<Vec<TextFileSegment>, String> {
    let mut passages = Vec::new();
    let mut buffered = String::new();
    let mut buffered_start = 0_u32;
    let mut buffered_end = 0_u32;

    let flush = |passages: &mut Vec<TextFileSegment>,
                 buffered: &mut String,
                 buffered_start: &mut u32,
                 buffered_end: &mut u32|
     -> std::result::Result<(), String> {
        if !buffered.trim().is_empty() {
            push_text_passage(
                passages,
                TextFileSegment {
                    start_line: *buffered_start,
                    end_line: *buffered_end,
                    content: std::mem::take(buffered),
                },
            )?;
        } else {
            buffered.clear();
        }
        *buffered_start = 0;
        *buffered_end = 0;
        Ok(())
    };

    for (index, line) in text.split('\n').enumerate() {
        let Ok(line_number) = u32::try_from(index + 1) else {
            break;
        };
        if line.trim().is_empty() {
            flush(
                &mut passages,
                &mut buffered,
                &mut buffered_start,
                &mut buffered_end,
            )?;
            continue;
        }
        let line_chars = line.chars().count();
        if line_chars > MAX_TEXT_PASSAGE_CHARS {
            flush(
                &mut passages,
                &mut buffered,
                &mut buffered_start,
                &mut buffered_end,
            )?;
            let mut chunk = String::new();
            let mut chunk_chars = 0;
            for character in line.chars() {
                chunk.push(character);
                chunk_chars += 1;
                if chunk_chars == MAX_TEXT_PASSAGE_CHARS {
                    push_text_passage(
                        &mut passages,
                        TextFileSegment {
                            start_line: line_number,
                            end_line: line_number,
                            content: std::mem::take(&mut chunk),
                        },
                    )?;
                    chunk_chars = 0;
                }
            }
            if !chunk.is_empty() {
                push_text_passage(
                    &mut passages,
                    TextFileSegment {
                        start_line: line_number,
                        end_line: line_number,
                        content: chunk,
                    },
                )?;
            }
            continue;
        }
        let separator_chars = usize::from(!buffered.is_empty());
        if buffered.chars().count() + separator_chars + line_chars > MAX_TEXT_PASSAGE_CHARS {
            flush(
                &mut passages,
                &mut buffered,
                &mut buffered_start,
                &mut buffered_end,
            )?;
        }
        if buffered.is_empty() {
            buffered_start = line_number;
        } else {
            buffered.push('\n');
        }
        buffered.push_str(line);
        buffered_end = line_number;
    }
    flush(
        &mut passages,
        &mut buffered,
        &mut buffered_start,
        &mut buffered_end,
    )?;
    Ok(passages)
}

fn attachment_media_type(filename: &str) -> &'static str {
    let extension = Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("txt" | "log" | "conf" | "ini") => "text/plain",
        Some("md" | "markdown") => "text/markdown",
        Some("csv") => "text/csv",
        Some("html" | "htm") => "text/html",
        Some("css") => "text/css",
        Some("xml") => "application/xml",
        Some("json" | "jsonl") => "application/json",
        Some("js" | "jsx" | "mjs" | "cjs") => "application/javascript",
        Some("toml") => "application/toml",
        Some("yaml" | "yml") => "application/yaml",
        Some(
            "ts" | "tsx" | "rs" | "py" | "pyi" | "rb" | "go" | "java" | "c" | "cc" | "cpp" | "h"
            | "hpp" | "swift" | "kt" | "kts" | "sh" | "bash" | "zsh" | "fish" | "scss" | "sass"
            | "less" | "sql" | "graphql" | "gql" | "proto" | "r" | "lua" | "ex" | "exs" | "erl"
            | "hrl" | "clj" | "cljs" | "scala" | "dart" | "vue" | "svelte",
        ) => "text/plain",
        Some("zip") => "application/zip",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("pptx") => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        Some("gz") => "application/gzip",
        Some("tar") => "application/x-tar",
        _ => "application/octet-stream",
    }
}

fn is_text_media_type(media_type: &str) -> bool {
    media_type.starts_with("text/")
        || matches!(
            media_type,
            "application/json"
                | "application/javascript"
                | "application/toml"
                | "application/xml"
                | "application/yaml"
        )
}

fn safe_attachment_filename(value: &str) -> String {
    let basename = value.rsplit(['/', '\\']).next().unwrap_or(value);
    let sanitized = basename
        .trim()
        .chars()
        .map(|character| {
            if character.is_control()
                || is_bidi_control(character)
                || matches!(character, '/' | '\\' | ':')
            {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    let sanitized = sanitized.trim_matches('.').trim();
    let sanitized = if sanitized.is_empty() {
        "Attachment"
    } else {
        sanitized
    };
    truncate_filename_preserving_extension(sanitized, MAX_MATERIALIZED_FILENAME_BYTES)
}

fn is_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

fn truncate_filename_preserving_extension(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let Some((stem, extension)) = value.rsplit_once('.') else {
        return truncate_utf8_bytes(value, max_bytes);
    };
    let suffix_bytes = extension.len().saturating_add(1);
    if stem.is_empty() || extension.is_empty() || suffix_bytes >= max_bytes {
        return truncate_utf8_bytes(value, max_bytes);
    }
    let stem = truncate_utf8_bytes(stem, max_bytes - suffix_bytes);
    format!("{stem}.{extension}")
}

fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[derive(Clone, Copy)]
enum ExternalAction {
    Open,
    Reveal,
}

async fn external_attachment_action<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, RuntimeState>,
    attachment_id: String,
    action: ExternalAction,
) -> Result<(), crate::database::commands::CommandError> {
    let client = state.database_client();
    let now_ms = state.now_ms();
    let max_bytes = state.media_limits().max_protocol_response_bytes;
    let directory = state.attachment_open_directory();
    let path = tauri::async_runtime::spawn_blocking(move || {
        let status = client.load_generic_attachment_status(attachment_id.clone())?;
        let payload = client.load_media_payload(attachment_id.clone(), now_ms, None, max_bytes)?;
        if payload.range.start != 0 || payload.bytes.len() as u64 != payload.total_byte_length {
            return Err(DatabaseError::InvalidInput(
                "only complete attachments can be opened externally".into(),
            ));
        }
        materialize_for_external_use(
            &directory,
            &attachment_id,
            &status.display_filename,
            &payload.bytes,
        )
    })
    .await
    .map_err(|error| crate::database::commands::CommandError::worker(error.to_string()))?
    .map_err(crate::database::commands::CommandError::from)?;
    run_external_action_on_main_thread(&app, path, action).await
}

fn materialize_for_external_use(
    directory: &Path,
    attachment_id: &str,
    filename: &str,
    bytes: &[u8],
) -> Result<PathBuf, DatabaseError> {
    validate_uuid_v7(attachment_id)?;
    let _guard = OPEN_DIRECTORY_LOCK.lock().map_err(|_| {
        DatabaseError::InvalidInput("attachment open directory lock was poisoned".into())
    })?;
    fs::create_dir_all(directory)?;
    remove_existing_materializations(directory, attachment_id)?;
    prune_open_directory(directory, MAX_OPEN_MATERIALIZATIONS - 1)?;

    let filename = safe_attachment_filename(filename);
    let path = directory.join(format!("{attachment_id}--{filename}"));
    let temporary_path = directory.join(format!("{attachment_id}.part"));
    remove_file_if_present(&temporary_path)?;
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

fn remove_existing_materializations(
    directory: &Path,
    attachment_id: &str,
) -> Result<(), DatabaseError> {
    let prefix = format!("{attachment_id}--");
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|filename| filename.starts_with(&prefix))
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn prune_open_directory(directory: &Path, keep: usize) -> Result<(), DatabaseError> {
    let mut materializations = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let Some(filename) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if filename.ends_with(".part") && is_open_materialization(&filename) {
            fs::remove_file(entry.path())?;
            continue;
        }
        if !filename.contains("--") || !is_open_materialization(&filename) {
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

fn is_open_materialization(filename: &str) -> bool {
    let id = filename
        .strip_suffix(".part")
        .or_else(|| filename.split_once("--").map(|(id, _)| id));
    id.is_some_and(|id| validate_uuid_v7(id).is_ok())
}

fn validate_uuid_v7(value: &str) -> Result<(), DatabaseError> {
    uuid::Uuid::parse_str(value)
        .ok()
        .filter(|id| id.get_version_num() == 7 && id.hyphenated().to_string().as_str() == value)
        .map(|_| ())
        .ok_or_else(|| {
            DatabaseError::InvalidInput("attachmentId must be a lowercase UUIDv7".into())
        })
}

async fn select_attachment_file<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<Option<PathBuf>, crate::database::commands::CommandError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let _ = sender.send(select_attachment_file_on_main_thread());
    })
    .map_err(|error| {
        crate::database::commands::CommandError::from(DatabaseError::InvalidInput(format!(
            "could not open the attachment picker: {error}"
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
fn select_attachment_file_on_main_thread() -> Result<Option<PathBuf>, DatabaseError> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};

    let mtm = MainThreadMarker::new().ok_or_else(|| {
        DatabaseError::InvalidInput(
            "the attachment picker was not opened on the main thread".into(),
        )
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
fn select_attachment_file_on_main_thread() -> Result<Option<PathBuf>, DatabaseError> {
    Err(DatabaseError::InvalidInput(
        "native attachment picking is available only on macOS".into(),
    ))
}

async fn run_external_action_on_main_thread<R: tauri::Runtime>(
    app: &AppHandle<R>,
    path: PathBuf,
    action: ExternalAction,
) -> Result<(), crate::database::commands::CommandError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let _ = sender.send(run_external_action(path, action));
    })
    .map_err(|error| {
        crate::database::commands::CommandError::from(DatabaseError::InvalidInput(format!(
            "could not ask macOS to open the attachment: {error}"
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

async fn run_source_url_on_main_thread<R: tauri::Runtime>(
    app: &AppHandle<R>,
    source_url: String,
) -> Result<(), crate::database::commands::CommandError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let _ = sender.send(run_source_url(source_url));
    })
    .map_err(|error| {
        crate::database::commands::CommandError::from(DatabaseError::InvalidInput(format!(
            "could not ask macOS to open the source: {error}"
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
fn run_external_action(path: PathBuf, action: ExternalAction) -> Result<(), DatabaseError> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSArray, NSString, NSURL};

    let _mtm = MainThreadMarker::new().ok_or_else(|| {
        DatabaseError::InvalidInput("the attachment was not opened on the main thread".into())
    })?;
    let path = NSString::from_str(&path.to_string_lossy());
    let url = NSURL::fileURLWithPath(&path);
    match action {
        ExternalAction::Open if NSWorkspace::sharedWorkspace().openURL(&url) => Ok(()),
        ExternalAction::Open => Err(DatabaseError::InvalidInput(
            "macOS could not open the attachment".into(),
        )),
        ExternalAction::Reveal => {
            let urls = NSArray::from_retained_slice(&[url]);
            NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&urls);
            Ok(())
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn run_external_action(_path: PathBuf, _action: ExternalAction) -> Result<(), DatabaseError> {
    Err(DatabaseError::InvalidInput(
        "opening attachments externally is available only on macOS".into(),
    ))
}

#[cfg(target_os = "macos")]
fn run_source_url(source_url: String) -> Result<(), DatabaseError> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSURL};

    let _mtm = MainThreadMarker::new().ok_or_else(|| {
        DatabaseError::InvalidInput("the source was not opened on the main thread".into())
    })?;
    let value = NSString::from_str(&source_url);
    // The URL was loaded by ID from Kosh's normalized HTTP(S)-only source
    // table; the native workspace never receives renderer-controlled URLs.
    let url = NSURL::URLWithString(&value)
        .ok_or_else(|| DatabaseError::InvalidInput("the stored source URL is invalid".into()))?;
    if NSWorkspace::sharedWorkspace().openURL(&url) {
        Ok(())
    } else {
        Err(DatabaseError::InvalidInput(
            "macOS could not open the source".into(),
        ))
    }
}

#[cfg(not(target_os = "macos"))]
fn run_source_url(_source_url: String) -> Result<(), DatabaseError> {
    Err(DatabaseError::InvalidInput(
        "opening source URLs is available only on macOS".into(),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use tauri::Listener;

    use super::{
        attachment_media_type, decode_text, emit_file_drop_notice, extract_text, looks_like_pdf,
        materialize_for_external_use, read_bounded_attachment, recover_attachment_open_directory,
        safe_attachment_filename, split_text_passages, FileDropNotice, FileDropSelection,
        FILE_DROP_EVENT, MAX_OPEN_MATERIALIZATIONS, MAX_TEXT_EXTRACTION_BYTES,
        MAX_TEXT_FILE_PASSAGES,
    };

    #[test]
    fn file_drop_notice_reaches_only_the_originating_webview() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app");
        let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("main webview");
        let quick_add = tauri::WebviewWindowBuilder::new(&app, "quick-add", Default::default())
            .build()
            .expect("quick-add webview");
        let main_notices = Arc::new(AtomicUsize::new(0));
        let quick_add_notices = Arc::new(AtomicUsize::new(0));
        let main_count = Arc::clone(&main_notices);
        let quick_add_count = Arc::clone(&quick_add_notices);
        main.listen(FILE_DROP_EVENT, move |_| {
            main_count.fetch_add(1, Ordering::SeqCst);
        });
        quick_add.listen(FILE_DROP_EVENT, move |_| {
            quick_add_count.fetch_add(1, Ordering::SeqCst);
        });

        emit_file_drop_notice(
            &main.as_ref().window(),
            FileDropNotice {
                selections: vec![FileDropSelection {
                    selection_id: "019f547b-6200-7000-8000-000000000996".to_owned(),
                    filename: "main-only.txt".to_owned(),
                }],
            },
        )
        .expect("targeted file-drop notice");

        assert_eq!(main_notices.load(Ordering::SeqCst), 1);
        assert_eq!(quick_add_notices.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn text_decoding_supports_utf8_and_bom_marked_utf16() {
        assert_eq!(decode_text(b"one\ntwo").expect("UTF-8"), "one\ntwo");
        assert_eq!(
            decode_text(&[0xff, 0xfe, b'o', 0, b'k', 0]).expect("UTF-16 LE"),
            "ok"
        );
        assert!(decode_text(&[0xff, 0xfe, b'o']).is_err());
        assert!(extract_text(&[0xff, 0xfe, 0x00]).is_err());
    }

    #[test]
    fn passages_have_exact_ordered_line_ranges_and_bound_long_lines() {
        let text = format!("first\nsecond\n\n{}\nlast", "x".repeat(2_001));
        let passages = split_text_passages(&text).expect("bounded passages");
        assert_eq!(
            passages
                .iter()
                .map(|passage| (passage.start_line, passage.end_line))
                .collect::<Vec<_>>(),
            [(1, 2), (4, 4), (4, 4), (4, 4), (5, 5)]
        );
        assert!(passages
            .iter()
            .all(|passage| passage.content.chars().count() <= 1_000));
    }

    #[test]
    fn maximum_single_line_extraction_is_split_in_linear_bounded_chunks() {
        let text = "é".repeat(MAX_TEXT_EXTRACTION_BYTES / 2);
        let passages = split_text_passages(&text).expect("bounded passages");

        assert_eq!(
            passages
                .iter()
                .map(|passage| passage.content.len())
                .sum::<usize>(),
            MAX_TEXT_EXTRACTION_BYTES
        );
        assert!(passages
            .iter()
            .all(|passage| passage.content.chars().count() <= 1_000));
        assert!(passages
            .iter()
            .all(|passage| (passage.start_line, passage.end_line) == (1, 1)));
    }

    #[test]
    fn tiny_paragraphs_stop_at_the_passage_limit() {
        let text = "x\n\n".repeat(MAX_TEXT_FILE_PASSAGES + 1);
        assert_eq!(
            split_text_passages(&text).expect_err("passage limit"),
            format!("Text extraction is limited to {MAX_TEXT_FILE_PASSAGES} passages")
        );
    }

    #[test]
    fn media_types_are_allowlisted_and_mismatches_remain_opaque() {
        assert_eq!(attachment_media_type("notes.md"), "text/markdown");
        assert_eq!(attachment_media_type("archive.zip"), "application/zip");
        assert_eq!(
            attachment_media_type("pretends-to-be-an-image.png"),
            "application/octet-stream"
        );
    }

    #[test]
    fn content_sniffing_recognizes_renamed_pdfs_without_trusting_extensions() {
        assert!(looks_like_pdf(b"%PDF-1.7 renamed document"));
        assert!(!looks_like_pdf(b"plain text in a file named notes.pdf"));
        assert!(!looks_like_pdf(
            b"documentation describing the %PDF- header is still plain text"
        ));
    }

    #[test]
    fn bounded_reads_reject_missing_empty_and_oversized_files() {
        let directory = tempfile::tempdir().expect("temporary attachment directory");
        let missing = directory.path().join("missing.txt");
        let empty = directory.path().join("empty.txt");
        let huge = directory.path().join("huge.txt");
        std::fs::write(&empty, []).expect("empty fixture");
        std::fs::write(&huge, b"12345").expect("oversized fixture");

        assert!(read_bounded_attachment(&missing, 4).is_err());
        assert!(read_bounded_attachment(&empty, 4).is_err());
        assert!(read_bounded_attachment(&huge, 4).is_err());
    }

    #[test]
    fn filenames_are_safe_and_bounded_for_materialization() {
        assert_eq!(safe_attachment_filename("../../notes:1.txt"), "notes_1.txt");
        assert_eq!(safe_attachment_filename(" \n "), "Attachment");
        assert!(safe_attachment_filename(&"é".repeat(200)).len() <= 160);
        let long_text_filename = format!("{}.txt", "n".repeat(200));
        let safe_text_filename = safe_attachment_filename(&long_text_filename);
        assert_eq!(safe_text_filename.len(), 160);
        assert!(safe_text_filename.ends_with(".txt"));
        assert_eq!(attachment_media_type(&safe_text_filename), "text/plain");
        assert_eq!(
            safe_attachment_filename("invoice\u{202e}fdp.exe"),
            "invoice_fdp.exe"
        );
    }

    #[test]
    fn external_materializations_are_private_bounded_and_recoverable() {
        let directory = tempfile::tempdir().expect("temporary open directory");
        for index in 0..=MAX_OPEN_MATERIALIZATIONS {
            let id = format!("019f547b-6200-7{index:03x}-8000-000000000001");
            materialize_for_external_use(
                directory.path(),
                &id,
                &format!("fixture-{index}.txt"),
                b"private",
            )
            .expect("materialize fixture");
        }
        assert_eq!(
            std::fs::read_dir(directory.path())
                .expect("read open directory")
                .count(),
            MAX_OPEN_MATERIALIZATIONS
        );
        #[cfg(unix)]
        for entry in std::fs::read_dir(directory.path()).expect("read open directory") {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                entry
                    .expect("open fixture")
                    .metadata()
                    .expect("fixture metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert_eq!(
            recover_attachment_open_directory(directory.path()).expect("recover open directory"),
            MAX_OPEN_MATERIALIZATIONS
        );
    }
}

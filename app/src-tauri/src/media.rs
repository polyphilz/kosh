use std::{fs, path::Path};

use tauri::{http, AppHandle, Manager, State};

use crate::{
    database::{
        media::MediaByteRange, DatabaseError, MediaIntegrityReport, MediaLimits,
        MediaMaintenanceReport,
    },
    runtime::RuntimeState,
};

const MAX_STAGING_RECOVERY_FILES: usize = 1_024;
const MEDIA_PATH_PREFIX: &str = "/attachment/";

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

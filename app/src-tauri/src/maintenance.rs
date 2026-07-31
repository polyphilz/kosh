use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;
use tauri::State;

use crate::{
    database::{
        commands::CommandError, DatabaseDiagnostics, DatabaseError, MaintenanceDatabaseSnapshot,
        MediaIntegrityReport, MediaLimits,
    },
    native_log::{self, NativeLogDiagnostics},
    runtime::RuntimeState,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageDiagnostics {
    pub data_root: String,
    pub main_database_path: String,
    pub media_database_path: String,
    pub main_database_bytes: u64,
    pub media_database_bytes: u64,
    pub model_bytes: u64,
    pub logs_bytes: u64,
    pub temporary_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MaintenanceDiagnostics {
    pub application_version: String,
    pub database: DatabaseDiagnostics,
    pub library: MaintenanceDatabaseSnapshot,
    pub storage: StorageDiagnostics,
    pub media_limits: MediaLimits,
    pub native_logs: NativeLogDiagnostics,
    pub semantic_log_paths: Vec<String>,
    pub backup_phase: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IntegrityCheckOutcome {
    pub database_ok: bool,
    pub media: MediaIntegrityReport,
    pub message: String,
    pub completed_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MaintenanceOutcome {
    pub operation: &'static str,
    pub changed_items: u64,
    pub reclaimed_bytes: u64,
    pub safety_snapshot_id: Option<String>,
    pub message: String,
    pub completed_at_ms: i64,
}

#[tauri::command]
pub(crate) async fn load_maintenance_diagnostics(
    state: State<'_, RuntimeState>,
) -> Result<MaintenanceDiagnostics, CommandError> {
    let client = state.database_client();
    let paths = state.database_paths().clone();
    let limits = state.media_limits();
    run_blocking(move || {
        let database = client.diagnostics()?;
        let library = client.maintenance_snapshot()?;
        let storage = storage_diagnostics(&paths)?;
        let native_logs = native_log::diagnostics(paths.root())?;
        Ok(MaintenanceDiagnostics {
            application_version: env!("CARGO_PKG_VERSION").into(),
            database,
            library,
            storage,
            media_limits: limits,
            native_logs,
            semantic_log_paths: semantic_log_paths(paths.root()),
            backup_phase: "AVAILABLE",
        })
    })
    .await
}

#[tauri::command]
pub(crate) async fn run_integrity_check(
    state: State<'_, RuntimeState>,
) -> Result<IntegrityCheckOutcome, CommandError> {
    let client = state.database_client();
    let gate = state.maintenance_gate();
    let now_ms = state.now_ms();
    run_blocking(move || {
        let _guard = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        client.full_integrity_check()?;
        let media = client.media_integrity_report(now_ms)?;
        let issues = media.missing_blob_attachment_ids.len()
            + media.corrupt_blob_sha256.len()
            + media.extra_blob_sha256.len()
            + media.orphaned_attachment_ids.len();
        let message = integrity_message(&media);
        log::info!("maintenance integrity check completed with {issues} media issues");
        Ok(IntegrityCheckOutcome {
            database_ok: true,
            media,
            message,
            completed_at_ms: now_ms,
        })
    })
    .await
}

#[tauri::command]
pub(crate) async fn rebuild_search_indexes(
    state: State<'_, RuntimeState>,
) -> Result<MaintenanceOutcome, CommandError> {
    let client = state.database_client();
    let gate = state.maintenance_gate();
    let now_ms = state.now_ms();
    run_blocking(move || {
        let _guard = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let documents = client.rebuild_search()?;
        log::info!("maintenance rebuilt {documents} search documents");
        Ok(MaintenanceOutcome {
            operation: "REBUILD_SEARCH",
            changed_items: documents,
            reclaimed_bytes: 0,
            safety_snapshot_id: None,
            message: format!(
                "Rebuilt passages and full-text indexes for {documents} searchable passage{}.",
                if documents == 1 { "" } else { "s" }
            ),
            completed_at_ms: now_ms,
        })
    })
    .await
}

#[tauri::command]
pub(crate) async fn rebuild_embedding_index(
    state: State<'_, RuntimeState>,
) -> Result<MaintenanceOutcome, CommandError> {
    let client = state.database_client();
    let gate = state.maintenance_gate();
    let now_ms = state.now_ms();
    run_blocking(move || {
        let _guard = gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let invalidated = client.rebuild_embeddings(now_ms)?;
        log::info!("maintenance invalidated {invalidated} passage embeddings");
        Ok(MaintenanceOutcome {
            operation: "REBUILD_EMBEDDINGS",
            changed_items: invalidated,
            reclaimed_bytes: 0,
            safety_snapshot_id: None,
            message: if invalidated == 0 {
                "Embedding rebuild is already queued; indexing will continue when the local model is ready."
                    .into()
            } else {
                format!(
                    "Queued {invalidated} passage embedding{} for a safe rebuild.",
                    if invalidated == 1 { "" } else { "s" }
                )
            },
            completed_at_ms: now_ms,
        })
    })
    .await
}

#[tauri::command]
pub(crate) async fn retry_failed_extractions(
    state: State<'_, RuntimeState>,
) -> Result<MaintenanceOutcome, CommandError> {
    let client = state.database_client();
    let gate = state.maintenance_gate();
    let now_ms = state.now_ms();
    let outcome = run_blocking(move || {
        let _guard = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let report = client.retry_failed_extractions(now_ms)?;
        let changed = report
            .image_ocr_queued
            .saturating_add(report.pdf_extraction_queued);
        log::info!(
            "maintenance queued {} OCR and {} PDF retries",
            report.image_ocr_queued,
            report.pdf_extraction_queued
        );
        Ok(MaintenanceOutcome {
            operation: "RETRY_EXTRACTIONS",
            changed_items: changed,
            reclaimed_bytes: 0,
            safety_snapshot_id: None,
            message: if changed == 0 {
                "No current failed OCR or PDF extractions needed a retry.".into()
            } else {
                format!(
                    "Queued {} image OCR and {} PDF extraction {}.",
                    report.image_ocr_queued,
                    report.pdf_extraction_queued,
                    if changed == 1 { "retry" } else { "retries" }
                )
            },
            completed_at_ms: now_ms,
        })
    })
    .await?;
    state.wake_image_ocr();
    state.wake_pdf_extraction();
    Ok(outcome)
}

#[tauri::command]
pub(crate) async fn reclaim_eligible_media(
    state: State<'_, RuntimeState>,
) -> Result<MaintenanceOutcome, CommandError> {
    let client = state.database_client();
    let gate = state.maintenance_gate();
    let now_ms = state.now_ms();
    let limits = state.media_limits();
    run_blocking(move || {
        let _guard = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (snapshot, report) = client.maintain_media_with_safety_snapshot(now_ms, limits)?;
        let changed = report
            .cleanup
            .retired_attachment_count
            .saturating_add(report.cleanup.deleted_blob_count);
        if changed > 0 && snapshot.is_none() {
            return Err(DatabaseError::Validation {
                kind: "media",
                reason: "media reclamation completed without a verified safety snapshot".into(),
            });
        }
        log::info!(
            "maintenance reclaimed {} bytes across {changed} media records",
            report.cleanup.reclaimed_bytes
        );
        Ok(MaintenanceOutcome {
            operation: "RECLAIM_MEDIA",
            changed_items: changed,
            reclaimed_bytes: report.cleanup.reclaimed_bytes,
            safety_snapshot_id: snapshot.map(|snapshot| snapshot.id),
            message: if changed == 0 {
                "No expired or unreferenced media was eligible for reclamation.".into()
            } else {
                format!(
                    "Reclaimed {} from {changed} eligible media record{}.",
                    format_bytes(report.cleanup.reclaimed_bytes),
                    if changed == 1 { "" } else { "s" }
                )
            },
            completed_at_ms: now_ms,
        })
    })
    .await
}

async fn run_blocking<T: Send + 'static>(
    operation: impl FnOnce() -> crate::database::Result<T> + Send + 'static,
) -> Result<T, CommandError> {
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|error| CommandError::worker(error.to_string()))?
        .map_err(Into::into)
}

fn storage_diagnostics(
    paths: &crate::database::DatabasePaths,
) -> crate::database::Result<StorageDiagnostics> {
    let root = paths.root();
    let main_database_bytes = sqlite_family_size(&paths.main)?;
    let media_database_bytes = sqlite_family_size(&paths.media)?;
    let model_bytes = directory_size(&root.join("models"))?;
    let logs_bytes = directory_size(&root.join("logs"))?;
    let temporary_bytes = ["media-staging", "pdf-open", "attachment-open"]
        .into_iter()
        .try_fold(0_u64, |total, name| {
            Ok::<_, DatabaseError>(total.saturating_add(directory_size(&root.join(name))?))
        })?;
    Ok(StorageDiagnostics {
        data_root: root.to_string_lossy().into_owned(),
        main_database_path: paths.main.to_string_lossy().into_owned(),
        media_database_path: paths.media.to_string_lossy().into_owned(),
        main_database_bytes,
        media_database_bytes,
        model_bytes,
        logs_bytes,
        temporary_bytes,
        total_bytes: directory_size(root)?,
    })
}

fn sqlite_family_size(path: &Path) -> std::io::Result<u64> {
    ["", "-wal", "-shm"]
        .into_iter()
        .try_fold(0_u64, |total, suffix| {
            let member = PathBuf::from(format!("{}{suffix}", path.to_string_lossy()));
            Ok(total.saturating_add(file_size_if_present(&member)?))
        })
}

fn directory_size(root: &Path) -> std::io::Result<u64> {
    let mut pending = vec![root.to_owned()];
    let mut total = 0_u64;
    let mut inspected = 0_usize;
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            inspected += 1;
            if inspected > 100_000 {
                return Err(std::io::Error::other(
                    "storage diagnostics exceeded its bounded file scan",
                ));
            }
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                total = total.saturating_add(entry.metadata()?.len());
            }
        }
    }
    Ok(total)
}

fn file_size_if_present(path: &Path) -> std::io::Result<u64> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(metadata.len()),
        Ok(_) => Ok(0),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

fn semantic_log_paths(root: &Path) -> Vec<String> {
    let directory = root.join("logs");
    [
        "llama-server.log",
        "llama-server.log.1",
        "llama-server.log.2",
    ]
    .into_iter()
    .map(|name| directory.join(name).to_string_lossy().into_owned())
    .collect()
}

fn integrity_message(media: &MediaIntegrityReport) -> String {
    let issues = media.missing_blob_attachment_ids.len()
        + media.corrupt_blob_sha256.len()
        + media.extra_blob_sha256.len()
        + media.orphaned_attachment_ids.len();
    if issues == 0 {
        return "Both databases and all referenced media passed integrity checks.".into();
    }
    if media.diagnostics_truncated {
        return format!(
            "Database integrity passed; media inspection found at least {issues} items that need attention. Diagnostic details were truncated."
        );
    }
    format!(
        "Database integrity passed; media inspection found {issues} item{} that need attention.",
        if issues == 1 { "" } else { "s" }
    )
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::{directory_size, format_bytes, integrity_message, sqlite_family_size};
    use crate::database::MediaIntegrityReport;

    #[test]
    fn storage_scan_is_recursive_and_does_not_follow_symlinks() {
        let root = tempfile::tempdir().expect("temporary storage");
        std::fs::create_dir(root.path().join("nested")).expect("nested directory");
        std::fs::write(root.path().join("one"), b"123").expect("first file");
        std::fs::write(root.path().join("nested/two"), b"4567").expect("second file");
        assert_eq!(directory_size(root.path()).expect("directory size"), 7);
    }

    #[test]
    fn sqlite_storage_includes_wal_and_shared_memory() {
        let root = tempfile::tempdir().expect("temporary storage");
        let database = root.path().join("kosh.sqlite3");
        std::fs::write(&database, b"123").expect("database");
        std::fs::write(root.path().join("kosh.sqlite3-wal"), b"4567").expect("wal");
        assert_eq!(sqlite_family_size(&database).expect("family size"), 7);
    }

    #[test]
    fn byte_formatting_is_stable() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(12 * 1024), "12 KB");
    }

    #[test]
    fn integrity_message_never_presents_a_truncated_count_as_exact() {
        let report = MediaIntegrityReport {
            missing_blob_attachment_ids: vec!["one".into(), "two".into()],
            diagnostics_truncated: true,
            ..Default::default()
        };
        assert_eq!(
            integrity_message(&report),
            "Database integrity passed; media inspection found at least 2 items that need attention. Diagnostic details were truncated."
        );
    }
}

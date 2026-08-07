mod attachments;
pub mod backup;
mod clipboard;
mod database;
#[cfg(target_os = "macos")]
mod distribution_signing;
mod embedding;
mod embedding_runtime;
mod maintenance;
mod media;
mod native_log;
mod passage_embedding_indexer;
pub mod relevance;
mod runtime;
mod startup_smoke;
mod windows;

#[cfg(feature = "test-support")]
pub mod test_support;

use std::path::PathBuf;

use runtime::RuntimeState;
use tauri::{Builder, Manager};
use tauri_plugin_deep_link::DeepLinkExt;

const DATA_DIR_ENV: &str = "KOSH_DATA_DIR";

fn select_data_dir(
    app_data_dir: PathBuf,
    debug_override: Option<PathBuf>,
    allow_debug_override: bool,
) -> PathBuf {
    if allow_debug_override {
        if let Some(path) = debug_override.filter(|path| !path.as_os_str().is_empty()) {
            return path;
        }
    }

    app_data_dir
}

#[cfg(not(feature = "test-support"))]
fn with_commands(builder: Builder<tauri::Wry>) -> Builder<tauri::Wry> {
    builder.invoke_handler(tauri::generate_handler![
        runtime::runtime_probe,
        clipboard::copy_text,
        backup::checkpoint::checkpoint_backup_status,
        backup::checkpoint::backup_now,
        backup::settings::load_backup_settings,
        backup::settings::test_backup_connection,
        backup::settings::configure_backup,
        backup::settings::set_backup_enabled,
        backup::settings::list_backup_checkpoints,
        backup::settings::preview_backup_restore,
        backup::settings::drill_backup_restore,
        backup::settings::take_over_backup,
        runtime::semantic_runtime_status,
        runtime::passage_embedding_index_status,
        runtime::prepare_semantic_runtime,
        runtime::retry_semantic_runtime,
        runtime::repair_semantic_runtime,
        runtime::semantic_runtime_logs,
        maintenance::load_maintenance_diagnostics,
        maintenance::run_integrity_check,
        maintenance::rebuild_search_indexes,
        maintenance::rebuild_embedding_index,
        maintenance::retry_failed_extractions,
        maintenance::reclaim_eligible_media,
        media::media_limits,
        media::media_integrity_scan,
        media::maintain_media,
        media::select_image,
        media::ingest_selected_image,
        media::capture_clipboard_image,
        media::ingest_clipboard_image,
        media::ingest_dropped_images,
        media::image_status,
        media::retry_image_ocr,
        media::image_ocr_diagnostics,
        attachments::select_attachment,
        attachments::ingest_selected_attachment,
        attachments::open_attachment_external,
        attachments::reveal_attachment_in_finder,
        attachments::open_source_url,
        attachments::set_file_drop_consumer_active,
        attachments::discard_file_drop_selections,
        database::commands::load_tidbit,
        database::commands::delete_tidbit,
        database::commands::restore_tidbit,
        database::commands::resolve_citation,
        database::commands::search_passages,
        database::commands::save_working_copy,
        database::commands::reserve_working_copy_for_media,
        database::commands::load_working_copy,
        database::commands::list_working_copies,
        database::commands::checkpoint_working_copy,
        database::commands::discard_working_copy,
        windows::acknowledge_quit,
        windows::cancel_update_relaunch,
        windows::cancel_quick_add_dismiss,
        windows::prepare_update_relaunch,
        windows::complete_quick_add_dismiss,
        windows::mark_quick_add_frontend_ready,
        windows::load_shortcut_settings,
        windows::set_automatic_update_checks,
        windows::set_quick_add_file_dialog_open,
        windows::set_shortcut_settings,
        windows::show_main,
        windows::show_quick_add,
    ])
}

#[cfg(feature = "test-support")]
fn with_commands<R: tauri::Runtime>(builder: Builder<R>) -> Builder<R> {
    // Native window commands require Wry/AppKit. The mock-runtime handler keeps
    // database and media commands available to integration tests; lifecycle
    // logic is covered by the platform unit tests in windows.rs.
    builder.invoke_handler(tauri::generate_handler![
        runtime::runtime_probe,
        clipboard::copy_text,
        backup::checkpoint::checkpoint_backup_status,
        backup::checkpoint::backup_now,
        backup::settings::load_backup_settings,
        backup::settings::test_backup_connection,
        backup::settings::configure_backup,
        backup::settings::set_backup_enabled,
        backup::settings::list_backup_checkpoints,
        backup::settings::preview_backup_restore,
        backup::settings::drill_backup_restore,
        backup::settings::take_over_backup,
        runtime::semantic_runtime_status,
        runtime::passage_embedding_index_status,
        runtime::prepare_semantic_runtime,
        runtime::retry_semantic_runtime,
        runtime::repair_semantic_runtime,
        runtime::semantic_runtime_logs,
        maintenance::load_maintenance_diagnostics,
        maintenance::run_integrity_check,
        maintenance::rebuild_search_indexes,
        maintenance::rebuild_embedding_index,
        maintenance::retry_failed_extractions,
        maintenance::reclaim_eligible_media,
        media::media_limits,
        media::media_integrity_scan,
        media::maintain_media,
        media::select_image,
        media::ingest_selected_image,
        media::capture_clipboard_image,
        media::ingest_clipboard_image,
        media::ingest_dropped_images,
        media::image_status,
        media::retry_image_ocr,
        media::image_ocr_diagnostics,
        attachments::select_attachment,
        attachments::ingest_selected_attachment,
        attachments::open_attachment_external,
        attachments::reveal_attachment_in_finder,
        attachments::open_source_url,
        attachments::set_file_drop_consumer_active,
        attachments::discard_file_drop_selections,
        database::commands::load_tidbit,
        database::commands::delete_tidbit,
        database::commands::restore_tidbit,
        database::commands::resolve_citation,
        database::commands::search_passages,
        database::commands::save_working_copy,
        database::commands::reserve_working_copy_for_media,
        database::commands::load_working_copy,
        database::commands::list_working_copies,
        database::commands::checkpoint_working_copy,
        database::commands::discard_working_copy,
    ])
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = with_commands(
        tauri::Builder::default()
            .plugin(tauri_plugin_single_instance::init(
                |app, _arguments, _working_directory| {
                    if let Err(error) = windows::show_main(app.clone()) {
                        log::error!("failed to show Kosh for a secondary launch: {error}");
                    }
                },
            ))
            .plugin(tauri_plugin_deep_link::init())
            .plugin(tauri_plugin_process::init())
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_global_shortcut::Builder::new().build())
            .register_uri_scheme_protocol("kosh-media", |context, request| {
                media::protocol_response(context.app_handle(), request)
            })
            .on_window_event(|window, event| {
                attachments::handle_file_drop(window, event);
                windows::handle_window_event(window, event);
            }),
    )
    .setup(|app| {
        let deep_link_app = app.handle().clone();
        app.deep_link().on_open_url(move |_| {
            if let Err(error) = windows::show_main(deep_link_app.clone()) {
                log::error!("failed to show Kosh for a deep link: {error}");
            }
        });
        let data_dir = select_data_dir(
            app.path().app_data_dir()?,
            std::env::var_os(DATA_DIR_ENV).map(PathBuf::from),
            cfg!(debug_assertions),
        );
        let resource_dir = app.path().resource_dir().ok();
        std::fs::create_dir_all(&data_dir)?;
        native_log::install(&data_dir);
        let runtime = RuntimeState::production(data_dir, resource_dir)?;
        let shortcut_settings = runtime.database_client().load_shortcut_settings()?;
        app.manage(runtime);
        let startup_smoke = startup_smoke::run_if_requested(app)?;
        windows::setup(app, shortcut_settings, !startup_smoke)?;
        app.state::<RuntimeState>()
            .reconcile_backup_takeover_async();
        Ok(())
    })
    .build(tauri::generate_context!())
    .expect("error while building Kosh");

    app.run(|app, event| match event {
        tauri::RunEvent::ExitRequested { code, api, .. } => {
            windows::handle_exit_requested(app, code, &api);
        }
        tauri::RunEvent::Reopen { .. } => {
            if let Err(error) = windows::show_main(app.clone()) {
                log::error!("failed to show Kosh after activation: {error}");
            }
        }
        tauri::RunEvent::Exit => {
            let runtime = app.state::<RuntimeState>();
            runtime.shutdown_for_exit();
        }
        _ => {}
    });
}

pub fn run_recovery_cli_if_requested() -> Option<i32> {
    backup::recovery_cli::run_if_requested()
}

pub use database::{
    AttachmentIngestInput, AttachmentKind, AttachmentRecord, CitationAttachment, CitationLocator,
    CitationResolution, CitationState, CitationTidbit, Database, DatabaseDiagnostics,
    DatabaseError, DatabasePaths, DeleteTidbitInput, ImageOcrDiagnostics, ImageOcrRecovery,
    ImageOcrStatus, ImageRecord, ImageStatusRecord, LexicalSearchMode, MediaCleanupResult,
    MediaIntegrityReport, MediaLimits, MediaMaintenanceReport, PassageSearchResult,
    RestoreTidbitInput, SearchExecutionMode, SearchField, SearchHighlight, SearchPassagesInput,
    SearchPassagesResponse, SemanticSearchReadiness, SetAutomaticUpdateChecksInput,
    SetShortcutSettingsInput, ShortcutSettings, SourceDraft, Tidbit, TidbitDraft, TidbitSource,
};
pub use embedding::{TextEmbeddingConfig, TextEmbeddingManifest};
pub use embedding_runtime::{
    EmbeddingRuntime, SemanticRuntimeError, SemanticRuntimeLogs, SemanticRuntimePhase,
    SemanticRuntimeStatus,
};
pub use passage_embedding_indexer::{PassageEmbeddingIndexPhase, PassageEmbeddingIndexStatus};
pub use runtime::RuntimeProbe;

#[cfg(test)]
mod tests {
    use super::select_data_dir;
    use std::path::PathBuf;

    #[test]
    fn debug_build_uses_nonempty_override() {
        let release = PathBuf::from("/release/kosh");
        let local = PathBuf::from("/workspace/kosh/app/.data/local");

        assert_eq!(select_data_dir(release, Some(local.clone()), true), local);
    }

    #[test]
    fn release_build_ignores_override() {
        let release = PathBuf::from("/release/kosh");
        let local = PathBuf::from("/workspace/kosh/app/.data/local");

        assert_eq!(
            select_data_dir(release.clone(), Some(local), false),
            release
        );
    }

    #[test]
    fn empty_override_falls_back_to_app_data() {
        let release = PathBuf::from("/release/kosh");

        assert_eq!(
            select_data_dir(release.clone(), Some(PathBuf::new()), true),
            release
        );
    }
}

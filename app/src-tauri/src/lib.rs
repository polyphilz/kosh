mod attachments;
mod claude;
mod database;
mod embedding;
mod embedding_runtime;
mod maintenance;
mod media;
mod native_log;
mod passage_embedding_indexer;
mod pdf;
pub mod relevance;
pub mod research;
mod runtime;
#[cfg(debug_assertions)]
mod startup_smoke;
mod windows;

#[cfg(feature = "test-support")]
pub mod test_support;

use std::path::PathBuf;

use runtime::RuntimeState;
use tauri::{Builder, Manager};

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
        claude::claude_setup_status,
        claude::claude_cli_defaults,
        claude::start_research_process,
        claude::rerun_research_process,
        claude::cancel_research_process,
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
        attachments::attachment_status,
        attachments::open_attachment_external,
        attachments::reveal_attachment_in_finder,
        attachments::open_source_url,
        attachments::set_file_drop_consumer_active,
        attachments::discard_file_drop_selections,
        pdf::select_pdf,
        pdf::ingest_selected_pdf,
        pdf::pdf_status,
        pdf::retry_pdf_extraction,
        pdf::open_pdf_external,
        database::commands::create_tidbit,
        database::commands::load_tidbit,
        database::commands::list_tidbits,
        database::commands::list_tidbit_revisions,
        database::commands::load_tidbit_revision,
        database::commands::edit_tidbit,
        database::commands::delete_tidbit,
        database::commands::restore_tidbit,
        database::commands::purge_tidbit,
        database::commands::list_research_runs,
        database::commands::load_research_run,
        database::commands::save_research_answer_as_tidbit,
        database::commands::resolve_citation,
        database::commands::search_passages,
        database::commands::save_draft,
        database::commands::load_draft,
        database::commands::clear_draft,
        windows::acknowledge_quit,
        windows::dismiss_quick_add,
        windows::load_shortcut_settings,
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
        claude::claude_setup_status,
        claude::claude_cli_defaults,
        claude::start_research_process,
        claude::rerun_research_process,
        claude::cancel_research_process,
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
        attachments::attachment_status,
        attachments::open_attachment_external,
        attachments::reveal_attachment_in_finder,
        attachments::open_source_url,
        attachments::set_file_drop_consumer_active,
        attachments::discard_file_drop_selections,
        pdf::select_pdf,
        pdf::ingest_selected_pdf,
        pdf::pdf_status,
        pdf::retry_pdf_extraction,
        pdf::open_pdf_external,
        database::commands::create_tidbit,
        database::commands::load_tidbit,
        database::commands::list_tidbits,
        database::commands::list_tidbit_revisions,
        database::commands::load_tidbit_revision,
        database::commands::edit_tidbit,
        database::commands::delete_tidbit,
        database::commands::restore_tidbit,
        database::commands::purge_tidbit,
        database::commands::list_research_runs,
        database::commands::load_research_run,
        database::commands::save_research_answer_as_tidbit,
        database::commands::resolve_citation,
        database::commands::search_passages,
        database::commands::save_draft,
        database::commands::load_draft,
        database::commands::clear_draft,
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
        let data_dir = select_data_dir(
            app.path().app_data_dir()?,
            std::env::var_os(DATA_DIR_ENV).map(PathBuf::from),
            cfg!(debug_assertions),
        );
        let resource_dir = app.path().resource_dir().ok();
        std::fs::create_dir_all(&data_dir)?;
        native_log::install(&data_dir)?;
        let runtime = RuntimeState::production(data_dir, resource_dir)?;
        let shortcut_settings = runtime.database_client().load_shortcut_settings()?;
        app.manage(runtime);
        windows::setup(app, shortcut_settings)?;
        #[cfg(debug_assertions)]
        let startup_smoke = startup_smoke::run_if_requested(app)?;
        #[cfg(not(debug_assertions))]
        let startup_smoke = false;
        if !startup_smoke {
            app.state::<RuntimeState>()
                .claude_processes()
                .recover_work_directories_async();
        }
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
            app.state::<RuntimeState>().claude_processes().shutdown();
        }
        _ => {}
    });
}

pub fn run_pdf_worker_if_requested() -> Option<i32> {
    pdf::run_worker_if_requested()
}

pub use database::{
    AttachmentExtractionStatus, AttachmentIngestInput, AttachmentKind, AttachmentRecord,
    CitationAttachment, CitationLocator, CitationResolution, CitationState, CitationTidbit,
    ClearDraftInput, Database, DatabaseDiagnostics, DatabaseError, DatabasePaths,
    DeleteTidbitInput, Draft, EditTidbitInput, GenericAttachmentRecord,
    GenericAttachmentStatusRecord, ImageOcrDiagnostics, ImageOcrRecovery, ImageOcrStatus,
    ImageRecord, ImageStatusRecord, LexicalSearchMode, ListTidbitRevisionsInput, ListTidbitsInput,
    MediaCleanupResult, MediaIntegrityReport, MediaLimits, MediaMaintenanceReport,
    PassageSearchResult, PdfExtractionStatus, PdfRecord, PdfStatusRecord, PurgeTidbitInput,
    RestoreTidbitInput, SaveDraftInput, SearchExecutionMode, SearchField, SearchHighlight,
    SearchPassagesInput, SearchPassagesResponse, SemanticSearchReadiness, SetShortcutSettingsInput,
    ShortcutSettings, SourceDraft, Tidbit, TidbitDraft, TidbitListCursor, TidbitListItem,
    TidbitListPage, TidbitListScope, TidbitRevision, TidbitRevisionAttachment, TidbitRevisionPage,
    TidbitRevisionSummary, TidbitSource, TIDBIT_PURGE_DELAY_MS,
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

mod database;
mod embedding;
mod embedding_runtime;
mod media;
mod passage_embedding_indexer;
mod pdf;
pub mod relevance;
mod runtime;

#[cfg(feature = "test-support")]
pub mod test_support;

use std::path::PathBuf;

use runtime::RuntimeState;
use tauri::{Builder, Manager, Runtime};

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

fn with_commands<R: Runtime>(builder: Builder<R>) -> Builder<R> {
    builder.invoke_handler(tauri::generate_handler![
        runtime::runtime_probe,
        runtime::semantic_runtime_status,
        runtime::passage_embedding_index_status,
        runtime::prepare_semantic_runtime,
        runtime::retry_semantic_runtime,
        runtime::repair_semantic_runtime,
        runtime::semantic_runtime_logs,
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
        pdf::select_pdf,
        pdf::ingest_selected_pdf,
        pdf::pdf_status,
        pdf::retry_pdf_extraction,
        pdf::open_pdf_external,
        database::commands::create_tidbit,
        database::commands::load_tidbit,
        database::commands::list_tidbits,
        database::commands::edit_tidbit,
        database::commands::delete_tidbit,
        database::commands::restore_tidbit,
        database::commands::resolve_citation,
        database::commands::search_passages,
        database::commands::save_draft,
        database::commands::load_draft,
        database::commands::clear_draft,
    ])
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    with_commands(
        tauri::Builder::default()
            .register_uri_scheme_protocol("kosh-media", |context, request| {
                media::protocol_response(context.app_handle(), request)
            })
            .on_window_event(|window, event| {
                media::handle_image_drop(window, event);
                pdf::handle_pdf_drop(window, event);
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
        app.manage(RuntimeState::production(data_dir, resource_dir)?);
        Ok(())
    })
    .run(tauri::generate_context!())
    .expect("error while running Kosh");
}

pub use database::{
    AttachmentIngestInput, AttachmentKind, AttachmentRecord, CitationAttachment, CitationLocator,
    CitationResolution, CitationState, CitationTidbit, ClearDraftInput, Database,
    DatabaseDiagnostics, DatabaseError, DatabasePaths, DeleteTidbitInput, Draft, EditTidbitInput,
    ImageOcrDiagnostics, ImageOcrRecovery, ImageOcrStatus, ImageRecord, ImageStatusRecord,
    LexicalSearchMode, ListTidbitsInput, MediaCleanupResult, MediaIntegrityReport, MediaLimits,
    MediaMaintenanceReport, PassageSearchResult, PdfExtractionStatus, PdfRecord, PdfStatusRecord,
    RestoreTidbitInput, SaveDraftInput, SearchExecutionMode, SearchField, SearchHighlight,
    SearchPassagesInput, SearchPassagesResponse, SemanticSearchReadiness, SourceDraft, Tidbit,
    TidbitDraft, TidbitListCursor, TidbitListItem, TidbitListPage, TidbitSource,
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

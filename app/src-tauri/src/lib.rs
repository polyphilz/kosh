mod database;
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
        database::commands::create_tidbit,
        database::commands::load_tidbit,
        database::commands::list_tidbits,
        database::commands::edit_tidbit,
        database::commands::delete_tidbit,
        database::commands::save_draft,
        database::commands::load_draft,
        database::commands::clear_draft,
    ])
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    with_commands(tauri::Builder::default())
        .setup(|app| {
            let data_dir = select_data_dir(
                app.path().app_data_dir()?,
                std::env::var_os(DATA_DIR_ENV).map(PathBuf::from),
                cfg!(debug_assertions),
            );
            std::fs::create_dir_all(&data_dir)?;
            app.manage(RuntimeState::production(data_dir)?);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Kosh");
}

pub use database::{
    ClearDraftInput, Database, DatabaseDiagnostics, DatabaseError, DatabasePaths,
    DeleteTidbitInput, Draft, EditTidbitInput, ListTidbitsInput, SaveDraftInput, SourceDraft,
    Tidbit, TidbitDraft, TidbitListCursor, TidbitListItem, TidbitListPage, TidbitSource,
};
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

use serde::Serialize;
use tauri::State;

use crate::runtime::RuntimeState;

use super::{
    drafts::SaveDraftWrite,
    tidbits::{CreateTidbitWrite, EditTidbitWrite},
    ClearDraftInput, DatabaseError, DeleteTidbitInput, Draft, EditTidbitInput, ListTidbitsInput,
    SaveDraftInput, Tidbit, TidbitDraft, TidbitListPage,
};

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CommandErrorCode {
    InvalidInput,
    NotFound,
    StaleTidbit,
    TidbitDeleted,
    DatabaseUnavailable,
    DatabaseError,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommandError {
    code: CommandErrorCode,
    message: String,
}

impl From<DatabaseError> for CommandError {
    fn from(error: DatabaseError) -> Self {
        let code = match &error {
            DatabaseError::InvalidInput(_) => CommandErrorCode::InvalidInput,
            DatabaseError::NotFound { .. } => CommandErrorCode::NotFound,
            DatabaseError::StaleTidbit { .. } => CommandErrorCode::StaleTidbit,
            DatabaseError::TidbitDeleted { .. } => CommandErrorCode::TidbitDeleted,
            DatabaseError::WriterUnavailable | DatabaseError::WriterPanicked => {
                CommandErrorCode::DatabaseUnavailable
            }
            _ => CommandErrorCode::DatabaseError,
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}

type CommandResult<T> = std::result::Result<T, CommandError>;

#[tauri::command]
pub(crate) async fn create_tidbit(
    state: State<'_, RuntimeState>,
    input: TidbitDraft,
) -> CommandResult<Tidbit> {
    let client = state.database_client();
    let now_ms = state.now_ms();
    let mut ids = state.next_ids(input.sources.len() + 2).into_iter();
    let write = CreateTidbitWrite {
        input,
        now_ms,
        tidbit_id: ids.next().expect("requested tidbit ID"),
        revision_id: ids.next().expect("requested revision ID"),
        source_ids: ids.collect(),
    };
    run_writer(move || client.create_tidbit(write)).await
}

#[tauri::command]
pub(crate) async fn load_tidbit(
    state: State<'_, RuntimeState>,
    id: String,
) -> CommandResult<Tidbit> {
    let client = state.database_client();
    run_writer(move || client.load_tidbit(id)).await
}

#[tauri::command]
pub(crate) async fn list_tidbits(
    state: State<'_, RuntimeState>,
    input: ListTidbitsInput,
) -> CommandResult<TidbitListPage> {
    let client = state.database_client();
    run_writer(move || client.list_tidbits(input)).await
}

#[tauri::command]
pub(crate) async fn edit_tidbit(
    state: State<'_, RuntimeState>,
    input: EditTidbitInput,
) -> CommandResult<Tidbit> {
    let client = state.database_client();
    let now_ms = state.now_ms();
    let mut ids = state.next_ids(input.sources.len() + 1).into_iter();
    let write = EditTidbitWrite {
        input,
        now_ms,
        revision_id: ids.next().expect("requested revision ID"),
        source_ids: ids.collect(),
    };
    run_writer(move || client.edit_tidbit(write)).await
}

#[tauri::command]
pub(crate) async fn delete_tidbit(
    state: State<'_, RuntimeState>,
    input: DeleteTidbitInput,
) -> CommandResult<Tidbit> {
    let client = state.database_client();
    let now_ms = state.now_ms();
    run_writer(move || client.delete_tidbit(input, now_ms)).await
}

#[tauri::command]
pub(crate) async fn save_draft(
    state: State<'_, RuntimeState>,
    input: SaveDraftInput,
) -> CommandResult<Draft> {
    let client = state.database_client();
    let write = SaveDraftWrite {
        input,
        now_ms: state.now_ms(),
        draft_id: state
            .next_ids(1)
            .into_iter()
            .next()
            .expect("requested draft ID"),
    };
    run_writer(move || client.save_draft(write)).await
}

#[tauri::command]
pub(crate) async fn load_draft(
    state: State<'_, RuntimeState>,
    context_key: String,
) -> CommandResult<Option<Draft>> {
    let client = state.database_client();
    run_writer(move || client.load_draft(context_key)).await
}

#[tauri::command]
pub(crate) async fn clear_draft(
    state: State<'_, RuntimeState>,
    input: ClearDraftInput,
) -> CommandResult<bool> {
    let client = state.database_client();
    run_writer(move || client.clear_draft(input)).await
}

async fn run_writer<T, F>(operation: F) -> CommandResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> super::Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|error| CommandError {
            code: CommandErrorCode::DatabaseUnavailable,
            message: format!("database command worker failed: {error}"),
        })?
        .map_err(Into::into)
}

use serde::Serialize;
use tauri::State;

use crate::embedding_runtime::SemanticRuntimePhase;
use crate::runtime::RuntimeState;

use super::{
    working_copies::{CheckpointWorkingCopyWrite, SaveWorkingCopyWrite},
    CheckpointWorkingCopyInput, CitationResolution, DatabaseError, DeleteTidbitInput,
    DiscardWorkingCopyInput, RestoreTidbitInput, SaveWorkingCopyInput, SearchPassagesInput,
    SearchPassagesResponse, SemanticSearchReadiness, Tidbit, WorkingCopy,
    WorkingCopyCheckpointResult, WorkingCopySaveResult,
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

impl CommandError {
    pub(crate) fn worker(message: String) -> Self {
        Self {
            code: CommandErrorCode::DatabaseUnavailable,
            message: format!("database command worker failed: {message}"),
        }
    }

    pub(crate) fn public_message(&self) -> &str {
        &self.message
    }
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
pub(crate) async fn load_tidbit(
    state: State<'_, RuntimeState>,
    id: String,
) -> CommandResult<Tidbit> {
    let client = state.database_client();
    run_writer(move || client.load_tidbit(id)).await
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
pub(crate) async fn restore_tidbit(
    state: State<'_, RuntimeState>,
    input: RestoreTidbitInput,
) -> CommandResult<Tidbit> {
    let client = state.database_client();
    let now_ms = state.now_ms();
    run_writer(move || client.restore_tidbit(input, now_ms)).await
}

#[tauri::command]
pub(crate) async fn resolve_citation(
    state: State<'_, RuntimeState>,
    passage_id: String,
) -> CommandResult<CitationResolution> {
    let client = state.database_client();
    run_writer(move || client.resolve_citation(passage_id)).await
}

#[tauri::command]
pub(crate) async fn search_passages(
    state: State<'_, RuntimeState>,
    input: SearchPassagesInput,
) -> CommandResult<SearchPassagesResponse> {
    let has_query = super::search::validate_search_input(&input)?;
    let client = state.database_client();
    let runtime = state.embedding_runtime();
    run_writer(move || {
        if input.mode == super::LexicalSearchMode::Exact {
            return client.search_passages_with_semantics(
                input,
                None,
                SemanticSearchReadiness::NotRequested,
            );
        }

        let index_readiness = client
            .passage_embedding_search_readiness()
            .unwrap_or(SemanticSearchReadiness::Failed);
        let runtime_phase = runtime.status().phase;
        let readiness = match index_readiness {
            SemanticSearchReadiness::Ready => readiness_for_runtime(runtime_phase),
            SemanticSearchReadiness::Indexing if runtime_phase == SemanticRuntimePhase::Ready => {
                SemanticSearchReadiness::Indexing
            }
            SemanticSearchReadiness::Indexing => readiness_for_runtime(runtime_phase),
            SemanticSearchReadiness::Failed => SemanticSearchReadiness::Failed,
            SemanticSearchReadiness::WaitingForRuntime | SemanticSearchReadiness::NotRequested => {
                SemanticSearchReadiness::Failed
            }
        };
        if !has_query || readiness != SemanticSearchReadiness::Ready {
            return client.search_passages_with_semantics(input, None, readiness);
        }

        match runtime.embed_query(&input.query) {
            Ok(query_embedding) => client.search_passages_with_semantics(
                input,
                Some(query_embedding),
                SemanticSearchReadiness::Ready,
            ),
            Err(error) => {
                log::warn!(
                    "semantic query embedding failed; using lexical search: {}",
                    error.public_message()
                );
                client.search_passages_with_semantics(
                    input,
                    None,
                    readiness_for_runtime(runtime.status().phase),
                )
            }
        }
    })
    .await
}

fn readiness_for_runtime(phase: SemanticRuntimePhase) -> SemanticSearchReadiness {
    match phase {
        SemanticRuntimePhase::Ready => SemanticSearchReadiness::Ready,
        SemanticRuntimePhase::Failed => SemanticSearchReadiness::Failed,
        SemanticRuntimePhase::NotDownloaded
        | SemanticRuntimePhase::VerificationRequired
        | SemanticRuntimePhase::Downloading
        | SemanticRuntimePhase::Verifying
        | SemanticRuntimePhase::Starting
        | SemanticRuntimePhase::Unavailable => SemanticSearchReadiness::WaitingForRuntime,
    }
}

#[tauri::command]
pub(crate) async fn save_working_copy(
    state: State<'_, RuntimeState>,
    input: SaveWorkingCopyInput,
) -> CommandResult<WorkingCopySaveResult> {
    let client = state.database_client();
    let write = SaveWorkingCopyWrite {
        input,
        now_ms: state.now_ms(),
        media_limits: state.media_limits(),
        allow_empty_ephemeral: false,
    };
    run_writer(move || client.save_working_copy(write)).await
}

#[tauri::command]
pub(crate) async fn reserve_working_copy_for_media(
    state: State<'_, RuntimeState>,
    input: SaveWorkingCopyInput,
) -> CommandResult<WorkingCopySaveResult> {
    let client = state.database_client();
    let write = SaveWorkingCopyWrite {
        input,
        now_ms: state.now_ms(),
        media_limits: state.media_limits(),
        allow_empty_ephemeral: true,
    };
    run_writer(move || client.save_working_copy(write)).await
}

#[tauri::command]
pub(crate) async fn load_working_copy(
    state: State<'_, RuntimeState>,
    note_id: String,
) -> CommandResult<Option<WorkingCopy>> {
    let client = state.database_client();
    run_writer(move || client.load_working_copy(note_id)).await
}

#[tauri::command]
pub(crate) async fn list_working_copies(
    state: State<'_, RuntimeState>,
) -> CommandResult<Vec<WorkingCopy>> {
    let client = state.database_client();
    run_writer(move || client.list_working_copies()).await
}

#[tauri::command]
pub(crate) async fn checkpoint_working_copy(
    state: State<'_, RuntimeState>,
    input: CheckpointWorkingCopyInput,
) -> CommandResult<WorkingCopyCheckpointResult> {
    let client = state.database_client();
    let note_id = input.note_id.clone();
    let working_copy = run_writer({
        let client = client.clone();
        move || client.load_working_copy(note_id)
    })
    .await?
    .ok_or_else(|| {
        CommandError::from(DatabaseError::NotFound {
            entity: "working copy",
            id: input.note_id.clone(),
        })
    })?;
    let mut ids = state.next_ids(working_copy.sources.len() + 1).into_iter();
    let write = CheckpointWorkingCopyWrite {
        input,
        now_ms: state.now_ms(),
        revision_id: ids.next().expect("requested working-copy revision ID"),
        source_ids: ids.collect(),
    };
    run_writer(move || client.checkpoint_working_copy(write)).await
}

#[tauri::command]
pub(crate) async fn discard_working_copy(
    state: State<'_, RuntimeState>,
    input: DiscardWorkingCopyInput,
) -> CommandResult<bool> {
    let client = state.database_client();
    let now_ms = state.now_ms();
    run_writer(move || client.discard_working_copy(input, now_ms)).await
}

async fn run_writer<T, F>(operation: F) -> CommandResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> super::Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|error| CommandError::worker(error.to_string()))?
        .map_err(Into::into)
}

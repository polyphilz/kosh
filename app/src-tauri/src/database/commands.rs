use serde::Serialize;
use tauri::State;

use crate::embedding_runtime::SemanticRuntimePhase;
use crate::runtime::RuntimeState;

use super::{
    drafts::SaveDraftWrite,
    research_runs::SaveResearchAnswerWrite,
    tidbits::{CreateTidbitWrite, EditTidbitWrite},
    working_copies::{CheckpointWorkingCopyWrite, SaveWorkingCopyWrite},
    CheckpointWorkingCopyInput, CitationResolution, ClearDraftInput, DatabaseError,
    DeleteTidbitInput, DiscardWorkingCopyInput, Draft, EditTidbitInput, ListResearchRunsInput,
    ListTidbitRevisionsInput, ListTidbitsInput, PurgeTidbitInput, ResearchRunPage,
    ResearchRunRecord, RestoreTidbitInput, SaveDraftInput, SaveWorkingCopyInput,
    SearchPassagesInput, SearchPassagesResponse, SemanticSearchReadiness, Tidbit, TidbitDraft,
    TidbitListPage, TidbitRevision, TidbitRevisionPage, WorkingCopy, WorkingCopyCheckpointResult,
    WorkingCopySaveResult,
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
pub(crate) async fn list_tidbit_revisions(
    state: State<'_, RuntimeState>,
    input: ListTidbitRevisionsInput,
) -> CommandResult<TidbitRevisionPage> {
    let client = state.database_client();
    run_writer(move || client.list_tidbit_revisions(input)).await
}

#[tauri::command]
pub(crate) async fn load_tidbit_revision(
    state: State<'_, RuntimeState>,
    tidbit_id: String,
    revision_id: String,
) -> CommandResult<TidbitRevision> {
    let client = state.database_client();
    run_writer(move || client.load_tidbit_revision(tidbit_id, revision_id)).await
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
pub(crate) async fn restore_tidbit(
    state: State<'_, RuntimeState>,
    input: RestoreTidbitInput,
) -> CommandResult<Tidbit> {
    let client = state.database_client();
    let now_ms = state.now_ms();
    run_writer(move || client.restore_tidbit(input, now_ms)).await
}

#[tauri::command]
pub(crate) async fn purge_tidbit(
    state: State<'_, RuntimeState>,
    input: PurgeTidbitInput,
) -> CommandResult<bool> {
    let client = state.database_client();
    let now_ms = state.now_ms();
    run_writer(move || client.purge_tidbit(input, now_ms)).await
}

#[tauri::command]
pub(crate) async fn list_research_runs(
    state: State<'_, RuntimeState>,
    input: ListResearchRunsInput,
) -> CommandResult<ResearchRunPage> {
    let client = state.database_client();
    run_writer(move || client.list_research_runs(input)).await
}

#[tauri::command]
pub(crate) async fn load_research_run(
    state: State<'_, RuntimeState>,
    id: String,
) -> CommandResult<ResearchRunRecord> {
    let client = state.database_client();
    run_writer(move || client.load_research_run(id)).await
}

#[tauri::command]
pub(crate) async fn save_research_answer_as_tidbit(
    state: State<'_, RuntimeState>,
    run_id: String,
) -> CommandResult<Tidbit> {
    let client = state.database_client();
    let now_ms = state.now_ms();
    let mut ids = state.next_ids(2).into_iter();
    let write = SaveResearchAnswerWrite {
        run_id,
        tidbit_id: ids.next().expect("requested tidbit ID"),
        revision_id: ids.next().expect("requested revision ID"),
        now_ms,
    };
    run_writer(move || client.save_research_answer_as_tidbit(write)).await
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
        media_limits: state.media_limits(),
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
    let now_ms = state.now_ms();
    run_writer(move || client.clear_draft_at(input, now_ms)).await
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
        draft_id: state
            .next_ids(1)
            .into_iter()
            .next()
            .expect("requested working-copy draft ID"),
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
        draft_id: state
            .next_ids(1)
            .into_iter()
            .next()
            .expect("requested working-copy draft ID"),
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

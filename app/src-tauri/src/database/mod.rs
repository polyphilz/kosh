pub(crate) mod commands;
mod connection;
pub(crate) mod drafts;
pub(crate) mod embedding_index;
mod error;
pub(crate) mod media;
mod migrations;
pub(crate) mod passages;
mod paths;
pub(crate) mod search;
mod tidbits;
mod validation;
mod writer;

#[cfg(test)]
mod drafts_tests;
#[cfg(test)]
mod embedding_index_tests;
#[cfg(test)]
mod media_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tidbits_tests;

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::Read,
    sync::{
        mpsc::{self, Receiver},
        Mutex,
    },
    thread::{self, JoinHandle},
};

use rusqlite::Connection;

pub use drafts::{ClearDraftInput, Draft, SaveDraftInput};
pub use error::{DatabaseError, Result};
pub use media::{
    AttachmentIngestInput, AttachmentKind, AttachmentRecord, MediaCleanupResult,
    MediaIntegrityReport, MediaLimits, MediaMaintenanceReport,
};
pub use passages::{
    CitationAttachment, CitationLocator, CitationResolution, CitationState, CitationTidbit,
};
pub use paths::DatabasePaths;
pub use search::{
    LexicalSearchMode, PassageSearchResult, SearchExecutionMode, SearchField, SearchHighlight,
    SearchPassagesInput, SearchPassagesResponse, SemanticSearchReadiness,
};
pub use tidbits::{
    DeleteTidbitInput, EditTidbitInput, ListTidbitsInput, RestoreTidbitInput, SourceDraft, Tidbit,
    TidbitDraft, TidbitListCursor, TidbitListItem, TidbitListPage, TidbitSource,
};
pub(crate) use writer::LexicalBenchmarkAttachmentWrite;
use writer::WriterMessage;
pub use writer::{DatabaseClient, DatabaseDiagnostics};

use connection::{DatabaseKind, FileState};

static ATTACHMENT_INGEST_GATE: Mutex<()> = Mutex::new(());

#[derive(Debug)]
struct DatabaseOwnership {
    writer_thread: Option<JoinHandle<()>>,
    library_lock: Option<File>,
}

#[derive(Debug)]
pub struct Database {
    paths: DatabasePaths,
    client: DatabaseClient,
    ownership: Mutex<DatabaseOwnership>,
}

impl Database {
    pub fn initialize(paths: DatabasePaths) -> Result<Self> {
        fs::create_dir_all(paths.root())?;
        let ownership_lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&paths.ownership_lock)?;
        match ownership_lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(DatabaseError::DatabaseInUse {
                    path: paths.root.clone(),
                });
            }
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }

        let main_state = connection::inspect_file(&paths.main)?;
        let media_state = connection::inspect_file(&paths.media)?;
        if main_state != media_state {
            let can_resume_pristine_pair = match (main_state, media_state) {
                (FileState::Existing, FileState::Fresh) => {
                    connection::is_pristine_identified(&paths.main, DatabaseKind::Main)?
                }
                (FileState::Fresh, FileState::Existing) => {
                    connection::is_pristine_identified(&paths.media, DatabaseKind::Media)?
                }
                _ => false,
            };
            if !can_resume_pristine_pair {
                return Err(DatabaseError::IncompletePair {
                    main_state: main_state.label(),
                    media_state: media_state.label(),
                });
            }
        }

        let mut main = connection::open_writer(&paths.main, DatabaseKind::Main, main_state)?;
        let mut media = connection::open_writer(&paths.media, DatabaseKind::Media, media_state)?;
        let main_status = migrations::inspect_main(&mut main)?;
        let media_status = migrations::inspect_media(&mut media)?;
        // Grouped migrations are transactional within each file. Cross-file work is
        // ordered media-first so a crash can leave only an orphaned media capability,
        // never authored metadata pointing at bytes that were not committed.
        if media_status.pending {
            migrations::run_media(&mut media)?;
        }
        if main_status.pending {
            migrations::run_main(&mut main)?;
        }
        if let Err(error) = embedding_index::ensure_vector_table(&main) {
            log::warn!("could not materialize the optional semantic vector table: {error}");
        }
        // Reap capabilities never persist across launches. The single writer
        // creates and consumes them in one transaction after a live main-
        // database reference check.
        media.execute("DELETE FROM media_blob_reap_authorization", [])?;
        validation::validate_migrated_pair(&mut main, &mut media, &paths.main, &paths.media)?;

        let (sender, receiver) = mpsc::channel();
        let client = DatabaseClient::new(sender.clone());
        let writer_thread = thread::Builder::new()
            .name("kosh-database-writer".into())
            .spawn(move || writer_loop(main, media, receiver, sender))?;

        let database = Self {
            paths,
            client,
            ownership: Mutex::new(DatabaseOwnership {
                writer_thread: Some(writer_thread),
                library_lock: Some(ownership_lock),
            }),
        };
        database.client.schedule_author_passage_reconciliation()?;
        Ok(database)
    }

    pub fn paths(&self) -> &DatabasePaths {
        &self.paths
    }

    pub fn client(&self) -> DatabaseClient {
        self.client.clone()
    }

    pub fn open_main_read_only(&self) -> Result<Connection> {
        connection::open_read_only(&self.paths.main, DatabaseKind::Main)
    }

    pub fn open_media_read_only(&self) -> Result<Connection> {
        connection::open_read_only(&self.paths.media, DatabaseKind::Media)
    }

    pub fn ingest_attachment(
        &self,
        input: AttachmentIngestInput,
        reader: impl Read,
    ) -> Result<AttachmentRecord> {
        let limits = input.limits.validate()?;
        let _ingest_guard = ATTACHMENT_INGEST_GATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let attachment_id = uuid::Uuid::now_v7().to_string();
        let ingest_lease_id = uuid::Uuid::now_v7().to_string();
        let stage_id = uuid::Uuid::now_v7().to_string();
        let staged = media::StagedAttachment::from_reader(
            reader,
            &self.paths.root.join("media-staging"),
            &stage_id,
            limits.max_attachment_bytes,
        )?;
        self.client
            .ingest_attachment(staged.write(media::IngestAttachmentMetadata {
                attachment_id,
                ingest_lease_id,
                draft_id: input.draft_id,
                display_filename: input.display_filename,
                media_type: input.media_type,
                now_ms: input.now_ms,
                limits,
            }))
    }

    pub fn shutdown(&self) -> Result<()> {
        let mut ownership = self
            .ownership
            .lock()
            .map_err(|_| DatabaseError::WriterPanicked)?;
        let outcome = if let Some(thread) = ownership.writer_thread.take() {
            let shutdown = self.client.shutdown();
            let joined = thread.join().map_err(|_| DatabaseError::WriterPanicked);
            joined.and(shutdown)
        } else {
            Ok(())
        };
        ownership.library_lock.take();
        outcome
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        let _ = self.client.shutdown();
        if let Ok(ownership) = self.ownership.get_mut() {
            if let Some(thread) = ownership.writer_thread.take() {
                let _ = thread.join();
            }
            ownership.library_lock.take();
        }
    }
}

fn writer_loop(
    mut main: Connection,
    mut media: Connection,
    receiver: Receiver<WriterMessage>,
    sender: mpsc::Sender<WriterMessage>,
) {
    while let Ok(message) = receiver.recv() {
        match message {
            WriterMessage::Diagnostics { reply } => {
                let _ = reply.send(writer::diagnostics(&mut main, &mut media));
            }
            WriterMessage::FullIntegrityCheck { reply } => {
                let _ = reply.send(validation::full_integrity_check_pair(&main, &media));
            }
            WriterMessage::ReconcileFts { reply } => {
                let _ = reply.send(validation::reconcile_fts(&mut main));
            }
            WriterMessage::ReconcileAuthorPassages { reply } => {
                let result = passages::reconcile_author_passages(&mut main);
                let _ = reply.send(result);
            }
            WriterMessage::ReconcileAuthorPassageBatch => {
                if passages::reconcile_author_passage_batch(
                    &mut main,
                    passages::BACKGROUND_RECONCILE_BATCH_SIZE,
                )
                .is_ok_and(|has_more| has_more)
                {
                    let _ = sender.send(WriterMessage::ReconcileAuthorPassageBatch);
                }
            }
            WriterMessage::LoadEmbeddingReconciliationBatch { limit, reply } => {
                let _ = reply.send(embedding_index::load_reconciliation_batch(&mut main, limit));
            }
            WriterMessage::InstallPassageEmbedding {
                pending,
                embedding,
                created_at_ms,
                reply,
            } => {
                let _ = reply.send(embedding_index::install_embedding(
                    &mut main,
                    &pending,
                    &embedding,
                    created_at_ms,
                ));
            }
            WriterMessage::PassageEmbeddingIndexProgress { reply } => {
                let _ = reply.send(embedding_index::progress(&main));
            }
            WriterMessage::PassageEmbeddingIndexNeedsReconciliation { reply } => {
                let _ = reply.send(embedding_index::needs_reconciliation(&main));
            }
            WriterMessage::PassageEmbeddingSearchReadiness { reply } => {
                let _ = reply.send(search::semantic_index_readiness(&main));
            }
            WriterMessage::ActivatePassageEmbeddingIndexIfComplete {
                activated_at_ms,
                reply,
            } => {
                let _ = reply.send(embedding_index::activate_if_complete(
                    &mut main,
                    activated_at_ms,
                ));
            }
            WriterMessage::RecordPassageEmbeddingIndexFailure {
                error,
                failed_at_ms,
                reply,
            } => {
                let _ = reply.send(embedding_index::record_retryable_failure(
                    &main,
                    &error,
                    failed_at_ms,
                ));
            }
            WriterMessage::IngestAttachment { write, reply } => {
                let _ = reply.send(media::ingest_attachment(&mut main, &mut media, write));
            }
            WriterMessage::LoadMediaPayload {
                attachment_id,
                now_ms,
                requested_range,
                max_response_bytes,
                reply,
            } => {
                let _ = reply.send(media::load_media_payload(
                    &main,
                    &media,
                    &attachment_id,
                    now_ms,
                    requested_range,
                    max_response_bytes,
                ));
            }
            WriterMessage::MediaIntegrityReport { now_ms, reply } => {
                let _ = reply.send(media::integrity_report(&main, &media, now_ms));
            }
            WriterMessage::MaintainMedia {
                now_ms,
                limits,
                reply,
            } => {
                let _ = reply.send(media::maintain_media(&mut main, &mut media, now_ms, limits));
            }
            WriterMessage::RecoverMediaLifecycleBatch {
                now_ms,
                limits,
                cursor,
            } => match media::recover_media_lifecycle_batch(
                &mut main, &mut media, now_ms, limits, cursor,
            ) {
                Ok(Some(cursor)) => {
                    let _ = sender.send(WriterMessage::RecoverMediaLifecycleBatch {
                        now_ms,
                        limits,
                        cursor: Some(cursor),
                    });
                }
                Ok(None) => {}
                Err(error) => {
                    log::warn!("background media lifecycle recovery could not complete: {error}");
                }
            },
            WriterMessage::CreateTidbit { write, reply } => {
                let _ = reply.send(tidbits::create_tidbit(&mut main, write));
            }
            WriterMessage::LoadTidbit { id, reply } => {
                let _ = reply.send(tidbits::load_tidbit(&main, &id));
            }
            WriterMessage::ListTidbits { input, reply } => {
                let _ = reply.send(tidbits::list_tidbits(&main, input));
            }
            WriterMessage::EditTidbit { write, reply } => {
                let _ = reply.send(tidbits::edit_tidbit(&mut main, write));
            }
            WriterMessage::DeleteTidbit {
                input,
                now_ms,
                reply,
            } => {
                let _ = reply.send(tidbits::delete_tidbit(&mut main, input, now_ms));
            }
            WriterMessage::RestoreTidbit {
                input,
                now_ms,
                reply,
            } => {
                let _ = reply.send(tidbits::restore_tidbit(&mut main, input, now_ms));
            }
            WriterMessage::ResolveCitation { passage_id, reply } => {
                let _ = reply.send(passages::resolve_citation(&main, &passage_id));
            }
            WriterMessage::SearchPassages {
                input,
                query_embedding,
                fallback_readiness,
                reply,
            } => {
                let _ = reply.send(search::search_passages_with_semantics(
                    &main,
                    input,
                    query_embedding.as_deref(),
                    fallback_readiness,
                ));
            }
            WriterMessage::RefreshAttachmentSearchDocuments {
                attachment_id,
                reply,
            } => {
                let _ = reply.send(search::replace_attachment_documents(
                    &mut main,
                    &attachment_id,
                ));
            }
            WriterMessage::InstallLexicalBenchmarkAttachments { writes, reply } => {
                let _ = reply.send(writer::install_lexical_benchmark_attachments(
                    &mut main, writes,
                ));
            }
            WriterMessage::SaveDraft { write, reply } => {
                let _ = reply.send(drafts::save_draft(&mut main, write));
            }
            WriterMessage::LoadDraft { context_key, reply } => {
                let _ = reply.send(drafts::load_draft(&main, &context_key));
            }
            WriterMessage::ClearDraft {
                input,
                now_ms,
                reply,
            } => {
                let _ = reply.send(drafts::clear_draft(&mut main, input, now_ms));
            }
            WriterMessage::Shutdown => break,
        }
    }
}

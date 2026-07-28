pub(crate) mod commands;
mod connection;
mod drafts;
mod error;
mod migrations;
mod passages;
mod paths;
mod tidbits;
mod validation;
mod writer;

#[cfg(test)]
mod drafts_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tidbits_tests;

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    sync::{
        mpsc::{self, Receiver},
        Mutex,
    },
    thread::{self, JoinHandle},
};

use rusqlite::Connection;

pub use drafts::{ClearDraftInput, Draft, SaveDraftInput};
pub use error::{DatabaseError, Result};
pub use passages::{
    CitationAttachment, CitationLocator, CitationResolution, CitationState, CitationTidbit,
};
pub use paths::DatabasePaths;
pub use tidbits::{
    DeleteTidbitInput, EditTidbitInput, ListTidbitsInput, RestoreTidbitInput, SourceDraft, Tidbit,
    TidbitDraft, TidbitListCursor, TidbitListItem, TidbitListPage, TidbitSource,
};
use writer::WriterMessage;
pub use writer::{DatabaseClient, DatabaseDiagnostics};

use connection::{DatabaseKind, FileState};

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
        // Reap capabilities never persist across launches. The single writer
        // creates and consumes them in one transaction after a live main-
        // database reference check.
        media.execute("DELETE FROM media_blob_reap_authorization", [])?;
        validation::validate_migrated_pair(&mut main, &mut media, &paths.main, &paths.media)?;

        let (sender, receiver) = mpsc::channel();
        let client = DatabaseClient::new(sender);
        let writer_thread = thread::Builder::new()
            .name("kosh-database-writer".into())
            .spawn(move || writer_loop(main, media, receiver))?;

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

fn writer_loop(mut main: Connection, mut media: Connection, receiver: Receiver<WriterMessage>) {
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
                if let Some(reply) = reply {
                    let _ = reply.send(result);
                }
            }
            WriterMessage::ReapMediaBlob {
                sha256,
                now,
                reason,
                reply,
            } => {
                let _ = reply.send(writer::reap_media_blob(
                    &mut main, &mut media, sha256, now, reason,
                ));
            }
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
            WriterMessage::SaveDraft { write, reply } => {
                let _ = reply.send(drafts::save_draft(&mut main, write));
            }
            WriterMessage::LoadDraft { context_key, reply } => {
                let _ = reply.send(drafts::load_draft(&main, &context_key));
            }
            WriterMessage::ClearDraft { input, reply } => {
                let _ = reply.send(drafts::clear_draft(&mut main, input));
            }
            WriterMessage::Shutdown => break,
        }
    }
}

use std::sync::mpsc::{self, Sender, SyncSender};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};

use super::{
    drafts::{ClearDraftInput, Draft, SaveDraftWrite},
    embedding_index::{
        InstallEmbeddingDisposition, PassageEmbeddingIndexProgress, PendingPassageEmbedding,
    },
    error::{DatabaseError, Result},
    migrations::MigrationHeads,
    passages::CitationResolution,
    search::{PassageSearchResult, SearchPassagesInput},
    tidbits::{
        CreateTidbitWrite, DeleteTidbitInput, EditTidbitWrite, ListTidbitsInput,
        RestoreTidbitInput, Tidbit, TidbitListPage,
    },
};

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseDiagnostics {
    pub migration_heads: MigrationHeads,
    pub main_journal_mode: String,
    pub media_journal_mode: String,
    pub main_foreign_keys: bool,
    pub media_foreign_keys: bool,
}

pub(crate) struct LexicalBenchmarkAttachmentWrite {
    pub revision_id: String,
    pub attachment_id: String,
    pub created_at_ms: i64,
    pub display_filename: String,
    pub media_type: String,
    pub byte_length: i64,
}

pub(super) enum WriterMessage {
    Diagnostics {
        reply: SyncSender<Result<DatabaseDiagnostics>>,
    },
    FullIntegrityCheck {
        reply: SyncSender<Result<()>>,
    },
    ReconcileFts {
        reply: SyncSender<Result<bool>>,
    },
    ReconcileAuthorPassages {
        reply: SyncSender<Result<()>>,
    },
    ReconcileAuthorPassageBatch,
    LoadEmbeddingReconciliationBatch {
        limit: u32,
        reply: SyncSender<Result<Vec<PendingPassageEmbedding>>>,
    },
    InstallPassageEmbedding {
        pending: PendingPassageEmbedding,
        embedding: Vec<f32>,
        created_at_ms: i64,
        reply: SyncSender<Result<InstallEmbeddingDisposition>>,
    },
    PassageEmbeddingIndexProgress {
        reply: SyncSender<Result<PassageEmbeddingIndexProgress>>,
    },
    ActivatePassageEmbeddingIndexIfComplete {
        activated_at_ms: i64,
        reply: SyncSender<Result<bool>>,
    },
    RecordPassageEmbeddingIndexFailure {
        error: String,
        failed_at_ms: i64,
        reply: SyncSender<Result<()>>,
    },
    ReapMediaBlob {
        sha256: Vec<u8>,
        now: i64,
        reason: String,
        reply: SyncSender<Result<bool>>,
    },
    CreateTidbit {
        write: CreateTidbitWrite,
        reply: SyncSender<Result<Tidbit>>,
    },
    LoadTidbit {
        id: String,
        reply: SyncSender<Result<Tidbit>>,
    },
    ListTidbits {
        input: ListTidbitsInput,
        reply: SyncSender<Result<TidbitListPage>>,
    },
    EditTidbit {
        write: EditTidbitWrite,
        reply: SyncSender<Result<Tidbit>>,
    },
    DeleteTidbit {
        input: DeleteTidbitInput,
        now_ms: i64,
        reply: SyncSender<Result<Tidbit>>,
    },
    RestoreTidbit {
        input: RestoreTidbitInput,
        now_ms: i64,
        reply: SyncSender<Result<Tidbit>>,
    },
    ResolveCitation {
        passage_id: String,
        reply: SyncSender<Result<CitationResolution>>,
    },
    SearchPassages {
        input: SearchPassagesInput,
        reply: SyncSender<Result<Vec<PassageSearchResult>>>,
    },
    RefreshAttachmentSearchDocuments {
        attachment_id: String,
        reply: SyncSender<Result<()>>,
    },
    InstallLexicalBenchmarkAttachments {
        writes: Vec<LexicalBenchmarkAttachmentWrite>,
        reply: SyncSender<Result<()>>,
    },
    SaveDraft {
        write: SaveDraftWrite,
        reply: SyncSender<Result<Draft>>,
    },
    LoadDraft {
        context_key: String,
        reply: SyncSender<Result<Option<Draft>>>,
    },
    ClearDraft {
        input: ClearDraftInput,
        reply: SyncSender<Result<bool>>,
    },
    Shutdown,
}

#[derive(Clone, Debug)]
pub struct DatabaseClient {
    sender: Sender<WriterMessage>,
}

impl DatabaseClient {
    pub(super) fn new(sender: Sender<WriterMessage>) -> Self {
        Self { sender }
    }

    pub fn diagnostics(&self) -> Result<DatabaseDiagnostics> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::Diagnostics { reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub fn reap_media_blob(&self, sha256: Vec<u8>, now: i64, reason: String) -> Result<bool> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::ReapMediaBlob {
                sha256,
                now,
                reason,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub fn full_integrity_check(&self) -> Result<()> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::FullIntegrityCheck { reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub fn reconcile_fts(&self) -> Result<bool> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::ReconcileFts { reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub fn reconcile_author_passages(&self) -> Result<()> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::ReconcileAuthorPassages { reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub fn refresh_attachment_search_documents(&self, attachment_id: String) -> Result<()> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::RefreshAttachmentSearchDocuments {
                attachment_id,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(super) fn schedule_author_passage_reconciliation(&self) -> Result<()> {
        self.sender
            .send(WriterMessage::ReconcileAuthorPassageBatch)
            .map_err(|_| DatabaseError::WriterUnavailable)
    }

    pub(crate) fn load_embedding_reconciliation_batch(
        &self,
        limit: u32,
    ) -> Result<Vec<PendingPassageEmbedding>> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::LoadEmbeddingReconciliationBatch { limit, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn install_passage_embedding(
        &self,
        pending: PendingPassageEmbedding,
        embedding: Vec<f32>,
        created_at_ms: i64,
    ) -> Result<InstallEmbeddingDisposition> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::InstallPassageEmbedding {
                pending,
                embedding,
                created_at_ms,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn passage_embedding_index_progress(&self) -> Result<PassageEmbeddingIndexProgress> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::PassageEmbeddingIndexProgress { reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn activate_passage_embedding_index_if_complete(
        &self,
        activated_at_ms: i64,
    ) -> Result<bool> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::ActivatePassageEmbeddingIndexIfComplete {
                activated_at_ms,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn record_passage_embedding_index_failure(
        &self,
        error: String,
        failed_at_ms: i64,
    ) -> Result<()> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::RecordPassageEmbeddingIndexFailure {
                error,
                failed_at_ms,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn create_tidbit(&self, write: CreateTidbitWrite) -> Result<Tidbit> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::CreateTidbit { write, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn create_tidbit_with_ids(
        &self,
        input: super::TidbitDraft,
        now_ms: i64,
        tidbit_id: String,
        revision_id: String,
        source_ids: Vec<String>,
    ) -> Result<Tidbit> {
        self.create_tidbit(CreateTidbitWrite {
            input,
            now_ms,
            tidbit_id,
            revision_id,
            source_ids,
        })
    }

    pub(crate) fn load_tidbit(&self, id: String) -> Result<Tidbit> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::LoadTidbit { id, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn list_tidbits(&self, input: ListTidbitsInput) -> Result<TidbitListPage> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::ListTidbits { input, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn edit_tidbit(&self, write: EditTidbitWrite) -> Result<Tidbit> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::EditTidbit { write, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn delete_tidbit(&self, input: DeleteTidbitInput, now_ms: i64) -> Result<Tidbit> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::DeleteTidbit {
                input,
                now_ms,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn restore_tidbit(&self, input: RestoreTidbitInput, now_ms: i64) -> Result<Tidbit> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::RestoreTidbit {
                input,
                now_ms,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn resolve_citation(&self, passage_id: String) -> Result<CitationResolution> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::ResolveCitation { passage_id, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn search_passages(
        &self,
        input: SearchPassagesInput,
    ) -> Result<Vec<PassageSearchResult>> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::SearchPassages { input, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn install_lexical_benchmark_attachments(
        &self,
        writes: Vec<LexicalBenchmarkAttachmentWrite>,
    ) -> Result<()> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::InstallLexicalBenchmarkAttachments { writes, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn save_draft(&self, write: SaveDraftWrite) -> Result<Draft> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::SaveDraft { write, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn load_draft(&self, context_key: String) -> Result<Option<Draft>> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::LoadDraft { context_key, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn clear_draft(&self, input: ClearDraftInput) -> Result<bool> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::ClearDraft { input, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(super) fn shutdown(&self) -> Result<()> {
        self.sender
            .send(WriterMessage::Shutdown)
            .map_err(|_| DatabaseError::WriterUnavailable)
    }
}

pub(super) fn diagnostics(
    main: &mut Connection,
    media: &mut Connection,
) -> Result<DatabaseDiagnostics> {
    let migration_heads = super::migrations::current_heads(main, media)?;
    let main_journal_mode =
        main.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?;
    let media_journal_mode =
        media.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?;
    let main_foreign_keys =
        main.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))? == 1;
    let media_foreign_keys =
        media.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))? == 1;

    Ok(DatabaseDiagnostics {
        migration_heads,
        main_journal_mode,
        media_journal_mode,
        main_foreign_keys,
        media_foreign_keys,
    })
}

pub(super) fn reap_media_blob(
    main: &mut Connection,
    media: &mut Connection,
    sha256: Vec<u8>,
    now: i64,
    reason: String,
) -> Result<bool> {
    if sha256.len() != 32 {
        return Err(DatabaseError::InvalidInput(
            "media digest must contain 32 bytes".into(),
        ));
    }
    if now < 0 {
        return Err(DatabaseError::InvalidInput(
            "media reap timestamp must not be negative".into(),
        ));
    }
    if reason.trim().is_empty() {
        return Err(DatabaseError::InvalidInput(
            "media reap reason must not be empty".into(),
        ));
    }

    // The library ownership lock excludes other Kosh writers. The main write
    // transaction additionally keeps the reference check serialized through
    // the media deletion if a non-cooperating SQLite writer appears.
    let main_transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let references: i64 = main_transaction.query_row(
        "SELECT count(*) FROM attachment WHERE sha256 = ?1",
        params![&sha256],
        |row| row.get(0),
    )?;
    if references > 0 {
        main_transaction.rollback()?;
        return Err(DatabaseError::MediaInUse { references });
    }

    let transaction = media.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let exists = transaction
        .query_row(
            "SELECT 1 FROM media_blob WHERE sha256 = ?1",
            params![&sha256],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !exists {
        transaction.rollback()?;
        main_transaction.rollback()?;
        return Ok(false);
    }

    transaction.execute(
        "DELETE FROM media_blob_reap_authorization WHERE sha256 = ?1",
        params![&sha256],
    )?;
    transaction.execute(
        "INSERT INTO media_blob_reap_authorization(sha256, authorized_at, reason)
         VALUES(?1, ?2, ?3)",
        params![&sha256, now, reason],
    )?;
    let deleted =
        transaction.execute("DELETE FROM media_blob WHERE sha256 = ?1", params![&sha256])?;
    transaction.commit()?;
    main_transaction.commit()?;
    Ok(deleted == 1)
}

pub(super) fn install_lexical_benchmark_attachments(
    main: &mut Connection,
    writes: Vec<LexicalBenchmarkAttachmentWrite>,
) -> Result<()> {
    let transaction = main.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for write in writes {
        let kind = if write.media_type.starts_with("image/") {
            "IMAGE"
        } else if write.media_type == "application/pdf" {
            "PDF"
        } else if write.media_type.starts_with("text/") {
            "TEXT"
        } else {
            "BINARY"
        };
        let extraction_state = if kind == "BINARY" {
            "NOT_APPLICABLE"
        } else {
            "PENDING"
        };
        let content_hash = Sha256::digest(write.attachment_id.as_bytes());
        transaction.execute(
            "INSERT OR IGNORE INTO attachment(
                id, created_at, updated_at, sha256, display_filename,
                media_type, byte_length, kind, extraction_state
             ) VALUES(?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                write.attachment_id,
                write.created_at_ms,
                content_hash.as_slice(),
                write.display_filename,
                write.media_type,
                write.byte_length,
                kind,
                extraction_state,
            ],
        )?;
        transaction.execute(
            "INSERT INTO tidbit_revision_attachment(
                tidbit_revision_id, attachment_id, sort_order, display_role
             ) VALUES(?1, ?2, 0, 'ATTACHMENT')",
            params![write.revision_id, write.attachment_id],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

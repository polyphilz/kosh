use std::sync::mpsc::{self, Sender, SyncSender};

use rusqlite::{params, Connection, TransactionBehavior};
use sha2::{Digest, Sha256};

use super::{
    drafts::{ClearDraftInput, Draft, SaveDraftWrite},
    embedding_index::{
        InstallEmbeddingDisposition, PassageEmbeddingIndexProgress, PendingPassageEmbedding,
    },
    error::{DatabaseError, Result},
    media::{
        AttachmentRecord, ImageOcrDiagnostics, ImageOcrJob, ImageOcrRecovery, ImageOcrRegion,
        ImageRecord, ImageStatusRecord, IngestAttachmentWrite, IngestImageWrite, MediaByteRange,
        MediaIntegrityReport, MediaIntegrityScan, MediaLimits, MediaMaintenanceReport,
        MediaMaintenanceScan, MediaPayload,
    },
    migrations::MigrationHeads,
    passages::CitationResolution,
    search::{
        PassageSearchResult, SearchPassagesInput, SearchPassagesResponse, SemanticSearchReadiness,
    },
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
    PassageEmbeddingIndexNeedsReconciliation {
        reply: SyncSender<Result<bool>>,
    },
    PassageEmbeddingSearchReadiness {
        reply: SyncSender<Result<SemanticSearchReadiness>>,
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
    IngestAttachment {
        write: IngestAttachmentWrite,
        reply: SyncSender<Result<AttachmentRecord>>,
    },
    IngestImage {
        write: IngestImageWrite,
        reply: SyncSender<Result<ImageRecord>>,
    },
    LoadImageStatus {
        attachment_id: String,
        reply: SyncSender<Result<ImageStatusRecord>>,
    },
    ClaimNextImageOcr {
        now_ms: i64,
        reply: SyncSender<Result<Option<ImageOcrJob>>>,
    },
    CompleteImageOcr {
        job: ImageOcrJob,
        result: std::result::Result<Vec<ImageOcrRegion>, String>,
        completed_at_ms: i64,
        reply: SyncSender<Result<()>>,
    },
    RetryImageOcr {
        attachment_id: String,
        now_ms: i64,
        reply: SyncSender<Result<ImageStatusRecord>>,
    },
    RecoverInterruptedImageOcr {
        stale_started_at_or_before: i64,
        now_ms: i64,
        reply: SyncSender<Result<ImageOcrRecovery>>,
    },
    ImageOcrDiagnostics {
        reply: SyncSender<Result<ImageOcrDiagnostics>>,
    },
    LoadMediaPayload {
        attachment_id: String,
        now_ms: i64,
        requested_range: Option<MediaByteRange>,
        max_response_bytes: u64,
        reply: SyncSender<Result<MediaPayload>>,
    },
    MediaIntegrityReport {
        scan: MediaIntegrityScan,
        reply: SyncSender<Result<MediaIntegrityReport>>,
    },
    MaintainMedia {
        scan: MediaMaintenanceScan,
        reply: SyncSender<Result<MediaMaintenanceReport>>,
    },
    RecoverMediaLifecycleBatch {
        now_ms: i64,
        limits: MediaLimits,
        cursor: Option<Vec<u8>>,
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
        query_embedding: Option<Vec<f32>>,
        fallback_readiness: SemanticSearchReadiness,
        reply: SyncSender<Result<SearchPassagesResponse>>,
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
        now_ms: i64,
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

    pub(crate) fn ingest_attachment(
        &self,
        write: IngestAttachmentWrite,
    ) -> Result<AttachmentRecord> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::IngestAttachment { write, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn ingest_image(&self, write: IngestImageWrite) -> Result<ImageRecord> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::IngestImage { write, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn load_image_status(&self, attachment_id: String) -> Result<ImageStatusRecord> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::LoadImageStatus {
                attachment_id,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn claim_next_image_ocr(&self, now_ms: i64) -> Result<Option<ImageOcrJob>> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::ClaimNextImageOcr { now_ms, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn complete_image_ocr(
        &self,
        job: ImageOcrJob,
        result: std::result::Result<Vec<ImageOcrRegion>, String>,
        completed_at_ms: i64,
    ) -> Result<()> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::CompleteImageOcr {
                job,
                result,
                completed_at_ms,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn retry_image_ocr(
        &self,
        attachment_id: String,
        now_ms: i64,
    ) -> Result<ImageStatusRecord> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::RetryImageOcr {
                attachment_id,
                now_ms,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn recover_interrupted_image_ocr(
        &self,
        stale_started_at_or_before: i64,
        now_ms: i64,
    ) -> Result<ImageOcrRecovery> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::RecoverInterruptedImageOcr {
                stale_started_at_or_before,
                now_ms,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn image_ocr_diagnostics(&self) -> Result<ImageOcrDiagnostics> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::ImageOcrDiagnostics { reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn load_media_payload(
        &self,
        attachment_id: String,
        now_ms: i64,
        requested_range: Option<MediaByteRange>,
        max_response_bytes: u64,
    ) -> Result<MediaPayload> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::LoadMediaPayload {
                attachment_id,
                now_ms,
                requested_range,
                max_response_bytes,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub fn media_integrity_report(&self, now_ms: i64) -> Result<MediaIntegrityReport> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::MediaIntegrityReport {
                scan: MediaIntegrityScan::new(now_ms)?,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub fn maintain_media(
        &self,
        now_ms: i64,
        limits: MediaLimits,
    ) -> Result<MediaMaintenanceReport> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::MaintainMedia {
                scan: MediaMaintenanceScan::new(now_ms, limits)?,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn schedule_media_lifecycle_recovery(
        &self,
        now_ms: i64,
        limits: MediaLimits,
    ) -> Result<()> {
        self.sender
            .send(WriterMessage::RecoverMediaLifecycleBatch {
                now_ms,
                limits,
                cursor: None,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)
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

    pub(crate) fn passage_embedding_index_needs_reconciliation(&self) -> Result<bool> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::PassageEmbeddingIndexNeedsReconciliation { reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn passage_embedding_search_readiness(&self) -> Result<SemanticSearchReadiness> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::PassageEmbeddingSearchReadiness { reply })
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
        Ok(self
            .search_passages_with_semantics(
                input,
                None,
                SemanticSearchReadiness::WaitingForRuntime,
            )?
            .results)
    }

    pub(crate) fn search_passages_with_semantics(
        &self,
        input: SearchPassagesInput,
        query_embedding: Option<Vec<f32>>,
        fallback_readiness: SemanticSearchReadiness,
    ) -> Result<SearchPassagesResponse> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::SearchPassages {
                input,
                query_embedding,
                fallback_readiness,
                reply,
            })
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

    pub(crate) fn clear_draft_at(&self, input: ClearDraftInput, now_ms: i64) -> Result<bool> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::ClearDraft {
                input,
                now_ms,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    #[cfg(test)]
    pub(crate) fn clear_draft(&self, input: ClearDraftInput) -> Result<bool> {
        let now_ms = input.expected_updated_at_ms;
        self.clear_draft_at(input, now_ms)
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

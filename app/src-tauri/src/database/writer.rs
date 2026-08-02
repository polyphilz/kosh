#![cfg_attr(feature = "test-support", allow(dead_code))]

use std::sync::{
    mpsc::{self, Sender, SyncSender},
    Arc, Mutex,
};

use rusqlite::{params, Connection, TransactionBehavior};
use sha2::{Digest, Sha256};

use crate::backup::{
    domain::{BackupSetId, CheckpointErrorCode, CheckpointId},
    litestream::LitestreamTxid,
};

use super::{
    backup_media::{
        OffsiteMediaUploadClaim, OffsiteMediaUploadFailureCode, OffsiteMediaUploadProgress,
    },
    backup_state::{
        BeginOffsiteBackupConfigIntentInput, BeginOffsiteBackupTakeoverIntentInput,
        OffsiteBackupConfig, OffsiteBackupConfigIntent, OffsiteBackupTakeoverIntent,
        SaveOffsiteBackupConfigInput,
    },
    drafts::{ClearDraftInput, Draft, SaveDraftWrite},
    embedding_index::{
        InstallEmbeddingDisposition, PassageEmbeddingIndexProgress, PendingPassageEmbedding,
    },
    error::{DatabaseError, Result},
    maintenance::{ExtractionRetryReport, MaintenanceDatabaseSnapshot},
    media::{
        AttachmentRecord, GenericAttachmentRecord, GenericAttachmentStatusRecord,
        ImageOcrDiagnostics, ImageOcrJob, ImageOcrRecovery, ImageOcrRegion, ImageRecord,
        ImageStatusRecord, IngestAttachmentWrite, IngestGenericAttachmentWrite, IngestImageWrite,
        IngestPdfWrite, MediaIntegrityReport, MediaIntegrityScan, MediaLimits,
        MediaMaintenanceReport, MediaMaintenanceScan, MediaPayload, MediaRangeRequest,
        PdfExtractionJob, PdfPageExtraction, PdfRecord, PdfStatusRecord,
    },
    migrations::MigrationHeads,
    offsite_checkpoint::{
        CheckpointMediaReference, OffsiteCheckpointScheduleState, PrepareOffsiteCheckpointInput,
        PreparedOffsiteCheckpoint,
    },
    passages::CitationResolution,
    research_runs::{
        AppendResearchEventWrite, CreateResearchRunWrite, ListResearchRunsInput, ResearchRunPage,
        ResearchRunRecord, SaveResearchAnswerWrite,
    },
    safety_snapshot::SafetySnapshotReport,
    search::{
        PassageSearchResult, SearchPassagesInput, SearchPassagesResponse, SemanticSearchReadiness,
    },
    settings::{SetAutomaticUpdateChecksInput, SetShortcutSettingsInput, ShortcutSettings},
    tidbits::{
        CreateTidbitWrite, DeleteTidbitInput, EditTidbitWrite, ListTidbitRevisionsInput,
        ListTidbitsInput, PurgeTidbitInput, RestoreTidbitInput, Tidbit, TidbitListPage,
        TidbitRevision, TidbitRevisionPage,
    },
};

#[cfg(test)]
use super::safety_snapshot::SafetySnapshotReason;

#[cfg(test)]
type MediaMaintenanceReplyReceiver =
    mpsc::Receiver<Result<(Option<SafetySnapshotReport>, MediaMaintenanceReport)>>;

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
    LoadOffsiteBackupConfig {
        enabled_only: bool,
        reply: SyncSender<Result<Option<OffsiteBackupConfig>>>,
    },
    SaveOffsiteBackupConfig {
        input: SaveOffsiteBackupConfigInput,
        reply: SyncSender<Result<OffsiteBackupConfig>>,
    },
    BeginOffsiteBackupConfigIntent {
        input: BeginOffsiteBackupConfigIntentInput,
        reply: SyncSender<Result<()>>,
    },
    LoadOffsiteBackupConfigIntent {
        reply: SyncSender<Result<Option<OffsiteBackupConfigIntent>>>,
    },
    CommitOffsiteBackupConfigIntent {
        operation_id: String,
        reply: SyncSender<Result<OffsiteBackupConfig>>,
    },
    CompleteOffsiteBackupConfigIntent {
        operation_id: String,
        reply: SyncSender<Result<()>>,
    },
    AbortOffsiteBackupConfigIntent {
        operation_id: String,
        reply: SyncSender<Result<()>>,
    },
    BeginOffsiteBackupTakeoverIntent {
        input: BeginOffsiteBackupTakeoverIntentInput,
        reply: SyncSender<Result<()>>,
    },
    LoadOffsiteBackupTakeoverIntent {
        reply: SyncSender<Result<Option<OffsiteBackupTakeoverIntent>>>,
    },
    CommitOffsiteBackupTakeoverIntent {
        operation_id: String,
        reply: SyncSender<Result<OffsiteBackupConfig>>,
    },
    AbortOffsiteBackupTakeoverIntent {
        operation_id: String,
        reply: SyncSender<Result<()>>,
    },
    LoadOffsiteCredentialCleanup {
        reply: SyncSender<Result<Vec<BackupSetId>>>,
    },
    CompleteOffsiteCredentialCleanup {
        backup_set_id: BackupSetId,
        reply: SyncSender<Result<()>>,
    },
    ReconcileOffsiteMediaUploads {
        now_ms: i64,
        reply: SyncSender<Result<u64>>,
    },
    RecoverInterruptedOffsiteMediaUploads {
        now_ms: i64,
        reply: SyncSender<Result<u64>>,
    },
    ClaimNextOffsiteMediaUpload {
        now_ms: i64,
        lease_id: String,
        reply: SyncSender<Result<Option<OffsiteMediaUploadClaim>>>,
    },
    AuthorizeOffsiteMediaRemoteWrite {
        claim: OffsiteMediaUploadClaim,
        reply: SyncSender<Result<bool>>,
    },
    AuthorizeOffsiteCheckpointRemoteOperation {
        config: OffsiteBackupConfig,
        reply: SyncSender<Result<bool>>,
    },
    CompleteOffsiteMediaUpload {
        claim: OffsiteMediaUploadClaim,
        remote_version: String,
        now_ms: i64,
        reply: SyncSender<Result<bool>>,
    },
    FailOffsiteMediaUpload {
        claim: OffsiteMediaUploadClaim,
        code: OffsiteMediaUploadFailureCode,
        retry_at_ms: Option<i64>,
        now_ms: i64,
        reply: SyncSender<Result<bool>>,
    },
    OffsiteMediaUploadProgress {
        reply: SyncSender<Result<OffsiteMediaUploadProgress>>,
    },
    RetryFailedOffsiteMediaUploads {
        now_ms: i64,
        reply: SyncSender<Result<u64>>,
    },
    RequeueUploadedOffsiteMedia {
        backup_set_id: BackupSetId,
        sha256: crate::backup::domain::ContentSha256,
        now_ms: i64,
        reply: SyncSender<Result<bool>>,
    },
    PrepareOffsiteCheckpoint {
        input: PrepareOffsiteCheckpointInput,
        reply: SyncSender<Result<PreparedOffsiteCheckpoint>>,
    },
    LoadOffsiteCheckpointMediaPage {
        checkpoint_id: CheckpointId,
        after_sha256: Option<crate::backup::domain::ContentSha256>,
        limit: u32,
        reply: SyncSender<Result<Vec<CheckpointMediaReference>>>,
    },
    MarkOffsiteCheckpointFenced {
        checkpoint_id: CheckpointId,
        txid: LitestreamTxid,
        reply: SyncSender<Result<()>>,
    },
    MarkOffsiteCheckpointReplicated {
        checkpoint_id: CheckpointId,
        reply: SyncSender<Result<()>>,
    },
    MarkOffsiteCheckpointPublished {
        checkpoint_id: CheckpointId,
        manifest_object_key: String,
        reply: SyncSender<Result<()>>,
    },
    MarkOffsiteCheckpointFailed {
        checkpoint_id: CheckpointId,
        error_code: CheckpointErrorCode,
        reply: SyncSender<Result<()>>,
    },
    FailIncompleteOffsiteCheckpoints {
        error_code: CheckpointErrorCode,
        reply: SyncSender<Result<u64>>,
    },
    LoadOffsiteCheckpointScheduleState {
        reply: SyncSender<Result<OffsiteCheckpointScheduleState>>,
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
    MaintenanceSnapshot {
        reply: SyncSender<Result<MaintenanceDatabaseSnapshot>>,
    },
    RebuildSearch {
        reply: SyncSender<Result<u64>>,
    },
    RebuildEmbeddings {
        now_ms: i64,
        reply: SyncSender<Result<u64>>,
    },
    RetryFailedExtractions {
        now_ms: i64,
        reply: SyncSender<Result<ExtractionRetryReport>>,
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
    IngestGenericAttachment {
        write: IngestGenericAttachmentWrite,
        reply: SyncSender<Result<GenericAttachmentRecord>>,
    },
    LoadGenericAttachmentStatus {
        attachment_id: String,
        reply: SyncSender<Result<GenericAttachmentStatusRecord>>,
    },
    IngestImage {
        write: IngestImageWrite,
        reply: SyncSender<Result<ImageRecord>>,
    },
    IngestPdf {
        write: IngestPdfWrite,
        reply: SyncSender<Result<PdfRecord>>,
    },
    LoadPdfStatus {
        attachment_id: String,
        reply: SyncSender<Result<PdfStatusRecord>>,
    },
    ClaimNextPdfExtraction {
        now_ms: i64,
        reply: SyncSender<Result<Option<PdfExtractionJob>>>,
    },
    CompletePdfExtraction {
        job: PdfExtractionJob,
        result: std::result::Result<Vec<PdfPageExtraction>, String>,
        completed_at_ms: i64,
        reply: SyncSender<Result<()>>,
    },
    RetryPdfExtraction {
        attachment_id: String,
        now_ms: i64,
        reply: SyncSender<Result<PdfStatusRecord>>,
    },
    RecoverInterruptedPdfExtraction {
        stale_started_at_or_before: i64,
        now_ms: i64,
        reply: SyncSender<Result<u64>>,
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
        requested_range: Option<MediaRangeRequest>,
        max_response_bytes: u64,
        reply: SyncSender<Result<MediaPayload>>,
    },
    MediaIntegrityReport {
        scan: MediaIntegrityScan,
        reply: SyncSender<Result<MediaIntegrityReport>>,
    },
    MaintainMediaWithSafetySnapshot {
        scan: MediaMaintenanceScan,
        snapshot: MediaMaintenanceSnapshotState,
        reply: SyncSender<Result<(Option<SafetySnapshotReport>, MediaMaintenanceReport)>>,
    },
    #[cfg(test)]
    CreateSafetySnapshotForTest {
        reason: SafetySnapshotReason,
        reply: SyncSender<Result<SafetySnapshotReport>>,
    },
    RecoverMediaLifecycleBatch {
        now_ms: i64,
        limits: MediaLimits,
        cursor: Option<Vec<u8>>,
        reply: SyncSender<Result<()>>,
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
    ListTidbitRevisions {
        input: ListTidbitRevisionsInput,
        reply: SyncSender<Result<TidbitRevisionPage>>,
    },
    LoadTidbitRevision {
        tidbit_id: String,
        revision_id: String,
        reply: SyncSender<Result<TidbitRevision>>,
    },
    LoadSourceUrl {
        source_id: String,
        reply: SyncSender<Result<String>>,
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
    PurgeTidbit {
        input: PurgeTidbitInput,
        now_ms: i64,
        reply: SyncSender<Result<bool>>,
    },
    CreateResearchRun {
        write: CreateResearchRunWrite,
        reply: SyncSender<Result<ResearchRunRecord>>,
    },
    AppendResearchEvent {
        write: AppendResearchEventWrite,
        reply: SyncSender<Result<()>>,
    },
    FailResearchRunStart {
        run_id: String,
        error: String,
        now_ms: i64,
        reply: SyncSender<Result<()>>,
    },
    InterruptActiveResearchRuns {
        now_ms: i64,
        reply: SyncSender<Result<u64>>,
    },
    ListResearchRuns {
        input: ListResearchRunsInput,
        reply: SyncSender<Result<ResearchRunPage>>,
    },
    LoadResearchRun {
        id: String,
        reply: SyncSender<Result<ResearchRunRecord>>,
    },
    SaveResearchAnswer {
        write: SaveResearchAnswerWrite,
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
    LoadShortcutSettings {
        reply: SyncSender<Result<ShortcutSettings>>,
    },
    SetShortcutSettings {
        input: SetShortcutSettingsInput,
        reply: SyncSender<Result<ShortcutSettings>>,
    },
    SetAutomaticUpdateChecks {
        input: SetAutomaticUpdateChecksInput,
        reply: SyncSender<Result<ShortcutSettings>>,
    },
    #[cfg(test)]
    PauseForTest {
        started: SyncSender<()>,
        release: mpsc::Receiver<()>,
    },
    Shutdown,
}

pub(super) enum MediaMaintenanceSnapshotState {
    PendingCandidates,
    NotNeeded,
    Verified(SafetySnapshotReport),
}

#[derive(Clone, Debug)]
pub struct DatabaseClient {
    sender: Sender<WriterMessage>,
    offsite_media_fence: Arc<Mutex<()>>,
}

impl DatabaseClient {
    pub(super) fn new(sender: Sender<WriterMessage>, offsite_media_fence: Arc<Mutex<()>>) -> Self {
        Self {
            sender,
            offsite_media_fence,
        }
    }

    fn request<T>(
        &self,
        message: impl FnOnce(SyncSender<Result<T>>) -> WriterMessage,
    ) -> Result<T> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(message(reply))
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    fn lock_offsite_media_fence(&self) -> std::sync::MutexGuard<'_, ()> {
        self.offsite_media_fence
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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

    pub(crate) fn load_offsite_backup_config(&self) -> Result<Option<OffsiteBackupConfig>> {
        self.load_offsite_backup_config_inner(false)
    }

    pub(crate) fn load_enabled_offsite_backup_config(&self) -> Result<Option<OffsiteBackupConfig>> {
        self.load_offsite_backup_config_inner(true)
    }

    fn load_offsite_backup_config_inner(
        &self,
        enabled_only: bool,
    ) -> Result<Option<OffsiteBackupConfig>> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::LoadOffsiteBackupConfig {
                enabled_only,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn save_offsite_backup_config(
        &self,
        input: SaveOffsiteBackupConfigInput,
    ) -> Result<OffsiteBackupConfig> {
        let _fence = self
            .offsite_media_fence
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::SaveOffsiteBackupConfig { input, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn begin_offsite_backup_config_intent(
        &self,
        input: BeginOffsiteBackupConfigIntentInput,
    ) -> Result<()> {
        let _fence = self.lock_offsite_media_fence();
        self.request(|reply| WriterMessage::BeginOffsiteBackupConfigIntent { input, reply })
    }

    pub(crate) fn load_offsite_backup_config_intent(
        &self,
    ) -> Result<Option<OffsiteBackupConfigIntent>> {
        self.request(|reply| WriterMessage::LoadOffsiteBackupConfigIntent { reply })
    }

    pub(crate) fn commit_offsite_backup_config_intent(
        &self,
        operation_id: String,
    ) -> Result<OffsiteBackupConfig> {
        let _fence = self.lock_offsite_media_fence();
        self.request(|reply| WriterMessage::CommitOffsiteBackupConfigIntent {
            operation_id,
            reply,
        })
    }

    pub(crate) fn complete_offsite_backup_config_intent(&self, operation_id: String) -> Result<()> {
        self.request(|reply| WriterMessage::CompleteOffsiteBackupConfigIntent {
            operation_id,
            reply,
        })
    }

    pub(crate) fn abort_offsite_backup_config_intent(&self, operation_id: String) -> Result<()> {
        self.request(|reply| WriterMessage::AbortOffsiteBackupConfigIntent {
            operation_id,
            reply,
        })
    }

    pub(crate) fn begin_offsite_backup_takeover_intent(
        &self,
        input: BeginOffsiteBackupTakeoverIntentInput,
    ) -> Result<()> {
        let _fence = self.lock_offsite_media_fence();
        self.request(|reply| WriterMessage::BeginOffsiteBackupTakeoverIntent { input, reply })
    }

    pub(crate) fn load_offsite_backup_takeover_intent(
        &self,
    ) -> Result<Option<OffsiteBackupTakeoverIntent>> {
        self.request(|reply| WriterMessage::LoadOffsiteBackupTakeoverIntent { reply })
    }

    pub(crate) fn commit_offsite_backup_takeover_intent(
        &self,
        operation_id: String,
    ) -> Result<OffsiteBackupConfig> {
        let _fence = self.lock_offsite_media_fence();
        self.request(|reply| WriterMessage::CommitOffsiteBackupTakeoverIntent {
            operation_id,
            reply,
        })
    }

    pub(crate) fn abort_offsite_backup_takeover_intent(&self, operation_id: String) -> Result<()> {
        self.request(|reply| WriterMessage::AbortOffsiteBackupTakeoverIntent {
            operation_id,
            reply,
        })
    }

    pub(crate) fn with_current_offsite_media_upload<T>(
        &self,
        claim: &OffsiteMediaUploadClaim,
        operation: impl FnOnce() -> T,
    ) -> Result<Option<T>> {
        let _fence = self
            .offsite_media_fence
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !self.authorize_offsite_media_remote_write(claim)? {
            return Ok(None);
        }
        Ok(Some(operation()))
    }

    pub(crate) fn with_current_offsite_checkpoint<T>(
        &self,
        config: &OffsiteBackupConfig,
        operation: impl FnOnce() -> T,
    ) -> Result<Option<T>> {
        let _fence = self
            .offsite_media_fence
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::AuthorizeOffsiteCheckpointRemoteOperation {
                config: config.clone(),
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        if !receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)??
        {
            return Ok(None);
        }
        Ok(Some(operation()))
    }

    fn authorize_offsite_media_remote_write(
        &self,
        claim: &OffsiteMediaUploadClaim,
    ) -> Result<bool> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::AuthorizeOffsiteMediaRemoteWrite {
                claim: claim.clone(),
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn load_offsite_credential_cleanup(&self) -> Result<Vec<BackupSetId>> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::LoadOffsiteCredentialCleanup { reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn complete_offsite_credential_cleanup(
        &self,
        backup_set_id: BackupSetId,
    ) -> Result<()> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::CompleteOffsiteCredentialCleanup {
                backup_set_id,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn reconcile_offsite_media_uploads(&self, now_ms: i64) -> Result<u64> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::ReconcileOffsiteMediaUploads { now_ms, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn recover_interrupted_offsite_media_uploads(&self, now_ms: i64) -> Result<u64> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::RecoverInterruptedOffsiteMediaUploads { now_ms, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn claim_next_offsite_media_upload(
        &self,
        now_ms: i64,
        lease_id: String,
    ) -> Result<Option<OffsiteMediaUploadClaim>> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::ClaimNextOffsiteMediaUpload {
                now_ms,
                lease_id,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn complete_offsite_media_upload(
        &self,
        claim: OffsiteMediaUploadClaim,
        remote_version: String,
        now_ms: i64,
    ) -> Result<bool> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::CompleteOffsiteMediaUpload {
                claim,
                remote_version,
                now_ms,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn fail_offsite_media_upload(
        &self,
        claim: OffsiteMediaUploadClaim,
        code: OffsiteMediaUploadFailureCode,
        retry_at_ms: Option<i64>,
        now_ms: i64,
    ) -> Result<bool> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::FailOffsiteMediaUpload {
                claim,
                code,
                retry_at_ms,
                now_ms,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn offsite_media_upload_progress(&self) -> Result<OffsiteMediaUploadProgress> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::OffsiteMediaUploadProgress { reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn retry_failed_offsite_media_uploads(&self, now_ms: i64) -> Result<u64> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::RetryFailedOffsiteMediaUploads { now_ms, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn requeue_uploaded_offsite_media(
        &self,
        backup_set_id: BackupSetId,
        sha256: crate::backup::domain::ContentSha256,
        now_ms: i64,
    ) -> Result<bool> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::RequeueUploadedOffsiteMedia {
                backup_set_id,
                sha256,
                now_ms,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn prepare_offsite_checkpoint(
        &self,
        input: PrepareOffsiteCheckpointInput,
    ) -> Result<PreparedOffsiteCheckpoint> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::PrepareOffsiteCheckpoint { input, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn load_offsite_checkpoint_media_page(
        &self,
        checkpoint_id: CheckpointId,
        after_sha256: Option<crate::backup::domain::ContentSha256>,
        limit: u32,
    ) -> Result<Vec<CheckpointMediaReference>> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::LoadOffsiteCheckpointMediaPage {
                checkpoint_id,
                after_sha256,
                limit,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn mark_offsite_checkpoint_fenced(
        &self,
        checkpoint_id: CheckpointId,
        txid: LitestreamTxid,
    ) -> Result<()> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::MarkOffsiteCheckpointFenced {
                checkpoint_id,
                txid,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn mark_offsite_checkpoint_replicated(
        &self,
        checkpoint_id: CheckpointId,
    ) -> Result<()> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::MarkOffsiteCheckpointReplicated {
                checkpoint_id,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn mark_offsite_checkpoint_published(
        &self,
        checkpoint_id: CheckpointId,
        manifest_object_key: String,
    ) -> Result<()> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::MarkOffsiteCheckpointPublished {
                checkpoint_id,
                manifest_object_key,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn mark_offsite_checkpoint_failed(
        &self,
        checkpoint_id: CheckpointId,
        error_code: CheckpointErrorCode,
    ) -> Result<()> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::MarkOffsiteCheckpointFailed {
                checkpoint_id,
                error_code,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn fail_incomplete_offsite_checkpoints(
        &self,
        error_code: CheckpointErrorCode,
    ) -> Result<u64> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::FailIncompleteOffsiteCheckpoints { error_code, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn load_offsite_checkpoint_schedule_state(
        &self,
    ) -> Result<OffsiteCheckpointScheduleState> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::LoadOffsiteCheckpointScheduleState { reply })
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

    pub(crate) fn ingest_generic_attachment(
        &self,
        write: IngestGenericAttachmentWrite,
    ) -> Result<GenericAttachmentRecord> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::IngestGenericAttachment { write, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn load_generic_attachment_status(
        &self,
        attachment_id: String,
    ) -> Result<GenericAttachmentStatusRecord> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::LoadGenericAttachmentStatus {
                attachment_id,
                reply,
            })
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

    pub(crate) fn ingest_pdf(&self, write: IngestPdfWrite) -> Result<PdfRecord> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::IngestPdf { write, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn load_pdf_status(&self, attachment_id: String) -> Result<PdfStatusRecord> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::LoadPdfStatus {
                attachment_id,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn claim_next_pdf_extraction(
        &self,
        now_ms: i64,
    ) -> Result<Option<PdfExtractionJob>> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::ClaimNextPdfExtraction { now_ms, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn complete_pdf_extraction(
        &self,
        job: PdfExtractionJob,
        result: std::result::Result<Vec<PdfPageExtraction>, String>,
        completed_at_ms: i64,
    ) -> Result<()> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::CompletePdfExtraction {
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

    pub(crate) fn retry_pdf_extraction(
        &self,
        attachment_id: String,
        now_ms: i64,
    ) -> Result<PdfStatusRecord> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::RetryPdfExtraction {
                attachment_id,
                now_ms,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn recover_interrupted_pdf_extraction(
        &self,
        stale_started_at_or_before: i64,
        now_ms: i64,
    ) -> Result<u64> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::RecoverInterruptedPdfExtraction {
                stale_started_at_or_before,
                now_ms,
                reply,
            })
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
        requested_range: Option<MediaRangeRequest>,
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

    pub(crate) fn maintain_media_with_safety_snapshot(
        &self,
        now_ms: i64,
        limits: MediaLimits,
    ) -> Result<(Option<SafetySnapshotReport>, MediaMaintenanceReport)> {
        let _fence = self
            .offsite_media_fence
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::MaintainMediaWithSafetySnapshot {
                scan: MediaMaintenanceScan::new(now_ms, limits)?,
                snapshot: MediaMaintenanceSnapshotState::PendingCandidates,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    #[cfg(test)]
    pub(crate) fn create_safety_snapshot_for_test(
        &self,
        reason: SafetySnapshotReason,
    ) -> Result<SafetySnapshotReport> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::CreateSafetySnapshotForTest { reason, reply })
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
        let _fence = self
            .offsite_media_fence
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::RecoverMediaLifecycleBatch {
                now_ms,
                limits,
                cursor: None,
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

    pub fn maintenance_snapshot(&self) -> Result<MaintenanceDatabaseSnapshot> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::MaintenanceSnapshot { reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub fn rebuild_search(&self) -> Result<u64> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::RebuildSearch { reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub fn rebuild_embeddings(&self, now_ms: i64) -> Result<u64> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::RebuildEmbeddings { now_ms, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub fn retry_failed_extractions(&self, now_ms: i64) -> Result<ExtractionRetryReport> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::RetryFailedExtractions { now_ms, reply })
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

    pub(crate) fn list_tidbit_revisions(
        &self,
        input: ListTidbitRevisionsInput,
    ) -> Result<TidbitRevisionPage> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::ListTidbitRevisions { input, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn load_tidbit_revision(
        &self,
        tidbit_id: String,
        revision_id: String,
    ) -> Result<TidbitRevision> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::LoadTidbitRevision {
                tidbit_id,
                revision_id,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn load_source_url(&self, source_id: String) -> Result<String> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::LoadSourceUrl { source_id, reply })
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

    pub(crate) fn purge_tidbit(&self, input: PurgeTidbitInput, now_ms: i64) -> Result<bool> {
        let _fence = self
            .offsite_media_fence
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::PurgeTidbit {
                input,
                now_ms,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn create_research_run(
        &self,
        write: CreateResearchRunWrite,
    ) -> Result<ResearchRunRecord> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::CreateResearchRun { write, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn append_research_event(&self, write: AppendResearchEventWrite) -> Result<()> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::AppendResearchEvent { write, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn fail_research_run_start(
        &self,
        run_id: String,
        error: String,
        now_ms: i64,
    ) -> Result<()> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::FailResearchRunStart {
                run_id,
                error,
                now_ms,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn interrupt_active_research_runs(&self, now_ms: i64) -> Result<u64> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::InterruptActiveResearchRuns { now_ms, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn list_research_runs(
        &self,
        input: ListResearchRunsInput,
    ) -> Result<ResearchRunPage> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::ListResearchRuns { input, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn load_research_run(&self, id: String) -> Result<ResearchRunRecord> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::LoadResearchRun { id, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn save_research_answer_as_tidbit(
        &self,
        write: SaveResearchAnswerWrite,
    ) -> Result<Tidbit> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::SaveResearchAnswer { write, reply })
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
    pub(crate) fn enqueue_media_maintenance_for_test(
        &self,
        now_ms: i64,
        limits: MediaLimits,
    ) -> Result<MediaMaintenanceReplyReceiver> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::MaintainMediaWithSafetySnapshot {
                scan: MediaMaintenanceScan::new(now_ms, limits)?,
                snapshot: MediaMaintenanceSnapshotState::PendingCandidates,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        Ok(receiver)
    }

    #[cfg(test)]
    pub(crate) fn enqueue_clear_draft_for_test(
        &self,
        input: ClearDraftInput,
        now_ms: i64,
    ) -> Result<mpsc::Receiver<Result<bool>>> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::ClearDraft {
                input,
                now_ms,
                reply,
            })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        Ok(receiver)
    }

    #[cfg(test)]
    pub(crate) fn pause_for_test(
        &self,
        started: SyncSender<()>,
        release: mpsc::Receiver<()>,
    ) -> Result<()> {
        self.sender
            .send(WriterMessage::PauseForTest { started, release })
            .map_err(|_| DatabaseError::WriterUnavailable)
    }

    pub(crate) fn load_shortcut_settings(&self) -> Result<ShortcutSettings> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::LoadShortcutSettings { reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn set_shortcut_settings(
        &self,
        input: SetShortcutSettingsInput,
    ) -> Result<ShortcutSettings> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::SetShortcutSettings { input, reply })
            .map_err(|_| DatabaseError::WriterUnavailable)?;
        receiver
            .recv()
            .map_err(|_| DatabaseError::WriterUnavailable)?
    }

    pub(crate) fn set_automatic_update_checks(
        &self,
        input: SetAutomaticUpdateChecksInput,
    ) -> Result<ShortcutSettings> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(WriterMessage::SetAutomaticUpdateChecks { input, reply })
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

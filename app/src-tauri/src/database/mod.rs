mod backup_media;
mod backup_state;
pub(crate) mod commands;
pub(crate) mod connection;
pub(crate) mod drafts;
pub(crate) mod embedding_index;
mod error;
mod maintenance;
pub(crate) mod media;
mod migrations;
mod offsite_checkpoint;
pub(crate) mod passages;
mod paths;
mod research_runs;
mod restore_install;
mod safety_snapshot;
pub(crate) mod search;
pub(crate) mod settings;
pub(crate) mod tidbits;
mod validation;
mod writer;

#[cfg(test)]
mod backup_media_tests;
#[cfg(test)]
mod backup_state_tests;
#[cfg(test)]
mod drafts_tests;
#[cfg(test)]
mod embedding_index_tests;
#[cfg(test)]
mod maintenance_tests;
#[cfg(test)]
mod media_tests;
#[cfg(test)]
mod offsite_checkpoint_tests;
#[cfg(test)]
mod reliability_tests;
#[cfg(test)]
mod research_runs_tests;
#[cfg(test)]
mod settings_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tidbits_tests;

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::Read,
    sync::{
        mpsc::{self, Receiver},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};

use rusqlite::Connection;

pub(crate) use backup_media::{OffsiteMediaUploadClaim, OffsiteMediaUploadFailureCode};
pub(crate) use backup_state::{
    BeginOffsiteBackupConfigIntentInput, BeginOffsiteBackupTakeoverIntentInput,
    CredentialIntentAction, OffsiteBackupConfig, OffsiteBackupConfigIntent,
    OffsiteBackupTakeoverIntent, OffsiteOperationState, SaveOffsiteBackupConfigInput,
};
pub use drafts::{ClearDraftInput, Draft, SaveDraftInput};
pub use error::{DatabaseError, Result};
pub use maintenance::MaintenanceDatabaseSnapshot;
pub use media::{
    AttachmentExtractionStatus, AttachmentIngestInput, AttachmentKind, AttachmentRecord,
    GenericAttachmentRecord, GenericAttachmentStatusRecord, ImageOcrDiagnostics, ImageOcrRecovery,
    ImageOcrStatus, ImageRecord, ImageStatusRecord, MediaCleanupResult, MediaIntegrityReport,
    MediaLimits, MediaMaintenanceReport, PdfExtractionStatus, PdfRecord, PdfStatusRecord,
};
pub(crate) use offsite_checkpoint::{
    CheckpointMediaReference, OffsiteCheckpointScheduleState, PrepareOffsiteCheckpointInput,
    PreparedOffsiteCheckpoint,
};
pub use passages::{
    CitationAttachment, CitationLocator, CitationResolution, CitationState, CitationTidbit,
};
pub use paths::DatabasePaths;
pub(crate) use research_runs::{AppendResearchEventWrite, CreateResearchRunWrite};
pub use research_runs::{ListResearchRunsInput, ResearchRunPage, ResearchRunRecord};
pub(crate) use restore_install::{
    create_empty_media_at as create_empty_restore_media_database_at,
    open_main_read_only_at as open_restore_main_read_only_at,
    validate_pair_at as validate_restored_pair_at,
};
#[cfg(test)]
pub(crate) use restore_install::{
    inspect_completed_install as inspect_completed_restore_install,
    install as install_restored_pair, RestoreInstallReport,
};
pub(crate) use safety_snapshot::available_space_bytes as available_storage_bytes;
pub(crate) use safety_snapshot::SafetySnapshotReason;
pub use search::{
    LexicalSearchMode, PassageSearchResult, SearchExecutionMode, SearchField, SearchHighlight,
    SearchPassagesInput, SearchPassagesResponse, SemanticSearchReadiness,
};
pub use settings::{
    validate_complete_bindings, KeyboardBinding, KoshCommand, SetAutomaticUpdateChecksInput,
    SetShortcutSettingsInput, ShortcutSettings, DEFAULT_MAIN_WINDOW_ACCELERATOR,
    DEFAULT_QUICK_ADD_ACCELERATOR,
};
pub use tidbits::{
    DeleteTidbitInput, EditTidbitInput, ListTidbitRevisionsInput, ListTidbitsInput,
    PurgeTidbitInput, RestoreTidbitInput, SourceDraft, Tidbit, TidbitDraft, TidbitListCursor,
    TidbitListItem, TidbitListPage, TidbitListScope, TidbitRevision, TidbitRevisionAttachment,
    TidbitRevisionPage, TidbitRevisionSummary, TidbitSource, TIDBIT_PURGE_DELAY_MS,
};
pub(crate) use writer::LexicalBenchmarkAttachmentWrite;
use writer::MediaMaintenanceSnapshotState;
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
        restore_install::recover_interrupted(&paths)?;

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
        if (main_status.pending || media_status.pending)
            && main_state == FileState::Existing
            && media_state == FileState::Existing
        {
            let report = safety_snapshot::create(
                &mut main,
                &mut media,
                &paths,
                SafetySnapshotReason::Migration,
            )?;
            log::info!(
                "created verified pre-migration safety snapshot {}",
                report.id
            );
        }
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
        settings::load_shortcut_settings(&main)?;

        let (sender, receiver) = mpsc::channel();
        let client = DatabaseClient::new(sender.clone(), Arc::new(Mutex::new(())));
        let writer_paths = paths.clone();
        let writer_thread = thread::Builder::new()
            .name("kosh-database-writer".into())
            .spawn(move || writer_loop(main, media, writer_paths, receiver, sender))?;

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
    paths: DatabasePaths,
    receiver: Receiver<WriterMessage>,
    sender: mpsc::Sender<WriterMessage>,
) {
    while let Ok(message) = receiver.recv() {
        match message {
            WriterMessage::Diagnostics { reply } => {
                let _ = reply.send(writer::diagnostics(&mut main, &mut media));
            }
            WriterMessage::LoadOffsiteBackupConfig {
                enabled_only,
                reply,
            } => {
                let result = if enabled_only {
                    backup_state::load_enabled(&main)
                } else {
                    backup_state::load(&main)
                };
                let _ = reply.send(result);
            }
            WriterMessage::SaveOffsiteBackupConfig { input, reply } => {
                let _ = reply.send(backup_state::save(&mut main, input));
            }
            WriterMessage::BeginOffsiteBackupConfigIntent { input, reply } => {
                let _ = reply.send(backup_state::begin_config_intent(&mut main, input));
            }
            WriterMessage::LoadOffsiteBackupConfigIntent { reply } => {
                let _ = reply.send(backup_state::load_config_intent(&main));
            }
            WriterMessage::CommitOffsiteBackupConfigIntent {
                operation_id,
                reply,
            } => {
                let _ = reply.send(backup_state::commit_config_intent(&mut main, &operation_id));
            }
            WriterMessage::CompleteOffsiteBackupConfigIntent {
                operation_id,
                reply,
            } => {
                let _ = reply.send(backup_state::complete_config_intent(
                    &mut main,
                    &operation_id,
                ));
            }
            WriterMessage::AbortOffsiteBackupConfigIntent {
                operation_id,
                reply,
            } => {
                let _ = reply.send(backup_state::abort_config_intent(&mut main, &operation_id));
            }
            WriterMessage::BeginOffsiteBackupTakeoverIntent { input, reply } => {
                let _ = reply.send(backup_state::begin_takeover_intent(&mut main, input));
            }
            WriterMessage::LoadOffsiteBackupTakeoverIntent { reply } => {
                let _ = reply.send(backup_state::load_takeover_intent(&main));
            }
            WriterMessage::CommitOffsiteBackupTakeoverIntent {
                operation_id,
                reply,
            } => {
                let _ = reply.send(backup_state::commit_takeover_intent(
                    &mut main,
                    &operation_id,
                ));
            }
            WriterMessage::AbortOffsiteBackupTakeoverIntent {
                operation_id,
                reply,
            } => {
                let _ = reply.send(backup_state::abort_takeover_intent(
                    &mut main,
                    &operation_id,
                ));
            }
            WriterMessage::LoadOffsiteCredentialCleanup { reply } => {
                let _ = reply.send(backup_state::load_credential_cleanup(&main));
            }
            WriterMessage::CompleteOffsiteCredentialCleanup {
                backup_set_id,
                reply,
            } => {
                let _ = reply.send(backup_state::complete_credential_cleanup(
                    &mut main,
                    &backup_set_id,
                ));
            }
            WriterMessage::ReconcileOffsiteMediaUploads { now_ms, reply } => {
                let _ = reply.send(backup_media::reconcile(&mut main, now_ms));
            }
            WriterMessage::RecoverInterruptedOffsiteMediaUploads { now_ms, reply } => {
                let _ = reply.send(backup_media::recover_interrupted(&main, now_ms));
            }
            WriterMessage::ClaimNextOffsiteMediaUpload {
                now_ms,
                lease_id,
                reply,
            } => {
                let _ = reply.send(backup_media::claim_next(&mut main, now_ms, lease_id));
            }
            WriterMessage::AuthorizeOffsiteMediaRemoteWrite { claim, reply } => {
                let _ = reply.send(backup_media::authorize_remote_write(&main, &claim));
            }
            WriterMessage::AuthorizeOffsiteCheckpointRemoteOperation { config, reply } => {
                let _ = reply.send(backup_state::is_current_enabled(&main, &config));
            }
            WriterMessage::CompleteOffsiteMediaUpload {
                claim,
                remote_version,
                now_ms,
                reply,
            } => {
                let _ = reply.send(backup_media::complete(
                    &main,
                    &claim,
                    &remote_version,
                    now_ms,
                ));
            }
            WriterMessage::FailOffsiteMediaUpload {
                claim,
                code,
                retry_at_ms,
                now_ms,
                reply,
            } => {
                let _ = reply.send(backup_media::fail(&main, &claim, code, retry_at_ms, now_ms));
            }
            WriterMessage::OffsiteMediaUploadProgress { reply } => {
                let _ = reply.send(backup_media::progress(&main));
            }
            WriterMessage::RetryFailedOffsiteMediaUploads { now_ms, reply } => {
                let _ = reply.send(backup_media::retry_failed(&main, now_ms));
            }
            WriterMessage::RequeueUploadedOffsiteMedia {
                backup_set_id,
                sha256,
                now_ms,
                reply,
            } => {
                let _ = reply.send(backup_media::requeue_uploaded(
                    &main,
                    &backup_set_id,
                    sha256,
                    now_ms,
                ));
            }
            WriterMessage::PrepareOffsiteCheckpoint { input, reply } => {
                let _ = reply.send(offsite_checkpoint::prepare(&mut main, &media, input));
            }
            WriterMessage::LoadOffsiteCheckpointMediaPage {
                checkpoint_id,
                after_sha256,
                limit,
                reply,
            } => {
                let _ = reply.send(offsite_checkpoint::load_media_page(
                    &main,
                    &checkpoint_id,
                    after_sha256,
                    limit,
                ));
            }
            WriterMessage::MarkOffsiteCheckpointFenced {
                checkpoint_id,
                txid,
                reply,
            } => {
                let _ = reply.send(offsite_checkpoint::mark_fenced(
                    &mut main,
                    &checkpoint_id,
                    txid,
                ));
            }
            WriterMessage::MarkOffsiteCheckpointReplicated {
                checkpoint_id,
                reply,
            } => {
                let _ = reply.send(offsite_checkpoint::mark_replicated(
                    &mut main,
                    &checkpoint_id,
                ));
            }
            WriterMessage::MarkOffsiteCheckpointPublished {
                checkpoint_id,
                manifest_object_key,
                reply,
            } => {
                let _ = reply.send(offsite_checkpoint::mark_published(
                    &mut main,
                    &checkpoint_id,
                    &manifest_object_key,
                ));
            }
            WriterMessage::MarkOffsiteCheckpointFailed {
                checkpoint_id,
                error_code,
                reply,
            } => {
                let _ = reply.send(offsite_checkpoint::mark_failed(
                    &mut main,
                    &checkpoint_id,
                    error_code,
                ));
            }
            WriterMessage::FailIncompleteOffsiteCheckpoints { error_code, reply } => {
                let _ = reply.send(offsite_checkpoint::fail_incomplete(&mut main, error_code));
            }
            WriterMessage::LoadOffsiteCheckpointScheduleState { reply } => {
                let _ = reply.send(offsite_checkpoint::schedule_state(&main));
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
            WriterMessage::MaintenanceSnapshot { reply } => {
                let _ = reply.send(maintenance::snapshot(&main));
            }
            WriterMessage::RebuildSearch { reply } => {
                let _ = reply.send(maintenance::rebuild_search(&mut main));
            }
            WriterMessage::RebuildEmbeddings { now_ms, reply } => {
                let _ = reply.send(maintenance::rebuild_embeddings(&mut main, now_ms));
            }
            WriterMessage::RetryFailedExtractions { now_ms, reply } => {
                let _ = reply.send(maintenance::retry_failed_extractions(&mut main, now_ms));
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
            WriterMessage::IngestGenericAttachment { write, reply } => {
                let _ = reply.send(media::ingest_generic_attachment(
                    &mut main, &mut media, write,
                ));
            }
            WriterMessage::LoadGenericAttachmentStatus {
                attachment_id,
                reply,
            } => {
                let _ = reply.send(media::load_generic_attachment_status(&main, &attachment_id));
            }
            WriterMessage::IngestImage { write, reply } => {
                let _ = reply.send(media::ingest_image(&mut main, &mut media, write));
            }
            WriterMessage::IngestPdf { write, reply } => {
                let _ = reply.send(media::ingest_pdf(&mut main, &mut media, write));
            }
            WriterMessage::LoadPdfStatus {
                attachment_id,
                reply,
            } => {
                let _ = reply.send(media::load_pdf_status(&main, &attachment_id));
            }
            WriterMessage::ClaimNextPdfExtraction { now_ms, reply } => {
                let _ = reply.send(media::claim_next_pdf_extraction(&mut main, &media, now_ms));
            }
            WriterMessage::CompletePdfExtraction {
                job,
                result,
                completed_at_ms,
                reply,
            } => {
                let _ = reply.send(media::complete_pdf_extraction(
                    &mut main,
                    &job,
                    result,
                    completed_at_ms,
                ));
            }
            WriterMessage::RetryPdfExtraction {
                attachment_id,
                now_ms,
                reply,
            } => {
                let _ = reply.send(media::retry_pdf_extraction(
                    &mut main,
                    &attachment_id,
                    now_ms,
                ));
            }
            WriterMessage::RecoverInterruptedPdfExtraction {
                stale_started_at_or_before,
                now_ms,
                reply,
            } => {
                let _ = reply.send(media::recover_interrupted_pdf_extraction(
                    &mut main,
                    stale_started_at_or_before,
                    now_ms,
                ));
            }
            WriterMessage::LoadImageStatus {
                attachment_id,
                reply,
            } => {
                let _ = reply.send(media::load_image_status(&main, &attachment_id));
            }
            WriterMessage::ClaimNextImageOcr { now_ms, reply } => {
                let _ = reply.send(media::claim_next_image_ocr(&mut main, &media, now_ms));
            }
            WriterMessage::CompleteImageOcr {
                job,
                result,
                completed_at_ms,
                reply,
            } => {
                let _ = reply.send(media::complete_image_ocr(
                    &mut main,
                    &job,
                    result,
                    completed_at_ms,
                ));
            }
            WriterMessage::RetryImageOcr {
                attachment_id,
                now_ms,
                reply,
            } => {
                let _ = reply.send(media::retry_image_ocr(&mut main, &attachment_id, now_ms));
            }
            WriterMessage::RecoverInterruptedImageOcr {
                stale_started_at_or_before,
                now_ms,
                reply,
            } => {
                let _ = reply.send(media::recover_interrupted_image_ocr(
                    &mut main,
                    stale_started_at_or_before,
                    now_ms,
                ));
            }
            WriterMessage::ImageOcrDiagnostics { reply } => {
                let _ = reply.send(media::image_ocr_diagnostics(&main));
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
            WriterMessage::MediaIntegrityReport { scan, reply } => match scan.step(&main, &media) {
                Ok(media::MediaIntegrityScanStep::Continue(scan)) => {
                    if let Err(error) =
                        sender.send(WriterMessage::MediaIntegrityReport { scan, reply })
                    {
                        let WriterMessage::MediaIntegrityReport { reply, .. } = error.0 else {
                            unreachable!("failed message retained its variant");
                        };
                        let _ = reply.send(Err(DatabaseError::WriterUnavailable));
                    }
                }
                Ok(media::MediaIntegrityScanStep::Complete(report)) => {
                    let _ = reply.send(Ok(report));
                }
                Err(error) => {
                    let _ = reply.send(Err(error));
                }
            },
            WriterMessage::MaintainMediaWithSafetySnapshot {
                scan,
                snapshot,
                reply,
            } => {
                let snapshot = match snapshot {
                    MediaMaintenanceSnapshotState::PendingCandidates => {
                        match media::media_blob_reclamation_preflight(
                            &mut main,
                            &media,
                            scan.now_ms(),
                            scan.limits(),
                        ) {
                            Ok(media::MediaBlobReclamationPreflight::Continue) => {
                                if let Err(error) =
                                    sender.send(WriterMessage::MaintainMediaWithSafetySnapshot {
                                        scan,
                                        snapshot: MediaMaintenanceSnapshotState::PendingCandidates,
                                        reply,
                                    })
                                {
                                    let WriterMessage::MaintainMediaWithSafetySnapshot {
                                        reply,
                                        ..
                                    } = error.0
                                    else {
                                        unreachable!("failed message retained its variant");
                                    };
                                    let _ = reply.send(Err(DatabaseError::WriterUnavailable));
                                }
                                continue;
                            }
                            Ok(media::MediaBlobReclamationPreflight::Eligible) => {
                                safety_snapshot::create(
                                    &mut main,
                                    &mut media,
                                    &paths,
                                    SafetySnapshotReason::MediaReclaim,
                                )
                                .map(MediaMaintenanceSnapshotState::Verified)
                            }
                            Ok(media::MediaBlobReclamationPreflight::NotNeeded) => {
                                match media::attachment_reclamation_is_eligible(
                                    &main,
                                    scan.now_ms(),
                                ) {
                                    Ok(true) => safety_snapshot::create(
                                        &mut main,
                                        &mut media,
                                        &paths,
                                        SafetySnapshotReason::MediaReclaim,
                                    )
                                    .map(MediaMaintenanceSnapshotState::Verified),
                                    Ok(false) => Ok(MediaMaintenanceSnapshotState::NotNeeded),
                                    Err(error) => Err(error),
                                }
                            }
                            Err(error) => Err(error),
                        }
                    }
                    snapshot => Ok(snapshot),
                };
                match snapshot {
                    Err(error) => {
                        let _ = reply.send(Err(error));
                    }
                    Ok(snapshot) => match scan.step(
                        &mut main,
                        &mut media,
                        matches!(&snapshot, MediaMaintenanceSnapshotState::Verified(_)),
                    ) {
                        Ok(media::MediaMaintenanceScanStep::Continue(scan)) => {
                            if let Err(error) =
                                sender.send(WriterMessage::MaintainMediaWithSafetySnapshot {
                                    scan,
                                    snapshot,
                                    reply,
                                })
                            {
                                let WriterMessage::MaintainMediaWithSafetySnapshot {
                                    reply, ..
                                } = error.0
                                else {
                                    unreachable!("failed message retained its variant");
                                };
                                let _ = reply.send(Err(DatabaseError::WriterUnavailable));
                            }
                        }
                        Ok(media::MediaMaintenanceScanStep::Complete(report)) => {
                            let snapshot = match snapshot {
                                MediaMaintenanceSnapshotState::Verified(snapshot) => Some(snapshot),
                                MediaMaintenanceSnapshotState::NotNeeded => None,
                                MediaMaintenanceSnapshotState::PendingCandidates => {
                                    unreachable!("maintenance preflight was not resolved")
                                }
                            };
                            let _ = reply.send(Ok((snapshot, report)));
                        }
                        Err(error) => {
                            let _ = reply.send(Err(error));
                        }
                    },
                }
            }
            #[cfg(test)]
            WriterMessage::CreateSafetySnapshotForTest { reason, reply } => {
                let _ = reply.send(safety_snapshot::create(
                    &mut main, &mut media, &paths, reason,
                ));
            }
            WriterMessage::RecoverMediaLifecycleBatch {
                now_ms,
                limits,
                cursor,
                reply,
            } => match media::recover_media_lifecycle_batch(
                &mut main, &mut media, now_ms, limits, cursor,
            ) {
                Ok(Some(cursor)) => {
                    if let Err(error) = sender.send(WriterMessage::RecoverMediaLifecycleBatch {
                        now_ms,
                        limits,
                        cursor: Some(cursor),
                        reply,
                    }) {
                        let WriterMessage::RecoverMediaLifecycleBatch { reply, .. } = error.0
                        else {
                            unreachable!("failed message retained its variant");
                        };
                        let _ = reply.send(Err(DatabaseError::WriterUnavailable));
                    }
                }
                Ok(None) => {
                    let _ = reply.send(Ok(()));
                }
                Err(error) => {
                    let _ = reply.send(Err(error));
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
            WriterMessage::ListTidbitRevisions { input, reply } => {
                let _ = reply.send(tidbits::list_tidbit_revisions(&main, input));
            }
            WriterMessage::LoadTidbitRevision {
                tidbit_id,
                revision_id,
                reply,
            } => {
                let _ = reply.send(tidbits::load_tidbit_revision(
                    &main,
                    &tidbit_id,
                    &revision_id,
                ));
            }
            WriterMessage::LoadSourceUrl { source_id, reply } => {
                let _ = reply.send(tidbits::load_source_url(&main, &source_id));
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
            WriterMessage::PurgeTidbit {
                input,
                now_ms,
                reply,
            } => {
                let _ = reply.send(tidbits::purge_tidbit(&mut main, input, now_ms));
            }
            WriterMessage::CreateResearchRun { write, reply } => {
                let _ = reply.send(research_runs::create(&mut main, write));
            }
            WriterMessage::AppendResearchEvent { write, reply } => {
                let _ = reply.send(research_runs::append_event(&mut main, write));
            }
            WriterMessage::FailResearchRunStart {
                run_id,
                error,
                now_ms,
                reply,
            } => {
                let _ = reply.send(research_runs::fail_start(&main, &run_id, &error, now_ms));
            }
            WriterMessage::InterruptActiveResearchRuns { now_ms, reply } => {
                let _ = reply.send(research_runs::interrupt_active(&main, now_ms));
            }
            WriterMessage::ListResearchRuns { input, reply } => {
                let _ = reply.send(research_runs::list(&main, input));
            }
            WriterMessage::LoadResearchRun { id, reply } => {
                let _ = reply.send(research_runs::load(&main, &id));
            }
            WriterMessage::SaveResearchAnswer { write, reply } => {
                let _ = reply.send(research_runs::save_answer_as_tidbit(&mut main, write));
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
            WriterMessage::LoadShortcutSettings { reply } => {
                let _ = reply.send(settings::load_shortcut_settings(&main));
            }
            WriterMessage::SetShortcutSettings { input, reply } => {
                let _ = reply.send(settings::set_shortcut_settings(&mut main, input));
            }
            WriterMessage::SetAutomaticUpdateChecks { input, reply } => {
                let _ = reply.send(settings::set_automatic_update_checks(&mut main, input));
            }
            #[cfg(test)]
            WriterMessage::PauseForTest { started, release } => {
                let _ = started.send(());
                let _ = release.recv();
            }
            WriterMessage::Shutdown => break,
        }
    }
}

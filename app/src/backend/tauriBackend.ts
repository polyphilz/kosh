import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { TauriCommand, TauriEvent } from "../tauriProtocol";
import type {
  Backend,
  BackupConnectionTestResult,
  BackupRestoreDrill,
  BackupRestorePreview,
  BackupSettingsSnapshot,
  BeginResearchProcessInput,
  CitationResolution,
  ClaudeCliDefaults,
  ClaudeSetupStatus,
  ClearDraftInput,
  ConfigureBackupInput,
  DeleteTidbitInput,
  DraftRecord,
  EditTidbitInput,
  GenericAttachmentStatusRecord,
  ImageDropIngestResult,
  ImageOcrDiagnostics,
  ImageRecord,
  ImageStatusRecord,
  IntegrityCheckOutcome,
  ListTidbitsInput,
  ListTidbitRevisionsInput,
  ListResearchRunsInput,
  MaintenanceDiagnostics,
  MaintenanceOutcome,
  PassageEmbeddingIndexStatus,
  PdfRecord,
  PdfStatusRecord,
  RuntimeProbe,
  ResearchProcessEvent,
  ResearchRunPage,
  ResearchRunRecord,
  RemoteBackupCheckpoint,
  SelectedAttachmentRecord,
  SetShortcutSettingsInput,
  SetBackupEnabledInput,
  RestoreTidbitInput,
  PurgeTidbitInput,
  SaveDraftInput,
  SearchPassagesInput,
  SearchPassagesResponse,
  SemanticRuntimeLogs,
  SemanticRuntimeStatus,
  ShortcutSettingsSnapshot,
  TidbitDraft,
  TidbitListPage,
  TidbitRecord,
  TidbitRevisionPage,
  TidbitRevisionRecord,
  StartResearchProcessOutput,
  TakeOverBackupInput,
  TestBackupConnectionInput,
  RestoreCheckpointInput,
} from "./contracts";

export const tauriBackend: Backend = {
  runtimeProbe: () => invoke<RuntimeProbe>(TauriCommand.RuntimeProbe),
  loadBackupSettings: () => invoke<BackupSettingsSnapshot>(TauriCommand.LoadBackupSettings),
  testBackupConnection: (input: TestBackupConnectionInput) =>
    invoke<BackupConnectionTestResult>(TauriCommand.TestBackupConnection, { input }),
  configureBackup: (input: ConfigureBackupInput) =>
    invoke<BackupSettingsSnapshot>(TauriCommand.ConfigureBackup, { input }),
  setBackupEnabled: (input: SetBackupEnabledInput) =>
    invoke<BackupSettingsSnapshot>(TauriCommand.SetBackupEnabled, { input }),
  backupNow: () => invoke<void>(TauriCommand.BackupNow),
  listBackupCheckpoints: () => invoke<RemoteBackupCheckpoint[]>(TauriCommand.ListBackupCheckpoints),
  previewBackupRestore: (input: RestoreCheckpointInput) =>
    invoke<BackupRestorePreview>(TauriCommand.PreviewBackupRestore, { input }),
  drillBackupRestore: (input: RestoreCheckpointInput) =>
    invoke<BackupRestoreDrill>(TauriCommand.DrillBackupRestore, { input }),
  takeOverBackup: (input: TakeOverBackupInput) =>
    invoke<BackupSettingsSnapshot>(TauriCommand.TakeOverBackup, { input }),
  semanticRuntimeStatus: () => invoke<SemanticRuntimeStatus>(TauriCommand.SemanticRuntimeStatus),
  prepareSemanticRuntime: () => invoke<SemanticRuntimeStatus>(TauriCommand.PrepareSemanticRuntime),
  retrySemanticRuntime: () => invoke<SemanticRuntimeStatus>(TauriCommand.RetrySemanticRuntime),
  repairSemanticRuntime: () => invoke<SemanticRuntimeStatus>(TauriCommand.RepairSemanticRuntime),
  semanticRuntimeLogs: () => invoke<SemanticRuntimeLogs>(TauriCommand.SemanticRuntimeLogs),
  passageEmbeddingIndexStatus: () =>
    invoke<PassageEmbeddingIndexStatus>(TauriCommand.PassageEmbeddingIndexStatus),
  loadMaintenanceDiagnostics: () =>
    invoke<MaintenanceDiagnostics>(TauriCommand.LoadMaintenanceDiagnostics),
  runIntegrityCheck: () => invoke<IntegrityCheckOutcome>(TauriCommand.RunIntegrityCheck),
  rebuildSearchIndexes: () => invoke<MaintenanceOutcome>(TauriCommand.RebuildSearchIndexes),
  rebuildEmbeddingIndex: () => invoke<MaintenanceOutcome>(TauriCommand.RebuildEmbeddingIndex),
  retryFailedExtractions: () => invoke<MaintenanceOutcome>(TauriCommand.RetryFailedExtractions),
  reclaimEligibleMedia: () => invoke<MaintenanceOutcome>(TauriCommand.ReclaimEligibleMedia),
  createTidbit: (input: TidbitDraft) => invoke<TidbitRecord>(TauriCommand.CreateTidbit, { input }),
  loadTidbit: (id: string) => invoke<TidbitRecord>(TauriCommand.LoadTidbit, { id }),
  listTidbits: (input: ListTidbitsInput) =>
    invoke<TidbitListPage>(TauriCommand.ListTidbits, { input }),
  listTidbitRevisions: (input: ListTidbitRevisionsInput) =>
    invoke<TidbitRevisionPage>(TauriCommand.ListTidbitRevisions, { input }),
  loadTidbitRevision: (tidbitId: string, revisionId: string) =>
    invoke<TidbitRevisionRecord>(TauriCommand.LoadTidbitRevision, { tidbitId, revisionId }),
  editTidbit: (input: EditTidbitInput) => invoke<TidbitRecord>(TauriCommand.EditTidbit, { input }),
  deleteTidbit: (input: DeleteTidbitInput) =>
    invoke<TidbitRecord>(TauriCommand.DeleteTidbit, { input }),
  restoreTidbit: (input: RestoreTidbitInput) =>
    invoke<TidbitRecord>(TauriCommand.RestoreTidbit, { input }),
  purgeTidbit: (input: PurgeTidbitInput) => invoke<boolean>(TauriCommand.PurgeTidbit, { input }),
  openSourceUrl: (sourceId: string) => invoke<void>(TauriCommand.OpenSourceUrl, { sourceId }),
  resolveCitation: (passageId: string) =>
    invoke<CitationResolution>(TauriCommand.ResolveCitation, { passageId }),
  searchPassages: (input: SearchPassagesInput) =>
    invoke<SearchPassagesResponse>(TauriCommand.SearchPassages, { input }),
  saveDraft: (input: SaveDraftInput) => invoke<DraftRecord>(TauriCommand.SaveDraft, { input }),
  loadDraft: (contextKey: string) =>
    invoke<DraftRecord | null>(TauriCommand.LoadDraft, { contextKey }),
  clearDraft: (input: ClearDraftInput) => invoke<boolean>(TauriCommand.ClearDraft, { input }),
  loadShortcutSettings: () => invoke<ShortcutSettingsSnapshot>(TauriCommand.LoadShortcutSettings),
  setShortcutSettings: (input: SetShortcutSettingsInput) =>
    invoke<ShortcutSettingsSnapshot>(TauriCommand.SetShortcutSettings, { input }),
  selectImage: () => invoke<string | null>(TauriCommand.SelectImage),
  ingestSelectedImage: (selectionId: string, draftId: string) =>
    invoke<ImageRecord>(TauriCommand.IngestSelectedImage, { selectionId, draftId }),
  captureClipboardImage: () => invoke<string>(TauriCommand.CaptureClipboardImage),
  ingestClipboardImage: (captureId: string, draftId: string) =>
    invoke<ImageRecord>(TauriCommand.IngestClipboardImage, { captureId, draftId }),
  ingestDroppedImages: (dropId: string, draftId: string) =>
    invoke<ImageDropIngestResult>(TauriCommand.IngestDroppedImages, { dropId, draftId }),
  imageStatus: (attachmentId: string) =>
    invoke<ImageStatusRecord>(TauriCommand.ImageStatus, { attachmentId }),
  retryImageOcr: (attachmentId: string) =>
    invoke<ImageStatusRecord>(TauriCommand.RetryImageOcr, { attachmentId }),
  imageOcrDiagnostics: () => invoke<ImageOcrDiagnostics>(TauriCommand.ImageOcrDiagnostics),
  selectPdf: () => invoke<string | null>(TauriCommand.SelectPdf),
  ingestSelectedPdf: (selectionId: string, draftId: string) =>
    invoke<PdfRecord>(TauriCommand.IngestSelectedPdf, { selectionId, draftId }),
  selectAttachment: () => invoke<string | null>(TauriCommand.SelectAttachment),
  ingestSelectedAttachment: (selectionId: string, draftId: string) =>
    invoke<SelectedAttachmentRecord>(TauriCommand.IngestSelectedAttachment, {
      selectionId,
      draftId,
    }),
  attachmentStatus: (attachmentId: string) =>
    invoke<GenericAttachmentStatusRecord>(TauriCommand.AttachmentStatus, { attachmentId }),
  openAttachmentExternal: (attachmentId: string) =>
    invoke<void>(TauriCommand.OpenAttachmentExternal, { attachmentId }),
  revealAttachmentInFinder: (attachmentId: string) =>
    invoke<void>(TauriCommand.RevealAttachmentInFinder, { attachmentId }),
  setFileDropConsumerActive: (active: boolean) =>
    invoke<void>(TauriCommand.SetFileDropConsumerActive, { active }),
  discardFileDropSelections: (selectionIds: string[]) =>
    invoke<void>(TauriCommand.DiscardFileDropSelections, { selectionIds }),
  pdfStatus: (attachmentId: string) =>
    invoke<PdfStatusRecord>(TauriCommand.PdfStatus, { attachmentId }),
  retryPdfExtraction: (attachmentId: string) =>
    invoke<PdfStatusRecord>(TauriCommand.RetryPdfExtraction, { attachmentId }),
  openPdfExternal: (attachmentId: string) =>
    invoke<void>(TauriCommand.OpenPdfExternal, { attachmentId }),
  claudeSetupStatus: () => invoke<ClaudeSetupStatus>(TauriCommand.ClaudeSetupStatus),
  claudeCliDefaults: () => invoke<ClaudeCliDefaults>(TauriCommand.ClaudeCliDefaults),
  startResearchProcess: (input: BeginResearchProcessInput) =>
    invoke<StartResearchProcessOutput>(TauriCommand.StartResearchProcess, { input }),
  rerunResearchProcess: (runId: string) =>
    invoke<StartResearchProcessOutput>(TauriCommand.RerunResearchProcess, { runId }),
  cancelResearchProcess: (runId: string) =>
    invoke<boolean>(TauriCommand.CancelResearchProcess, { runId }),
  listResearchRuns: (input: ListResearchRunsInput) =>
    invoke<ResearchRunPage>(TauriCommand.ListResearchRuns, { input }),
  loadResearchRun: (id: string) => invoke<ResearchRunRecord>(TauriCommand.LoadResearchRun, { id }),
  saveResearchAnswerAsTidbit: (runId: string) =>
    invoke<TidbitRecord>(TauriCommand.SaveResearchAnswerAsTidbit, { runId }),
  onResearchProcessEvent: async (handler: (event: ResearchProcessEvent) => void) =>
    listen<ResearchProcessEvent>(TauriEvent.ResearchProcess, ({ payload }) => {
      handler(payload);
    }),
};

export interface RuntimeProbe {
  dataDir: string;
  nowMs: number;
  requestId: string;
  startupSmokeCanary?: string | null;
  startupSmokeCapture?: boolean;
}

export type SemanticRuntimePhase =
  | "NOT_DOWNLOADED"
  | "VERIFICATION_REQUIRED"
  | "DOWNLOADING"
  | "VERIFYING"
  | "STARTING"
  | "READY"
  | "UNAVAILABLE"
  | "FAILED";

export interface SemanticRuntimeStatus {
  phase: SemanticRuntimePhase;
  downloadedBytes: number;
  modelBytes: number;
  modelDiskUsageBytes: number;
  runtimeRunning: boolean;
  verified: boolean;
  message: string | null;
}

export interface SemanticRuntimeLogs {
  text: string;
  truncated: boolean;
}

export type BlockEmbeddingIndexPhase = "WAITING_FOR_RUNTIME" | "INDEXING" | "READY" | "FAILED";

export interface BlockEmbeddingIndexStatus {
  phase: BlockEmbeddingIndexPhase;
  embeddingIndexId: string;
  indexKey: string;
  indexedBlocks: number;
  totalBlocks: number;
  active: boolean;
  message: string | null;
}

export interface MaintenanceQueueCounts {
  pending: number;
  running: number;
  retryWait: number;
  ready: number;
  failed: number;
}

export interface MaintenanceIndexDiagnostic {
  name: string;
  version: string;
  status: string;
  error: string | null;
}

export interface MaintenanceDiagnostics {
  applicationVersion: string;
  database: {
    migrationHeads: {
      main: number | null;
      media: number | null;
    };
    mainJournalMode: string;
    mediaJournalMode: string;
    mainForeignKeys: boolean;
    mediaForeignKeys: boolean;
  };
  library: {
    activeTidbits: number;
    trashedTidbits: number;
    currentNotes: number;
    searchableBlocks: number;
    attachments: number;
    attachmentBytes: number;
    imageOcr: MaintenanceQueueCounts;
    indexes: MaintenanceIndexDiagnostic[];
  };
  storage: {
    dataRoot: string;
    mainDatabasePath: string;
    mediaDatabasePath: string;
    mainDatabaseBytes: number;
    mediaDatabaseBytes: number;
    modelBytes: number;
    logsBytes: number;
    temporaryBytes: number;
    totalBytes: number;
  };
  mediaLimits: {
    maxAttachmentBytes: number;
    maxAttachmentsPerDraft: number;
    maxProtocolResponseBytes: number;
    draftLeaseDurationMs: number;
    orphanGracePeriodMs: number;
    maxReapsPerMaintenance: number;
  };
  nativeLogs: {
    paths: string[];
    maxFileBytes: number;
    maxFiles: number;
    diskUsageBytes: number;
  };
  semanticLogPaths: string[];
  backupPhase: "AVAILABLE";
}

export type BackupCredentialState = "STORED" | "MISSING" | "UNAVAILABLE" | "INVALID";

export type BackupJurisdiction = "DEFAULT" | "EU" | "FEDRAMP";

export interface BackupConfig {
  revision: number;
  backupSetId: string;
  replicaEpochId: string;
  enabled: boolean;
  provider: "R2";
  jurisdiction: BackupJurisdiction;
  accountId: string;
  bucket: string;
  createdAtMs: number;
  updatedAtMs: number;
}

export type RelationalBackupPhase =
  | "OFF"
  | "STARTING"
  | "RUNNING"
  | "DEGRADED"
  | "WAITING_FOR_CREDENTIALS"
  | "UNAVAILABLE"
  | "BLOCKED";

export type RelationalBackupErrorCode =
  | "CREDENTIALS_MISSING"
  | "KEYCHAIN_UNAVAILABLE"
  | "BINARY_UNAVAILABLE"
  | "CONFIGURATION_INVALID"
  | "LAUNCH_FAILED"
  | "CONTROL_UNAVAILABLE"
  | "PROCESS_EXITED"
  | "REMOTE_SYNC_FAILED"
  | "REMOTE_OWNER_CONFLICT"
  | "REMOTE_OWNER_INVALID"
  | "WRITER_IDENTITY_UNAVAILABLE"
  | "WORKER_UNAVAILABLE";

export interface RelationalBackupStatus {
  phase: RelationalBackupPhase;
  latestLocalTxid: string | null;
  latestRemoteTxid: string | null;
  lastRemoteConfirmedAtMs: number | null;
  restartCount: number;
  lastErrorCode: RelationalBackupErrorCode | null;
}

export interface MediaBackupStatus {
  referenced: number;
  pending: number;
  running: number;
  retryWait: number;
  uploaded: number;
  failed: number;
  untracked: number;
  nextAttemptAtMs: number | null;
}

export type CheckpointBackupPhase =
  | "OFF"
  | "WAITING_FOR_MEDIA"
  | "FENCING"
  | "WAITING_FOR_REPLICA"
  | "VALIDATING"
  | "PUBLISHING"
  | "IDLE"
  | "DEGRADED"
  | "BLOCKED"
  | "UNAVAILABLE";

export type CheckpointBackupErrorCode =
  | "NETWORK"
  | "NETWORK_TIMEOUT"
  | "RATE_LIMITED"
  | "SERVICE_UNAVAILABLE"
  | "CREDENTIALS_MISSING"
  | "KEYCHAIN_UNAVAILABLE"
  | "INVALID_CONFIGURATION"
  | "AUTHENTICATION_REJECTED"
  | "AUTHORIZATION_REJECTED"
  | "OWNER_CONFLICT"
  | "OWNER_INVALID"
  | "IMMUTABLE_OBJECT_CONFLICT"
  | "LOCAL_MEDIA_MISSING"
  | "WORKER_UNAVAILABLE"
  | "LITESTREAM_UNAVAILABLE"
  | "FENCE_TIMEOUT"
  | "REPLICA_BEHIND"
  | "MALFORMED_MANIFEST"
  | "REMOTE_MEDIA_MISSING"
  | "REMOTE_MEDIA_CORRUPT";

export interface CheckpointBackupStatus {
  phase: CheckpointBackupPhase;
  contentRevision: number | null;
  lastPublishedContentRevision: number | null;
  lastPublishedAtMs: number | null;
  lastErrorCode: CheckpointBackupErrorCode | null;
}

export interface BackupSettingsSnapshot {
  config: BackupConfig | null;
  credentialState: BackupCredentialState;
  credentialCleanupPending: boolean;
  relational: RelationalBackupStatus;
  media: MediaBackupStatus;
  checkpoint: CheckpointBackupStatus;
  retention: {
    exactTransactionDays: number;
    checkpointPolicy: string;
    mediaPolicy: string;
  };
}

export interface ConfigureBackupInput {
  expectedRevision: number;
  backupSetId: string | null;
  accountId: string;
  jurisdiction: BackupJurisdiction;
  bucket: string;
  accessKeyId: string | null;
  secretAccessKey: string | null;
}

export interface TestBackupConnectionInput {
  backupSetId: string | null;
  accountId: string;
  jurisdiction: BackupJurisdiction;
  bucket: string;
  accessKeyId: string | null;
  secretAccessKey: string | null;
}

export interface SetBackupEnabledInput {
  expectedRevision: number;
  enabled: boolean;
}

export interface BackupConnectionTestResult {
  verified: boolean;
  cleanupComplete: boolean;
  testedAtMs: number;
}

export interface RemoteBackupCheckpoint {
  checkpointId: string;
  replicaEpochId: string;
  createdAt: string;
  koshVersion: string;
  contentRevision: number;
  referencedMediaCount: number;
  referencedMediaBytes: number;
}

export interface RestoreCheckpointInput {
  checkpointId: string;
}

export interface BackupRestoreOwner {
  backupSetId: string;
  replicaEpochId: string;
  writerId: string;
  version: string;
  isCurrentInstallation: boolean;
}

export interface BackupRestorePreview {
  checkpoint: RemoteBackupCheckpoint;
  owner: BackupRestoreOwner;
  planFileCount: number;
  planTotalBytes: number;
}

export interface BackupRestoreDrill {
  checkpointId: string;
  restoredMediaCount: number;
  restoredMediaBytes: number;
  completedAtMs: number;
}

export interface TakeOverBackupInput {
  expectedRevision: number;
  expectedOwnerBackupSetId: string;
  expectedOwnerReplicaEpochId: string;
  expectedOwnerWriterId: string;
  expectedOwnerVersion: string;
  confirmation: "TAKE OVER";
}

export interface MediaIntegrityReport {
  missingBlobAttachmentIds: string[];
  corruptBlobSha256: string[];
  extraBlobSha256: string[];
  orphanedAttachmentIds: string[];
  diagnosticsTruncated: boolean;
}

export interface IntegrityCheckOutcome {
  databaseOk: boolean;
  media: MediaIntegrityReport;
  message: string;
  completedAtMs: number;
}

export type MaintenanceOperation =
  | "REBUILD_SEARCH"
  | "REBUILD_EMBEDDINGS"
  | "RETRY_EXTRACTIONS"
  | "RECLAIM_MEDIA";

export interface MaintenanceOutcome {
  operation: MaintenanceOperation;
  changedItems: number;
  reclaimedBytes: number;
  safetySnapshotId: string | null;
  message: string;
  completedAtMs: number;
}

export interface SourceDraft {
  label: string | null;
  url: string | null;
}

export interface DeleteTidbitInput {
  id: string;
  expectedContentVersionId: string;
}

export interface RestoreTidbitInput {
  id: string;
  expectedContentVersionId: string;
}

export type LexicalSearchMode = "DEFAULT" | "EXACT";

export type SearchField = "HEADING_CONTEXT" | "BODY" | "ATTACHMENT_NAME" | "EXTRACTED_TEXT";

export interface SearchBlocksInput {
  query: string;
  mode: LexicalSearchMode;
  limit: number;
}

export interface SearchHighlight {
  field: SearchField;
  startChar: number;
  endChar: number;
}

export interface BlockSearchResult {
  noteId: string;
  blockId: string;
  blockType: string;
  blockOrdinal: number;
  displayTitle: string;
  headingContext: string[];
  excerpt: string;
  attachmentNames: string[];
  score: number;
  matchedFields: SearchField[];
  highlights: SearchHighlight[];
}

export type SearchExecutionMode = "EXACT" | "HYBRID" | "LEXICAL_ONLY";

export type SemanticSearchReadiness =
  | "READY"
  | "INDEXING"
  | "WAITING_FOR_RUNTIME"
  | "FAILED"
  | "NOT_REQUESTED";

export interface SearchBlocksResponse {
  results: BlockSearchResult[];
  executionMode: SearchExecutionMode;
  semanticReadiness: SemanticSearchReadiness;
}

export const KoshCommand = {
  MainWindow: "MAIN_WINDOW",
  QuickAdd: "QUICK_ADD",
} as const;

export type KoshCommand = (typeof KoshCommand)[keyof typeof KoshCommand];

export interface KeyboardBinding {
  command: KoshCommand;
  accelerator: string;
}

export interface ShortcutSettingsSnapshot {
  revision: number;
  automaticUpdateChecksEnabled: boolean;
  keyboardBindings: KeyboardBinding[];
  shortcutErrors: string[];
}

export interface SetAutomaticUpdateChecksInput {
  expectedRevision: number;
  enabled: boolean;
}

export interface SetShortcutSettingsInput {
  expectedRevision: number;
  keyboardBindings: KeyboardBinding[];
}

export const DEFAULT_QUICK_ADD_ACCELERATOR = "control+alt+super+KeyK";
export const DEFAULT_MAIN_WINDOW_ACCELERATOR = "control+alt+super+KeyO";
export const DEFAULT_KEYBOARD_BINDINGS: readonly KeyboardBinding[] = [
  {
    accelerator: DEFAULT_QUICK_ADD_ACCELERATOR,
    command: KoshCommand.QuickAdd,
  },
  {
    accelerator: DEFAULT_MAIN_WINDOW_ACCELERATOR,
    command: KoshCommand.MainWindow,
  },
];

export interface SaveWorkingCopyInput {
  noteId: string;
  baseContentVersionId: string | null;
  editGeneration: number;
  documentJson: string;
  bodyMarkdown: string;
  sources: SourceDraft[];
}

export interface WorkingCopyRecord extends SaveWorkingCopyInput {
  id: string;
  mediaReservation: boolean;
  createdAtMs: number;
  updatedAtMs: number;
}

export type WorkingCopySaveStatus = "SAVED" | "CLEARED" | "STALE";

export interface WorkingCopySaveResult {
  status: WorkingCopySaveStatus;
  acceptedEditGeneration: number;
  workingCopy: WorkingCopyRecord | null;
}

export interface CheckpointWorkingCopyInput {
  noteId: string;
  expectedEditGeneration: number;
}

export interface DiscardWorkingCopyInput {
  noteId: string;
  expectedEditGeneration: number;
}

export type WorkingCopyCheckpointStatus = "CHECKPOINTED" | "STALE";

export interface WorkingCopyCheckpointResult {
  status: WorkingCopyCheckpointStatus;
  consumedEditGeneration: number | null;
  note: TidbitRecord | null;
  workingCopy: WorkingCopyRecord | null;
}

export type ImageOcrStatus = "PENDING" | "RUNNING" | "RETRY_WAIT" | "READY" | "FAILED";

export interface ImageRecord {
  id: string;
  ingestLeaseId: string;
  displayFilename: string;
  mediaType: string;
  byteLength: number;
  kind: "IMAGE";
  naturalWidth: number;
  naturalHeight: number;
  ocrStatus: ImageOcrStatus;
  ocrError: string | null;
}

export interface ImageStatusRecord {
  attachmentId: string;
  naturalWidth: number;
  naturalHeight: number;
  ocrStatus: ImageOcrStatus;
  ocrError: string | null;
  nextAttemptAtMs: number | null;
}

export interface ImageDropIngestResult {
  images: ImageRecord[];
  failures: Array<{
    filename: string;
    message: string;
  }>;
}

export interface ImageOcrDiagnostics {
  pending: number;
  running: number;
  retryWait: number;
  ready: number;
  failed: number;
  oldestEligibleAtMs: number | null;
  lastError: string | null;
}

export interface FileAttachmentRecord {
  id: string;
  ingestLeaseId: string;
  displayFilename: string;
  mediaType: string;
  byteLength: number;
  kind: "FILE";
}

export type SelectedAttachmentRecord =
  | { recordKind: "IMAGE"; record: ImageRecord }
  | { recordKind: "FILE"; record: FileAttachmentRecord };

export interface TidbitSource {
  id: string;
  label: string | null;
  url: string | null;
}

export interface TidbitRecord {
  id: string;
  contentVersionId: string;
  versionNumber: number;
  createdAtMs: number;
  updatedAtMs: number;
  deletedAtMs: number | null;
  displayTitle: string;
  documentJson: string;
  bodyMarkdown: string;
  sources: TidbitSource[];
}

export interface Backend {
  runtimeProbe(): Promise<RuntimeProbe>;
  loadBackupSettings(): Promise<BackupSettingsSnapshot>;
  testBackupConnection(input: TestBackupConnectionInput): Promise<BackupConnectionTestResult>;
  configureBackup(input: ConfigureBackupInput): Promise<BackupSettingsSnapshot>;
  setBackupEnabled(input: SetBackupEnabledInput): Promise<BackupSettingsSnapshot>;
  backupNow(): Promise<void>;
  listBackupCheckpoints(): Promise<RemoteBackupCheckpoint[]>;
  previewBackupRestore(input: RestoreCheckpointInput): Promise<BackupRestorePreview>;
  drillBackupRestore(input: RestoreCheckpointInput): Promise<BackupRestoreDrill>;
  takeOverBackup(input: TakeOverBackupInput): Promise<BackupSettingsSnapshot>;
  semanticRuntimeStatus(): Promise<SemanticRuntimeStatus>;
  prepareSemanticRuntime(): Promise<SemanticRuntimeStatus>;
  retrySemanticRuntime(): Promise<SemanticRuntimeStatus>;
  repairSemanticRuntime(): Promise<SemanticRuntimeStatus>;
  semanticRuntimeLogs(): Promise<SemanticRuntimeLogs>;
  blockEmbeddingIndexStatus(): Promise<BlockEmbeddingIndexStatus>;
  loadMaintenanceDiagnostics(): Promise<MaintenanceDiagnostics>;
  runIntegrityCheck(): Promise<IntegrityCheckOutcome>;
  rebuildSearchIndexes(): Promise<MaintenanceOutcome>;
  rebuildEmbeddingIndex(): Promise<MaintenanceOutcome>;
  retryFailedExtractions(): Promise<MaintenanceOutcome>;
  reclaimEligibleMedia(): Promise<MaintenanceOutcome>;
  loadTidbit(id: string): Promise<TidbitRecord>;
  deleteTidbit(input: DeleteTidbitInput): Promise<TidbitRecord>;
  restoreTidbit(input: RestoreTidbitInput): Promise<TidbitRecord>;
  openSourceUrl(sourceId: string): Promise<void>;
  searchBlocks(input: SearchBlocksInput): Promise<SearchBlocksResponse>;
  saveWorkingCopy(input: SaveWorkingCopyInput): Promise<WorkingCopySaveResult>;
  reserveWorkingCopyForMedia(input: SaveWorkingCopyInput): Promise<WorkingCopySaveResult>;
  discardWorkingCopy(input: DiscardWorkingCopyInput): Promise<boolean>;
  loadWorkingCopy(noteId: string): Promise<WorkingCopyRecord | null>;
  listWorkingCopies(): Promise<WorkingCopyRecord[]>;
  checkpointWorkingCopy(input: CheckpointWorkingCopyInput): Promise<WorkingCopyCheckpointResult>;
  loadShortcutSettings(): Promise<ShortcutSettingsSnapshot>;
  setAutomaticUpdateChecks(input: SetAutomaticUpdateChecksInput): Promise<ShortcutSettingsSnapshot>;
  setShortcutSettings(input: SetShortcutSettingsInput): Promise<ShortcutSettingsSnapshot>;
  selectImage(): Promise<string | null>;
  ingestSelectedImage(selectionId: string, draftId: string): Promise<ImageRecord>;
  captureClipboardImage(): Promise<string>;
  ingestClipboardImage(captureId: string, draftId: string): Promise<ImageRecord>;
  ingestDroppedImages(dropId: string, draftId: string): Promise<ImageDropIngestResult>;
  imageStatus(attachmentId: string): Promise<ImageStatusRecord>;
  retryImageOcr(attachmentId: string): Promise<ImageStatusRecord>;
  imageOcrDiagnostics(): Promise<ImageOcrDiagnostics>;
  selectAttachment(): Promise<string | null>;
  ingestSelectedAttachment(selectionId: string, draftId: string): Promise<SelectedAttachmentRecord>;
  openAttachmentExternal(attachmentId: string): Promise<void>;
  revealAttachmentInFinder(attachmentId: string): Promise<void>;
  setFileDropConsumerActive(active: boolean): Promise<void>;
  discardFileDropSelections(selectionIds: string[]): Promise<void>;
}

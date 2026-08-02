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

export type PassageEmbeddingIndexPhase = "WAITING_FOR_RUNTIME" | "INDEXING" | "READY" | "FAILED";

export interface PassageEmbeddingIndexStatus {
  phase: PassageEmbeddingIndexPhase;
  embeddingIndexId: string;
  indexKey: string;
  indexedPassages: number;
  totalPassages: number;
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

export interface MaintenanceResearchCounts {
  queued: number;
  running: number;
  completed: number;
  canceled: number;
  failed: number;
  interrupted: number;
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
    revisions: number;
    authoredPassages: number;
    attachmentPassages: number;
    searchDocuments: number;
    attachments: number;
    attachmentBytes: number;
    imageOcr: MaintenanceQueueCounts;
    pdfExtraction: MaintenanceQueueCounts;
    research: MaintenanceResearchCounts;
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

export interface TidbitDraft {
  title: string | null;
  bodyMarkdown: string;
  sources: SourceDraft[];
}

export interface EditTidbitInput extends TidbitDraft {
  id: string;
  expectedRevisionId: string;
}

export interface DeleteTidbitInput {
  id: string;
  expectedRevisionId: string;
}

export interface RestoreTidbitInput {
  id: string;
  expectedRevisionId: string;
}

export interface PurgeTidbitInput {
  id: string;
  expectedRevisionId: string;
}

export type CitationState = "CURRENT" | "HISTORICAL";

export type CitationLocator =
  | {
      kind: "MARKDOWN_BLOCKS";
      startBlock: number;
      endBlock: number;
      sourceStartByte: number | null;
      sourceEndByte: number | null;
      startChar: number | null;
      endChar: number | null;
      startLine: number | null;
      endLine: number | null;
    }
  | {
      kind: "PDF_PAGE";
      page: number;
    }
  | {
      kind: "OCR_REGION";
      page: number | null;
      region: unknown;
    }
  | {
      kind: "TEXT_LINES";
      startLine: number;
      endLine: number;
    };

export interface CitationTidbit {
  id: string;
  revisionId: string;
  revisionNumber: number;
  title: string | null;
  displayTitle: string;
  deleted: boolean;
}

export interface CitationAttachment {
  id: string;
  extractionId: string;
  displayFilename: string;
  mediaType: string;
  deleted: boolean;
}

export interface CitationResolution {
  passageId: string;
  excerpt: string;
  headingContext: string[];
  constructionVersion: string;
  state: CitationState;
  locator: CitationLocator;
  tidbit: CitationTidbit | null;
  attachment: CitationAttachment | null;
  sources: TidbitSource[];
}

export type LexicalSearchMode = "DEFAULT" | "EXACT";

export type SearchField =
  | "TITLE"
  | "HEADING_CONTEXT"
  | "BODY"
  | "SOURCE_LABEL"
  | "SOURCE_DOMAIN"
  | "ATTACHMENT_NAME"
  | "EXTRACTED_TEXT";

export interface SearchPassagesInput {
  query: string;
  mode: LexicalSearchMode;
  limit: number;
}

export interface SearchHighlight {
  field: SearchField;
  startChar: number;
  endChar: number;
}

export interface PassageSearchResult {
  passageId: string;
  score: number;
  matchedFields: SearchField[];
  highlights: SearchHighlight[];
  citation: CitationResolution;
}

export type SearchExecutionMode = "EXACT" | "HYBRID" | "LEXICAL_ONLY";

export type SemanticSearchReadiness =
  | "READY"
  | "INDEXING"
  | "WAITING_FOR_RUNTIME"
  | "FAILED"
  | "NOT_REQUESTED";

export interface SearchPassagesResponse {
  results: PassageSearchResult[];
  executionMode: SearchExecutionMode;
  semanticReadiness: SemanticSearchReadiness;
}

export interface SaveDraftInput extends TidbitDraft {
  contextKey: string;
  tidbitId: string | null;
  baseRevisionId: string | null;
}

export interface ClearDraftInput {
  contextKey: string;
  expectedUpdatedAtMs: number;
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

export interface DraftRecord extends SaveDraftInput {
  id: string;
  createdAtMs: number;
  updatedAtMs: number;
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

export type PdfExtractionStatus = "PENDING" | "RUNNING" | "RETRY_WAIT" | "READY" | "FAILED";

export interface PdfRecord {
  id: string;
  ingestLeaseId: string;
  displayFilename: string;
  mediaType: "application/pdf";
  byteLength: number;
  kind: "PDF";
  pageCount: number;
  extractionStatus: PdfExtractionStatus;
  extractionError: string | null;
}

export interface PdfStatusRecord {
  attachmentId: string;
  displayFilename: string;
  pageCount: number;
  extractedPageCount: number;
  unavailablePageCount: number;
  extractionStatus: PdfExtractionStatus;
  extractionError: string | null;
  nextAttemptAtMs: number | null;
}

export type AttachmentExtractionStatus = "READY" | "FAILED" | "NOT_APPLICABLE";

export interface GenericAttachmentRecord {
  id: string;
  ingestLeaseId: string;
  displayFilename: string;
  mediaType: string;
  byteLength: number;
  kind: "TEXT" | "BINARY";
  extractionStatus: AttachmentExtractionStatus;
  extractionError: string | null;
  extractedLineCount: number;
}

export interface GenericAttachmentStatusRecord {
  attachmentId: string;
  displayFilename: string;
  mediaType: string;
  byteLength: number;
  kind: "TEXT" | "BINARY";
  extractionStatus: AttachmentExtractionStatus;
  extractionError: string | null;
  extractedLineCount: number;
}

export type SelectedAttachmentRecord =
  | { recordKind: "IMAGE"; record: ImageRecord }
  | { recordKind: "PDF"; record: PdfRecord }
  | { recordKind: "GENERIC"; record: GenericAttachmentRecord };

export interface TidbitSource {
  id: string;
  label: string | null;
  url: string | null;
}

export interface TidbitRecord {
  id: string;
  currentRevisionId: string;
  revisionNumber: number;
  createdAtMs: number;
  updatedAtMs: number;
  deletedAtMs: number | null;
  title: string | null;
  displayTitle: string;
  bodyMarkdown: string;
  sources: TidbitSource[];
}

export interface TidbitListCursor {
  updatedAtMs: number;
  id: string;
}

export interface ListTidbitsInput {
  limit: number;
  cursor: TidbitListCursor | null;
  scope: "ACTIVE" | "DELETED";
}

export interface TidbitListItem {
  id: string;
  currentRevisionId: string;
  createdAtMs: number;
  updatedAtMs: number;
  deletedAtMs: number | null;
  purgeEligibleAtMs: number | null;
  title: string | null;
  displayTitle: string;
  bodyPreview: string;
}

export interface TidbitListPage {
  items: TidbitListItem[];
  nextCursor: TidbitListCursor | null;
}

export interface ListTidbitRevisionsInput {
  tidbitId: string;
  limit: number;
  beforeRevisionNumber: number | null;
}

export interface TidbitRevisionSummary {
  id: string;
  revisionNumber: number;
  createdAtMs: number;
  title: string | null;
  displayTitle: string;
  bodyPreview: string;
  sourceCount: number;
  attachmentCount: number;
  isCurrent: boolean;
}

export interface TidbitRevisionPage {
  items: TidbitRevisionSummary[];
  nextBeforeRevisionNumber: number | null;
}

export interface TidbitRevisionAttachment {
  id: string;
  displayFilename: string;
  mediaType: string;
  byteLength: number;
  kind: "IMAGE" | "PDF" | "TEXT" | "BINARY";
  extractionState: "PENDING" | "READY" | "FAILED" | "NOT_APPLICABLE";
  displayRole: "INLINE" | "ATTACHMENT";
  sortOrder: number;
  deletedAtMs: number | null;
}

export interface TidbitRevisionRecord {
  id: string;
  tidbitId: string;
  revisionNumber: number;
  createdAtMs: number;
  title: string | null;
  displayTitle: string;
  bodyMarkdown: string;
  sources: TidbitSource[];
  attachments: TidbitRevisionAttachment[];
  isCurrent: boolean;
  tidbitDeleted: boolean;
}

export const TIDBIT_PURGE_DELAY_MS = 30 * 24 * 60 * 60 * 1_000;

export type ClaudeSetupPhase = "READY" | "MISSING" | "UNAUTHENTICATED" | "UNAVAILABLE";

export interface ClaudeCliDefaults {
  model: string | null;
  effort: string | null;
}

export interface ClaudeSetupStatus {
  phase: ClaudeSetupPhase;
  binaryPath: string | null;
  version: string | null;
  defaults: ClaudeCliDefaults;
  message: string;
}

export interface BeginResearchProcessInput {
  prompt: string;
  model: string | null;
  effort: string | null;
  timeoutSeconds: number | null;
}

export interface StartResearchProcessOutput {
  runId: string;
  replacedRunId?: string;
}

export type ResearchRunStatus =
  | "QUEUED"
  | "RUNNING"
  | "COMPLETED"
  | "CANCELED"
  | "FAILED"
  | "INTERRUPTED";

export type ResearchProcessOutcome =
  | "SUCCEEDED"
  | "FAILED"
  | "CANCELED"
  | "REPLACED"
  | "TIMED_OUT"
  | "SHUTDOWN";

export type ResearchProcessEvent =
  | { runId: string; sequence: number; kind: "STARTED" }
  | { runId: string; sequence: number; kind: "METADATA"; model?: string }
  | { runId: string; sequence: number; kind: "UNTRUSTED_TEXT_DELTA"; text: string }
  | {
      runId: string;
      sequence: number;
      kind: "TOOL_ACTIVITY";
      tool: string;
      phase: "STARTED" | "FINISHED";
    }
  | {
      runId: string;
      sequence: number;
      kind: "GROUNDED_FINAL_OUTPUT";
      answer: GroundedResearchAnswer;
    }
  | {
      runId: string;
      sequence: number;
      kind: "FINISHED";
      outcome: ResearchProcessOutcome;
      error?: string;
      stderrTruncated: boolean;
    };

export type GroundedEvidenceKind = "AUTHORED_TIDBIT" | "PDF_PAGE" | "IMAGE_OCR" | "TEXT_LINES";

export interface GroundedResearchCitation {
  number: number;
  label: string;
  evidenceKind: GroundedEvidenceKind;
  evidence: CitationResolution;
}

export interface GroundedCitationMention {
  citationNumber: number;
  startByte: number;
  endByte: number;
}

export interface GroundedOutputIssue {
  code:
    | "UNKNOWN_CITATION"
    | "MALFORMED_CITATION"
    | "CITATION_IN_CODE"
    | "UNCITED_PARAGRAPH"
    | "CITATION_LIMIT_EXCEEDED";
  startByte: number;
  endByte: number;
  message: string;
}

export interface GroundedResearchAnswer {
  markdown: string;
  citations: GroundedResearchCitation[];
  mentions: GroundedCitationMention[];
  issues: GroundedOutputIssue[];
}

export interface ResearchRunCursor {
  updatedAtMs: number;
  id: string;
}

export interface ListResearchRunsInput {
  limit: number;
  cursor: ResearchRunCursor | null;
}

export interface ResearchRunSummary {
  id: string;
  rerunOfId: string | null;
  query: string;
  status: ResearchRunStatus;
  requestedModel: string | null;
  requestedEffort: string | null;
  actualModel: string | null;
  createdAtMs: number;
  startedAtMs: number | null;
  completedAtMs: number | null;
  updatedAtMs: number;
  error: string | null;
  stderrTruncated: boolean;
  savedTidbitId: string | null;
}

export interface ResearchCitationFreshness {
  citationNumber: number;
  citedRevisionId: string | null;
  currentRevisionId: string | null;
  hasNewerRevision: boolean;
  isHistorical: boolean;
  tidbitDeleted: boolean;
}

export interface ResearchRunRecord extends ResearchRunSummary {
  events: ResearchProcessEvent[];
  finalAnswer: GroundedResearchAnswer | null;
  citationFreshness: ResearchCitationFreshness[];
}

export interface ResearchRunPage {
  items: ResearchRunSummary[];
  nextCursor: ResearchRunCursor | null;
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
  passageEmbeddingIndexStatus(): Promise<PassageEmbeddingIndexStatus>;
  loadMaintenanceDiagnostics(): Promise<MaintenanceDiagnostics>;
  runIntegrityCheck(): Promise<IntegrityCheckOutcome>;
  rebuildSearchIndexes(): Promise<MaintenanceOutcome>;
  rebuildEmbeddingIndex(): Promise<MaintenanceOutcome>;
  retryFailedExtractions(): Promise<MaintenanceOutcome>;
  reclaimEligibleMedia(): Promise<MaintenanceOutcome>;
  createTidbit(input: TidbitDraft): Promise<TidbitRecord>;
  loadTidbit(id: string): Promise<TidbitRecord>;
  listTidbits(input: ListTidbitsInput): Promise<TidbitListPage>;
  listTidbitRevisions(input: ListTidbitRevisionsInput): Promise<TidbitRevisionPage>;
  loadTidbitRevision(tidbitId: string, revisionId: string): Promise<TidbitRevisionRecord>;
  editTidbit(input: EditTidbitInput): Promise<TidbitRecord>;
  deleteTidbit(input: DeleteTidbitInput): Promise<TidbitRecord>;
  restoreTidbit(input: RestoreTidbitInput): Promise<TidbitRecord>;
  purgeTidbit(input: PurgeTidbitInput): Promise<boolean>;
  openSourceUrl(sourceId: string): Promise<void>;
  resolveCitation(passageId: string): Promise<CitationResolution>;
  searchPassages(input: SearchPassagesInput): Promise<SearchPassagesResponse>;
  saveDraft(input: SaveDraftInput): Promise<DraftRecord>;
  loadDraft(contextKey: string): Promise<DraftRecord | null>;
  clearDraft(input: ClearDraftInput): Promise<boolean>;
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
  selectPdf(): Promise<string | null>;
  ingestSelectedPdf(selectionId: string, draftId: string): Promise<PdfRecord>;
  selectAttachment(): Promise<string | null>;
  ingestSelectedAttachment(selectionId: string, draftId: string): Promise<SelectedAttachmentRecord>;
  attachmentStatus(attachmentId: string): Promise<GenericAttachmentStatusRecord>;
  openAttachmentExternal(attachmentId: string): Promise<void>;
  revealAttachmentInFinder(attachmentId: string): Promise<void>;
  setFileDropConsumerActive(active: boolean): Promise<void>;
  discardFileDropSelections(selectionIds: string[]): Promise<void>;
  pdfStatus(attachmentId: string): Promise<PdfStatusRecord>;
  retryPdfExtraction(attachmentId: string): Promise<PdfStatusRecord>;
  openPdfExternal(attachmentId: string): Promise<void>;
  claudeSetupStatus(): Promise<ClaudeSetupStatus>;
  claudeCliDefaults(): Promise<ClaudeCliDefaults>;
  startResearchProcess(input: BeginResearchProcessInput): Promise<StartResearchProcessOutput>;
  rerunResearchProcess(runId: string): Promise<StartResearchProcessOutput>;
  cancelResearchProcess(runId: string): Promise<boolean>;
  listResearchRuns(input: ListResearchRunsInput): Promise<ResearchRunPage>;
  loadResearchRun(id: string): Promise<ResearchRunRecord>;
  saveResearchAnswerAsTidbit(runId: string): Promise<TidbitRecord>;
  onResearchProcessEvent(handler: (event: ResearchProcessEvent) => void): Promise<() => void>;
}

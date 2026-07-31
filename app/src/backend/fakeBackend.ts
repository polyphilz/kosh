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
  GroundedResearchAnswer,
  ResearchProcessEvent,
  ResearchRunPage,
  ResearchRunRecord,
  RemoteBackupCheckpoint,
  SelectedAttachmentRecord,
  RestoreTidbitInput,
  PurgeTidbitInput,
  SaveDraftInput,
  SearchField,
  SearchPassagesInput,
  SearchPassagesResponse,
  SemanticRuntimeLogs,
  SemanticRuntimeStatus,
  SetBackupEnabledInput,
  SetShortcutSettingsInput,
  ShortcutSettingsSnapshot,
  SourceDraft,
  TidbitDraft,
  TidbitListPage,
  TidbitRecord,
  TidbitRevisionPage,
  TidbitRevisionRecord,
  TidbitSource,
  StartResearchProcessOutput,
  TakeOverBackupInput,
  TestBackupConnectionInput,
  RestoreCheckpointInput,
} from "./contracts";
import { DEFAULT_KEYBOARD_BINDINGS } from "./contracts";
import { TIDBIT_PURGE_DELAY_MS } from "./contracts";
import { neutralizeUntrustedMediaReferences } from "../markdown/mediaTokens";

interface FakeCitationSnapshot {
  revision: TidbitRecord;
}

export const browserRuntimeProbe: RuntimeProbe = {
  dataDir: "/tmp/kosh-browser-fixture",
  nowMs: 1_785_201_600_000,
  requestId: "fixture-request-1",
  startupSmokeCanary: null,
  startupSmokeCapture: false,
};

const FAKE_BACKUP_OWNER_WRITER_ID = "fixture-current-installation-writer";
const FAKE_BACKUP_OWNER_VERSION = '"fixture-owner-v1"';

export class FakeBackend implements Backend {
  private readonly probe: RuntimeProbe;
  private semanticStatus: SemanticRuntimeStatus = {
    phase: "NOT_DOWNLOADED",
    downloadedBytes: 0,
    modelBytes: 232_883_776,
    modelDiskUsageBytes: 0,
    runtimeRunning: false,
    verified: false,
    message: null,
  };
  private readonly drafts = new Map<string, DraftRecord>();
  private readonly revisionOwners = new Map<string, string>();
  private readonly citations = new Map<string, FakeCitationSnapshot>();
  private readonly sourceIds = new Map<string, string>();
  private readonly tidbits = new Map<string, TidbitRecord>();
  private readonly revisions = new Map<string, TidbitRevisionRecord>();
  private readonly researchRuns = new Map<string, ResearchRunRecord>();
  private readonly researchListeners = new Set<(event: ResearchProcessEvent) => void>();
  private readonly researchTimers = new Map<string, ReturnType<typeof setTimeout>>();
  private shortcutSettings: ShortcutSettingsSnapshot = {
    revision: 1,
    keyboardBindings: DEFAULT_KEYBOARD_BINDINGS.map((binding) => ({ ...binding })),
    shortcutErrors: [],
  };
  private backupSettings: BackupSettingsSnapshot = {
    config: null,
    credentialState: "MISSING",
    credentialCleanupPending: false,
    relational: {
      phase: "OFF",
      latestLocalTxid: null,
      latestRemoteTxid: null,
      lastRemoteConfirmedAtMs: null,
      restartCount: 0,
      lastErrorCode: null,
    },
    media: {
      referenced: 0,
      pending: 0,
      running: 0,
      retryWait: 0,
      uploaded: 0,
      failed: 0,
      untracked: 0,
      nextAttemptAtMs: null,
    },
    checkpoint: {
      phase: "OFF",
      contentRevision: null,
      lastPublishedContentRevision: null,
      lastPublishedAtMs: null,
      lastErrorCode: null,
    },
    retention: {
      exactTransactionDays: 30,
      checkpointPolicy:
        "Complete checkpoint manifests are immutable and are not automatically deleted in v1.",
      mediaPolicy: "Content-addressed media is immutable and is not automatically deleted in v1.",
    },
  };
  private backupCheckpoints: RemoteBackupCheckpoint[] = [];
  private sequence = 0;

  constructor(probe: RuntimeProbe = browserRuntimeProbe, tidbits: TidbitRecord[] = []) {
    this.probe = probe;
    for (const tidbit of tidbits) {
      this.tidbits.set(tidbit.id, cloneTidbit(tidbit));
      this.revisions.set(tidbit.currentRevisionId, revisionFromTidbit(tidbit));
      this.revisionOwners.set(tidbit.currentRevisionId, tidbit.id);
      this.registerCitation(tidbit);
      this.sequence = Math.max(
        this.sequence,
        generatedIdSequence(tidbit.id),
        generatedIdSequence(tidbit.currentRevisionId),
        ...tidbit.sources.map((source) => generatedIdSequence(source.id)),
      );
      for (const source of tidbit.sources) {
        const identity = sourceIdentity(source);
        if (!this.sourceIds.has(identity)) {
          this.sourceIds.set(identity, source.id);
        }
      }
    }
  }

  async runtimeProbe(): Promise<RuntimeProbe> {
    return { ...this.probe };
  }

  async loadBackupSettings(): Promise<BackupSettingsSnapshot> {
    return cloneBackupSettings(this.backupSettings);
  }

  async testBackupConnection(
    input: TestBackupConnectionInput,
  ): Promise<BackupConnectionTestResult> {
    validateFakeBackupTarget(input);
    validateFakeCredentials(input, this.backupSettings.credentialState === "STORED");
    return {
      verified: true,
      cleanupComplete: true,
      testedAtMs: this.probe.nowMs,
    };
  }

  async configureBackup(input: ConfigureBackupInput): Promise<BackupSettingsSnapshot> {
    validateFakeBackupTarget(input);
    if (input.expectedRevision !== (this.backupSettings.config?.revision ?? 0)) {
      throw new Error("Backup settings changed. Refresh and try again.");
    }
    validateFakeCredentials(input, this.backupSettings.credentialState === "STORED");
    const previous = this.backupSettings.config;
    const backupSetId =
      input.backupSetId ?? previous?.backupSetId ?? "019f547b-6200-7000-8000-000000000b01";
    const sameSet = previous?.backupSetId === backupSetId;
    this.backupSettings = {
      ...this.backupSettings,
      config: {
        revision: input.expectedRevision + 1,
        backupSetId,
        replicaEpochId:
          sameSet && previous ? previous.replicaEpochId : "019f547b-6200-7000-8000-000000000e01",
        enabled: false,
        provider: "R2",
        jurisdiction: input.jurisdiction,
        accountId: input.accountId,
        bucket: input.bucket,
        createdAtMs: previous?.createdAtMs ?? this.probe.nowMs,
        updatedAtMs: this.probe.nowMs,
      },
      credentialState: "STORED",
      relational: {
        ...this.backupSettings.relational,
        phase: "OFF",
        lastErrorCode: null,
      },
      checkpoint: {
        ...this.backupSettings.checkpoint,
        phase: "OFF",
        lastErrorCode: null,
      },
    };
    if (!sameSet) this.backupCheckpoints = [];
    return cloneBackupSettings(this.backupSettings);
  }

  async setBackupEnabled(input: SetBackupEnabledInput): Promise<BackupSettingsSnapshot> {
    const config = this.backupSettings.config;
    if (!config) throw new Error("Set up an R2 recovery target first.");
    if (input.expectedRevision !== config.revision) {
      throw new Error("Backup settings changed. Refresh and try again.");
    }
    if (input.enabled && this.backupSettings.credentialState !== "STORED") {
      throw new Error("R2 credentials are not stored for this backup set.");
    }
    this.backupSettings = {
      ...this.backupSettings,
      config: {
        ...config,
        revision: config.revision + 1,
        enabled: input.enabled,
        updatedAtMs: this.probe.nowMs,
      },
      relational: {
        ...this.backupSettings.relational,
        phase: input.enabled ? "RUNNING" : "OFF",
        lastErrorCode: null,
      },
      checkpoint: {
        ...this.backupSettings.checkpoint,
        phase: input.enabled ? "IDLE" : "OFF",
        lastErrorCode: null,
      },
    };
    return cloneBackupSettings(this.backupSettings);
  }

  async backupNow(): Promise<void> {
    const config = this.backupSettings.config;
    if (!config?.enabled) throw new Error("Turn on backup before creating a recovery point.");
    const checkpoint: RemoteBackupCheckpoint = {
      checkpointId: `019f547b-6200-7000-8000-${(this.backupCheckpoints.length + 1)
        .toString()
        .padStart(12, "0")}`,
      replicaEpochId: config.replicaEpochId,
      createdAt: "2026-07-27T18:00:00Z",
      koshVersion: "0.1.0-fixture",
      contentRevision: this.backupCheckpoints.length + 1,
      referencedMediaCount: 0,
      referencedMediaBytes: 0,
    };
    this.backupCheckpoints = [checkpoint, ...this.backupCheckpoints];
    this.backupSettings = {
      ...this.backupSettings,
      relational: {
        ...this.backupSettings.relational,
        latestLocalTxid: "0000000000000001",
        latestRemoteTxid: "0000000000000001",
        lastRemoteConfirmedAtMs: this.probe.nowMs,
      },
      checkpoint: {
        phase: "IDLE",
        contentRevision: checkpoint.contentRevision,
        lastPublishedContentRevision: checkpoint.contentRevision,
        lastPublishedAtMs: this.probe.nowMs,
        lastErrorCode: null,
      },
    };
  }

  async listBackupCheckpoints(): Promise<RemoteBackupCheckpoint[]> {
    if (!this.backupSettings.config) throw new Error("Set up an R2 recovery target first.");
    return this.backupCheckpoints.map((checkpoint) => ({ ...checkpoint }));
  }

  async previewBackupRestore(input: RestoreCheckpointInput): Promise<BackupRestorePreview> {
    const config = this.backupSettings.config;
    if (!config) throw new Error("Set up an R2 recovery target first.");
    const checkpoint = this.backupCheckpoints.find(
      (candidate) => candidate.checkpointId === input.checkpointId,
    );
    if (!checkpoint) throw new Error("That recovery point is no longer available.");
    return {
      checkpoint: { ...checkpoint },
      owner: {
        backupSetId: config.backupSetId,
        replicaEpochId: config.replicaEpochId,
        writerId: FAKE_BACKUP_OWNER_WRITER_ID,
        version: FAKE_BACKUP_OWNER_VERSION,
        isCurrentInstallation: true,
      },
      planFileCount: 3,
      planTotalBytes: 12_288,
    };
  }

  async drillBackupRestore(input: RestoreCheckpointInput): Promise<BackupRestoreDrill> {
    const preview = await this.previewBackupRestore(input);
    return {
      checkpointId: preview.checkpoint.checkpointId,
      restoredMediaCount: preview.checkpoint.referencedMediaCount,
      restoredMediaBytes: preview.checkpoint.referencedMediaBytes,
      completedAtMs: this.probe.nowMs,
    };
  }

  async takeOverBackup(input: TakeOverBackupInput): Promise<BackupSettingsSnapshot> {
    const config = this.backupSettings.config;
    if (!config) throw new Error("Set up an R2 recovery target first.");
    if (
      config.enabled ||
      this.backupSettings.relational.phase !== "OFF" ||
      this.backupSettings.checkpoint.phase !== "OFF"
    ) {
      throw new Error("Turn off backup before takeover.");
    }
    if (
      input.confirmation !== "TAKE OVER" ||
      input.expectedRevision !== config.revision ||
      input.expectedOwnerBackupSetId !== config.backupSetId ||
      input.expectedOwnerReplicaEpochId !== config.replicaEpochId ||
      input.expectedOwnerWriterId !== FAKE_BACKUP_OWNER_WRITER_ID ||
      input.expectedOwnerVersion !== FAKE_BACKUP_OWNER_VERSION
    ) {
      throw new Error("The remote owner changed after preview.");
    }
    this.backupSettings = {
      ...this.backupSettings,
      config: {
        ...config,
        revision: config.revision + 1,
        replicaEpochId: "019f547b-6200-7000-8000-000000000e02",
        updatedAtMs: this.probe.nowMs,
      },
    };
    return cloneBackupSettings(this.backupSettings);
  }

  async semanticRuntimeStatus(): Promise<SemanticRuntimeStatus> {
    return { ...this.semanticStatus };
  }

  async prepareSemanticRuntime(): Promise<SemanticRuntimeStatus> {
    this.semanticStatus = {
      ...this.semanticStatus,
      phase: "READY",
      downloadedBytes: this.semanticStatus.modelBytes,
      modelDiskUsageBytes: this.semanticStatus.modelBytes,
      runtimeRunning: true,
      verified: true,
      message: null,
    };
    return { ...this.semanticStatus };
  }

  async retrySemanticRuntime(): Promise<SemanticRuntimeStatus> {
    return this.prepareSemanticRuntime();
  }

  async repairSemanticRuntime(): Promise<SemanticRuntimeStatus> {
    this.semanticStatus = {
      ...this.semanticStatus,
      phase: "NOT_DOWNLOADED",
      downloadedBytes: 0,
      modelDiskUsageBytes: 0,
      runtimeRunning: false,
      verified: false,
      message: null,
    };
    return this.prepareSemanticRuntime();
  }

  async semanticRuntimeLogs(): Promise<SemanticRuntimeLogs> {
    return { text: "", truncated: false };
  }

  async passageEmbeddingIndexStatus(): Promise<PassageEmbeddingIndexStatus> {
    const ready = this.semanticStatus.phase === "READY";
    return {
      phase: ready ? "READY" : "WAITING_FOR_RUNTIME",
      embeddingIndexId: "019f547b-6200-7000-8000-000000000002",
      indexKey: "jina_v1",
      indexedPassages: 0,
      totalPassages: 0,
      active: ready,
      message: null,
    };
  }

  async loadMaintenanceDiagnostics(): Promise<MaintenanceDiagnostics> {
    const activeTidbits = [...this.tidbits.values()].filter(
      (tidbit) => tidbit.deletedAtMs === null,
    ).length;
    const trashedTidbits = this.tidbits.size - activeTidbits;
    const research = [...this.researchRuns.values()].reduce(
      (counts, run) => {
        const key = run.status.toLowerCase() as keyof typeof counts;
        counts[key] += 1;
        return counts;
      },
      {
        queued: 0,
        running: 0,
        completed: 0,
        canceled: 0,
        failed: 0,
        interrupted: 0,
      },
    );
    const dataRoot = this.probe.dataDir;
    return {
      applicationVersion: "0.1.0-fixture",
      database: {
        migrationHeads: { main: 17, media: 3 },
        mainJournalMode: "wal",
        mediaJournalMode: "wal",
        mainForeignKeys: true,
        mediaForeignKeys: true,
      },
      library: {
        activeTidbits,
        trashedTidbits,
        revisions: this.revisions.size,
        authoredPassages: this.revisions.size,
        attachmentPassages: 0,
        searchDocuments: activeTidbits,
        attachments: 0,
        attachmentBytes: 0,
        imageOcr: {
          pending: 0,
          running: 0,
          retryWait: 0,
          ready: 0,
          failed: 0,
        },
        pdfExtraction: {
          pending: 0,
          running: 0,
          retryWait: 0,
          ready: 0,
          failed: 0,
        },
        research,
        indexes: [
          { name: "PASSAGE_BUILD", version: "markdown-v1", status: "IDLE", error: null },
          { name: "PASSAGE_FTS", version: "fts-v1", status: "IDLE", error: null },
          {
            name: "PASSAGE_EMBEDDING",
            version: "jina_v1",
            status: this.semanticStatus.phase === "READY" ? "IDLE" : "DIRTY",
            error: null,
          },
        ],
      },
      storage: {
        dataRoot,
        mainDatabasePath: `${dataRoot}/kosh.sqlite3`,
        mediaDatabasePath: `${dataRoot}/media.sqlite3`,
        mainDatabaseBytes: 98_304,
        mediaDatabaseBytes: 32_768,
        modelBytes: this.semanticStatus.modelDiskUsageBytes,
        logsBytes: 4_096,
        temporaryBytes: 0,
        totalBytes: 135_168 + this.semanticStatus.modelDiskUsageBytes,
      },
      mediaLimits: {
        maxAttachmentBytes: 33_554_432,
        maxAttachmentsPerDraft: 32,
        maxProtocolResponseBytes: 33_554_432,
        draftLeaseDurationMs: 86_400_000,
        orphanGracePeriodMs: 604_800_000,
        maxReapsPerMaintenance: 32,
      },
      nativeLogs: {
        paths: [
          `${dataRoot}/logs/kosh.log`,
          `${dataRoot}/logs/kosh.log.1`,
          `${dataRoot}/logs/kosh.log.2`,
        ],
        maxFileBytes: 524_288,
        maxFiles: 3,
        diskUsageBytes: 2_048,
      },
      semanticLogPaths: [`${dataRoot}/logs/llama-server.log`],
      backupPhase: "AVAILABLE",
    };
  }

  async runIntegrityCheck(): Promise<IntegrityCheckOutcome> {
    return {
      databaseOk: true,
      media: {
        missingBlobAttachmentIds: [],
        corruptBlobSha256: [],
        extraBlobSha256: [],
        orphanedAttachmentIds: [],
        diagnosticsTruncated: false,
      },
      message: "Both databases and all referenced media passed integrity checks.",
      completedAtMs: this.probe.nowMs,
    };
  }

  async rebuildSearchIndexes(): Promise<MaintenanceOutcome> {
    return this.maintenanceOutcome(
      "REBUILD_SEARCH",
      [...this.tidbits.values()].filter((tidbit) => tidbit.deletedAtMs === null).length,
      "Rebuilt passages and full-text indexes.",
    );
  }

  async rebuildEmbeddingIndex(): Promise<MaintenanceOutcome> {
    return this.maintenanceOutcome(
      "REBUILD_EMBEDDINGS",
      0,
      "Embedding rebuild is already queued; indexing will continue when the local model is ready.",
    );
  }

  async retryFailedExtractions(): Promise<MaintenanceOutcome> {
    return this.maintenanceOutcome(
      "RETRY_EXTRACTIONS",
      0,
      "No current failed OCR or PDF extractions needed a retry.",
    );
  }

  async reclaimEligibleMedia(): Promise<MaintenanceOutcome> {
    return this.maintenanceOutcome(
      "RECLAIM_MEDIA",
      0,
      "No expired or unreferenced media was eligible for reclamation.",
    );
  }

  async selectImage(): Promise<string | null> {
    return null;
  }

  private maintenanceOutcome(
    operation: MaintenanceOutcome["operation"],
    changedItems: number,
    message: string,
  ): MaintenanceOutcome {
    return {
      operation,
      changedItems,
      reclaimedBytes: 0,
      safetySnapshotId: operation === "RECLAIM_MEDIA" ? "media-reclaim-browser-fixture" : null,
      message,
      completedAtMs: this.probe.nowMs,
    };
  }

  async ingestSelectedImage(_selectionId: string, _draftId: string): Promise<ImageRecord> {
    throw new Error("Selected images are unavailable in the browser fixture");
  }

  async captureClipboardImage(): Promise<string> {
    throw new Error("Native clipboard images are unavailable in the browser fixture");
  }

  async ingestClipboardImage(_captureId: string, _draftId: string): Promise<ImageRecord> {
    throw new Error("Captured clipboard images are unavailable in the browser fixture");
  }

  async ingestDroppedImages(_dropId: string, _draftId: string): Promise<ImageDropIngestResult> {
    return { failures: [], images: [] };
  }

  async imageStatus(attachmentId: string): Promise<ImageStatusRecord> {
    throw new Error(`image ${attachmentId} was not found`);
  }

  async retryImageOcr(attachmentId: string): Promise<ImageStatusRecord> {
    throw new Error(`image ${attachmentId} was not found`);
  }

  async imageOcrDiagnostics(): Promise<ImageOcrDiagnostics> {
    return {
      failed: 0,
      lastError: null,
      oldestEligibleAtMs: null,
      pending: 0,
      ready: 0,
      retryWait: 0,
      running: 0,
    };
  }

  async selectPdf(): Promise<string | null> {
    return null;
  }

  async ingestSelectedPdf(_selectionId: string, _draftId: string): Promise<PdfRecord> {
    throw new Error("Selected PDFs are unavailable in the browser fixture");
  }

  async selectAttachment(): Promise<string | null> {
    return null;
  }

  async ingestSelectedAttachment(
    _selectionId: string,
    _draftId: string,
  ): Promise<SelectedAttachmentRecord> {
    throw new Error("Selected attachments are unavailable in the browser fixture");
  }

  async attachmentStatus(attachmentId: string): Promise<GenericAttachmentStatusRecord> {
    throw new Error(`attachment ${attachmentId} was not found`);
  }

  async openAttachmentExternal(_attachmentId: string): Promise<void> {
    throw new Error("Opening attachments is unavailable in the browser fixture");
  }

  async revealAttachmentInFinder(_attachmentId: string): Promise<void> {
    throw new Error("Revealing attachments is unavailable in the browser fixture");
  }

  async setFileDropConsumerActive(_active: boolean): Promise<void> {}

  async discardFileDropSelections(_selectionIds: string[]): Promise<void> {}

  async pdfStatus(attachmentId: string): Promise<PdfStatusRecord> {
    throw new Error(`PDF ${attachmentId} was not found`);
  }

  async retryPdfExtraction(attachmentId: string): Promise<PdfStatusRecord> {
    throw new Error(`PDF ${attachmentId} was not found`);
  }

  async openPdfExternal(_attachmentId: string): Promise<void> {
    throw new Error("Opening PDFs externally is unavailable in the browser fixture");
  }

  async createTidbit(input: TidbitDraft): Promise<TidbitRecord> {
    const sequence = this.nextSequence();
    const bodyMarkdown = validateBody(input.bodyMarkdown);
    const title = normalizeText(input.title);
    const sources = this.prepareSources(input.sources);
    const tidbit: TidbitRecord = {
      id: `fake-tidbit-${sequence}`,
      currentRevisionId: `fake-revision-${sequence}`,
      revisionNumber: 1,
      createdAtMs: this.probe.nowMs + sequence,
      updatedAtMs: this.probe.nowMs + sequence,
      deletedAtMs: null,
      title,
      displayTitle: deriveDisplayTitle(title, bodyMarkdown),
      bodyMarkdown,
      sources,
    };
    this.tidbits.set(tidbit.id, tidbit);
    this.revisions.set(tidbit.currentRevisionId, revisionFromTidbit(tidbit));
    this.revisionOwners.set(tidbit.currentRevisionId, tidbit.id);
    this.registerCitation(tidbit);
    return cloneTidbit(tidbit);
  }

  async loadTidbit(id: string): Promise<TidbitRecord> {
    return cloneTidbit(this.requireTidbit(id));
  }

  async listTidbits(input: ListTidbitsInput): Promise<TidbitListPage> {
    if (!Number.isSafeInteger(input.limit) || input.limit < 1 || input.limit > 100) {
      throw new Error("limit must be between 1 and 100");
    }
    const sorted = [...this.tidbits.values()]
      .filter((tidbit) =>
        input.scope === "DELETED" ? tidbit.deletedAtMs !== null : tidbit.deletedAtMs === null,
      )
      .sort(
        (left, right) => right.updatedAtMs - left.updatedAtMs || right.id.localeCompare(left.id),
      );
    const afterCursor = input.cursor
      ? sorted.filter(
          (tidbit) =>
            tidbit.updatedAtMs < input.cursor!.updatedAtMs ||
            (tidbit.updatedAtMs === input.cursor!.updatedAtMs && tidbit.id < input.cursor!.id),
        )
      : sorted;
    const hasMore = afterCursor.length > input.limit;
    const page = afterCursor.slice(0, input.limit);
    const last = page[page.length - 1];
    return {
      items: page.map((tidbit) => ({
        id: tidbit.id,
        currentRevisionId: tidbit.currentRevisionId,
        createdAtMs: tidbit.createdAtMs,
        updatedAtMs: tidbit.updatedAtMs,
        deletedAtMs: tidbit.deletedAtMs,
        purgeEligibleAtMs:
          tidbit.deletedAtMs === null ? null : tidbit.deletedAtMs + TIDBIT_PURGE_DELAY_MS,
        title: tidbit.title,
        displayTitle: tidbit.displayTitle,
        bodyPreview: collapseAndTruncate(tidbit.bodyMarkdown, 240),
      })),
      nextCursor:
        hasMore && last
          ? {
              updatedAtMs: last.updatedAtMs,
              id: last.id,
            }
          : null,
    };
  }

  async listTidbitRevisions(input: ListTidbitRevisionsInput): Promise<TidbitRevisionPage> {
    if (!Number.isSafeInteger(input.limit) || input.limit < 1 || input.limit > 100) {
      throw new Error("limit must be between 1 and 100");
    }
    const current = this.requireTidbit(input.tidbitId);
    const revisions = [...this.revisions.values()]
      .filter(
        (revision) =>
          revision.tidbitId === input.tidbitId &&
          (input.beforeRevisionNumber === null ||
            revision.revisionNumber < input.beforeRevisionNumber),
      )
      .sort((left, right) => right.revisionNumber - left.revisionNumber);
    const hasMore = revisions.length > input.limit;
    const page = revisions.slice(0, input.limit);
    return {
      items: page.map((revision) => ({
        id: revision.id,
        revisionNumber: revision.revisionNumber,
        createdAtMs: revision.createdAtMs,
        title: revision.title,
        displayTitle: revision.displayTitle,
        bodyPreview: collapseAndTruncate(revision.bodyMarkdown, 240),
        sourceCount: revision.sources.length,
        attachmentCount: revision.attachments.length,
        isCurrent: revision.id === current.currentRevisionId,
      })),
      nextBeforeRevisionNumber:
        hasMore && page.length ? page[page.length - 1]!.revisionNumber : null,
    };
  }

  async loadTidbitRevision(tidbitId: string, revisionId: string): Promise<TidbitRevisionRecord> {
    this.requireTidbit(tidbitId);
    const revision = this.revisions.get(revisionId);
    if (!revision || revision.tidbitId !== tidbitId) {
      throw new Error(`tidbit revision ${revisionId} was not found`);
    }
    const current = this.requireTidbit(tidbitId);
    return cloneValue({
      ...revision,
      isCurrent: revision.id === current.currentRevisionId,
      tidbitDeleted: current.deletedAtMs !== null,
    });
  }

  async editTidbit(input: EditTidbitInput): Promise<TidbitRecord> {
    const current = this.requireTidbit(input.id);
    if (current.deletedAtMs !== null) {
      throw new Error(`tidbit ${input.id} is deleted`);
    }
    if (current.currentRevisionId !== input.expectedRevisionId) {
      throw new Error(`tidbit ${input.id} is stale`);
    }
    const sequence = this.nextSequence();
    const bodyMarkdown = validateBody(input.bodyMarkdown);
    const title = normalizeText(input.title);
    const updated: TidbitRecord = {
      ...current,
      currentRevisionId: `fake-revision-${sequence}`,
      revisionNumber: current.revisionNumber + 1,
      updatedAtMs: Math.max(current.updatedAtMs + 1, this.probe.nowMs + sequence),
      title,
      displayTitle: deriveDisplayTitle(title, bodyMarkdown),
      bodyMarkdown,
      sources: this.prepareSources(input.sources),
    };
    this.tidbits.set(updated.id, updated);
    this.revisions.set(updated.currentRevisionId, revisionFromTidbit(updated));
    this.revisionOwners.set(updated.currentRevisionId, updated.id);
    this.registerCitation(updated);
    return cloneTidbit(updated);
  }

  async deleteTidbit(input: DeleteTidbitInput): Promise<TidbitRecord> {
    const current = this.requireTidbit(input.id);
    if (current.deletedAtMs !== null) {
      throw new Error(`tidbit ${input.id} is deleted`);
    }
    if (current.currentRevisionId !== input.expectedRevisionId) {
      throw new Error(`tidbit ${input.id} is stale`);
    }
    const deletedAtMs = Math.max(current.updatedAtMs + 1, this.probe.nowMs + this.nextSequence());
    const deleted = {
      ...current,
      updatedAtMs: deletedAtMs,
      deletedAtMs,
    };
    this.tidbits.set(deleted.id, deleted);
    return cloneTidbit(deleted);
  }

  async restoreTidbit(input: RestoreTidbitInput): Promise<TidbitRecord> {
    const current = this.requireTidbit(input.id);
    if (current.deletedAtMs === null) {
      throw new Error(`tidbit ${input.id} is not deleted`);
    }
    if (current.currentRevisionId !== input.expectedRevisionId) {
      throw new Error(`tidbit ${input.id} is stale`);
    }
    const restored = {
      ...current,
      updatedAtMs: Math.max(current.updatedAtMs + 1, this.probe.nowMs + this.nextSequence()),
      deletedAtMs: null,
    };
    this.tidbits.set(restored.id, restored);
    return cloneTidbit(restored);
  }

  async purgeTidbit(input: PurgeTidbitInput): Promise<boolean> {
    const current = this.requireTidbit(input.id);
    if (current.currentRevisionId !== input.expectedRevisionId) {
      throw new Error(`tidbit ${input.id} is stale`);
    }
    if (current.deletedAtMs === null) {
      throw new Error(`tidbit ${input.id} is not deleted`);
    }
    if (this.probe.nowMs < current.deletedAtMs + TIDBIT_PURGE_DELAY_MS) {
      throw new Error(
        `tidbit ${input.id} cannot be permanently deleted until ${
          current.deletedAtMs + TIDBIT_PURGE_DELAY_MS
        }`,
      );
    }
    this.tidbits.delete(input.id);
    for (const [revisionId, revision] of this.revisions) {
      if (revision.tidbitId === input.id) {
        this.revisions.delete(revisionId);
        this.revisionOwners.delete(revisionId);
        this.citations.delete(`fake-passage:${revisionId}`);
      }
    }
    return true;
  }

  async openSourceUrl(sourceId: string): Promise<void> {
    const source = [...this.revisions.values()]
      .flatMap((revision) => revision.sources)
      .find((candidate) => candidate.id === sourceId && candidate.url !== null);
    if (!source) {
      throw new Error(`source URL ${sourceId} was not found`);
    }
  }

  async resolveCitation(passageId: string): Promise<CitationResolution> {
    const snapshot = this.citations.get(passageId);
    if (!snapshot) {
      throw new Error(`passage ${passageId} was not found`);
    }
    const current = this.requireTidbit(snapshot.revision.id);
    const isCurrent =
      current.deletedAtMs === null &&
      current.currentRevisionId === snapshot.revision.currentRevisionId;
    return {
      passageId,
      excerpt: snapshot.revision.bodyMarkdown,
      headingContext: [],
      constructionVersion: "fake-markdown-blocks-v1",
      state: isCurrent ? "CURRENT" : "HISTORICAL",
      locator: {
        kind: "MARKDOWN_BLOCKS",
        startBlock: 0,
        endBlock: 0,
        sourceStartByte: 0,
        sourceEndByte: new TextEncoder().encode(snapshot.revision.bodyMarkdown).length,
        startChar: null,
        endChar: null,
        startLine: null,
        endLine: null,
      },
      tidbit: {
        id: snapshot.revision.id,
        revisionId: snapshot.revision.currentRevisionId,
        revisionNumber: snapshot.revision.revisionNumber,
        title: snapshot.revision.title,
        displayTitle: snapshot.revision.displayTitle,
        deleted: current.deletedAtMs !== null,
      },
      attachment: null,
      sources: snapshot.revision.sources.map((source) => ({ ...source })),
    };
  }

  async searchPassages(input: SearchPassagesInput): Promise<SearchPassagesResponse> {
    if (!Number.isSafeInteger(input.limit) || input.limit < 1 || input.limit > 100) {
      throw new Error("limit must be between 1 and 100");
    }
    if ([...input.query].length > 512) {
      throw new Error("query must contain at most 512 characters");
    }
    const atoms = parseSearchAtoms(input.query);
    const semanticReady = this.semanticStatus.phase === "READY";
    const executionMode = input.mode === "EXACT" ? "EXACT" : "LEXICAL_ONLY";
    const semanticReadiness =
      input.mode === "EXACT" ? "NOT_REQUESTED" : semanticReady ? "READY" : "WAITING_FOR_RUNTIME";
    if (atoms.length === 0) {
      return { results: [], executionMode, semanticReadiness };
    }
    const matches = [...this.tidbits.values()]
      .filter((tidbit) => tidbit.deletedAtMs === null)
      .flatMap((tidbit) => {
        const fields: Array<[SearchField, string]> = [
          ["TITLE", tidbit.title ?? ""],
          ["BODY", tidbit.bodyMarkdown],
          ["SOURCE_LABEL", tidbit.sources.flatMap((source) => source.label ?? []).join("\n")],
          ["SOURCE_DOMAIN", tidbit.sources.flatMap((source) => source.url ?? []).join("\n")],
        ];
        const matchedAtoms = atoms.map((atom) =>
          fields.some(([, value]) =>
            normalizeSearchText(value).includes(normalizeSearchText(atom)),
          ),
        );
        const matchedAtomCount = matchedAtoms.filter(Boolean).length;
        const qualifies =
          input.mode === "EXACT"
            ? matchedAtoms.every(Boolean)
            : matchedAtomCount >= Math.min(atoms.length, 2);
        if (!qualifies) {
          return [];
        }
        const matchedFields = fields
          .filter(([, value]) =>
            atoms.some((atom) => normalizeSearchText(value).includes(normalizeSearchText(atom))),
          )
          .map(([field]) => field);
        const highlights = fields.flatMap(([field, value]) =>
          atoms.flatMap((atom) => searchSpans(value, atom, field)),
        );
        return [
          {
            tidbit,
            matchedFields,
            highlights: highlights.slice(0, 32),
            score: matchedAtomCount,
          },
        ];
      })
      .sort(
        (left, right) =>
          right.score - left.score ||
          right.tidbit.updatedAtMs - left.tidbit.updatedAtMs ||
          left.tidbit.id.localeCompare(right.tidbit.id),
      )
      .slice(0, input.limit);

    const results = await Promise.all(
      matches.map(async ({ tidbit, matchedFields, highlights, score }) => {
        const passageId = `fake-passage:${tidbit.currentRevisionId}`;
        return {
          passageId,
          score,
          matchedFields,
          highlights,
          citation: await this.resolveCitation(passageId),
        };
      }),
    );
    return { results, executionMode, semanticReadiness };
  }

  async saveDraft(input: SaveDraftInput): Promise<DraftRecord> {
    this.validateDraftContext(input);
    const existing = this.drafts.get(input.contextKey);
    const sequence = this.nextSequence();
    const draft: DraftRecord = {
      id: existing?.id ?? `fake-draft-${sequence}`,
      contextKey: input.contextKey,
      tidbitId: input.tidbitId,
      baseRevisionId: input.baseRevisionId,
      createdAtMs: existing?.createdAtMs ?? this.probe.nowMs + sequence,
      updatedAtMs: existing
        ? Math.max(existing.updatedAtMs + 1, this.probe.nowMs + sequence)
        : this.probe.nowMs + sequence,
      title: input.title === "" ? null : input.title,
      bodyMarkdown: input.bodyMarkdown,
      sources: input.sources.map((source) => ({ ...source })),
    };
    this.drafts.set(draft.contextKey, draft);
    return cloneDraft(draft);
  }

  async loadDraft(contextKey: string): Promise<DraftRecord | null> {
    validateDraftContextKey(contextKey);
    const draft = this.drafts.get(contextKey);
    return draft ? cloneDraft(draft) : null;
  }

  async clearDraft(input: ClearDraftInput): Promise<boolean> {
    validateDraftContextKey(input.contextKey);
    if (!Number.isSafeInteger(input.expectedUpdatedAtMs) || input.expectedUpdatedAtMs < 0) {
      throw new Error("draft timestamp must be a non-negative JavaScript-safe integer");
    }
    const draft = this.drafts.get(input.contextKey);
    if (!draft || draft.updatedAtMs !== input.expectedUpdatedAtMs) {
      return false;
    }
    return this.drafts.delete(input.contextKey);
  }

  async loadShortcutSettings(): Promise<ShortcutSettingsSnapshot> {
    return cloneShortcutSettings(this.shortcutSettings);
  }

  async setShortcutSettings(input: SetShortcutSettingsInput): Promise<ShortcutSettingsSnapshot> {
    if (input.expectedRevision !== this.shortcutSettings.revision) {
      throw new Error(
        `shortcut settings changed before this update: revision is ${this.shortcutSettings.revision}, expected ${input.expectedRevision}`,
      );
    }
    if (
      input.keyboardBindings.length !== DEFAULT_KEYBOARD_BINDINGS.length ||
      new Set(input.keyboardBindings.map((binding) => binding.command)).size !==
        DEFAULT_KEYBOARD_BINDINGS.length
    ) {
      throw new Error("keyboardBindings must contain every Kosh command exactly once");
    }
    if (
      new Set(input.keyboardBindings.map((binding) => binding.accelerator.toLowerCase())).size !==
      input.keyboardBindings.length
    ) {
      throw new Error("two Kosh commands cannot use the same shortcut");
    }
    this.shortcutSettings = {
      revision: this.shortcutSettings.revision + 1,
      keyboardBindings: input.keyboardBindings.map((binding) => ({ ...binding })),
      shortcutErrors: [],
    };
    return cloneShortcutSettings(this.shortcutSettings);
  }

  async claudeSetupStatus(): Promise<ClaudeSetupStatus> {
    return {
      phase: "READY",
      binaryPath: "/usr/local/bin/claude",
      version: "fixture",
      defaults: { model: "sonnet", effort: "high" },
      message: "Claude Code is ready for Kosh research.",
    };
  }

  async claudeCliDefaults(): Promise<ClaudeCliDefaults> {
    return { model: "sonnet", effort: "high" };
  }

  async startResearchProcess(
    input: BeginResearchProcessInput,
  ): Promise<StartResearchProcessOutput> {
    if (!input.prompt.trim()) {
      throw new Error("the research prompt must not be empty");
    }
    for (const active of this.researchRuns.values()) {
      if (active.status === "QUEUED" || active.status === "RUNNING") {
        const timer = this.researchTimers.get(active.id);
        if (timer) clearTimeout(timer);
        this.researchTimers.delete(active.id);
        this.emitResearch({
          runId: active.id,
          sequence: active.events.length + 1,
          kind: "FINISHED",
          outcome: "REPLACED",
          stderrTruncated: false,
        });
      }
    }
    const sequence = this.nextSequence();
    const runId = `fake-research-${sequence}`;
    const now = this.probe.nowMs + sequence;
    this.researchRuns.set(runId, {
      id: runId,
      rerunOfId: null,
      query: input.prompt,
      status: "QUEUED",
      requestedModel: input.model,
      requestedEffort: input.effort,
      actualModel: null,
      createdAtMs: now,
      startedAtMs: null,
      completedAtMs: null,
      updatedAtMs: now,
      error: null,
      stderrTruncated: false,
      savedTidbitId: null,
      events: [],
      finalAnswer: null,
      citationFreshness: [],
    });
    this.emitResearch({ runId, sequence: 1, kind: "STARTED" });
    this.scheduleResearch(runId, input.prompt);
    return { runId };
  }

  async rerunResearchProcess(runId: string): Promise<StartResearchProcessOutput> {
    const previous = this.requireResearchRun(runId);
    const output = await this.startResearchProcess({
      prompt: previous.query,
      model: previous.requestedModel,
      effort: previous.requestedEffort,
      timeoutSeconds: null,
    });
    this.requireResearchRun(output.runId).rerunOfId = runId;
    return output;
  }

  async cancelResearchProcess(runId: string): Promise<boolean> {
    const run = this.requireResearchRun(runId);
    if (run.status !== "QUEUED" && run.status !== "RUNNING") {
      return false;
    }
    const timer = this.researchTimers.get(runId);
    if (timer) {
      clearTimeout(timer);
      this.researchTimers.delete(runId);
    }
    this.emitResearch({
      runId,
      sequence: run.events.length + 1,
      kind: "FINISHED",
      outcome: "CANCELED",
      stderrTruncated: false,
    });
    return true;
  }

  async listResearchRuns(input: ListResearchRunsInput): Promise<ResearchRunPage> {
    if (!Number.isSafeInteger(input.limit) || input.limit < 1 || input.limit > 100) {
      throw new Error("limit must be between 1 and 100");
    }
    const sorted = [...this.researchRuns.values()].sort(
      (left, right) => right.updatedAtMs - left.updatedAtMs || right.id.localeCompare(left.id),
    );
    const start = input.cursor
      ? sorted.findIndex(
          (run) =>
            run.updatedAtMs < input.cursor!.updatedAtMs ||
            (run.updatedAtMs === input.cursor!.updatedAtMs && run.id < input.cursor!.id),
        )
      : 0;
    const normalizedStart = start < 0 ? sorted.length : start;
    const items = sorted.slice(normalizedStart, normalizedStart + input.limit);
    const hasMore = normalizedStart + input.limit < sorted.length;
    const last = items.at(-1);
    return {
      items: items.map(researchSummary),
      nextCursor: hasMore && last ? { updatedAtMs: last.updatedAtMs, id: last.id } : null,
    };
  }

  async loadResearchRun(id: string): Promise<ResearchRunRecord> {
    const record = cloneResearchRun(this.requireResearchRun(id));
    record.citationFreshness = (record.finalAnswer?.citations ?? []).map((citation) => {
      const citedRevisionId = citation.evidence.tidbit?.revisionId ?? null;
      const currentTidbit = citation.evidence.tidbit
        ? this.tidbits.get(citation.evidence.tidbit.id)
        : undefined;
      const currentRevisionId = currentTidbit?.currentRevisionId ?? null;
      const tidbitDeleted =
        citation.evidence.tidbit !== null &&
        (currentTidbit === undefined || currentTidbit.deletedAtMs !== null);
      const hasNewerRevision = citedRevisionId !== null && citedRevisionId !== currentRevisionId;
      return {
        citationNumber: citation.number,
        citedRevisionId,
        currentRevisionId,
        hasNewerRevision,
        isHistorical:
          citedRevisionId !== null &&
          (hasNewerRevision || tidbitDeleted || currentTidbit === undefined),
        tidbitDeleted,
      };
    });
    return record;
  }

  async saveResearchAnswerAsTidbit(runId: string): Promise<TidbitRecord> {
    const run = this.requireResearchRun(runId);
    if (run.status !== "COMPLETED" || !run.finalAnswer) {
      throw new Error("only completed research answers can become tidbits");
    }
    if (run.savedTidbitId) {
      return cloneTidbit(this.requireTidbit(run.savedTidbitId));
    }
    const tidbit = await this.createTidbit({
      title: `Research: ${truncate(run.query, 86)}`,
      bodyMarkdown: neutralizeUntrustedMediaReferences(run.finalAnswer.markdown),
      sources: [],
    });
    run.savedTidbitId = tidbit.id;
    run.updatedAtMs = Math.max(run.updatedAtMs + 1, tidbit.updatedAtMs);
    return tidbit;
  }

  async onResearchProcessEvent(
    handler: (event: ResearchProcessEvent) => void,
  ): Promise<() => void> {
    this.researchListeners.add(handler);
    return () => {
      this.researchListeners.delete(handler);
    };
  }

  private scheduleResearch(runId: string, prompt: string): void {
    const delay = prompt.includes("[slow]") ? 500 : 20;
    const timer = setTimeout(() => {
      this.researchTimers.delete(runId);
      void this.completeResearch(runId, prompt);
    }, delay);
    this.researchTimers.set(runId, timer);
  }

  private async completeResearch(runId: string, prompt: string): Promise<void> {
    const run = this.researchRuns.get(runId);
    if (!run || (run.status !== "QUEUED" && run.status !== "RUNNING")) {
      return;
    }
    let sequence = run.events.length + 1;
    this.emitResearch({
      runId,
      sequence: sequence++,
      kind: "METADATA",
      model: run.requestedModel ?? "sonnet",
    });
    if (prompt.includes("[fail]")) {
      this.emitResearch({
        runId,
        sequence,
        kind: "FINISHED",
        outcome: "FAILED",
        error: "Fixture research failed safely.",
        stderrTruncated: false,
      });
      return;
    }
    this.emitResearch({
      runId,
      sequence: sequence++,
      kind: "TOOL_ACTIVITY",
      tool: "kosh_v1_hybrid_search",
      phase: "STARTED",
    });
    this.emitResearch({
      runId,
      sequence: sequence++,
      kind: "UNTRUSTED_TEXT_DELTA",
      text: "Inspecting the most relevant passages…",
    });
    this.emitResearch({
      runId,
      sequence: sequence++,
      kind: "TOOL_ACTIVITY",
      tool: "kosh_v1_hybrid_search",
      phase: "FINISHED",
    });
    const answer = await this.fixtureResearchAnswer();
    this.emitResearch({
      runId,
      sequence: sequence++,
      kind: "GROUNDED_FINAL_OUTPUT",
      answer,
    });
    this.emitResearch({
      runId,
      sequence,
      kind: "FINISHED",
      outcome: "SUCCEEDED",
      stderrTruncated: false,
    });
  }

  private async fixtureResearchAnswer(): Promise<GroundedResearchAnswer> {
    const first = [...this.tidbits.values()].find((tidbit) => tidbit.deletedAtMs === null);
    const evidence = first
      ? await this.resolveCitation(`fake-passage:${first.currentRevisionId}`)
      : fallbackCitation();
    const markdown = `Kosh found a durable answer in your local library.【1】`;
    const markerStart = new TextEncoder().encode(markdown.slice(0, markdown.indexOf("【"))).length;
    return {
      markdown,
      citations: [
        {
          number: 1,
          label:
            evidence.tidbit?.displayTitle ?? evidence.attachment?.displayFilename ?? "Evidence",
          evidenceKind: evidence.tidbit ? "AUTHORED_TIDBIT" : "TEXT_LINES",
          evidence,
        },
      ],
      mentions: [
        {
          citationNumber: 1,
          startByte: markerStart,
          endByte: new TextEncoder().encode(markdown).length,
        },
      ],
      issues: [],
    };
  }

  private emitResearch(event: ResearchProcessEvent): void {
    const run = this.requireResearchRun(event.runId);
    if (event.sequence !== run.events.length + 1) {
      throw new Error("fake research event sequence is not contiguous");
    }
    run.events.push(cloneValue(event));
    run.updatedAtMs = Math.max(run.updatedAtMs + 1, this.probe.nowMs + this.nextSequence());
    if (event.kind === "STARTED") {
      run.status = "RUNNING";
      run.startedAtMs = run.updatedAtMs;
    } else if (event.kind === "METADATA") {
      run.actualModel = event.model ?? null;
    } else if (event.kind === "GROUNDED_FINAL_OUTPUT") {
      run.finalAnswer = cloneValue(event.answer);
      run.citationFreshness = event.answer.citations.map((citation) => ({
        citationNumber: citation.number,
        citedRevisionId: citation.evidence.tidbit?.revisionId ?? null,
        currentRevisionId: citation.evidence.tidbit?.revisionId ?? null,
        hasNewerRevision: false,
        isHistorical: citation.evidence.state === "HISTORICAL",
        tidbitDeleted: citation.evidence.tidbit?.deleted ?? false,
      }));
    } else if (event.kind === "FINISHED") {
      run.status =
        event.outcome === "SUCCEEDED"
          ? "COMPLETED"
          : event.outcome === "CANCELED" || event.outcome === "REPLACED"
            ? "CANCELED"
            : event.outcome === "SHUTDOWN"
              ? "INTERRUPTED"
              : "FAILED";
      run.completedAtMs = run.updatedAtMs;
      run.error = event.error ?? null;
      run.stderrTruncated = event.stderrTruncated;
    }
    for (const listener of this.researchListeners) {
      listener(cloneValue(event));
    }
  }

  private requireResearchRun(id: string): ResearchRunRecord {
    const run = this.researchRuns.get(id);
    if (!run) {
      throw new Error(`research run ${id} was not found`);
    }
    return run;
  }

  private nextSequence(): number {
    this.sequence += 1;
    return this.sequence;
  }

  private registerCitation(revision: TidbitRecord): void {
    const passageId = `fake-passage:${revision.currentRevisionId}`;
    this.citations.set(passageId, {
      revision: cloneTidbit(revision),
    });
  }

  private requireTidbit(id: string): TidbitRecord {
    const tidbit = this.tidbits.get(id);
    if (!tidbit) {
      throw new Error(`tidbit ${id} was not found`);
    }
    return tidbit;
  }

  private prepareSources(inputs: SourceDraft[]): TidbitSource[] {
    const sources = inputs.map(normalizeSource);
    const identities = new Set<string>();
    for (const source of sources) {
      const identity = sourceIdentity(source);
      if (identities.has(identity)) {
        throw new Error("sources must not contain duplicates");
      }
      identities.add(identity);
    }
    return sources.map((source) => {
      const identity = sourceIdentity(source);
      let id = this.sourceIds.get(identity);
      if (!id) {
        id = `fake-source-${this.nextSequence()}`;
        this.sourceIds.set(identity, id);
      }
      return { ...source, id };
    });
  }

  private validateDraftContext(input: SaveDraftInput): void {
    validateDraftContextKey(input.contextKey);
    if (input.contextKey === "capture" || input.contextKey === "quick-add") {
      if (input.tidbitId !== null || input.baseRevisionId !== null) {
        throw new Error("capture draft must not have edit metadata");
      }
      return;
    }
    if (!input.tidbitId || !input.baseRevisionId || input.contextKey !== `edit:${input.tidbitId}`) {
      throw new Error("edit draft needs matching edit metadata");
    }
    if (this.revisionOwners.get(input.baseRevisionId) !== input.tidbitId) {
      throw new Error("draft base revision must belong to its tidbit");
    }
  }
}

function cloneTidbit(tidbit: TidbitRecord): TidbitRecord {
  return {
    ...tidbit,
    sources: tidbit.sources.map((source) => ({ ...source })),
  };
}

function cloneBackupSettings(settings: BackupSettingsSnapshot): BackupSettingsSnapshot {
  return {
    ...settings,
    config: settings.config ? { ...settings.config } : null,
    relational: { ...settings.relational },
    media: { ...settings.media },
    checkpoint: { ...settings.checkpoint },
    retention: { ...settings.retention },
  };
}

function validateFakeBackupTarget(input: {
  accountId: string;
  bucket: string;
  backupSetId: string | null;
}) {
  if (!/^[0-9a-f]{32}$/.test(input.accountId)) {
    throw new Error("Enter a 32-character Cloudflare account ID.");
  }
  if (
    input.bucket.length < 3 ||
    input.bucket.length > 63 ||
    !/^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(input.bucket)
  ) {
    throw new Error("Enter a valid lowercase R2 bucket name.");
  }
  if (
    input.backupSetId !== null &&
    !/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(input.backupSetId)
  ) {
    throw new Error("Enter a canonical Kosh backup set ID.");
  }
}

function validateFakeCredentials(
  input: { accessKeyId: string | null; secretAccessKey: string | null },
  stored: boolean,
) {
  if (input.accessKeyId === null && input.secretAccessKey === null && stored) return;
  if (!input.accessKeyId || !input.secretAccessKey) {
    throw new Error("Enter both the R2 access key ID and secret access key.");
  }
  if (!/^[0-9a-f]{32}$/.test(input.accessKeyId)) {
    throw new Error("The R2 access key ID must be 32 lowercase hexadecimal characters.");
  }
  if (!/^[0-9a-f]{64}$/.test(input.secretAccessKey)) {
    throw new Error("The R2 secret access key must be 64 lowercase hexadecimal characters.");
  }
}

function revisionFromTidbit(tidbit: TidbitRecord): TidbitRevisionRecord {
  return {
    id: tidbit.currentRevisionId,
    tidbitId: tidbit.id,
    revisionNumber: tidbit.revisionNumber,
    createdAtMs: tidbit.revisionNumber === 1 ? tidbit.createdAtMs : tidbit.updatedAtMs,
    title: tidbit.title,
    displayTitle: tidbit.displayTitle,
    bodyMarkdown: tidbit.bodyMarkdown,
    sources: tidbit.sources.map((source) => ({ ...source })),
    attachments: [],
    isCurrent: true,
    tidbitDeleted: tidbit.deletedAtMs !== null,
  };
}

function cloneDraft(draft: DraftRecord): DraftRecord {
  return {
    ...draft,
    sources: draft.sources.map((source) => ({ ...source })),
  };
}

function cloneShortcutSettings(settings: ShortcutSettingsSnapshot): ShortcutSettingsSnapshot {
  return {
    ...settings,
    keyboardBindings: settings.keyboardBindings.map((binding) => ({ ...binding })),
    shortcutErrors: [...settings.shortcutErrors],
  };
}

function researchSummary(run: ResearchRunRecord) {
  const { events: _events, finalAnswer: _answer, citationFreshness: _freshness, ...summary } = run;
  return { ...summary };
}

function cloneResearchRun(run: ResearchRunRecord): ResearchRunRecord {
  return cloneValue(run);
}

function cloneValue<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function fallbackCitation(): CitationResolution {
  return {
    passageId: "fake-passage:fallback",
    excerpt: "Kosh fixture evidence is stored locally and cited exactly.",
    headingContext: [],
    constructionVersion: "fake-text-lines-v1",
    state: "CURRENT",
    locator: {
      kind: "TEXT_LINES",
      startLine: 1,
      endLine: 1,
    },
    tidbit: null,
    attachment: {
      id: "fake-attachment-fallback",
      extractionId: "fake-extraction-fallback",
      displayFilename: "fixture.txt",
      mediaType: "text/plain",
      deleted: false,
    },
    sources: [],
  };
}

function validateDraftContextKey(contextKey: string): void {
  if (contextKey === "capture" || contextKey === "quick-add" || /^edit:.+/u.test(contextKey)) {
    return;
  }
  throw new Error("draft context must be capture, quick-add, or edit:<tidbitId>");
}

function normalizeText(value: string | null): string | null {
  const normalized = value?.trim() ?? "";
  return normalized ? normalized : null;
}

function normalizeSource(input: SourceDraft): Omit<TidbitSource, "id"> {
  const label = normalizeText(input.label);
  const rawUrl = normalizeText(input.url);
  let url: string | null = null;
  if (rawUrl) {
    const parsed = new URL(rawUrl);
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      throw new Error("source URL must use HTTP or HTTPS");
    }
    parsed.hash = "";
    url = parsed.toString();
  }
  if (!label && !url) {
    throw new Error("each source needs a label or HTTP(S) URL");
  }
  return { label, url };
}

function sourceIdentity(source: Pick<TidbitSource, "label" | "url">): string {
  return JSON.stringify([source.label, source.url]);
}

function validateBody(value: string): string {
  if (!value.trim()) {
    throw new Error("bodyMarkdown must contain non-whitespace text");
  }
  return value;
}

function deriveDisplayTitle(title: string | null, bodyMarkdown: string): string {
  if (title) {
    return truncate(title, 96);
  }
  const line = bodyMarkdown
    .split(/\r?\n/u)
    .map((candidate) => candidate.trim())
    .find((candidate) => candidate && !candidate.startsWith("```") && !candidate.startsWith("~~~"));
  const stripped = line?.replace(/^[#>*+\-\s]+/u, "") || "Untitled tidbit";
  return truncate(stripped, 96);
}

function collapseAndTruncate(value: string, limit: number): string {
  return truncate(value.trim().split(/\s+/u).join(" "), limit);
}

function truncate(value: string, limit: number): string {
  const characters = [...value];
  return characters.length > limit ? `${characters.slice(0, limit).join("")}…` : value;
}

function generatedIdSequence(value: string): number {
  const match = /^fake-(?:tidbit|revision|source)-(\d+)$/u.exec(value);
  if (!match) {
    return 0;
  }
  const sequence = Number(match[1]);
  return Number.isSafeInteger(sequence) ? sequence : 0;
}

function parseSearchAtoms(query: string): string[] {
  return [...query.matchAll(/"([^"]+)"|(\S+)/gu)]
    .map((match) => (match[1] ?? match[2] ?? "").trim())
    .filter(Boolean);
}

function normalizeSearchText(value: string): string {
  return normalizeSearchTextWithMapping(value).text;
}

function normalizeSearchTextWithMapping(value: string): {
  text: string;
  originalIndices: number[];
} {
  const characters: string[] = [];
  const originalIndices: number[] = [];
  let originalIndex = 0;
  for (const originalCharacter of value) {
    for (const decomposedCharacter of originalCharacter.normalize("NFKD")) {
      if (/\p{M}/u.test(decomposedCharacter)) {
        continue;
      }
      for (const lowercaseCharacter of decomposedCharacter.toLowerCase()) {
        characters.push(lowercaseCharacter);
        originalIndices.push(originalIndex);
      }
    }
    originalIndex += 1;
  }
  return { text: characters.join(""), originalIndices };
}

function searchSpans(value: string, atom: string, field: SearchField) {
  const normalizedValue = normalizeSearchTextWithMapping(value);
  const haystack = [...normalizedValue.text];
  const needle = [...normalizeSearchText(atom)];
  if (needle.length === 0 || needle.length > haystack.length) {
    return [];
  }
  const start = haystack.findIndex((_, candidateStart) =>
    needle.every((character, offset) => haystack[candidateStart + offset] === character),
  );
  if (start < 0) {
    return [];
  }
  const startChar = normalizedValue.originalIndices[start];
  const finalOriginalIndex = normalizedValue.originalIndices[start + needle.length - 1];
  if (startChar === undefined || finalOriginalIndex === undefined) {
    return [];
  }
  return [
    {
      field,
      startChar,
      endChar: finalOriginalIndex + 1,
    },
  ];
}

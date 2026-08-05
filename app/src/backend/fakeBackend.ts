import type {
  Backend,
  BackupConnectionTestResult,
  BackupRestoreDrill,
  BackupRestorePreview,
  BackupSettingsSnapshot,
  CitationResolution,
  CheckpointWorkingCopyInput,
  ConfigureBackupInput,
  DeleteTidbitInput,
  DiscardWorkingCopyInput,
  GenericAttachmentStatusRecord,
  ImageDropIngestResult,
  ImageOcrDiagnostics,
  ImageRecord,
  ImageStatusRecord,
  IntegrityCheckOutcome,
  MaintenanceDiagnostics,
  MaintenanceOutcome,
  PassageEmbeddingIndexStatus,
  PdfRecord,
  PdfStatusRecord,
  RuntimeProbe,
  RemoteBackupCheckpoint,
  SelectedAttachmentRecord,
  RestoreTidbitInput,
  SaveWorkingCopyInput,
  SearchField,
  SearchPassagesInput,
  SearchPassagesResponse,
  SemanticRuntimeLogs,
  SemanticRuntimeStatus,
  SetBackupEnabledInput,
  SetAutomaticUpdateChecksInput,
  SetShortcutSettingsInput,
  ShortcutSettingsSnapshot,
  SourceDraft,
  TidbitRecord,
  TidbitSource,
  WorkingCopyCheckpointResult,
  WorkingCopyRecord,
  WorkingCopySaveResult,
  TakeOverBackupInput,
  TestBackupConnectionInput,
  RestoreCheckpointInput,
} from "./contracts";
import { DEFAULT_KEYBOARD_BINDINGS } from "./contracts";
import { createKoshDocumentFromMarkdown } from "../editor/document";
import { hasMeaningfulAuthoredContent } from "../notes/content";

interface FakeCitationSnapshot {
  revision: TidbitRecord;
}

export interface FakeNoteInput {
  bodyMarkdown: string;
  documentJson?: string;
  sources: SourceDraft[];
}

interface ReplaceNoteForTestInput extends FakeNoteInput {
  id: string;
  expectedRevisionId: string;
}

interface ListNotesForTestInput {
  limit: number;
  cursor: { updatedAtMs: number; id: string } | null;
  scope: "ACTIVE" | "DELETED";
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
  private readonly workingCopies = new Map<string, WorkingCopyRecord>();
  private readonly citations = new Map<string, FakeCitationSnapshot>();
  private readonly sourceIds = new Map<string, string>();
  private readonly tidbits = new Map<string, TidbitRecord>();
  private shortcutSettings: ShortcutSettingsSnapshot = {
    revision: 1,
    automaticUpdateChecksEnabled: true,
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
      const normalized = {
        ...tidbit,
        documentJson: tidbit.documentJson ?? createKoshDocumentFromMarkdown(tidbit.bodyMarkdown),
      };
      this.tidbits.set(normalized.id, cloneTidbit(normalized));
      this.registerCitation(normalized);
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
    const dataRoot = this.probe.dataDir;
    return {
      applicationVersion: "0.1.0-fixture",
      database: {
        migrationHeads: { main: 1, media: 1 },
        mainJournalMode: "wal",
        mediaJournalMode: "wal",
        mainForeignKeys: true,
        mediaForeignKeys: true,
      },
      library: {
        activeTidbits,
        trashedTidbits,
        revisions: this.citations.size,
        authoredPassages: this.citations.size,
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
        indexes: [
          { name: "PASSAGE_FTS", version: "lexical-v4", status: "IDLE", error: null },
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

  async seedNote(input: FakeNoteInput): Promise<TidbitRecord> {
    const sequence = this.nextSequence();
    const bodyMarkdown = validateBody(input.bodyMarkdown);
    const sources = this.prepareSources(input.sources);
    const tidbit: TidbitRecord = {
      id: `fake-tidbit-${sequence}`,
      currentRevisionId: `fake-revision-${sequence}`,
      revisionNumber: 1,
      createdAtMs: this.probe.nowMs + sequence,
      updatedAtMs: this.probe.nowMs + sequence,
      deletedAtMs: null,
      displayTitle: deriveDisplayTitle(bodyMarkdown),
      documentJson: input.documentJson ?? createKoshDocumentFromMarkdown(bodyMarkdown),
      bodyMarkdown,
      sources,
    };
    this.tidbits.set(tidbit.id, tidbit);
    this.registerCitation(tidbit);
    return cloneTidbit(tidbit);
  }

  async loadTidbit(id: string): Promise<TidbitRecord> {
    return cloneTidbit(this.requireTidbit(id));
  }

  async listNotesForTest(input: ListNotesForTestInput) {
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

  async replaceNoteForTest(input: ReplaceNoteForTestInput): Promise<TidbitRecord> {
    const current = this.requireTidbit(input.id);
    if (current.deletedAtMs !== null) {
      throw new Error(`tidbit ${input.id} is deleted`);
    }
    if (current.currentRevisionId !== input.expectedRevisionId) {
      throw new Error(`tidbit ${input.id} is stale`);
    }
    const sequence = this.nextSequence();
    const bodyMarkdown = validateBody(input.bodyMarkdown);
    const updated: TidbitRecord = {
      ...current,
      currentRevisionId: `fake-revision-${sequence}`,
      revisionNumber: current.revisionNumber + 1,
      updatedAtMs: Math.max(current.updatedAtMs + 1, this.probe.nowMs + sequence),
      displayTitle: deriveDisplayTitle(bodyMarkdown),
      documentJson: input.documentJson ?? createKoshDocumentFromMarkdown(bodyMarkdown),
      bodyMarkdown,
      sources: this.prepareSources(input.sources),
    };
    this.tidbits.set(updated.id, updated);
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

  async openSourceUrl(sourceId: string): Promise<void> {
    const source = [...this.citations.values()]
      .flatMap(({ revision }) => revision.sources)
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
          note: {
            id: tidbit.id,
            revisionId: tidbit.currentRevisionId,
            revisionNumber: tidbit.revisionNumber,
            displayTitle: tidbit.displayTitle,
            deleted: false,
          },
          citation: await this.resolveCitation(passageId),
        };
      }),
    );
    return { results, executionMode, semanticReadiness };
  }

  async saveWorkingCopy(input: SaveWorkingCopyInput): Promise<WorkingCopySaveResult> {
    return this.saveWorkingCopyInternal(input, false);
  }

  async reserveWorkingCopyForMedia(input: SaveWorkingCopyInput): Promise<WorkingCopySaveResult> {
    return this.saveWorkingCopyInternal(input, true);
  }

  private saveWorkingCopyInternal(
    input: SaveWorkingCopyInput,
    allowEmptyEphemeral: boolean,
  ): WorkingCopySaveResult {
    input = {
      ...input,
      documentJson: input.documentJson ?? createKoshDocumentFromMarkdown(input.bodyMarkdown),
    };
    validateEditGeneration(input.editGeneration, "editGeneration");
    const currentNote = this.tidbits.get(input.noteId);
    if (input.baseRevisionId === null) {
      if (currentNote) throw new Error("an existing note requires its current base revision");
    } else if (!currentNote || currentNote.currentRevisionId !== input.baseRevisionId) {
      throw new Error(`note ${input.noteId} is stale`);
    }
    const existing = this.workingCopies.get(input.noteId);
    if (existing && input.editGeneration < existing.editGeneration) {
      return {
        status: "STALE",
        acceptedEditGeneration: existing.editGeneration,
        workingCopy: cloneWorkingCopy(existing),
      };
    }
    if (existing && input.editGeneration === existing.editGeneration) {
      if (!sameWorkingCopy(existing, input)) {
        throw new Error("an edit generation cannot be reused for different content");
      }
      const classified =
        existing.mediaReservation && !allowEmptyEphemeral
          ? { ...existing, mediaReservation: false }
          : existing;
      this.workingCopies.set(input.noteId, classified);
      return {
        status: "SAVED",
        acceptedEditGeneration: existing.editGeneration,
        workingCopy: cloneWorkingCopy(classified),
      };
    }
    if (
      !allowEmptyEphemeral &&
      input.baseRevisionId === null &&
      !hasMeaningfulAuthoredContent(input.bodyMarkdown)
    ) {
      this.workingCopies.delete(input.noteId);
      return {
        status: "CLEARED",
        acceptedEditGeneration: input.editGeneration,
        workingCopy: null,
      };
    }
    const sequence = this.nextSequence();
    const saved: WorkingCopyRecord = {
      ...input,
      mediaReservation: allowEmptyEphemeral,
      sources: input.sources.map((source) => ({ ...source })),
      id: existing?.id ?? `fake-working-copy-${sequence}`,
      createdAtMs: existing?.createdAtMs ?? this.probe.nowMs + sequence,
      updatedAtMs: existing
        ? Math.max(existing.updatedAtMs + 1, this.probe.nowMs + sequence)
        : this.probe.nowMs + sequence,
    };
    this.workingCopies.set(input.noteId, saved);
    return {
      status: "SAVED",
      acceptedEditGeneration: saved.editGeneration,
      workingCopy: cloneWorkingCopy(saved),
    };
  }

  async discardWorkingCopy(input: DiscardWorkingCopyInput): Promise<boolean> {
    validateEditGeneration(input.expectedEditGeneration, "expectedEditGeneration");
    const workingCopy = this.workingCopies.get(input.noteId);
    if (!workingCopy || workingCopy.editGeneration !== input.expectedEditGeneration) {
      return false;
    }
    return this.workingCopies.delete(input.noteId);
  }

  async loadWorkingCopy(noteId: string): Promise<WorkingCopyRecord | null> {
    const workingCopy = this.workingCopies.get(noteId);
    return workingCopy ? cloneWorkingCopy(workingCopy) : null;
  }

  async listWorkingCopies(): Promise<WorkingCopyRecord[]> {
    return [...this.workingCopies.values()]
      .sort(
        (left, right) =>
          right.updatedAtMs - left.updatedAtMs || left.noteId.localeCompare(right.noteId),
      )
      .map(cloneWorkingCopy);
  }

  async checkpointWorkingCopy(
    input: CheckpointWorkingCopyInput,
  ): Promise<WorkingCopyCheckpointResult> {
    validateEditGeneration(input.expectedEditGeneration, "expectedEditGeneration");
    const workingCopy = this.workingCopies.get(input.noteId);
    if (!workingCopy) throw new Error(`working copy ${input.noteId} was not found`);
    if (workingCopy.editGeneration !== input.expectedEditGeneration) {
      return {
        status: "STALE",
        consumedEditGeneration: null,
        note: null,
        workingCopy: cloneWorkingCopy(workingCopy),
      };
    }
    const sequence = this.nextSequence();
    const current = this.tidbits.get(input.noteId);
    if (workingCopy.baseRevisionId === null && current) {
      throw new Error("ephemeral working-copy identity already belongs to a note");
    }
    if (
      workingCopy.baseRevisionId !== null &&
      (!current || current.currentRevisionId !== workingCopy.baseRevisionId)
    ) {
      throw new Error(`note ${input.noteId} is stale`);
    }
    if (
      workingCopy.baseRevisionId === null &&
      !hasMeaningfulAuthoredContent(workingCopy.bodyMarkdown)
    ) {
      throw new Error("an ephemeral note must contain authored text or media");
    }
    const sources = this.prepareSources(workingCopy.sources);
    const note: TidbitRecord = current
      ? {
          ...current,
          currentRevisionId: `fake-revision-${sequence}`,
          revisionNumber: current.revisionNumber + 1,
          updatedAtMs: Math.max(current.updatedAtMs + 1, this.probe.nowMs + sequence),
          displayTitle: deriveDisplayTitle(workingCopy.bodyMarkdown),
          documentJson: workingCopy.documentJson,
          bodyMarkdown: workingCopy.bodyMarkdown,
          sources,
        }
      : {
          id: input.noteId,
          currentRevisionId: `fake-revision-${sequence}`,
          revisionNumber: 1,
          createdAtMs: this.probe.nowMs + sequence,
          updatedAtMs: this.probe.nowMs + sequence,
          deletedAtMs: null,
          displayTitle: deriveDisplayTitle(workingCopy.bodyMarkdown),
          documentJson: workingCopy.documentJson,
          bodyMarkdown: workingCopy.bodyMarkdown,
          sources,
        };
    this.tidbits.set(note.id, note);
    this.registerCitation(note);
    this.workingCopies.delete(input.noteId);
    return {
      status: "CHECKPOINTED",
      consumedEditGeneration: workingCopy.editGeneration,
      note: cloneTidbit(note),
      workingCopy: null,
    };
  }

  async loadShortcutSettings(): Promise<ShortcutSettingsSnapshot> {
    return cloneShortcutSettings(this.shortcutSettings);
  }

  async setAutomaticUpdateChecks(
    input: SetAutomaticUpdateChecksInput,
  ): Promise<ShortcutSettingsSnapshot> {
    if (input.expectedRevision !== this.shortcutSettings.revision) {
      throw new Error(
        `settings changed before this update: revision is ${this.shortcutSettings.revision}, expected ${input.expectedRevision}`,
      );
    }
    this.shortcutSettings = {
      ...this.shortcutSettings,
      revision: this.shortcutSettings.revision + 1,
      automaticUpdateChecksEnabled: input.enabled,
    };
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
      ...this.shortcutSettings,
      revision: this.shortcutSettings.revision + 1,
      keyboardBindings: input.keyboardBindings.map((binding) => ({ ...binding })),
      shortcutErrors: [],
    };
    return cloneShortcutSettings(this.shortcutSettings);
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

function cloneWorkingCopy(workingCopy: WorkingCopyRecord): WorkingCopyRecord {
  return {
    ...workingCopy,
    sources: workingCopy.sources.map((source) => ({ ...source })),
  };
}

function sameWorkingCopy(workingCopy: WorkingCopyRecord, input: SaveWorkingCopyInput): boolean {
  return (
    workingCopy.baseRevisionId === input.baseRevisionId &&
    workingCopy.documentJson === input.documentJson &&
    workingCopy.bodyMarkdown === input.bodyMarkdown &&
    JSON.stringify(workingCopy.sources) === JSON.stringify(input.sources)
  );
}

function validateEditGeneration(value: number, field: string): void {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new Error(`${field} must be a positive JavaScript-safe integer`);
  }
}

function cloneShortcutSettings(settings: ShortcutSettingsSnapshot): ShortcutSettingsSnapshot {
  return {
    ...settings,
    keyboardBindings: settings.keyboardBindings.map((binding) => ({ ...binding })),
    shortcutErrors: [...settings.shortcutErrors],
  };
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

function deriveDisplayTitle(bodyMarkdown: string): string {
  const line = bodyMarkdown
    .split(/\r?\n/u)
    .map((candidate) => candidate.trim())
    .find((candidate) => candidate && !candidate.startsWith("```") && !candidate.startsWith("~~~"));
  const stripped = line?.replace(/^[#>*+\-\s]+/u, "") || "Untitled note";
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

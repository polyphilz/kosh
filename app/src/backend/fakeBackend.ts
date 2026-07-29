import type {
  Backend,
  BeginResearchProcessInput,
  CitationResolution,
  ClaudeCliDefaults,
  ClaudeSetupStatus,
  ClearDraftInput,
  DeleteTidbitInput,
  DraftRecord,
  EditTidbitInput,
  GenericAttachmentStatusRecord,
  ImageDropIngestResult,
  ImageOcrDiagnostics,
  ImageRecord,
  ImageStatusRecord,
  ListTidbitsInput,
  ListResearchRunsInput,
  PassageEmbeddingIndexStatus,
  PdfRecord,
  PdfStatusRecord,
  RuntimeProbe,
  GroundedResearchAnswer,
  ResearchProcessEvent,
  ResearchRunPage,
  ResearchRunRecord,
  SelectedAttachmentRecord,
  RestoreTidbitInput,
  SaveDraftInput,
  SearchField,
  SearchPassagesInput,
  SearchPassagesResponse,
  SemanticRuntimeLogs,
  SemanticRuntimeStatus,
  SetShortcutSettingsInput,
  ShortcutSettingsSnapshot,
  SourceDraft,
  TidbitDraft,
  TidbitListPage,
  TidbitRecord,
  TidbitSource,
  StartResearchProcessOutput,
} from "./contracts";
import { DEFAULT_KEYBOARD_BINDINGS } from "./contracts";
import { neutralizeUntrustedMediaReferences } from "../markdown/mediaTokens";

interface FakeCitationSnapshot {
  revision: TidbitRecord;
}

export const browserRuntimeProbe: RuntimeProbe = {
  dataDir: "/tmp/kosh-browser-fixture",
  nowMs: 1_785_201_600_000,
  requestId: "fixture-request-1",
};

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
  private readonly researchRuns = new Map<string, ResearchRunRecord>();
  private readonly researchListeners = new Set<(event: ResearchProcessEvent) => void>();
  private readonly researchTimers = new Map<string, ReturnType<typeof setTimeout>>();
  private shortcutSettings: ShortcutSettingsSnapshot = {
    revision: 1,
    keyboardBindings: DEFAULT_KEYBOARD_BINDINGS.map((binding) => ({ ...binding })),
    shortcutErrors: [],
  };
  private sequence = 0;

  constructor(probe: RuntimeProbe = browserRuntimeProbe, tidbits: TidbitRecord[] = []) {
    this.probe = probe;
    for (const tidbit of tidbits) {
      this.tidbits.set(tidbit.id, cloneTidbit(tidbit));
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

  async selectImage(): Promise<string | null> {
    return null;
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
      .filter((tidbit) => tidbit.deletedAtMs === null)
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

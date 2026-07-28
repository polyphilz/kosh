import type {
  Backend,
  CitationResolution,
  ClearDraftInput,
  DeleteTidbitInput,
  DraftRecord,
  EditTidbitInput,
  ListTidbitsInput,
  RuntimeProbe,
  RestoreTidbitInput,
  SaveDraftInput,
  SourceDraft,
  TidbitDraft,
  TidbitListPage,
  TidbitRecord,
  TidbitSource,
} from "./contracts";

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
  private readonly drafts = new Map<string, DraftRecord>();
  private readonly revisionOwners = new Map<string, string>();
  private readonly citations = new Map<string, FakeCitationSnapshot>();
  private readonly sourceIds = new Map<string, string>();
  private readonly tidbits = new Map<string, TidbitRecord>();
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
    if (input.contextKey === "capture") {
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

function validateDraftContextKey(contextKey: string): void {
  if (contextKey === "capture" || /^edit:.+/u.test(contextKey)) {
    return;
  }
  throw new Error("draft context must be capture or edit:<tidbitId>");
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

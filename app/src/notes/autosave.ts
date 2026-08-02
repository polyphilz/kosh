import type {
  CheckpointWorkingCopyInput,
  SaveWorkingCopyInput,
  SourceDraft,
  TidbitRecord,
  WorkingCopyCheckpointResult,
  WorkingCopyRecord,
  WorkingCopySaveResult,
} from "../backend/contracts";

export const WORKING_COPY_DEBOUNCE_MS = 350;
export const CHECKPOINT_IDLE_MS = 2_000;

export type NoteSavePhase =
  | "EPHEMERAL"
  | "DIRTY"
  | "SAVING"
  | "DURABLE"
  | "CHECKPOINTING"
  | "CLEAN"
  | "ERROR";

export type NoteFlushReason = "IDLE" | "NAVIGATION" | "HIDE" | "QUIT" | "UPDATE_RESTART";

export interface NoteAutosaveSnapshot {
  noteId: string;
  baseRevisionId: string | null;
  editGeneration: number;
  durableGeneration: number;
  checkpointedGeneration: number;
  bodyMarkdown: string;
  sources: SourceDraft[];
  phase: NoteSavePhase;
  error: string | null;
}

export interface NoteWorkingCopyGateway {
  saveWorkingCopy(input: SaveWorkingCopyInput): Promise<WorkingCopySaveResult>;
  checkpointWorkingCopy(input: CheckpointWorkingCopyInput): Promise<WorkingCopyCheckpointResult>;
}

interface TimerScheduler {
  clearTimeout(id: number): void;
  setTimeout(handler: () => void, delay: number): number;
}

export interface NoteAutosaveOptions {
  checkpointDelayMs?: number;
  scheduler?: TimerScheduler;
  workingCopyDelayMs?: number;
}

type Listener = () => void;

export class NoteAutosaveCoordinator {
  private readonly gateway: NoteWorkingCopyGateway;
  private readonly scheduler: TimerScheduler;
  private readonly workingCopyDelayMs: number;
  private readonly checkpointDelayMs: number;
  private readonly listeners = new Set<Listener>();
  private state: NoteAutosaveSnapshot;
  private queue: Promise<void> = Promise.resolve();
  private workingCopyTimer: number | null = null;
  private checkpointTimer: number | null = null;
  private disposed = false;

  constructor(
    gateway: NoteWorkingCopyGateway,
    initial: Pick<NoteAutosaveSnapshot, "noteId" | "baseRevisionId" | "bodyMarkdown" | "sources"> &
      Partial<
        Pick<
          NoteAutosaveSnapshot,
          "editGeneration" | "durableGeneration" | "checkpointedGeneration"
        >
      >,
    options: NoteAutosaveOptions = {},
  ) {
    this.gateway = gateway;
    this.scheduler = options.scheduler ?? window;
    this.workingCopyDelayMs = options.workingCopyDelayMs ?? WORKING_COPY_DEBOUNCE_MS;
    this.checkpointDelayMs = options.checkpointDelayMs ?? CHECKPOINT_IDLE_MS;
    const editGeneration = initial.editGeneration ?? 0;
    const durableGeneration = initial.durableGeneration ?? 0;
    const checkpointedGeneration = initial.checkpointedGeneration ?? 0;
    this.state = {
      noteId: initial.noteId,
      baseRevisionId: initial.baseRevisionId,
      editGeneration,
      durableGeneration,
      checkpointedGeneration,
      bodyMarkdown: initial.bodyMarkdown,
      sources: cloneSources(initial.sources),
      phase: phaseForInitialState(initial.baseRevisionId, initial.bodyMarkdown, editGeneration),
      error: null,
    };
  }

  static ephemeral(
    gateway: NoteWorkingCopyGateway,
    options: NoteAutosaveOptions & { noteId?: string } = {},
  ): NoteAutosaveCoordinator {
    return new NoteAutosaveCoordinator(
      gateway,
      {
        noteId: options.noteId ?? createUuidV7(),
        baseRevisionId: null,
        bodyMarkdown: "",
        sources: [],
      },
      options,
    );
  }

  static recovered(
    gateway: NoteWorkingCopyGateway,
    workingCopy: WorkingCopyRecord,
    options: NoteAutosaveOptions = {},
  ): NoteAutosaveCoordinator {
    return new NoteAutosaveCoordinator(
      gateway,
      {
        noteId: workingCopy.noteId,
        baseRevisionId: workingCopy.baseRevisionId,
        editGeneration: workingCopy.editGeneration,
        durableGeneration: workingCopy.editGeneration,
        bodyMarkdown: workingCopy.bodyMarkdown,
        sources: workingCopy.sources,
      },
      options,
    );
  }

  readonly getSnapshot = (): NoteAutosaveSnapshot => this.state;

  readonly subscribe = (listener: Listener): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  update(bodyMarkdown: string, sources: SourceDraft[] = this.state.sources): void {
    if (this.disposed) throw new Error("the note autosave coordinator is disposed");
    if (bodyMarkdown === this.state.bodyMarkdown && sourcesEqual(sources, this.state.sources)) {
      return;
    }
    const editGeneration = nextGeneration(this.state.editGeneration);
    this.publish({
      ...this.state,
      bodyMarkdown,
      sources: cloneSources(sources),
      editGeneration,
      phase: "DIRTY",
      error: null,
    });
    this.schedulePersistence();
  }

  async persistWorkingCopy(): Promise<void> {
    this.clearWorkingCopyTimer();
    await this.enqueue(async () => {
      const target = authoredSnapshot(this.state);
      if (target.editGeneration <= this.state.durableGeneration) return;
      await this.saveTarget(target);
    });
  }

  async flush(reason: NoteFlushReason): Promise<TidbitRecord | null> {
    this.clearTimers();
    return this.enqueue(async () => this.flushNewest(reason));
  }

  async retry(): Promise<TidbitRecord | null> {
    return this.flush("IDLE");
  }

  dispose(): void {
    this.disposed = true;
    this.clearTimers();
    this.listeners.clear();
  }

  private async flushNewest(_reason: NoteFlushReason): Promise<TidbitRecord | null> {
    while (true) {
      const target = authoredSnapshot(this.state);
      if (target.editGeneration === 0) {
        this.publish({ ...this.state, phase: "EPHEMERAL", error: null });
        return null;
      }
      if (target.editGeneration > this.state.durableGeneration) {
        await this.saveTarget(target);
      }
      if (target.editGeneration !== this.state.editGeneration) continue;
      if (
        this.state.baseRevisionId === null &&
        !hasMeaningfulAuthoredContent(target.bodyMarkdown)
      ) {
        this.publish({
          ...this.state,
          durableGeneration: Math.max(this.state.durableGeneration, target.editGeneration),
          phase: "EPHEMERAL",
          error: null,
        });
        return null;
      }
      this.publish({ ...this.state, phase: "CHECKPOINTING", error: null });
      let result: WorkingCopyCheckpointResult;
      try {
        result = await this.gateway.checkpointWorkingCopy({
          noteId: target.noteId,
          expectedEditGeneration: target.editGeneration,
        });
      } catch (reason) {
        this.fail(reason);
        throw reason;
      }
      if (result.status === "STALE") {
        const accepted = result.workingCopy?.editGeneration ?? 0;
        const error = new Error(
          `working copy changed before checkpoint (database generation ${accepted}, local generation ${target.editGeneration})`,
        );
        this.fail(error);
        throw error;
      }
      if (!result.note || result.consumedEditGeneration !== target.editGeneration) {
        const error = new Error("checkpoint did not consume the requested working-copy generation");
        this.fail(error);
        throw error;
      }
      const note = result.note;
      const hasNewerLocalEdit = this.state.editGeneration !== target.editGeneration;
      this.publish({
        ...this.state,
        baseRevisionId: note.currentRevisionId,
        durableGeneration: Math.max(this.state.durableGeneration, target.editGeneration),
        checkpointedGeneration: target.editGeneration,
        phase: hasNewerLocalEdit ? "DIRTY" : "CLEAN",
        error: null,
      });
      if (!hasNewerLocalEdit) return note;
    }
  }

  private async saveTarget(target: AuthoredSnapshot): Promise<void> {
    this.publish({ ...this.state, phase: "SAVING", error: null });
    let result: WorkingCopySaveResult;
    try {
      result = await this.gateway.saveWorkingCopy({
        noteId: target.noteId,
        baseRevisionId: target.baseRevisionId,
        editGeneration: target.editGeneration,
        bodyMarkdown: target.bodyMarkdown,
        sources: cloneSources(target.sources),
      });
    } catch (reason) {
      this.fail(reason);
      throw reason;
    }
    if (result.status === "STALE") {
      const accepted = result.acceptedEditGeneration;
      if (accepted > this.state.editGeneration) {
        const error = new Error(
          `database working copy is newer than this editor (${accepted} > ${this.state.editGeneration})`,
        );
        this.fail(error);
        throw error;
      }
      return;
    }
    const durableGeneration = Math.max(this.state.durableGeneration, result.acceptedEditGeneration);
    const unchanged = this.state.editGeneration === target.editGeneration;
    this.publish({
      ...this.state,
      durableGeneration,
      phase: unchanged ? (result.status === "CLEARED" ? "EPHEMERAL" : "DURABLE") : "DIRTY",
      error: null,
    });
  }

  private schedulePersistence(): void {
    this.clearTimers();
    this.workingCopyTimer = this.scheduler.setTimeout(() => {
      this.workingCopyTimer = null;
      void this.persistWorkingCopy().catch(() => undefined);
    }, this.workingCopyDelayMs);
    this.checkpointTimer = this.scheduler.setTimeout(() => {
      this.checkpointTimer = null;
      void this.flush("IDLE").catch(() => undefined);
    }, this.checkpointDelayMs);
  }

  private enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.queue.catch(() => undefined).then(operation);
    this.queue = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  private fail(reason: unknown): void {
    this.publish({
      ...this.state,
      phase: "ERROR",
      error: reason instanceof Error ? reason.message : String(reason),
    });
  }

  private publish(state: NoteAutosaveSnapshot): void {
    this.state = state;
    this.listeners.forEach((listener) => listener());
  }

  private clearTimers(): void {
    this.clearWorkingCopyTimer();
    if (this.checkpointTimer !== null) {
      this.scheduler.clearTimeout(this.checkpointTimer);
      this.checkpointTimer = null;
    }
  }

  private clearWorkingCopyTimer(): void {
    if (this.workingCopyTimer !== null) {
      this.scheduler.clearTimeout(this.workingCopyTimer);
      this.workingCopyTimer = null;
    }
  }
}

interface AuthoredSnapshot {
  noteId: string;
  baseRevisionId: string | null;
  editGeneration: number;
  bodyMarkdown: string;
  sources: SourceDraft[];
}

function authoredSnapshot(state: NoteAutosaveSnapshot): AuthoredSnapshot {
  return {
    noteId: state.noteId,
    baseRevisionId: state.baseRevisionId,
    editGeneration: state.editGeneration,
    bodyMarkdown: state.bodyMarkdown,
    sources: cloneSources(state.sources),
  };
}

function phaseForInitialState(
  baseRevisionId: string | null,
  bodyMarkdown: string,
  editGeneration: number,
): NoteSavePhase {
  if (editGeneration > 0) return "DURABLE";
  return baseRevisionId === null && !hasMeaningfulAuthoredContent(bodyMarkdown)
    ? "EPHEMERAL"
    : "CLEAN";
}

function nextGeneration(generation: number): number {
  if (
    !Number.isSafeInteger(generation) ||
    generation < 0 ||
    generation >= Number.MAX_SAFE_INTEGER
  ) {
    throw new Error("note edit generation overflow");
  }
  return generation + 1;
}

function cloneSources(sources: readonly SourceDraft[]): SourceDraft[] {
  return sources.map((source) => ({ ...source }));
}

function sourcesEqual(left: readonly SourceDraft[], right: readonly SourceDraft[]): boolean {
  return (
    left.length === right.length &&
    left.every(
      (source, index) => source.label === right[index]?.label && source.url === right[index]?.url,
    )
  );
}

export function hasMeaningfulAuthoredContent(markdown: string): boolean {
  const mediaAware = markdown.replace(
    /\{\{kosh:(?:image|attachment|pdf):[^{}\r\n]+\}\}/gu,
    "media",
  );
  const withoutTags = mediaAware.replace(/<[^>]*>/gu, "");
  return withoutTags.replace(/[`*_#>+\-[\]()~$\\\s]/gu, "").length > 0;
}

export function createUuidV7(
  nowMs = Date.now(),
  random = crypto.getRandomValues.bind(crypto),
): string {
  if (!Number.isSafeInteger(nowMs) || nowMs < 0 || nowMs > 0xffff_ffff_ffff) {
    throw new Error("UUIDv7 timestamp must fit in 48 bits");
  }
  const bytes = random(new Uint8Array(16));
  let timestamp = nowMs;
  for (let index = 5; index >= 0; index -= 1) {
    bytes[index] = timestamp & 0xff;
    timestamp = Math.floor(timestamp / 256);
  }
  bytes[6] = 0x70 | (bytes[6]! & 0x0f);
  bytes[8] = 0x80 | (bytes[8]! & 0x3f);
  const hex = [...bytes].map((byte) => byte.toString(16).padStart(2, "0"));
  return `${hex.slice(0, 4).join("")}-${hex.slice(4, 6).join("")}-${hex.slice(6, 8).join("")}-${hex.slice(8, 10).join("")}-${hex.slice(10).join("")}`;
}

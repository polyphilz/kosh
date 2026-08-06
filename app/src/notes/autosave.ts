import type {
  CheckpointWorkingCopyInput,
  DiscardWorkingCopyInput,
  SaveWorkingCopyInput,
  SourceDraft,
  TidbitRecord,
  WorkingCopyCheckpointResult,
  WorkingCopyRecord,
  WorkingCopySaveResult,
} from "../backend/contracts";
import {
  createDurableKoshDocument,
  createEmptyKoshDocument,
  createKoshDocumentFromMarkdown,
  createKoshDocumentFromPlainText,
} from "../editor/document";

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
  baseContentVersionId: string | null;
  editGeneration: number;
  durableGeneration: number;
  checkpointedGeneration: number;
  documentJson: string;
  bodyMarkdown: string;
  sources: SourceDraft[];
  phase: NoteSavePhase;
  error: string | null;
}

export interface NoteWorkingCopyGateway {
  saveWorkingCopy(input: SaveWorkingCopyInput): Promise<WorkingCopySaveResult>;
  reserveWorkingCopyForMedia(input: SaveWorkingCopyInput): Promise<WorkingCopySaveResult>;
  discardWorkingCopy(input: DiscardWorkingCopyInput): Promise<boolean>;
  checkpointWorkingCopy(input: CheckpointWorkingCopyInput): Promise<WorkingCopyCheckpointResult>;
}

export interface NoteMediaReservation {
  draftId: string;
  generation: number;
  discardable: boolean;
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
  private renderedState: NoteAutosaveSnapshot;
  private queue: Promise<void> = Promise.resolve();
  private workingCopyTimer: number | null = null;
  private checkpointTimer: number | null = null;
  private workingCopyId: string | null;
  private recoveredMediaReservationGeneration: number | null;
  private notificationTimer: number | null = null;
  private notificationPending = false;
  private disposed = false;

  constructor(
    gateway: NoteWorkingCopyGateway,
    initial: Pick<
      NoteAutosaveSnapshot,
      "noteId" | "baseContentVersionId" | "bodyMarkdown" | "sources"
    > &
      Partial<
        Pick<
          NoteAutosaveSnapshot,
          "documentJson" | "editGeneration" | "durableGeneration" | "checkpointedGeneration"
        >
      >,
    options: NoteAutosaveOptions = {},
    workingCopyId: string | null = null,
    recoveredMediaReservationGeneration: number | null = null,
  ) {
    this.gateway = gateway;
    this.scheduler = options.scheduler ?? window;
    this.workingCopyDelayMs = options.workingCopyDelayMs ?? WORKING_COPY_DEBOUNCE_MS;
    this.checkpointDelayMs = options.checkpointDelayMs ?? CHECKPOINT_IDLE_MS;
    this.workingCopyId = workingCopyId;
    this.recoveredMediaReservationGeneration = recoveredMediaReservationGeneration;
    const editGeneration = initial.editGeneration ?? 0;
    const durableGeneration = initial.durableGeneration ?? 0;
    const checkpointedGeneration = initial.checkpointedGeneration ?? 0;
    this.state = {
      noteId: initial.noteId,
      baseContentVersionId: initial.baseContentVersionId,
      editGeneration,
      durableGeneration,
      checkpointedGeneration,
      documentJson: initial.documentJson ?? createKoshDocumentFromMarkdown(initial.bodyMarkdown),
      bodyMarkdown: initial.bodyMarkdown,
      sources: cloneSources(initial.sources),
      phase: phaseForInitialState(initial.baseContentVersionId, editGeneration),
      error: null,
    };
    this.renderedState = this.state;
  }

  static ephemeral(
    gateway: NoteWorkingCopyGateway,
    options: NoteAutosaveOptions & { noteId?: string } = {},
  ): NoteAutosaveCoordinator {
    return new NoteAutosaveCoordinator(
      gateway,
      {
        noteId: options.noteId ?? createUuidV7(),
        baseContentVersionId: null,
        documentJson: createEmptyKoshDocument(),
        bodyMarkdown: "",
        sources: [],
      },
      options,
      null,
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
        baseContentVersionId: workingCopy.baseContentVersionId,
        editGeneration: workingCopy.editGeneration,
        durableGeneration: workingCopy.editGeneration,
        documentJson: workingCopy.documentJson,
        bodyMarkdown: workingCopy.bodyMarkdown,
        sources: workingCopy.sources,
      },
      options,
      workingCopy.id,
      workingCopy.mediaReservation ? workingCopy.editGeneration : null,
    );
  }

  readonly getSnapshot = (): NoteAutosaveSnapshot => this.state;

  readonly getRenderedSnapshot = (): NoteAutosaveSnapshot => this.renderedState;

  readonly subscribe = (listener: Listener): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  update(
    bodyMarkdown: string,
    sources: SourceDraft[] = this.state.sources,
    documentJson = createKoshDocumentFromPlainText(bodyMarkdown),
  ): void {
    if (this.disposed) throw new Error("the note autosave coordinator is disposed");
    if (
      documentJson === this.state.documentJson &&
      bodyMarkdown === this.state.bodyMarkdown &&
      sourcesEqual(sources, this.state.sources)
    ) {
      return;
    }
    const editGeneration = nextGeneration(this.state.editGeneration);
    this.publish({
      ...this.state,
      documentJson,
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
      if (target.editGeneration <= this.state.checkpointedGeneration) return;
      if (target.editGeneration <= this.state.durableGeneration) return;
      await this.saveTarget(target);
    });
  }

  async prepareMedia(): Promise<NoteMediaReservation> {
    this.clearTimers();
    return this.enqueue(async () => {
      const current = authoredSnapshot(this.state);
      if (current.editGeneration > this.state.durableGeneration) {
        await this.saveTarget(current);
      }
      if (this.workingCopyId !== null) {
        return {
          draftId: this.workingCopyId,
          generation: this.state.editGeneration,
          discardable: false,
        };
      }

      const generation = nextGeneration(this.state.editGeneration);
      const target = { ...authoredSnapshot(this.state), editGeneration: generation };
      this.publish({
        ...this.state,
        editGeneration: generation,
        phase: "SAVING",
        error: null,
      });
      let result: WorkingCopySaveResult;
      try {
        result = await this.gateway.reserveWorkingCopyForMedia({
          noteId: target.noteId,
          baseContentVersionId: target.baseContentVersionId,
          editGeneration: target.editGeneration,
          documentJson: target.documentJson,
          bodyMarkdown: target.bodyMarkdown,
          sources: cloneSources(target.sources),
        });
      } catch (reason) {
        this.fail(reason);
        throw reason;
      }
      if (result.status !== "SAVED" || result.workingCopy === null) {
        const error = new Error("media reservation did not create a working copy");
        this.fail(error);
        throw error;
      }
      this.workingCopyId = result.workingCopy.id;
      const unchanged = this.state.editGeneration === generation;
      this.publish({
        ...this.state,
        durableGeneration: Math.max(this.state.durableGeneration, generation),
        phase: unchanged ? "DURABLE" : "DIRTY",
        error: null,
      });
      return {
        draftId: result.workingCopy.id,
        generation,
        discardable: true,
      };
    });
  }

  async discardMediaReservation(reservation: NoteMediaReservation): Promise<boolean> {
    if (!reservation.discardable) {
      this.scheduleCheckpoint();
      return false;
    }
    return this.enqueue(async () => {
      if (this.state.editGeneration !== reservation.generation) return false;
      let discarded: boolean;
      try {
        discarded = await this.gateway.discardWorkingCopy({
          noteId: this.state.noteId,
          expectedEditGeneration: reservation.generation,
        });
      } catch (reason) {
        this.fail(reason);
        throw reason;
      }
      if (!discarded) return false;
      this.workingCopyId = null;
      this.publish({
        ...this.state,
        durableGeneration: Math.max(this.state.durableGeneration, reservation.generation),
        checkpointedGeneration: Math.max(this.state.checkpointedGeneration, reservation.generation),
        phase: this.state.baseContentVersionId === null ? "EPHEMERAL" : "CLEAN",
        error: null,
      });
      return true;
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
    if (this.notificationTimer !== null) {
      this.scheduler.clearTimeout(this.notificationTimer);
      this.notificationTimer = null;
    }
    this.notificationPending = false;
    this.listeners.clear();
  }

  private async flushNewest(_reason: NoteFlushReason): Promise<TidbitRecord | null> {
    while (true) {
      const target = authoredSnapshot(this.state);
      if (target.editGeneration <= this.state.checkpointedGeneration) {
        this.publish({
          ...this.state,
          phase: this.state.baseContentVersionId === null ? "EPHEMERAL" : "CLEAN",
          error: null,
        });
        return null;
      }
      if (target.editGeneration === 0) {
        this.publish({ ...this.state, phase: "EPHEMERAL", error: null });
        return null;
      }
      const reservationGeneration = this.recoveredMediaReservationGeneration;
      if (reservationGeneration !== null) {
        if (target.editGeneration === reservationGeneration) {
          let discarded: boolean;
          try {
            discarded = await this.gateway.discardWorkingCopy({
              noteId: target.noteId,
              expectedEditGeneration: reservationGeneration,
            });
          } catch (reason) {
            this.fail(reason);
            throw reason;
          }
          if (!discarded) {
            const error = new Error("recovered media reservation changed before reconciliation");
            this.fail(error);
            throw error;
          }
          this.workingCopyId = null;
          this.recoveredMediaReservationGeneration = null;
          this.publish({
            ...this.state,
            checkpointedGeneration: Math.max(
              this.state.checkpointedGeneration,
              reservationGeneration,
            ),
            phase: this.state.baseContentVersionId === null ? "EPHEMERAL" : "CLEAN",
            error: null,
          });
          return null;
        }
        await this.saveTarget(target);
        continue;
      }
      if (target.editGeneration > this.state.durableGeneration) {
        await this.saveTarget(target);
      }
      if (target.editGeneration !== this.state.editGeneration) continue;
      if (this.state.baseContentVersionId === null && this.workingCopyId === null) {
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
      this.workingCopyId = null;
      const hasNewerLocalEdit = this.state.editGeneration !== target.editGeneration;
      this.publish({
        ...this.state,
        baseContentVersionId: note.contentVersionId,
        durableGeneration: Math.max(this.state.durableGeneration, target.editGeneration),
        checkpointedGeneration: target.editGeneration,
        phase: hasNewerLocalEdit ? "DIRTY" : "CLEAN",
        error: null,
      });
      if (!hasNewerLocalEdit) return note;
    }
  }

  private async saveTarget(target: AuthoredSnapshot): Promise<WorkingCopySaveResult> {
    this.publish({ ...this.state, phase: "SAVING", error: null });
    let result: WorkingCopySaveResult;
    try {
      result = await this.gateway.saveWorkingCopy({
        noteId: target.noteId,
        baseContentVersionId: target.baseContentVersionId,
        editGeneration: target.editGeneration,
        documentJson: target.documentJson,
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
      return result;
    }
    this.workingCopyId = result.workingCopy?.id ?? null;
    this.recoveredMediaReservationGeneration = null;
    const durableGeneration = Math.max(this.state.durableGeneration, result.acceptedEditGeneration);
    const unchanged = this.state.editGeneration === target.editGeneration;
    this.publish({
      ...this.state,
      durableGeneration,
      phase: unchanged ? (result.status === "CLEARED" ? "EPHEMERAL" : "DURABLE") : "DIRTY",
      error: null,
    });
    return result;
  }

  private schedulePersistence(): void {
    this.clearTimers();
    this.workingCopyTimer = this.scheduler.setTimeout(() => {
      this.workingCopyTimer = null;
      void this.persistWorkingCopy().catch(() => undefined);
    }, this.workingCopyDelayMs);
    this.scheduleCheckpoint();
  }

  private scheduleCheckpoint(): void {
    if (this.disposed || this.state.editGeneration <= this.state.checkpointedGeneration) return;
    if (this.checkpointTimer !== null) this.scheduler.clearTimeout(this.checkpointTimer);
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
    if (this.notificationTimer !== null) {
      this.notificationPending = true;
      return;
    }
    this.notificationPending = false;
    this.notificationTimer = this.scheduler.setTimeout(() => {
      this.notificationTimer = null;
      if (this.disposed || !this.notificationPending) return;
      this.notificationPending = false;
      this.notifyListeners();
    }, 0);
    this.notifyListeners();
  }

  private notifyListeners(): void {
    this.renderedState = this.state;
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
  baseContentVersionId: string | null;
  editGeneration: number;
  documentJson: string;
  bodyMarkdown: string;
  sources: SourceDraft[];
}

function authoredSnapshot(state: NoteAutosaveSnapshot): AuthoredSnapshot {
  return {
    noteId: state.noteId,
    baseContentVersionId: state.baseContentVersionId,
    editGeneration: state.editGeneration,
    documentJson: createDurableKoshDocument(state.documentJson),
    bodyMarkdown: state.bodyMarkdown,
    sources: cloneSources(state.sources),
  };
}

function phaseForInitialState(
  baseContentVersionId: string | null,
  editGeneration: number,
): NoteSavePhase {
  if (editGeneration > 0) return "DURABLE";
  return baseContentVersionId === null ? "EPHEMERAL" : "CLEAN";
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

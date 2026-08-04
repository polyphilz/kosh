import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  TidbitRecord,
  WorkingCopyCheckpointResult,
  WorkingCopyRecord,
  WorkingCopySaveResult,
} from "../../src/backend/contracts";
import {
  CHECKPOINT_IDLE_MS,
  NoteAutosaveCoordinator,
  WORKING_COPY_DEBOUNCE_MS,
  createUuidV7,
  type NoteFlushReason,
  type NoteWorkingCopyGateway,
} from "../../src/notes/autosave";
import { createKoshDocumentFromMarkdown } from "../../src/editor/document";

const NOTE_ID = "019f547b-6200-7000-8000-000000008001";
const REVISION_1 = "019f547b-6200-7000-8000-000000008002";
const REVISION_2 = "019f547b-6200-7000-8000-000000008003";

function note(revisionId = REVISION_1, revisionNumber = 1, bodyMarkdown = "alpha"): TidbitRecord {
  return {
    id: NOTE_ID,
    currentRevisionId: revisionId,
    revisionNumber,
    createdAtMs: 1,
    updatedAtMs: revisionNumber,
    deletedAtMs: null,
    displayTitle: bodyMarkdown || "Untitled note",
    documentJson: createKoshDocumentFromMarkdown(bodyMarkdown),
    bodyMarkdown,
    sources: [],
  };
}

function saved(
  generation: number,
  bodyMarkdown: string,
  baseRevisionId: string | null = null,
): WorkingCopySaveResult {
  return {
    status: "SAVED",
    acceptedEditGeneration: generation,
    workingCopy: workingCopy(generation, bodyMarkdown, baseRevisionId),
  };
}

function workingCopy(
  generation: number,
  bodyMarkdown: string,
  baseRevisionId: string | null = null,
  mediaReservation = false,
): WorkingCopyRecord {
  return {
    id: `draft-${generation}`,
    noteId: NOTE_ID,
    baseRevisionId,
    editGeneration: generation,
    mediaReservation,
    documentJson: createKoshDocumentFromMarkdown(bodyMarkdown),
    bodyMarkdown,
    sources: [],
    createdAtMs: 1,
    updatedAtMs: generation,
  };
}

function checkpointed(
  generation: number,
  record: TidbitRecord = note(),
): WorkingCopyCheckpointResult {
  return {
    status: "CHECKPOINTED",
    consumedEditGeneration: generation,
    note: record,
    workingCopy: null,
  };
}

function gateway(): {
  saveWorkingCopy: ReturnType<typeof vi.fn<NoteWorkingCopyGateway["saveWorkingCopy"]>>;
  reserveWorkingCopyForMedia: ReturnType<
    typeof vi.fn<NoteWorkingCopyGateway["reserveWorkingCopyForMedia"]>
  >;
  discardWorkingCopy: ReturnType<typeof vi.fn<NoteWorkingCopyGateway["discardWorkingCopy"]>>;
  checkpointWorkingCopy: ReturnType<typeof vi.fn<NoteWorkingCopyGateway["checkpointWorkingCopy"]>>;
} {
  return {
    saveWorkingCopy: vi.fn(async (input) =>
      saved(input.editGeneration, input.bodyMarkdown, input.baseRevisionId),
    ),
    reserveWorkingCopyForMedia: vi.fn(async (input) =>
      saved(input.editGeneration, input.bodyMarkdown, input.baseRevisionId),
    ),
    discardWorkingCopy: vi.fn(async () => true),
    checkpointWorkingCopy: vi.fn(async (input) => checkpointed(input.expectedEditGeneration)),
  };
}

function deferred<T>(): {
  promise: Promise<T>;
  resolve(value: T): void;
} {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("note autosave coordinator", () => {
  it("keeps a blank ephemeral note out of durable note history", async () => {
    const backend = gateway();
    backend.saveWorkingCopy.mockResolvedValue({
      status: "CLEARED",
      acceptedEditGeneration: 1,
      workingCopy: null,
    });
    const coordinator = NoteAutosaveCoordinator.ephemeral(backend, { noteId: NOTE_ID });

    coordinator.update("# \n\n- ");
    await coordinator.persistWorkingCopy();
    const result = await coordinator.flush("QUIT");

    expect(result).toBeNull();
    expect(backend.saveWorkingCopy).toHaveBeenCalledOnce();
    expect(backend.checkpointWorkingCopy).not.toHaveBeenCalled();
    expect(coordinator.getSnapshot()).toMatchObject({
      phase: "EPHEMERAL",
      durableGeneration: 1,
      checkpointedGeneration: 0,
    });
  });

  it("coalesces rapid edits into one short-debounce working-copy write", async () => {
    const backend = gateway();
    const coordinator = NoteAutosaveCoordinator.ephemeral(backend, { noteId: NOTE_ID });

    coordinator.update("a");
    await vi.advanceTimersByTimeAsync(WORKING_COPY_DEBOUNCE_MS - 1);
    coordinator.update("alphabet");
    await vi.advanceTimersByTimeAsync(WORKING_COPY_DEBOUNCE_MS);

    expect(backend.saveWorkingCopy).toHaveBeenCalledOnce();
    expect(backend.saveWorkingCopy).toHaveBeenCalledWith(
      expect.objectContaining({
        noteId: NOTE_ID,
        baseRevisionId: null,
        editGeneration: 2,
        bodyMarkdown: "alphabet",
        sources: [],
      }),
    );
    expect(coordinator.getSnapshot()).toMatchObject({ phase: "DURABLE", durableGeneration: 2 });
  });

  it("coalesces rapid render notifications around the newest edit", async () => {
    const backend = gateway();
    const coordinator = NoteAutosaveCoordinator.ephemeral(backend, { noteId: NOTE_ID });
    const listener = vi.fn();
    coordinator.subscribe(listener);

    for (let index = 1; index <= 100; index += 1) {
      coordinator.update("a".repeat(index));
    }

    expect(coordinator.getSnapshot()).toMatchObject({
      bodyMarkdown: "a".repeat(100),
      editGeneration: 100,
    });
    expect(coordinator.getRenderedSnapshot()).toMatchObject({
      bodyMarkdown: "a",
      editGeneration: 1,
    });
    expect(listener).toHaveBeenCalledOnce();

    await vi.advanceTimersByTimeAsync(0);

    expect(listener).toHaveBeenCalledTimes(2);
    expect(coordinator.getRenderedSnapshot()).toMatchObject({
      bodyMarkdown: "a".repeat(100),
      editGeneration: 100,
    });
  });

  it("turns an idle durable copy into one titleless revision", async () => {
    const backend = gateway();
    const coordinator = NoteAutosaveCoordinator.ephemeral(backend, { noteId: NOTE_ID });

    coordinator.update("alpha");
    await vi.advanceTimersByTimeAsync(CHECKPOINT_IDLE_MS);

    expect(backend.saveWorkingCopy).toHaveBeenCalledOnce();
    expect(backend.checkpointWorkingCopy).toHaveBeenCalledWith({
      noteId: NOTE_ID,
      expectedEditGeneration: 1,
    });
    expect(coordinator.getSnapshot()).toMatchObject({
      baseRevisionId: REVISION_1,
      phase: "CLEAN",
      checkpointedGeneration: 1,
    });
  });

  it.each<NoteFlushReason>(["NAVIGATION", "HIDE", "QUIT", "UPDATE_RESTART"])(
    "fences unsaved work before %s",
    async (reason) => {
      const backend = gateway();
      const coordinator = NoteAutosaveCoordinator.ephemeral(backend, { noteId: NOTE_ID });
      coordinator.update(`before ${reason}`);

      await expect(coordinator.flush(reason)).resolves.toMatchObject({
        currentRevisionId: REVISION_1,
      });

      expect(backend.saveWorkingCopy).toHaveBeenCalledOnce();
      expect(backend.checkpointWorkingCopy).toHaveBeenCalledOnce();
      expect(coordinator.getSnapshot().phase).toBe("CLEAN");
    },
  );

  it("does not checkpoint the same generation again across lifecycle fences", async () => {
    const backend = gateway();
    const coordinator = NoteAutosaveCoordinator.ephemeral(backend, { noteId: NOTE_ID });
    coordinator.update("already safe");

    await coordinator.flush("HIDE");
    await coordinator.flush("QUIT");

    expect(backend.saveWorkingCopy).toHaveBeenCalledOnce();
    expect(backend.checkpointWorkingCopy).toHaveBeenCalledOnce();
  });

  it("does not let an older checkpoint completion mark a newer edit clean", async () => {
    const backend = gateway();
    const firstCheckpoint = deferred<WorkingCopyCheckpointResult>();
    backend.checkpointWorkingCopy
      .mockImplementationOnce(() => firstCheckpoint.promise)
      .mockResolvedValueOnce(checkpointed(2, note(REVISION_2, 2, "alpha beta")));
    const coordinator = NoteAutosaveCoordinator.ephemeral(backend, { noteId: NOTE_ID });
    coordinator.update("alpha");

    const flush = coordinator.flush("NAVIGATION");
    await vi.waitFor(() => expect(backend.checkpointWorkingCopy).toHaveBeenCalledOnce());
    coordinator.update("alpha beta");
    expect(coordinator.getSnapshot().phase).toBe("DIRTY");
    firstCheckpoint.resolve(checkpointed(1));

    await expect(flush).resolves.toMatchObject({ currentRevisionId: REVISION_2 });
    expect(backend.saveWorkingCopy).toHaveBeenNthCalledWith(
      2,
      expect.objectContaining({
        noteId: NOTE_ID,
        baseRevisionId: REVISION_1,
        editGeneration: 2,
        bodyMarkdown: "alpha beta",
        sources: [],
      }),
    );
    expect(backend.checkpointWorkingCopy).toHaveBeenNthCalledWith(2, {
      noteId: NOTE_ID,
      expectedEditGeneration: 2,
    });
    expect(coordinator.getSnapshot()).toMatchObject({
      baseRevisionId: REVISION_2,
      checkpointedGeneration: 2,
      phase: "CLEAN",
    });
  });

  it("surfaces a failed save until an explicit retry succeeds", async () => {
    const backend = gateway();
    backend.saveWorkingCopy.mockRejectedValueOnce(new Error("disk full"));
    const coordinator = NoteAutosaveCoordinator.ephemeral(backend, { noteId: NOTE_ID });
    coordinator.update("recover me");

    await expect(coordinator.persistWorkingCopy()).rejects.toThrow("disk full");
    expect(coordinator.getSnapshot()).toMatchObject({ phase: "ERROR", error: "disk full" });

    await expect(coordinator.retry()).resolves.toMatchObject({ currentRevisionId: REVISION_1 });
    expect(coordinator.getSnapshot()).toMatchObject({ phase: "CLEAN", error: null });
  });

  it("rejects a lifecycle fence when checkpointing fails", async () => {
    const backend = gateway();
    backend.checkpointWorkingCopy.mockRejectedValueOnce(new Error("checkpoint unavailable"));
    const coordinator = NoteAutosaveCoordinator.ephemeral(backend, { noteId: NOTE_ID });
    coordinator.update("must survive");

    await expect(coordinator.flush("QUIT")).rejects.toThrow("checkpoint unavailable");
    expect(coordinator.getSnapshot()).toMatchObject({
      phase: "ERROR",
      error: "checkpoint unavailable",
      durableGeneration: 1,
    });
  });

  it("rehydrates an interrupted working copy without pretending it is checkpointed", () => {
    const backend = gateway();
    const coordinator = NoteAutosaveCoordinator.recovered(
      backend,
      workingCopy(7, "recoverable note", REVISION_1),
    );

    expect(coordinator.getSnapshot()).toMatchObject({
      noteId: NOTE_ID,
      baseRevisionId: REVISION_1,
      editGeneration: 7,
      durableGeneration: 7,
      checkpointedGeneration: 0,
      phase: "DURABLE",
    });
  });

  it("clears an abandoned blank media reservation before lifecycle checkpoint", async () => {
    const backend = gateway();
    const coordinator = NoteAutosaveCoordinator.recovered(backend, workingCopy(7, "", null, true));

    await expect(coordinator.flush("QUIT")).resolves.toBeNull();

    expect(backend.discardWorkingCopy).toHaveBeenCalledWith({
      noteId: NOTE_ID,
      expectedEditGeneration: 7,
    });
    expect(backend.checkpointWorkingCopy).not.toHaveBeenCalled();
    expect(coordinator.getSnapshot()).toMatchObject({
      editGeneration: 7,
      durableGeneration: 7,
      phase: "EPHEMERAL",
    });
  });

  it("discards an abandoned media reservation for an unchanged existing note", async () => {
    const backend = gateway();
    const coordinator = NoteAutosaveCoordinator.recovered(
      backend,
      workingCopy(7, "alpha", REVISION_1, true),
    );

    await expect(coordinator.flush("QUIT")).resolves.toBeNull();

    expect(backend.discardWorkingCopy).toHaveBeenCalledWith({
      noteId: NOTE_ID,
      expectedEditGeneration: 7,
    });
    expect(backend.saveWorkingCopy).not.toHaveBeenCalled();
    expect(backend.checkpointWorkingCopy).not.toHaveBeenCalled();
    expect(coordinator.getSnapshot()).toMatchObject({ phase: "CLEAN" });
  });

  it("restores the idle checkpoint after canceling media on a durable working copy", async () => {
    const backend = gateway();
    const coordinator = NoteAutosaveCoordinator.ephemeral(backend, { noteId: NOTE_ID });
    coordinator.update("alpha");
    await coordinator.persistWorkingCopy();

    const reservation = await coordinator.prepareMedia();
    expect(reservation.discardable).toBe(false);
    await expect(coordinator.discardMediaReservation(reservation)).resolves.toBe(false);
    await vi.advanceTimersByTimeAsync(CHECKPOINT_IDLE_MS);

    expect(backend.checkpointWorkingCopy).toHaveBeenCalledWith({
      noteId: NOTE_ID,
      expectedEditGeneration: 1,
    });
  });

  it("reserves an untouched note for media and checkpoints the inserted token", async () => {
    const backend = gateway();
    const coordinator = NoteAutosaveCoordinator.ephemeral(backend, { noteId: NOTE_ID });

    const reservation = await coordinator.prepareMedia();
    coordinator.update("{{kosh:image:019f547b-6200-7000-8000-000000008099}}");
    await expect(coordinator.flush("QUIT")).resolves.toMatchObject({ id: NOTE_ID });

    expect(reservation).toEqual({ draftId: "draft-1", generation: 1, discardable: true });
    expect(backend.reserveWorkingCopyForMedia).toHaveBeenCalledOnce();
    expect(backend.saveWorkingCopy).toHaveBeenCalledWith(
      expect.objectContaining({ editGeneration: 2 }),
    );
    expect(backend.checkpointWorkingCopy).toHaveBeenCalledWith({
      noteId: NOTE_ID,
      expectedEditGeneration: 2,
    });
  });

  it("discards a canceled blank media reservation without creating a note", async () => {
    const backend = gateway();
    const coordinator = NoteAutosaveCoordinator.ephemeral(backend, { noteId: NOTE_ID });

    const reservation = await coordinator.prepareMedia();
    await expect(coordinator.discardMediaReservation(reservation)).resolves.toBe(true);
    await expect(coordinator.flush("QUIT")).resolves.toBeNull();

    expect(backend.discardWorkingCopy).toHaveBeenCalledWith({
      noteId: NOTE_ID,
      expectedEditGeneration: 1,
    });
    expect(backend.checkpointWorkingCopy).not.toHaveBeenCalled();
    expect(coordinator.getSnapshot()).toMatchObject({
      phase: "EPHEMERAL",
      checkpointedGeneration: 1,
    });
  });

  it("does not discard a reservation after a newer authored edit", async () => {
    const backend = gateway();
    const coordinator = NoteAutosaveCoordinator.ephemeral(backend, { noteId: NOTE_ID });

    const reservation = await coordinator.prepareMedia();
    coordinator.update("keep this newer edit");

    await expect(coordinator.discardMediaReservation(reservation)).resolves.toBe(false);
    expect(backend.discardWorkingCopy).not.toHaveBeenCalled();
    await expect(coordinator.flush("QUIT")).resolves.toMatchObject({ id: NOTE_ID });
  });
});

describe("note autosave primitives", () => {
  it("creates canonical time-ordered UUIDv7 identities", () => {
    const id = createUuidV7(0x01_02_03_04_05_06, (bytes) => {
      bytes.fill(0xff);
      return bytes;
    });

    expect(id).toBe("01020304-0506-7fff-bfff-ffffffffffff");
    expect(id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u);
  });
});

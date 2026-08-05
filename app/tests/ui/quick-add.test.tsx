import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRouter,
} from "@tanstack/react-router";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../../src/backend/context";
import type { ImageRecord, SelectedAttachmentRecord } from "../../src/backend/contracts";
import { FakeBackend } from "../../src/backend/fakeBackend";
import { AppearanceProvider } from "../../src/components/Appearance";
import {
  QuitCoordinator,
  type PrepareQuitNotice,
  type QuitCanceledNotice,
  type QuitNative,
} from "../../src/lifecycle/quit";
import { QuickAddWindow } from "../../src/quickAdd/QuickAddWindow";
import {
  QuickAddDismissAction,
  type QuickAddDismissRequest,
  type QuickAddNative,
} from "../../src/quickAdd/native";
import { TauriEvent } from "../../src/tauriProtocol";

const tauriEvents = vi.hoisted(() => {
  const listeners = new Map<string, (event: { payload: unknown }) => void>();
  return {
    clear: () => listeners.clear(),
    emit: (event: string, payload: unknown) => listeners.get(event)?.({ payload }),
    listen: vi.fn(async (event: string, listener: (event: { payload: unknown }) => void) => {
      listeners.set(event, listener);
      return () => listeners.delete(event);
    }),
  };
});

vi.mock("@tauri-apps/api/event", () => ({ listen: tauriEvents.listen }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ label: "quick-add" }),
}));

afterEach(() => {
  tauriEvents.clear();
  delete (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
});

describe("global quick add", () => {
  it("announces readiness only after installing the dismissal listener", async () => {
    const native = createNative();
    renderQuickAdd(new FakeBackend(), native.controller);

    await waitFor(() => expect(native.controller.markReady).toHaveBeenCalledOnce());
    expect(screen.queryByTestId("note-gutter-selection-rail")).not.toBeInTheDocument();
    expect(native.controller.onDismissRequested.mock.invocationCallOrder[0]!).toBeLessThan(
      native.controller.markReady.mock.invocationCallOrder[0]!,
    );
  });

  it("checkpoints a titleless note before Command-Enter dismisses it", async () => {
    const backend = new FakeBackend();
    const native = createNative();
    renderQuickAdd(backend, native.controller);
    const editor = await screen.findByRole("textbox", { name: "Quick note" });
    await setEditorText(userEvent.setup(), "The shower thought must survive dismissal.");

    fireEvent.keyDown(editor, { key: "Enter", metaKey: true });

    await waitFor(() => expect(native.dismiss).toHaveBeenCalledWith(QuickAddDismissAction.Dismiss));
    const notes = await backend.listNotesForTest({ cursor: null, limit: 10, scope: "ACTIVE" });
    expect(notes.items).toHaveLength(1);
    const note = await backend.loadTidbit(notes.items[0]!.id);
    expect(note).toMatchObject({
      bodyMarkdown: "The shower thought must survive dismissal.",
    });
    const search = await backend.searchPassages({
      limit: 10,
      mode: "DEFAULT",
      query: "shower thought",
    });
    expect(search.results[0]?.note.id).toBe(note.id);
  });

  it("keeps Command-Enter available while inline math is being edited", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const native = createNative();
    renderQuickAdd(backend, native.controller);

    await chooseSlashItem(user, "Inline math");
    await user.click(await screen.findByRole("button", { name: "Edit inline math: a_i" }));
    const source = await screen.findByLabelText("Inline math source");
    await user.clear(source);
    await user.type(source, "x^2");
    fireEvent.keyDown(source, { key: "Enter", metaKey: true });

    await waitFor(() => expect(native.dismiss).toHaveBeenCalledWith(QuickAddDismissAction.Dismiss));
    expect(
      (await backend.listNotesForTest({ cursor: null, limit: 10, scope: "ACTIVE" })).items,
    ).toHaveLength(1);
  });

  it("keeps a failed checkpoint visible and retries the same native action", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    vi.spyOn(backend, "checkpointWorkingCopy").mockRejectedValueOnce(
      new Error("checkpoint unavailable"),
    );
    const native = createNative();
    renderQuickAdd(backend, native.controller);
    await setEditorText(user, "Keep this editor recoverable.");
    await waitFor(() => expect(native.controller.onDismissRequested).toHaveBeenCalledOnce());

    act(() => native.request(QuickAddDismissAction.ShowMain));

    expect(await screen.findByRole("alert")).toHaveTextContent("checkpoint unavailable");
    expect(native.dismiss).not.toHaveBeenCalled();
    expect(native.cancelDismiss).toHaveBeenCalledOnce();
    expect(screen.getByRole("textbox", { name: "Quick note" })).not.toBeDisabled();

    await user.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() =>
      expect(native.dismiss).toHaveBeenCalledWith(QuickAddDismissAction.ShowMain),
    );
    expect(
      (await backend.listNotesForTest({ cursor: null, limit: 10, scope: "ACTIVE" })).items,
    ).toHaveLength(1);
  });

  it("keeps a failed pending attachment visible before allowing a deliberate retry", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const native = createNative();
    let failIngestion: ((reason: Error) => void) | undefined;
    vi.spyOn(backend, "captureClipboardImage").mockResolvedValue(
      "01980c8e-6c00-7000-8000-000000000289",
    );
    vi.spyOn(backend, "ingestClipboardImage").mockImplementation(
      () =>
        new Promise<ImageRecord>((_resolve, reject) => {
          failIngestion = reject;
        }),
    );
    renderQuickAdd(backend, native.controller);
    const editor = await screen.findByRole("textbox", { name: "Quick note" });
    fireEvent.paste(editor, { clipboardData: { items: [{ type: "image/png" }] } });
    await screen.findByRole("status", { name: "Processing pasted image" }, { timeout: 3_000 });

    act(() => native.request(QuickAddDismissAction.ShowMain));
    await act(async () => failIngestion?.(new Error("image ingest unavailable")));

    expect(await screen.findByRole("alert")).toHaveTextContent("image ingest unavailable");
    expect(native.dismiss).not.toHaveBeenCalled();
    expect(native.cancelDismiss).toHaveBeenCalledOnce();

    act(() => native.request(QuickAddDismissAction.Settings));
    await waitFor(() => expect(native.cancelDismiss).toHaveBeenCalledTimes(2));
    expect(native.dismiss).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() =>
      expect(native.dismiss).toHaveBeenCalledWith(QuickAddDismissAction.Settings),
    );
  });

  it("retries the latest dismissal action when an in-flight checkpoint fails", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    let rejectCheckpoint: ((reason: Error) => void) | undefined;
    vi.spyOn(backend, "checkpointWorkingCopy").mockImplementationOnce(
      () =>
        new Promise((_resolve, reject) => {
          rejectCheckpoint = reject;
        }),
    );
    const native = createNative();
    renderQuickAdd(backend, native.controller);
    await setEditorText(user, "Keep the newest dismissal intent.");

    act(() => native.request(QuickAddDismissAction.Dismiss));
    await waitFor(() => expect(backend.checkpointWorkingCopy).toHaveBeenCalledOnce());
    act(() => native.request(QuickAddDismissAction.Settings));
    await act(async () => rejectCheckpoint?.(new Error("checkpoint unavailable")));

    expect(await screen.findByRole("alert")).toHaveTextContent("checkpoint unavailable");
    await user.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() =>
      expect(native.dismiss).toHaveBeenCalledWith(QuickAddDismissAction.Settings),
    );
  });

  it("discards file drops that arrive after lifecycle flushing starts", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const native = createNative();
    const originalCheckpoint = backend.checkpointWorkingCopy.bind(backend);
    let releaseCheckpoint: (() => void) | undefined;
    vi.spyOn(backend, "checkpointWorkingCopy").mockImplementation(async (input) => {
      await new Promise<void>((resolve) => {
        releaseCheckpoint = resolve;
      });
      return originalCheckpoint(input);
    });
    const discard = vi.spyOn(backend, "discardFileDropSelections");
    const ingest = vi.spyOn(backend, "ingestSelectedAttachment");
    renderQuickAdd(backend, native.controller);
    await setEditorText(user, "Fence the final checkpoint.");
    await waitFor(() =>
      expect(tauriEvents.listen).toHaveBeenCalledWith(
        TauriEvent.FileDrop,
        expect.any(Function),
        expect.any(Object),
      ),
    );

    act(() => native.request(QuickAddDismissAction.Dismiss));
    await waitFor(() => expect(backend.checkpointWorkingCopy).toHaveBeenCalledOnce());
    act(() =>
      tauriEvents.emit(TauriEvent.FileDrop, {
        selections: [{ filename: "too-late.txt", selectionId: "selection-too-late" }],
      }),
    );

    await waitFor(() => expect(discard).toHaveBeenCalledWith(["selection-too-late"]));
    expect(ingest).not.toHaveBeenCalled();
    await act(async () => releaseCheckpoint?.());
    await waitFor(() => expect(native.dismiss).toHaveBeenCalledOnce());
  });

  it("does not materialize an untouched ephemeral note", async () => {
    const backend = new FakeBackend();
    const native = createNative();
    renderQuickAdd(backend, native.controller);
    const editor = await screen.findByRole("textbox", { name: "Quick note" });

    fireEvent.keyDown(editor, { key: "Escape" });

    await waitFor(() => expect(native.dismiss).toHaveBeenCalledOnce());
    expect(
      (await backend.listNotesForTest({ cursor: null, limit: 10, scope: "ACTIVE" })).items,
    ).toEqual([]);
    expect(await backend.listWorkingCopies()).toEqual([]);
  });

  it("keeps existing sources editable after the note body is cleared", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const native = createNative();
    renderQuickAdd(backend, native.controller);
    await setEditorText(user, "Source-backed draft");

    await user.click(screen.getByRole("button", { name: "Sources" }));
    await user.type(screen.getByLabelText("URL"), "https://example.com/source");
    await waitFor(() => expect(screen.getByRole("button", { name: "Sources 1" })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "Close sources" }));
    await user.clear(screen.getByRole("textbox", { name: "Quick note" }));

    const sources = screen.getByRole("button", { name: "Sources 1" });
    expect(sources).toBeEnabled();
    await user.click(sources);
    await user.click(screen.getByRole("button", { name: "Remove source 1" }));
    await waitFor(() => expect(backend.listWorkingCopies()).resolves.toEqual([]));
  });

  it("coalesces repeated dismissal requests into one checkpoint", async () => {
    const backend = new FakeBackend();
    const native = createNative();
    renderQuickAdd(backend, native.controller);
    const editor = await screen.findByRole("textbox", { name: "Quick note" });
    await setEditorText(userEvent.setup(), "Checkpoint this exactly once.");

    fireEvent.keyDown(editor, { key: "Escape" });
    fireEvent.keyDown(editor, { key: "Escape" });

    await waitFor(() => expect(native.dismiss).toHaveBeenCalledOnce());
    expect(
      (await backend.listNotesForTest({ cursor: null, limit: 10, scope: "ACTIVE" })).items,
    ).toHaveLength(1);
    expect(await backend.listWorkingCopies()).toEqual([]);
  });

  it("starts a fresh ephemeral identity after every successful dismissal", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const native = createNative();
    renderQuickAdd(backend, native.controller);
    await setEditorText(user, "First global note.");
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Quick note" }), {
      key: "Enter",
      metaKey: true,
    });
    await waitFor(() => expect(native.dismiss).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(screen.getByRole("textbox", { name: "Quick note" })).toHaveTextContent(""),
    );

    await setEditorText(user, "Second global note.");
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Quick note" }), {
      key: "Enter",
      metaKey: true,
    });
    await waitFor(() => expect(native.dismiss).toHaveBeenCalledTimes(2));

    const notes = await backend.listNotesForTest({ cursor: null, limit: 10, scope: "ACTIVE" });
    expect(notes.items).toHaveLength(2);
    expect(new Set(notes.items.map((note) => note.id)).size).toBe(2);
  });

  it("lets an open slash menu consume Escape before dismissing Quick Add", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const native = createNative();
    renderQuickAdd(backend, native.controller);
    const editor = await screen.findByRole("textbox", { name: "Quick note" });
    await user.type(editor, "/");
    await screen.findByText("Paragraph");

    fireEvent.keyDown(editor, { key: "Escape" });

    await waitFor(() => expect(screen.queryByText("Paragraph")).toBeNull());
    expect(editor).toHaveTextContent("/");
    expect(native.dismiss).not.toHaveBeenCalled();

    fireEvent.keyDown(editor, { key: "Escape" });
    await waitFor(() => expect(native.dismiss).toHaveBeenCalledOnce());
  });

  it("uses the note working-copy lease for media and checkpoints the attachment", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const native = createNative();
    const attachment: SelectedAttachmentRecord = {
      recordKind: "FILE",
      record: {
        byteLength: 4_096,
        displayFilename: "chapter.txt",
        id: "01980c8e-6c00-7000-8000-000000000282",
        ingestLeaseId: "01980c8e-6c00-7000-8000-000000000283",
        kind: "FILE",
        mediaType: "text/plain",
      },
    };
    vi.spyOn(backend, "selectAttachment").mockResolvedValue("01980c8e-6c00-7000-8000-000000000285");
    const ingestAttachment = vi
      .spyOn(backend, "ingestSelectedAttachment")
      .mockResolvedValue(attachment);
    renderQuickAdd(backend, native.controller);
    await screen.findByRole("textbox", { name: "Quick note" });

    await chooseSlashItem(user, "File");

    await waitFor(() => expect(ingestAttachment).toHaveBeenCalledOnce());
    expect(native.setFileDialogOpen.mock.calls).toEqual([[true], [false]]);
    expect(ingestAttachment.mock.calls[0]![1]).toMatch(/^fake-working-copy-/u);
    expect(await screen.findByText("chapter.txt")).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Quick note" }), {
      key: "Enter",
      metaKey: true,
    });

    await waitFor(() => expect(native.dismiss).toHaveBeenCalledOnce());
    const notes = await backend.listNotesForTest({ cursor: null, limit: 10, scope: "ACTIVE" });
    expect((await backend.loadTidbit(notes.items[0]!.id)).bodyMarkdown).toContain(
      attachment.record.id,
    );
    expect(await backend.listWorkingCopies()).toEqual([]);
  });

  it("registers one persistent session for shown and dismissal requests", async () => {
    const backend = new FakeBackend();
    const native = createNative();
    renderQuickAdd(backend, native.controller);
    await screen.findByRole("textbox", { name: "Quick note" });
    await waitFor(() => {
      expect(native.controller.onShown).toHaveBeenCalledOnce();
      expect(native.controller.onDismissRequested).toHaveBeenCalledOnce();
    });

    act(() => native.show());

    expect(screen.getAllByRole("main", { name: "Quick add" })).toHaveLength(1);
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Quick note" })).toHaveFocus());
  });

  it("checkpoints the latest keystroke before acknowledging quit", async () => {
    const backend = new FakeBackend();
    const native = createNative();
    const quit = createQuitNative();
    renderQuickAdd(backend, native.controller, quit.controller);
    await screen.findByRole("textbox", { name: "Quick note" });
    await setEditorText(userEvent.setup(), "The keystroke immediately before Quit must survive.");

    act(() => quit.prepare(41));

    await waitFor(() => expect(quit.acknowledge).toHaveBeenCalledWith(41, null));
    const notes = await backend.listNotesForTest({ cursor: null, limit: 10, scope: "ACTIVE" });
    expect((await backend.loadTidbit(notes.items[0]!.id)).bodyMarkdown).toContain(
      "immediately before Quit",
    );
    expect(screen.getByRole("textbox", { name: "Quick note" })).toHaveAttribute(
      "contenteditable",
      "false",
    );

    act(() => quit.cancel(41));
    await waitFor(() =>
      expect(screen.getByRole("textbox", { name: "Quick note" })).toHaveAttribute(
        "contenteditable",
        "true",
      ),
    );
  });

  it("waits for pending media before acknowledging quit", async () => {
    const backend = new FakeBackend();
    const native = createNative();
    const quit = createQuitNative();
    let finishIngestion: ((image: ImageRecord) => void) | undefined;
    vi.spyOn(backend, "captureClipboardImage").mockResolvedValue(
      "01980c8e-6c00-7000-8000-000000000286",
    );
    vi.spyOn(backend, "ingestClipboardImage").mockImplementation(
      () =>
        new Promise<ImageRecord>((resolve) => {
          finishIngestion = resolve;
        }),
    );
    renderQuickAdd(backend, native.controller, quit.controller);
    const editor = await screen.findByRole("textbox", { name: "Quick note" });
    fireEvent.paste(editor, { clipboardData: { items: [{ type: "image/png" }] } });
    await screen.findByRole("status", { name: "Processing pasted image" }, { timeout: 3_000 });

    act(() => quit.prepare(42));
    await act(async () => Promise.resolve());
    expect(quit.acknowledge).not.toHaveBeenCalled();

    act(() =>
      finishIngestion?.({
        byteLength: 1_024,
        displayFilename: "pending.png",
        id: "01980c8e-6c00-7000-8000-000000000287",
        ingestLeaseId: "01980c8e-6c00-7000-8000-000000000288",
        kind: "IMAGE",
        mediaType: "image/png",
        naturalHeight: 100,
        naturalWidth: 100,
        ocrError: null,
        ocrStatus: "PENDING",
      }),
    );

    await waitFor(() => expect(quit.acknowledge).toHaveBeenCalledWith(42, null));
    const notes = await backend.listNotesForTest({ cursor: null, limit: 10, scope: "ACTIVE" });
    expect((await backend.loadTidbit(notes.items[0]!.id)).bodyMarkdown).toContain(
      "01980c8e-6c00-7000-8000-000000000287",
    );
  });

  it("cancels quit when pending media fails", async () => {
    const backend = new FakeBackend();
    const native = createNative();
    const quit = createQuitNative();
    let failIngestion: ((reason: Error) => void) | undefined;
    vi.spyOn(backend, "captureClipboardImage").mockResolvedValue(
      "01980c8e-6c00-7000-8000-000000000290",
    );
    vi.spyOn(backend, "ingestClipboardImage").mockImplementation(
      () =>
        new Promise<ImageRecord>((_resolve, reject) => {
          failIngestion = reject;
        }),
    );
    renderQuickAdd(backend, native.controller, quit.controller);
    const editor = await screen.findByRole("textbox", { name: "Quick note" });
    fireEvent.paste(editor, { clipboardData: { items: [{ type: "image/png" }] } });
    await screen.findByRole("status", { name: "Processing pasted image" }, { timeout: 3_000 });

    act(() => quit.prepare(44));
    await act(async () => failIngestion?.(new Error("quit media ingest failed")));

    await waitFor(() =>
      expect(quit.acknowledge).toHaveBeenCalledWith(
        44,
        "Could not add attachment: quit media ingest failed",
      ),
    );
    expect(native.dismiss).not.toHaveBeenCalled();
  });

  it("cancels quit when checkpointing fails and unlocks on native cancellation", async () => {
    const backend = new FakeBackend();
    vi.spyOn(backend, "checkpointWorkingCopy").mockRejectedValueOnce(new Error("disk offline"));
    const native = createNative();
    const quit = createQuitNative();
    renderQuickAdd(backend, native.controller, quit.controller);
    await screen.findByRole("textbox", { name: "Quick note" });
    await setEditorText(userEvent.setup(), "Do not quit past a failed checkpoint.");

    act(() => quit.prepare(43));

    await waitFor(() => expect(quit.acknowledge).toHaveBeenCalledWith(43, "disk offline"));
    expect(screen.getByRole("textbox", { name: "Quick note" })).toHaveAttribute(
      "contenteditable",
      "false",
    );
    act(() => quit.cancel(43));
    await waitFor(() =>
      expect(screen.getByRole("textbox", { name: "Quick note" })).toHaveAttribute(
        "contenteditable",
        "true",
      ),
    );
  });
});

function renderQuickAdd(backend: FakeBackend, native: QuickAddNative, quitNative?: QuitNative) {
  const rootRoute = createRootRoute({
    component: () => (
      <>
        {quitNative && <QuitCoordinator native={quitNative} />}
        <QuickAddWindow native={native} />
      </>
    ),
  });
  const router = createRouter({
    history: createMemoryHistory(),
    routeTree: rootRoute,
  });
  return render(
    <BackendProvider backend={backend}>
      <AppearanceProvider>
        <RouterProvider router={router} />
      </AppearanceProvider>
    </BackendProvider>,
  );
}

function createNative() {
  let shown: (() => void) | undefined;
  let dismissRequested: ((request: QuickAddDismissRequest) => void) | undefined;
  const dismiss = vi.fn(async (_action: QuickAddDismissAction) => undefined);
  const cancelDismiss = vi.fn(async () => undefined);
  const markReady = vi.fn(async () => undefined);
  const setFileDialogOpen = vi.fn(async (_open: boolean) => undefined);
  return {
    controller: {
      cancelDismiss,
      dismiss,
      markReady,
      onDismissRequested: vi.fn(async (listener: (request: QuickAddDismissRequest) => void) => {
        dismissRequested = listener;
        return () => {
          dismissRequested = undefined;
        };
      }),
      onShown: vi.fn(async (listener: () => void) => {
        shown = listener;
        return () => {
          shown = undefined;
        };
      }),
      setFileDialogOpen,
    } satisfies QuickAddNative,
    cancelDismiss,
    dismiss,
    request: (action: QuickAddDismissAction) => dismissRequested?.({ action }),
    setFileDialogOpen,
    show: () => shown?.(),
  };
}

function createQuitNative() {
  let prepare: ((notice: PrepareQuitNotice) => void) | undefined;
  let cancel: ((notice: QuitCanceledNotice) => void) | undefined;
  const acknowledge = vi.fn(async (_requestId: number, _error: string | null) => undefined);
  return {
    acknowledge,
    cancel: (requestId: number) => cancel?.({ requestId }),
    controller: {
      acknowledge,
      onCanceled: vi.fn(async (listener: (notice: QuitCanceledNotice) => void) => {
        cancel = listener;
        return () => {
          cancel = undefined;
        };
      }),
      onPrepare: vi.fn(async (listener: (notice: PrepareQuitNotice) => void) => {
        prepare = listener;
        return () => {
          prepare = undefined;
        };
      }),
    } satisfies QuitNative,
    prepare: (requestId: number) => prepare?.({ requestId }),
  };
}

async function setEditorText(user: ReturnType<typeof userEvent.setup>, value: string) {
  const textbox = await screen.findByRole("textbox", { name: "Quick note" });
  await user.clear(textbox);
  await user.type(textbox, value);
}

async function chooseSlashItem(user: ReturnType<typeof userEvent.setup>, name: string) {
  const textbox = await screen.findByRole("textbox", { name: "Quick note" });
  const insertionPoint = textbox
    .querySelectorAll(".bn-inline-content")
    .item(textbox.querySelectorAll(".bn-inline-content").length - 1);
  await user.click(insertionPoint || textbox);
  await user.type(insertionPoint || textbox, "/");
  await user.click(await screen.findByRole("option", { name }));
}

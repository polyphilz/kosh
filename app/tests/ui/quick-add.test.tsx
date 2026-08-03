import {
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRouter,
} from "@tanstack/react-router";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
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
import type { QuickAddNative } from "../../src/quickAdd/native";

describe("global quick add", () => {
  it("recovers its isolated draft, saves with Command-Enter, and resets before dismissal", async () => {
    const backend = new FakeBackend();
    await backend.saveDraft({
      baseRevisionId: null,
      bodyMarkdown: "Recovered from a hidden quick-add window.",
      contextKey: "quick-add",
      sources: [{ label: "Notebook", url: null }],
      tidbitId: null,
      title: "Recovered globally",
    });
    const createTidbit = vi.spyOn(backend, "createTidbit");
    const native = createNative();
    renderQuickAdd(backend, native.controller);

    expect(await screen.findByRole("textbox", { name: /^Title/u })).toHaveValue(
      "Recovered globally",
    );
    expect(screen.getByRole("textbox", { name: "Source 1 label" })).toHaveValue("Notebook");
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Tidbit" }), {
      key: "Enter",
      metaKey: true,
    });

    await waitFor(() => expect(createTidbit).toHaveBeenCalledOnce());
    await waitFor(() => expect(native.dismiss).toHaveBeenCalledOnce());
    await expect(backend.loadDraft("quick-add")).resolves.toBeNull();
    expect(createTidbit.mock.calls[0]![0].bodyMarkdown).toContain("Recovered from");
  });

  it("uses Escape only when cancellation is safe and preserves dirty input until confirmed", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const native = createNative();
    renderQuickAdd(backend, native.controller);
    const editor = await screen.findByRole("textbox", { name: "Tidbit" });
    await setEditorText(user, "Do not lose this shower thought.");

    fireEvent.keyDown(editor, { key: "Escape" });
    expect(screen.getByRole("dialog", { name: "Discard this draft?" })).toBeInTheDocument();
    expect(native.dismiss).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Keep editing" }));
    expect(screen.queryByRole("dialog", { name: "Discard this draft?" })).toBeNull();
    expect(editor).toHaveTextContent("Do not lose this shower thought.");

    fireEvent.keyDown(editor, { key: "Escape" });
    await user.click(screen.getByRole("button", { name: "Discard draft" }));
    await waitFor(() => expect(native.dismiss).toHaveBeenCalledOnce());
    await expect(backend.loadDraft("quick-add")).resolves.toBeNull();
  });

  it("lets an open slash menu consume Escape before canceling quick add", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const native = createNative();
    renderQuickAdd(backend, native.controller);
    const editor = await screen.findByRole("textbox", { name: "Tidbit" });
    await user.type(editor, "/");
    await screen.findByText("Paragraph");

    fireEvent.keyDown(editor, { key: "Escape" });

    await waitFor(() => expect(screen.queryByText("Paragraph")).toBeNull());
    expect(editor).toHaveTextContent("/");
    expect(native.dismiss).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog", { name: "Discard this draft?" })).toBeNull();

    fireEvent.keyDown(editor, { key: "Escape" });
    expect(screen.getByRole("dialog", { name: "Discard this draft?" })).toBeInTheDocument();
  });

  it("pastes images and selects attachments through the quick-add draft lease", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const native = createNative();
    const image: ImageRecord = {
      byteLength: 2_048,
      displayFilename: "clipboard.png",
      id: "01980c8e-6c00-7000-8000-000000000280",
      ingestLeaseId: "01980c8e-6c00-7000-8000-000000000281",
      kind: "IMAGE",
      mediaType: "image/png",
      naturalHeight: 600,
      naturalWidth: 800,
      ocrError: null,
      ocrStatus: "PENDING",
    };
    const attachment: SelectedAttachmentRecord = {
      recordKind: "GENERIC",
      record: {
        byteLength: 4_096,
        displayFilename: "chapter.txt",
        extractedLineCount: 12,
        extractionError: null,
        extractionStatus: "READY",
        id: "01980c8e-6c00-7000-8000-000000000282",
        ingestLeaseId: "01980c8e-6c00-7000-8000-000000000283",
        kind: "TEXT",
        mediaType: "text/plain",
      },
    };
    const captureClipboardImage = vi
      .spyOn(backend, "captureClipboardImage")
      .mockResolvedValue("01980c8e-6c00-7000-8000-000000000284");
    vi.spyOn(backend, "ingestClipboardImage").mockResolvedValue(image);
    vi.spyOn(backend, "selectAttachment").mockResolvedValue("01980c8e-6c00-7000-8000-000000000285");
    const ingestAttachment = vi
      .spyOn(backend, "ingestSelectedAttachment")
      .mockResolvedValue(attachment);
    const saveDraft = vi.spyOn(backend, "saveDraft");
    renderQuickAdd(backend, native.controller);
    const editor = await screen.findByRole("textbox", { name: "Tidbit" });

    fireEvent.paste(editor, {
      clipboardData: { items: [{ type: "image/png" }] },
    });
    await waitFor(() => expect(captureClipboardImage).toHaveBeenCalledOnce(), { timeout: 3_000 });
    expect(await screen.findByLabelText("Alt text", {}, { timeout: 3_000 })).toBeInTheDocument();
    await chooseSlashItem(user, "File");

    await waitFor(() => expect(ingestAttachment).toHaveBeenCalledOnce());
    expect(native.setFileDialogOpen.mock.calls).toEqual([[true], [false]]);
    expect(saveDraft.mock.calls.some(([input]) => input.contextKey === "quick-add")).toBe(true);
    expect(ingestAttachment.mock.calls[0]![1]).toMatch(/^fake-draft-/u);
    expect(await screen.findByText("chapter.txt")).toBeInTheDocument();
  });

  it("registers one persistent composer for native invocations", async () => {
    const backend = new FakeBackend();
    const native = createNative();
    renderQuickAdd(backend, native.controller);
    await screen.findByRole("textbox", { name: "Tidbit" });
    await waitFor(() => expect(native.controller.onShown).toHaveBeenCalledOnce());
    expect(screen.getAllByRole("main", { name: "Quick add" })).toHaveLength(1);
  });

  it("flushes the latest quick draft before acknowledging quit", async () => {
    const backend = new FakeBackend();
    const native = createNative();
    const quit = createQuitNative();
    renderQuickAdd(backend, native.controller, quit.controller);
    await screen.findByRole("textbox", { name: "Tidbit" });
    await setEditorText(userEvent.setup(), "The keystroke immediately before Quit must survive.");

    act(() => quit.prepare(41));

    await waitFor(() => expect(quit.acknowledge).toHaveBeenCalledWith(41, null));
    await expect(backend.loadDraft("quick-add")).resolves.toMatchObject({
      bodyMarkdown: "The keystroke immediately before Quit must survive.",
    });
    expect(screen.getByRole("textbox", { name: /^Title/u })).toBeDisabled();

    act(() => quit.cancel(41));
    await waitFor(() =>
      expect(screen.getByRole("textbox", { name: /^Title/u })).not.toBeDisabled(),
    );
  });

  it("cancels quit instead of interrupting pending media ingestion", async () => {
    const backend = new FakeBackend();
    const native = createNative();
    const quit = createQuitNative();
    let finishIngestion: ((image: ImageRecord) => void) | undefined;
    const captureClipboardImage = vi
      .spyOn(backend, "captureClipboardImage")
      .mockResolvedValue("01980c8e-6c00-7000-8000-000000000286");
    vi.spyOn(backend, "ingestClipboardImage").mockImplementation(
      () =>
        new Promise<ImageRecord>((resolve) => {
          finishIngestion = resolve;
        }),
    );
    renderQuickAdd(backend, native.controller, quit.controller);
    const editor = await screen.findByRole("textbox", { name: "Tidbit" });

    fireEvent.paste(editor, {
      clipboardData: { items: [{ type: "image/png" }] },
    });
    await waitFor(() => expect(captureClipboardImage).toHaveBeenCalledOnce(), { timeout: 3_000 });
    await screen.findByRole("button", { name: "Adding attachment…" }, { timeout: 3_000 });
    act(() => quit.prepare(42));

    await waitFor(() =>
      expect(quit.acknowledge).toHaveBeenCalledWith(
        42,
        "Wait for pending attachments before quitting",
      ),
    );

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
    expect(await screen.findByLabelText("Alt text")).toBeInTheDocument();
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
  const dismiss = vi.fn(async () => undefined);
  const setFileDialogOpen = vi.fn(async (_open: boolean) => undefined);
  return {
    controller: {
      dismiss,
      onShown: vi.fn(async (listener: () => void) => {
        shown = listener;
        return () => {
          shown = undefined;
        };
      }),
      setFileDialogOpen,
    } satisfies QuickAddNative,
    dismiss,
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
  const textbox = screen.getByRole("textbox", { name: "Tidbit" });
  await user.clear(textbox);
  await user.type(textbox, value);
}

async function chooseSlashItem(user: ReturnType<typeof userEvent.setup>, name: string) {
  const textbox = screen.getByRole("textbox", { name: "Tidbit" });
  const insertionPoint = textbox
    .querySelectorAll(".bn-inline-content")
    .item(textbox.querySelectorAll(".bn-inline-content").length - 1);
  await user.click(insertionPoint || textbox);
  await user.type(insertionPoint || textbox, "/");
  await user.click(await screen.findByRole("option", { name }));
}

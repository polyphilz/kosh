import { RouterProvider, createMemoryHistory } from "@tanstack/react-router";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../../src/backend/context";
import type { CitationResolution, ImageRecord } from "../../src/backend/contracts";
import { FakeBackend } from "../../src/backend/fakeBackend";
import { richTextEditorViewFromDOM } from "../../src/markdown/editorViewRegistry";
import { createAppRouter } from "../../src/router";

describe("tidbit capture and editing routes", () => {
  it("keeps the composer locked until failed recovery is retried", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    await backend.saveDraft({
      contextKey: "capture",
      tidbitId: null,
      baseRevisionId: null,
      title: "Do not overwrite",
      bodyMarkdown: "Recovered only after retry.",
      sources: [],
    });
    vi.spyOn(backend, "loadDraft")
      .mockRejectedValueOnce(new Error("temporary read failure"))
      .mockRejectedValueOnce(new Error("temporary read failure"));

    renderRoute(backend, "/add");
    expect(await screen.findByRole("alert")).toHaveTextContent("Draft recovery failed");
    expect(screen.getByRole("textbox", { name: /^Title/u })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Save tidbit" })).toBeDisabled();

    await user.click(screen.getByRole("button", { name: "Retry draft recovery" }));
    expect(await screen.findByRole("textbox", { name: /^Title/u })).toHaveValue("Do not overwrite");
    expect(screen.getByRole("textbox", { name: /^Title/u })).toBeEnabled();
  });

  it("restores an interrupted draft, autosaves changes, and explicitly discards it", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    await backend.saveDraft({
      contextKey: "capture",
      tidbitId: null,
      baseRevisionId: null,
      title: "Recovered title",
      bodyMarkdown: "Recovered **body**.",
      sources: [{ label: "Notebook", url: "https://example.com/notes" }],
    });

    const first = renderRoute(backend, "/add");
    expect(await screen.findByRole("textbox", { name: /^Title/u })).toHaveValue("Recovered title");
    await waitFor(() => {
      expect(screen.getByRole("textbox", { name: "Tidbit" })).toHaveTextContent("Recovered body.");
    });
    expect(screen.getByRole("textbox", { name: "Source 1 label" })).toHaveValue("Notebook");

    fireEvent.change(screen.getByRole("textbox", { name: /^Title/u }), {
      target: { value: "Recovered and changed" },
    });
    expect(await screen.findByText("Draft saved locally")).toBeInTheDocument();
    await waitFor(async () => {
      expect((await backend.loadDraft("capture"))?.title).toBe("Recovered and changed");
    });

    first.unmount();
    renderRoute(backend, "/add");
    expect(await screen.findByRole("textbox", { name: /^Title/u })).toHaveValue(
      "Recovered and changed",
    );
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByRole("dialog", { name: "Discard this draft?" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Discard draft" }));

    expect(await screen.findByRole("heading", { name: "Search" })).toBeInTheDocument();
    await expect(backend.loadDraft("capture")).resolves.toBeNull();
  });

  it("creates with Command-Enter, edits through the shared composer, and soft deletes", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    vi.spyOn(backend, "clearDraft").mockRejectedValueOnce(new Error("temporary clear failure"));
    renderRoute(backend, "/add");
    const title = await screen.findByRole("textbox", { name: /^Title/u });
    fireEvent.change(title, { target: { value: "Precise memory" } });
    setEditorText("A citation-ready **tidbit**.");
    await user.click(screen.getByRole("button", { name: "Add source" }));
    await user.type(screen.getByRole("textbox", { name: "Source 1 label" }), "Reference");
    await user.type(
      screen.getByRole("textbox", { name: "Source 1 URL" }),
      "https://example.com/page",
    );

    fireEvent.keyDown(screen.getByRole("textbox", { name: "Tidbit" }), {
      key: "Enter",
      metaKey: true,
    });
    expect(await screen.findByRole("heading", { name: "Precise memory" })).toBeInTheDocument();
    expect(screen.getByText("Reference")).toBeInTheDocument();
    expect(screen.getByText("https://example.com/page")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Edit" }));
    expect(await screen.findByRole("heading", { name: "Edit tidbit" })).toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: /^Title/u }), {
      target: { value: "Revised memory" },
    });
    appendEditorText(" More exact.");
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByRole("heading", { name: "Revised memory" })).toBeInTheDocument();
    expect(screen.getByText(/More exact/u)).toBeInTheDocument();

    const active = await backend.listTidbits({ limit: 10, cursor: null });
    expect(active.items).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: "Delete" }));
    await user.click(screen.getByRole("button", { name: "Delete tidbit" }));
    expect(await screen.findByRole("heading", { name: "Search" })).toBeInTheDocument();
    expect((await backend.listTidbits({ limit: 10, cursor: null })).items).toEqual([]);
    expect((await backend.loadTidbit(active.items[0]!.id)).deletedAtMs).not.toBeNull();
  });

  it("keeps a canceled image picker out of draft and dirty state", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const selectImage = vi.spyOn(backend, "selectImage").mockResolvedValue(null);
    const ingestSelectedImage = vi.spyOn(backend, "ingestSelectedImage");
    const saveDraft = vi.spyOn(backend, "saveDraft");
    renderRoute(backend, "/add");
    await screen.findByRole("textbox", { name: "Tidbit" });

    await user.click(screen.getByRole("button", { name: "Add image" }));
    await waitFor(() => expect(selectImage).toHaveBeenCalledOnce());
    await waitFor(() => expect(screen.getByRole("button", { name: "Cancel" })).toBeEnabled());

    expect(saveDraft).not.toHaveBeenCalled();
    expect(ingestSelectedImage).not.toHaveBeenCalled();
    await expect(backend.loadDraft("capture")).resolves.toBeNull();
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByRole("heading", { name: "Search" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "Discard this draft?" })).toBeNull();
  });

  it("blocks keyboard and programmatic submission while image ingestion is pending", async () => {
    const backend = new FakeBackend();
    let resolveImage!: (image: ImageRecord) => void;
    const pendingImage = new Promise<ImageRecord>((resolve) => {
      resolveImage = resolve;
    });
    const captureClipboardImage = vi
      .spyOn(backend, "captureClipboardImage")
      .mockResolvedValue("01980c8e-6c00-7000-8000-000000000260");
    vi.spyOn(backend, "ingestClipboardImage").mockReturnValue(pendingImage);
    const createTidbit = vi.spyOn(backend, "createTidbit");
    renderRoute(backend, "/add");
    const editor = await screen.findByRole("textbox", { name: "Tidbit" });
    setEditorText("Do not save before the image.");

    fireEvent.paste(editor, {
      clipboardData: { items: [{ type: "image/png" }] },
    });
    expect(captureClipboardImage).toHaveBeenCalledOnce();
    const pendingButton = await screen.findByRole("button", { name: "Adding attachment…" });
    expect(pendingButton).toBeDisabled();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();

    fireEvent.keyDown(editor, { key: "Enter", metaKey: true });
    fireEvent.submit(pendingButton.closest("form")!);
    await act(async () => Promise.resolve());
    expect(createTidbit).not.toHaveBeenCalled();

    await act(async () => {
      resolveImage({
        byteLength: 1_024,
        displayFilename: "pending.png",
        id: "01980c8e-6c00-7000-8000-000000000261",
        ingestLeaseId: "01980c8e-6c00-7000-8000-000000000262",
        kind: "IMAGE",
        mediaType: "image/png",
        naturalHeight: 600,
        naturalWidth: 800,
        ocrError: null,
        ocrStatus: "PENDING",
      });
    });
    await waitFor(() => expect(screen.getByRole("button", { name: "Save tidbit" })).toBeEnabled());
  });

  it("captures paste bytes before draft I/O and saves unblurred image metadata", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const callOrder: string[] = [];
    vi.spyOn(backend, "captureClipboardImage").mockImplementation(async () => {
      callOrder.push("capture");
      return "01980c8e-6c00-7000-8000-000000000263";
    });
    const originalSaveDraft = backend.saveDraft.bind(backend);
    vi.spyOn(backend, "saveDraft").mockImplementation(async (input) => {
      callOrder.push("draft");
      return originalSaveDraft(input);
    });
    vi.spyOn(backend, "ingestClipboardImage").mockImplementation(async () => {
      callOrder.push("ingest");
      return {
        byteLength: 1_024,
        displayFilename: "snapshot.png",
        id: "01980c8e-6c00-7000-8000-000000000264",
        ingestLeaseId: "01980c8e-6c00-7000-8000-000000000265",
        kind: "IMAGE",
        mediaType: "image/png",
        naturalHeight: 600,
        naturalWidth: 800,
        ocrError: null,
        ocrStatus: "PENDING",
      };
    });
    const createTidbit = vi.spyOn(backend, "createTidbit");
    renderRoute(backend, "/add");
    const editor = await screen.findByRole("textbox", { name: "Tidbit" });

    fireEvent.paste(editor, {
      clipboardData: { items: [{ type: "image/png" }] },
    });
    const altInput = await screen.findByLabelText("Alt text");
    await user.click(altInput);
    fireEvent.input(altInput, { target: { value: "Diagram from the clipboard" } });
    await user.keyboard("{Meta>}{Enter}{/Meta}");

    await waitFor(() => expect(createTidbit).toHaveBeenCalledOnce());
    expect(callOrder.slice(0, 3)).toEqual(["capture", "draft", "ingest"]);
    expect(createTidbit.mock.calls[0]![0].bodyMarkdown).toContain(
      "alt=Diagram%20from%20the%20clipboard",
    );
  });

  it("re-resolves an opened citation after saving a new tidbit revision", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const original = await backend.createTidbit({
      title: "Citation lifecycle",
      bodyMarkdown: "Original cited evidence.",
      sources: [],
    });
    const passageId = `fake-passage:${original.currentRevisionId}`;
    renderRoute(backend, `/tidbits/${original.id}?passage=${encodeURIComponent(passageId)}`);

    expect(await screen.findByText("Current revision", { exact: true })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Edit" }));
    appendEditorText(" Updated.");
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    expect(await screen.findByText("Historical revision", { exact: true })).toBeInTheDocument();
    expect(screen.getByText("Original cited evidence.", { exact: true })).toBeInTheDocument();
    expect(screen.getByText(/Original cited evidence\. Updated\./u)).toBeInTheDocument();
  });

  it("rejects attachment passages supplied to a tidbit deep link", async () => {
    const backend = new FakeBackend();
    const tidbit = await backend.createTidbit({
      title: "Unrelated tidbit",
      bodyMarkdown: "Authored note.",
      sources: [],
    });
    const attachmentCitation: CitationResolution = {
      passageId: "attachment-passage",
      excerpt: "Unrelated attachment evidence.",
      headingContext: [],
      constructionVersion: "attachment-v1",
      state: "CURRENT",
      locator: { kind: "PDF_PAGE", page: 4 },
      tidbit: null,
      attachment: {
        id: "attachment-1",
        extractionId: "extraction-1",
        displayFilename: "unrelated.pdf",
        mediaType: "application/pdf",
        deleted: false,
      },
      sources: [],
    };
    vi.spyOn(backend, "resolveCitation").mockResolvedValue(attachmentCitation);

    renderRoute(backend, `/tidbits/${tidbit.id}?passage=attachment-passage`);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The citation does not belong to this tidbit.",
    );
    expect(screen.queryByText("Unrelated attachment evidence.")).not.toBeInTheDocument();
  });

  it("keeps a recovery draft when an edit loses its optimistic revision race", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const original = await backend.createTidbit({
      title: "Original",
      bodyMarkdown: "Original body",
      sources: [],
    });
    renderRoute(backend, `/tidbits/${original.id}`);
    await user.click(await screen.findByRole("button", { name: "Edit" }));
    expect(await screen.findByRole("heading", { name: "Edit tidbit" })).toBeInTheDocument();

    await backend.editTidbit({
      id: original.id,
      expectedRevisionId: original.currentRevisionId,
      title: "Changed elsewhere",
      bodyMarkdown: "Newer body",
      sources: [],
    });
    fireEvent.change(screen.getByRole("textbox", { name: /^Title/u }), {
      target: { value: "My competing edit" },
    });
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("changed elsewhere");
    expect((await backend.loadDraft(`edit:${original.id}`))?.title).toBe("My competing edit");
  });
});

function renderRoute(backend: FakeBackend, path: string) {
  const router = createAppRouter(
    createMemoryHistory({
      initialEntries: [path],
    }),
  );
  return render(
    <StrictMode>
      <BackendProvider backend={backend}>
        <RouterProvider router={router} />
      </BackendProvider>
    </StrictMode>,
  );
}

function editorView() {
  const textbox = screen.getByRole("textbox", { name: "Tidbit" });
  const view = richTextEditorViewFromDOM(textbox);
  if (!view) throw new Error("rich text editor view is unavailable");
  return view;
}

function setEditorText(value: string) {
  const view = editorView();
  act(() => {
    view.dispatch(
      view.state.tr
        .delete(1, view.state.doc.content.size - 1)
        .insertText(value, 1)
        .scrollIntoView(),
    );
  });
}

function appendEditorText(value: string) {
  const view = editorView();
  act(() => {
    view.dispatch(view.state.tr.insertText(value, view.state.doc.content.size - 1));
  });
}

import { RouterProvider, createMemoryHistory } from "@tanstack/react-router";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../../src/backend/context";
import { FakeBackend } from "../../src/backend/fakeBackend";
import { richTextEditorViewFromDOM } from "../../src/markdown/editorViewRegistry";
import { createAppRouter } from "../../src/router";

describe("tidbit capture and editing routes", () => {
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

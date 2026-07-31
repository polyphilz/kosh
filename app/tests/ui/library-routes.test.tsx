import { RouterProvider, createMemoryHistory } from "@tanstack/react-router";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../../src/backend/context";
import { TIDBIT_PURGE_DELAY_MS, type TidbitRecord } from "../../src/backend/contracts";
import { FakeBackend } from "../../src/backend/fakeBackend";
import { createAppRouter } from "../../src/router";

describe("library lifecycle routes", () => {
  it("paginates all tidbits and keeps recent browsing bounded", async () => {
    const now = Date.now();
    const seeds = Array.from({ length: 34 }, (_, index) => seedTidbit(index, now - index * 1_000));
    const backend = new FakeBackend(
      { dataDir: "/tmp/kosh-library", nowMs: now, requestId: "library-page" },
      seeds,
    );
    const user = userEvent.setup();
    renderRoute(backend, "/library");

    expect(await screen.findByRole("heading", { name: "Library" })).toBeInTheDocument();
    await waitFor(() => expect(screen.getAllByRole("listitem")).toHaveLength(12));
    expect(screen.queryByRole("button", { name: "Load more" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("link", { name: "All tidbits" }));
    await waitFor(() => expect(screen.getAllByRole("listitem")).toHaveLength(30));
    await user.click(screen.getByRole("button", { name: "Load more" }));
    await waitFor(() => expect(screen.getAllByRole("listitem")).toHaveLength(34));
  });

  it("ignores an older page when the user changes library views", async () => {
    const now = Date.now();
    const active = Array.from({ length: 31 }, (_, index) => seedTidbit(index, now - index * 1_000));
    const deleted = {
      ...seedTidbit(40, now - 50_000),
      deletedAtMs: now - 1_000,
      updatedAtMs: now - 1_000,
    };
    const backend = new FakeBackend(
      { dataDir: "/tmp/kosh-library", nowMs: now, requestId: "view-race" },
      [...active, deleted],
    );
    const listTidbits = backend.listTidbits.bind(backend);
    let releaseOlderPage: (() => void) | undefined;
    vi.spyOn(backend, "listTidbits").mockImplementation(async (input) => {
      if (input.scope === "ACTIVE" && input.cursor !== null) {
        await new Promise<void>((resolve) => {
          releaseOlderPage = resolve;
        });
      }
      return listTidbits(input);
    });
    const user = userEvent.setup();
    renderRoute(backend, "/library?view=all");

    await waitFor(() => expect(screen.getAllByRole("listitem")).toHaveLength(30));
    await user.click(screen.getByRole("button", { name: "Load more" }));
    await waitFor(() => expect(releaseOlderPage).toBeTypeOf("function"));
    await user.click(screen.getByRole("link", { name: "Trash" }));
    expect(await screen.findByRole("link", { name: /Seed 40/u })).toBeInTheDocument();
    releaseOlderPage?.();
    await waitFor(() => expect(screen.getAllByRole("listitem")).toHaveLength(1));
  });

  it("opens Trash, restores a tidbit, and preserves its history", async () => {
    const now = Date.now();
    const deleted = {
      ...seedTidbit(1, now - 10_000),
      deletedAtMs: now - 1_000,
      updatedAtMs: now - 1_000,
    };
    const backend = new FakeBackend(
      { dataDir: "/tmp/kosh-library", nowMs: now, requestId: "restore" },
      [deleted],
    );
    const user = userEvent.setup();
    renderRoute(backend, "/library?view=trash");

    await user.click(await screen.findByRole("link", { name: /Seed 1/u }));
    expect(await screen.findByText("This tidbit is in Trash.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete permanently" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Restore" }));
    expect(await screen.findByText("Tidbit restored")).toBeInTheDocument();
    await expect(backend.loadTidbit(deleted.id)).resolves.toMatchObject({ deletedAtMs: null });
    expect(screen.getByRole("heading", { name: "Revision history" })).toBeInTheDocument();
  });

  it("requires explicit confirmation before an eligible permanent purge", async () => {
    const now = Date.now();
    const deleted = {
      ...seedTidbit(2, now - TIDBIT_PURGE_DELAY_MS - 20_000),
      deletedAtMs: now - TIDBIT_PURGE_DELAY_MS - 1,
      updatedAtMs: now - TIDBIT_PURGE_DELAY_MS - 1,
    };
    const backend = new FakeBackend(
      { dataDir: "/tmp/kosh-library", nowMs: now, requestId: "purge" },
      [deleted],
    );
    const user = userEvent.setup();
    renderRoute(backend, `/tidbits/${deleted.id}?from=library&view=trash`);

    await user.click(await screen.findByRole("button", { name: "Delete permanently" }));
    const dialog = screen.getByRole("dialog", {
      name: "Permanently delete this tidbit?",
    });
    expect(
      within(dialog).getByText(/Research answers keep their exact citation snapshots/u),
    ).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Delete every revision" }));
    expect(await screen.findByRole("heading", { name: "Library" })).toBeInTheDocument();
    await expect(backend.loadTidbit(deleted.id)).rejects.toThrow("not found");
  });

  it("navigates immutable history and exposes copy and trusted source actions", async () => {
    const backend = new FakeBackend();
    const original = await backend.createTidbit({
      bodyMarkdown: "# Original\n\nExact **Markdown**.",
      sources: [{ label: "Docs", url: "https://example.com/docs" }],
      title: "Original title",
    });
    await backend.editTidbit({
      bodyMarkdown: "Current body",
      expectedRevisionId: original.currentRevisionId,
      id: original.id,
      sources: [],
      title: "Current title",
    });
    const openSource = vi.spyOn(backend, "openSourceUrl");
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue(undefined);
    renderRoute(backend, `/tidbits/${original.id}?from=library&view=all`);

    const originalRevision = await screen.findByRole("button", {
      name: /Revision 1Original title/u,
    });
    await user.click(originalRevision);
    expect(await screen.findByRole("heading", { name: "Original title" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Copy Markdown" }));
    expect(writeText).toHaveBeenCalledWith("# Original\n\nExact **Markdown**.");
    await user.click(screen.getByRole("button", { name: "Copy text" }));
    expect(writeText).toHaveBeenLastCalledWith("Original\n\nExact Markdown.");
    await user.click(screen.getByRole("button", { name: "Open" }));
    expect(openSource).toHaveBeenCalledWith(original.sources[0]!.id);
    expect(screen.getByRole("link", { name: "← Back to library" })).toHaveAttribute(
      "href",
      "/library?view=all",
    );
  });
});

function seedTidbit(index: number, updatedAtMs: number): TidbitRecord {
  return {
    bodyMarkdown: `Seed body ${index}`,
    createdAtMs: updatedAtMs - 100,
    currentRevisionId: `seed-revision-${index}`,
    deletedAtMs: null,
    displayTitle: `Seed ${index}`,
    id: `seed-tidbit-${index}`,
    revisionNumber: 1,
    sources: [],
    title: `Seed ${index}`,
    updatedAtMs,
  };
}

function renderRoute(backend: FakeBackend, path: string) {
  const router = createAppRouter(createMemoryHistory({ initialEntries: [path] }));
  return render(
    <StrictMode>
      <BackendProvider backend={backend}>
        <RouterProvider router={router} />
      </BackendProvider>
    </StrictMode>,
  );
}

import { RouterProvider, createMemoryHistory } from "@tanstack/react-router";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { MaintenanceOutcome, BlockEmbeddingIndexPhase } from "../../src/backend/contracts";
import { BackendProvider } from "../../src/backend/context";
import { FakeBackend } from "../../src/backend/fakeBackend";
import { AppearanceProvider } from "../../src/components/Appearance";
import { createAppRouter } from "../../src/router";

describe("settings diagnostics and maintenance", () => {
  it("shows exact local state and requires confirmation before an idempotent rebuild", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    await backend.seedNote({
      bodyMarkdown: "A searchable local passage.",
      sources: [],
    });
    const rebuild = vi.spyOn(backend, "rebuildSearchIndexes");
    renderSettings(backend);

    expect(await screen.findByText("1 active tidbits")).toBeInTheDocument();
    expect(screen.getByText("0 in Trash · 1 current note")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Offsite recovery" })).toBeInTheDocument();
    await user.click(screen.getByText("Local paths"));
    expect(screen.getByText("/tmp/kosh-browser-fixture/kosh.sqlite3")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Rebuild search" }));
    expect(rebuild).not.toHaveBeenCalled();
    const firstDialog = screen.getByRole("dialog", { name: "Rebuild block search?" });
    expect(firstDialog).toBeInTheDocument();
    await user.click(within(firstDialog).getByRole("button", { name: "Rebuild search" }));
    await waitFor(() => expect(rebuild).toHaveBeenCalledOnce());
    expect(await screen.findByText("Rebuilt current block search indexes.")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Rebuild search" }));
    await user.click(
      within(screen.getByRole("dialog", { name: "Rebuild block search?" })).getByRole("button", {
        name: "Rebuild search",
      }),
    );
    await waitFor(() => expect(rebuild).toHaveBeenCalledTimes(2));
  });

  it("reports in-flight progress, disables competing work, and recovers after failure", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const pending = deferred<MaintenanceOutcome>();
    const rebuild = vi
      .spyOn(backend, "rebuildEmbeddingIndex")
      .mockImplementationOnce(() => pending.promise);
    const retry = vi
      .spyOn(backend, "retryFailedExtractions")
      .mockRejectedValueOnce(new Error("controlled extraction failure"));
    renderSettings(backend);
    await screen.findByText("0 active tidbits");

    await user.click(screen.getByRole("button", { name: "Rebuild embeddings" }));
    await user.click(
      within(screen.getByRole("dialog", { name: "Rebuild semantic embeddings?" })).getByRole(
        "button",
        { name: "Rebuild embeddings" },
      ),
    );
    expect(await screen.findByText("Queueing embedding rebuild…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Check integrity" })).toBeDisabled();
    pending.resolve({
      operation: "REBUILD_EMBEDDINGS",
      changedItems: 0,
      reclaimedBytes: 0,
      safetySnapshotId: null,
      message: "Embedding rebuild completed safely.",
      completedAtMs: 1,
    });
    expect(await screen.findByText("Embedding rebuild completed safely.")).toBeInTheDocument();
    expect(rebuild).toHaveBeenCalledOnce();

    await user.click(screen.getByRole("button", { name: "Retry failed extraction" }));
    await user.click(
      within(screen.getByRole("dialog", { name: "Retry failed extraction?" })).getByRole("button", {
        name: "Retry failed jobs",
      }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("controlled extraction failure");

    await user.click(screen.getByRole("button", { name: "Retry failed extraction" }));
    await user.click(
      within(screen.getByRole("dialog", { name: "Retry failed extraction?" })).getByRole("button", {
        name: "Retry failed jobs",
      }),
    );
    await waitFor(() => expect(retry).toHaveBeenCalledTimes(2));
    expect(
      await screen.findByText("No current failed OCR extractions needed a retry."),
    ).toBeInTheDocument();
  });

  it.each([
    ["FAILED", "Index failed"],
    ["WAITING_FOR_RUNTIME", "Index waiting"],
  ] satisfies [BlockEmbeddingIndexPhase, string][])(
    "reports a ready runtime with a %s embedding index as unhealthy",
    async (phase, label) => {
      const backend = new FakeBackend();
      await backend.prepareSemanticRuntime();
      vi.spyOn(backend, "blockEmbeddingIndexStatus").mockResolvedValue({
        phase,
        embeddingIndexId: "019f547b-6200-7000-8000-000000000002",
        indexKey: "jina_v1",
        indexedBlocks: 2,
        totalBlocks: 3,
        active: false,
        message: "controlled index state",
      });
      renderSettings(backend);

      const semanticSearch = (await screen.findByText("Semantic search")).closest(
        ".settings-diagnostic",
      );
      expect(semanticSearch).not.toBeNull();
      expect(within(semanticSearch as HTMLElement).getByText(label)).toBeInTheDocument();
      expect(semanticSearch).toHaveClass("settings-diagnostic--warning");
    },
  );
});

function renderSettings(backend: FakeBackend) {
  const router = createAppRouter(
    createMemoryHistory({
      initialEntries: ["/settings"],
    }),
  );
  return render(
    <BackendProvider backend={backend}>
      <AppearanceProvider>
        <RouterProvider router={router} />
      </AppearanceProvider>
    </BackendProvider>,
  );
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

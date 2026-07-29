import { RouterProvider, createMemoryHistory } from "@tanstack/react-router";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type {
  ResearchProcessEvent,
  ResearchRunCursor,
  ResearchRunRecord,
  ResearchRunSummary,
} from "../../src/backend/contracts";
import { BackendProvider } from "../../src/backend/context";
import { FakeBackend } from "../../src/backend/fakeBackend";
import { createAppRouter } from "../../src/router";

describe("research route", () => {
  it("streams a grounded answer, opens exact evidence, persists history, and saves a tidbit", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const evidence = await backend.createTidbit({
      title: "Local evidence",
      bodyMarkdown: "The exact local passage.",
      sources: [{ label: "Notebook", url: "https://example.com/notebook" }],
    });
    const first = renderRoute(backend);

    expect(await screen.findByRole("heading", { name: "Research" })).toBeInTheDocument();
    await user.type(
      screen.getByRole("textbox", { name: "What should Kosh investigate?" }),
      "What is in my notes?",
    );
    await user.click(screen.getByRole("button", { name: "Research" }));

    expect(await screen.findByText(/Kosh found a durable answer/u)).toBeInTheDocument();
    expect(screen.getByText("Completed")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Open citation 1" }));
    expect(await screen.findByRole("heading", { name: "Local evidence" })).toBeInTheDocument();
    expect(screen.getByText("The exact local passage.")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Save answer as tidbit" }));
    expect(await screen.findByRole("link", { name: "Open saved tidbit" })).toBeInTheDocument();
    expect(
      (await backend.listTidbits({ limit: 10, cursor: null, scope: "ACTIVE" })).items,
    ).toHaveLength(2);
    const revisedEvidence = await backend.editTidbit({
      id: evidence.id,
      expectedRevisionId: evidence.currentRevisionId,
      title: "Revised local evidence",
      bodyMarkdown: "A newer passage that must not replace the cited snapshot.",
      sources: evidence.sources,
    });
    await backend.deleteTidbit({
      id: revisedEvidence.id,
      expectedRevisionId: revisedEvidence.currentRevisionId,
    });

    first.unmount();
    renderRoute(backend);
    expect(await screen.findByText(/Kosh found a durable answer/u)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open saved tidbit" })).toBeInTheDocument();
    expect(screen.getByText(/1 cited tidbit has a newer revision/u)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Open citation 1" }));
    expect(screen.getByText("Historical passage", { exact: true })).toBeInTheDocument();
    expect(screen.getByText("The exact local passage.")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Open tidbit at passage" })).toBeNull();
  });

  it("supports cancellation and safe failure without losing rerun controls", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    renderRoute(backend);
    const query = await screen.findByRole("textbox", {
      name: "What should Kosh investigate?",
    });

    fireEvent.change(query, { target: { value: "[slow] cancellable question" } });
    await user.click(screen.getByRole("button", { name: "Research" }));
    await user.click(await screen.findByRole("button", { name: "Cancel" }));
    expect(await screen.findByRole("heading", { name: "Research canceled" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Run again" })).toBeInTheDocument();

    fireEvent.change(query, { target: { value: "[fail] safe failure" } });
    await user.click(screen.getByRole("button", { name: "Research" }));
    expect(await screen.findByText("Fixture research failed safely.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Run again" })).toBeInTheDocument();
    await waitFor(async () => {
      expect((await backend.listResearchRuns({ limit: 10, cursor: null })).items).toHaveLength(2);
    });
  });

  it("loads every durable history page", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const firstPage = Array.from({ length: 100 }, (_, index) =>
      researchRecord(
        `run-${index.toString().padStart(3, "0")}`,
        `Recent run ${index}`,
        200 - index,
      ),
    );
    const oldest = researchRecord("run-oldest", "Oldest run", 1);
    const cursor: ResearchRunCursor = {
      updatedAtMs: firstPage.at(-1)!.updatedAtMs,
      id: firstPage.at(-1)!.id,
    };
    const listRuns = vi.spyOn(backend, "listResearchRuns").mockImplementation(async (input) => {
      if (input.cursor === null) {
        return { items: firstPage.map(researchSummary), nextCursor: cursor };
      }
      return { items: [researchSummary(oldest)], nextCursor: null };
    });
    vi.spyOn(backend, "loadResearchRun").mockImplementation(async (id) => {
      const record = [...firstPage, oldest].find((item) => item.id === id);
      if (!record) throw new Error(`unknown fixture run ${id}`);
      return record;
    });

    renderRoute(backend);
    await screen.findByRole("heading", { name: firstPage[0]!.query });
    await user.click(screen.getByRole("button", { name: "Load older runs" }));

    expect(await screen.findByRole("button", { name: /Oldest run/u })).toBeInTheDocument();
    expect(listRuns).toHaveBeenLastCalledWith({ limit: 100, cursor });
    expect(screen.queryByRole("button", { name: "Load older runs" })).toBeNull();
  });

  it("ignores stale history success and error responses", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const initial = researchRecord("run-initial", "Initial run", 3);
    const slow = researchRecord("run-slow", "Slow run", 2);
    const latest = researchRecord("run-latest", "Latest run", 1);
    const staleSuccess = deferred<ResearchRunRecord>();
    const staleFailure = deferred<ResearchRunRecord>();
    let slowLoads = 0;
    vi.spyOn(backend, "listResearchRuns").mockResolvedValue({
      items: [initial, slow, latest].map(researchSummary),
      nextCursor: null,
    });
    vi.spyOn(backend, "loadResearchRun").mockImplementation(async (id) => {
      if (id === slow.id) {
        slowLoads += 1;
        return slowLoads === 1 ? staleSuccess.promise : staleFailure.promise;
      }
      return id === initial.id ? initial : latest;
    });

    renderRoute(backend);
    await screen.findByRole("heading", { name: initial.query });
    await user.click(screen.getByRole("button", { name: /Slow run/u }));
    await user.click(screen.getByRole("button", { name: /Latest run/u }));
    expect(await screen.findByRole("heading", { name: latest.query })).toBeInTheDocument();
    await act(async () => {
      staleSuccess.resolve(slow);
      await staleSuccess.promise;
    });
    expect(screen.getByRole("heading", { name: latest.query })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Slow run/u }));
    await user.click(screen.getByRole("button", { name: /Latest run/u }));
    await act(async () => {
      staleFailure.reject(new Error("stale load failed"));
      await staleFailure.promise.catch(() => undefined);
    });
    expect(screen.getByRole("heading", { name: latest.query })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("coalesces an event burst into one in-flight and one trailing history read", async () => {
    const backend = new FakeBackend();
    const runId = "run-streaming";
    const initial = researchRecord(runId, "Streaming run", 1);
    initial.finalAnswer!.markdown = "Initial persisted answer.";
    const latest = researchRecord(runId, "Streaming run", 4);
    latest.finalAnswer!.markdown = "Latest persisted answer.";
    const events: ResearchProcessEvent[] = [
      { runId, sequence: 1, kind: "STARTED" },
      {
        runId,
        sequence: 2,
        kind: "GROUNDED_FINAL_OUTPUT",
        answer: latest.finalAnswer!,
      },
      {
        runId,
        sequence: 3,
        kind: "FINISHED",
        outcome: "SUCCEEDED",
        stderrTruncated: false,
      },
    ];
    latest.events = events;
    const partial = { ...latest, events: [events[0]!] };
    const firstRefresh = deferred<ResearchRunRecord>();
    const trailingRefresh = deferred<ResearchRunRecord>();
    let listener: ((event: ResearchProcessEvent) => void) | undefined;
    let activeReads = 0;
    let maximumActiveReads = 0;
    vi.spyOn(backend, "listResearchRuns").mockResolvedValue({
      items: [researchSummary(initial)],
      nextCursor: null,
    });
    vi.spyOn(backend, "onResearchProcessEvent").mockImplementation(async (handler) => {
      listener = handler;
      return () => undefined;
    });
    const loadRun = vi.spyOn(backend, "loadResearchRun").mockImplementation(async () => {
      if (loadRun.mock.calls.length === 1) return initial;
      activeReads += 1;
      maximumActiveReads = Math.max(maximumActiveReads, activeReads);
      try {
        return await (loadRun.mock.calls.length === 2
          ? firstRefresh.promise
          : trailingRefresh.promise);
      } finally {
        activeReads -= 1;
      }
    });

    renderRoute(backend);
    expect(await screen.findByText("Initial persisted answer.")).toBeInTheDocument();
    expect(listener).toBeDefined();
    for (const event of events) listener!(event);
    expect(loadRun).toHaveBeenCalledTimes(2);
    expect(maximumActiveReads).toBe(1);

    await act(async () => {
      firstRefresh.resolve(partial);
      await firstRefresh.promise;
    });
    await waitFor(() => expect(loadRun).toHaveBeenCalledTimes(3));
    expect(maximumActiveReads).toBe(1);
    await act(async () => {
      trailingRefresh.resolve(latest);
      await trailingRefresh.promise;
    });

    expect(await screen.findByText("Latest persisted answer.")).toBeInTheDocument();
    expect(loadRun).toHaveBeenCalledTimes(3);
    expect(maximumActiveReads).toBe(1);
  });

  it("does not let initial history replace a terminal live refresh", async () => {
    const backend = new FakeBackend();
    const runId = "run-racing-history";
    const stale = researchRecord(runId, "Racing history", 1);
    stale.status = "RUNNING";
    stale.completedAtMs = null;
    stale.finalAnswer = null;
    const completed = researchRecord(runId, "Racing history", 2);
    const terminalEvent: ResearchProcessEvent = {
      runId,
      sequence: 1,
      kind: "FINISHED",
      outcome: "SUCCEEDED",
      stderrTruncated: false,
    };
    completed.events = [terminalEvent];
    const initialPage = deferred<{
      items: ResearchRunSummary[];
      nextCursor: ResearchRunCursor | null;
    }>();
    const liveRefresh = deferred<ResearchRunRecord>();
    let listener: ((event: ResearchProcessEvent) => void) | undefined;
    vi.spyOn(backend, "listResearchRuns").mockReturnValue(initialPage.promise);
    vi.spyOn(backend, "onResearchProcessEvent").mockImplementation(async (handler) => {
      listener = handler;
      return () => undefined;
    });
    const loadRun = vi.spyOn(backend, "loadResearchRun").mockImplementation(async () => {
      return loadRun.mock.calls.length === 1 ? liveRefresh.promise : completed;
    });

    renderRoute(backend);
    await waitFor(() => expect(listener).toBeDefined());
    await act(async () => {
      listener!(terminalEvent);
    });
    await waitFor(() => expect(loadRun).toHaveBeenCalledTimes(1));
    await act(async () => {
      liveRefresh.resolve(completed);
      await liveRefresh.promise;
      await Promise.resolve();
    });

    await act(async () => {
      initialPage.resolve({ items: [researchSummary(stale)], nextCursor: null });
      await initialPage.promise;
    });

    const history = await screen.findByLabelText("Research history");
    expect(await within(history).findByText(/Completed ·/u)).toBeInTheDocument();
    expect(within(history).queryByText(/Running ·/u)).toBeNull();
    expect(await screen.findByText(completed.finalAnswer!.markdown)).toBeInTheDocument();
  });
});

function renderRoute(backend: FakeBackend) {
  const router = createAppRouter(
    createMemoryHistory({
      initialEntries: ["/research"],
    }),
  );
  return render(
    <BackendProvider backend={backend}>
      <RouterProvider router={router} />
    </BackendProvider>,
  );
}

function researchRecord(id: string, query: string, updatedAtMs: number): ResearchRunRecord {
  return {
    id,
    rerunOfId: null,
    query,
    status: "COMPLETED",
    requestedModel: null,
    requestedEffort: null,
    actualModel: "fixture",
    createdAtMs: updatedAtMs,
    startedAtMs: updatedAtMs,
    completedAtMs: updatedAtMs,
    updatedAtMs,
    error: null,
    stderrTruncated: false,
    savedTidbitId: null,
    events: [],
    finalAnswer: {
      markdown: `${query} answer.`,
      citations: [],
      mentions: [],
      issues: [],
    },
    citationFreshness: [],
  };
}

function researchSummary(record: ResearchRunRecord): ResearchRunSummary {
  const {
    events: _events,
    finalAnswer: _finalAnswer,
    citationFreshness: _citationFreshness,
    ...summary
  } = record;
  return summary;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

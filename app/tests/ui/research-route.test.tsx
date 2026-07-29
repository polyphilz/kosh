import { RouterProvider, createMemoryHistory } from "@tanstack/react-router";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
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
    expect((await backend.listTidbits({ limit: 10, cursor: null })).items).toHaveLength(2);
    await backend.editTidbit({
      id: evidence.id,
      expectedRevisionId: evidence.currentRevisionId,
      title: "Revised local evidence",
      bodyMarkdown: "A newer passage that must not replace the cited snapshot.",
      sources: evidence.sources,
    });

    first.unmount();
    renderRoute(backend);
    expect(await screen.findByText(/Kosh found a durable answer/u)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open saved tidbit" })).toBeInTheDocument();
    expect(screen.getByText(/1 cited tidbit has a newer revision/u)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Open citation 1" }));
    expect(screen.getByText("The exact local passage.")).toBeInTheDocument();
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

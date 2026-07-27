import { RouterProvider, createMemoryHistory } from "@tanstack/react-router";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { BackendProvider } from "../../src/backend/context";
import { FakeBackend } from "../../src/backend/fakeBackend";
import { createAppRouter } from "../../src/router";

describe("runtime route", () => {
  it("renders a deterministic probe through the typed backend", async () => {
    const backend = new FakeBackend({
      dataDir: "/tmp/kosh-ui-fixture",
      nowMs: 1234,
      requestId: "ui-request-1",
    });
    const router = createAppRouter(
      createMemoryHistory({
        initialEntries: ["/runtime"],
      }),
    );

    render(
      <BackendProvider backend={backend}>
        <RouterProvider router={router} />
      </BackendProvider>,
    );

    expect(await screen.findByRole("heading", { name: "Runtime" })).toBeInTheDocument();
    expect(await screen.findByText("/tmp/kosh-ui-fixture")).toBeInTheDocument();
    expect(screen.getByText("ui-request-1")).toBeInTheDocument();
    expect(screen.getByText("1234")).toBeInTheDocument();
  });
});

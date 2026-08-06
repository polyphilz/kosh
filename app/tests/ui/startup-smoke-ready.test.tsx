import { emit } from "@tauri-apps/api/event";
import { render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../../src/backend/context";
import { FakeBackend } from "../../src/backend/fakeBackend";
import { StartupSmokeReady } from "../../src/components/StartupSmokeReady";

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn(),
}));

describe("StartupSmokeReady", () => {
  beforeEach(() => {
    vi.mocked(emit).mockReset().mockResolvedValue();
  });

  afterEach(() => {
    delete (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  });

  it("does nothing outside the Tauri runtime", () => {
    render(
      <BackendProvider backend={new FakeBackend()}>
        <div>Rendered app</div>
        <StartupSmokeReady surface="main" />
      </BackendProvider>,
    );

    expect(emit).not.toHaveBeenCalled();
  });

  it("does nothing in a normal Tauri launch without a smoke request", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    const backend = new FakeBackend({
      dataDir: "/tmp/kosh-startup",
      nowMs: 42,
      requestId: "probe-1",
    });
    const runtimeProbe = vi.spyOn(backend, "runtimeProbe");
    const root = document.createElement("div");
    root.id = "root";
    document.body.append(root);

    render(
      <BackendProvider backend={backend}>
        <div>Rendered app</div>
        <StartupSmokeReady surface="quick-add" />
      </BackendProvider>,
      { container: root },
    );

    await waitFor(() => expect(runtimeProbe).toHaveBeenCalledOnce());
    expect(emit).not.toHaveBeenCalled();
  });

  it("proves exact current-block search when startup smoke requests it", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    const backend = new FakeBackend({
      dataDir: "/tmp/kosh-startup",
      nowMs: 42,
      requestId: "probe-2",
      startupSmokeCanary: "koshstartupcanaryv1",
    });
    const tidbit = await backend.seedNote({
      bodyMarkdown: "koshstartupcanaryv1",
      sources: [
        {
          label: "Kosh startup smoke",
          url: "https://example.invalid/kosh-progressive-operability",
        },
      ],
    });
    const root = document.createElement("div");
    root.id = "root";
    document.body.append(root);

    render(
      <BackendProvider backend={backend}>
        <div>Rendered app</div>
        <StartupSmokeReady surface="main" />
      </BackendProvider>,
      { container: root },
    );

    await waitFor(() => {
      expect(emit).toHaveBeenCalledWith(
        "kosh://startup-smoke-ready",
        expect.objectContaining({
          surface: "main",
          captureCreated: false,
          canary: expect.objectContaining({
            blockId: expect.any(String),
            executionMode: "EXACT",
            noteId: tidbit.id,
            contentVersionId: tidbit.contentVersionId,
            resultCount: 1,
            sourceUrl: "https://example.invalid/kosh-progressive-operability",
          }),
        }),
      );
    });
  });

  it("creates the fresh canary through capture IPC before proving search", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    const backend = new FakeBackend({
      dataDir: "/tmp/kosh-startup",
      nowMs: 42,
      requestId: "probe-3",
      startupSmokeCanary: "koshstartupcanaryv1",
      startupSmokeCapture: true,
    });
    const root = document.createElement("div");
    root.id = "root";
    document.body.append(root);

    render(
      <BackendProvider backend={backend}>
        <div>Rendered app</div>
        <StartupSmokeReady surface="main" />
      </BackendProvider>,
      { container: root },
    );

    await waitFor(() => {
      expect(emit).toHaveBeenCalledWith(
        "kosh://startup-smoke-ready",
        expect.objectContaining({
          surface: "main",
          captureCreated: true,
          canary: expect.objectContaining({
            blockId: expect.any(String),
            executionMode: "EXACT",
            noteId: expect.any(String),
            resultCount: 1,
            sourceUrl: "https://example.invalid/kosh-progressive-operability",
          }),
        }),
      );
    });
    await expect(
      backend.searchBlocks({ query: "koshstartupcanaryv1", mode: "EXACT", limit: 10 }),
    ).resolves.toMatchObject({ results: [{ blockId: expect.any(String) }] });
  });
});

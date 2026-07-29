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

  it("emits rendered React and backend IPC evidence", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    const root = document.createElement("div");
    root.id = "root";
    document.body.append(root);

    render(
      <BackendProvider
        backend={
          new FakeBackend({
            dataDir: "/tmp/kosh-startup",
            nowMs: 42,
            requestId: "probe-1",
          })
        }
      >
        <div>Rendered app</div>
        <StartupSmokeReady surface="quick-add" />
      </BackendProvider>,
      { container: root },
    );

    await waitFor(() => {
      expect(emit).toHaveBeenCalledWith("kosh://startup-smoke-ready", {
        surface: "quick-add",
        rendered: true,
        documentReadyState: "complete",
        rootChildCount: 1,
        frontendOrigin: window.location.origin,
        probeDataDir: "/tmp/kosh-startup",
        probeRequestId: "probe-1",
        canary: null,
      });
    });
  });

  it("proves exact search and citation resolution when startup smoke requests it", async () => {
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
    const tidbit = await backend.createTidbit({
      title: "Kosh progressive startup canary",
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
          canary: {
            citationState: "CURRENT",
            executionMode: "EXACT",
            passageId: `fake-passage:${tidbit.currentRevisionId}`,
            resolvedPassageId: `fake-passage:${tidbit.currentRevisionId}`,
            revisionId: tidbit.currentRevisionId,
            resultCount: 1,
            sourceUrl: "https://example.invalid/kosh-progressive-operability",
          },
        }),
      );
    });
  });
});

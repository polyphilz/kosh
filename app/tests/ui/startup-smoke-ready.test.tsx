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
        probeDataDir: "/tmp/kosh-startup",
        probeRequestId: "probe-1",
      });
    });
  });
});

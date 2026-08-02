import { render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  listen: vi.fn(),
  tauriEnvironment: false,
}));

vi.mock("@tauri-apps/api/core", () => ({
  isTauri: () => mocks.tauriEnvironment,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listen,
}));

vi.mock("../../src/shortcuts/context.tsx", () => ({
  useShortcutSettings: () => ({
    settings: { automaticUpdateChecksEnabled: false },
  }),
}));

beforeEach(() => {
  mocks.listen.mockResolvedValue(vi.fn());
});

afterEach(() => {
  mocks.tauriEnvironment = false;
  vi.clearAllMocks();
});

test("does not register native update listeners outside Tauri", async () => {
  const { AppUpdater } = await import("../../src/updater/AppUpdater.tsx");

  render(<AppUpdater />);

  expect(mocks.listen).not.toHaveBeenCalled();
});

test("registers manual update listeners in Tauri when automatic updates are disabled", async () => {
  mocks.tauriEnvironment = true;
  const { AppUpdater } = await import("../../src/updater/AppUpdater.tsx");

  render(<AppUpdater />);

  await waitFor(() => expect(mocks.listen).toHaveBeenCalledTimes(1));
  expect(mocks.listen).toHaveBeenCalledWith("kosh://check-for-updates", expect.any(Function));
});

test("enables updates only for an explicitly marked Tauri release", async () => {
  const { updaterIsEnabled } = await import("../../src/updater/index.ts");

  expect(updaterIsEnabled(true, "true")).toBe(true);
  expect(updaterIsEnabled(true, undefined)).toBe(false);
  expect(updaterIsEnabled(true, "false")).toBe(false);
  expect(updaterIsEnabled(false, "true")).toBe(false);
});

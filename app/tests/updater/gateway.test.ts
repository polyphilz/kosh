import { beforeEach, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  check: vi.fn(),
  close: vi.fn(),
  downloadAndInstall: vi.fn(),
  invoke: vi.fn(),
  relaunch: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invoke,
}));

vi.mock("@tauri-apps/plugin-updater", () => ({
  check: mocks.check,
}));

vi.mock("@tauri-apps/plugin-process", () => ({
  relaunch: mocks.relaunch,
}));

beforeEach(() => {
  vi.resetModules();
  vi.clearAllMocks();
  mocks.check.mockResolvedValue({
    body: null,
    close: mocks.close,
    currentVersion: "0.1.0",
    date: null,
    downloadAndInstall: mocks.downloadAndInstall,
    version: "0.2.0",
  });
  mocks.downloadAndInstall.mockResolvedValue(undefined);
  mocks.invoke.mockResolvedValue(41);
  mocks.relaunch.mockResolvedValue(undefined);
});

test("preserves open drafts before relaunching an installed update", async () => {
  const { tauriUpdateGateway } = await import("../../src/updater/gateway.ts");

  await tauriUpdateGateway.relaunch();

  expect(mocks.invoke).toHaveBeenCalledWith("prepare_update_relaunch");
  expect(mocks.invoke).toHaveBeenCalledOnce();
  expect(mocks.relaunch).toHaveBeenCalledOnce();
  expect(mocks.invoke.mock.invocationCallOrder[0]).toBeLessThan(
    mocks.relaunch.mock.invocationCallOrder[0]!,
  );
});

test("releases preserved drafts when relaunching fails", async () => {
  const relaunchError = new Error("restart unavailable");
  mocks.relaunch.mockRejectedValue(relaunchError);
  const { tauriUpdateGateway } = await import("../../src/updater/gateway.ts");

  await expect(tauriUpdateGateway.relaunch()).rejects.toBe(relaunchError);

  expect(mocks.invoke).toHaveBeenNthCalledWith(1, "prepare_update_relaunch");
  expect(mocks.invoke).toHaveBeenNthCalledWith(2, "cancel_update_relaunch", {
    requestId: 41,
  });
});

test("preserves the relaunch error when releasing drafts also fails", async () => {
  const relaunchError = new Error("restart unavailable");
  const cleanupError = new Error("cleanup unavailable");
  const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
  mocks.relaunch.mockRejectedValue(relaunchError);
  mocks.invoke.mockResolvedValueOnce(41).mockRejectedValueOnce(cleanupError);
  const { tauriUpdateGateway } = await import("../../src/updater/gateway.ts");

  await expect(tauriUpdateGateway.relaunch()).rejects.toBe(relaunchError);

  expect(consoleError).toHaveBeenCalledWith(
    "Could not release drafts after the update restart failed",
    cleanupError,
  );
  consoleError.mockRestore();
});

test("bounds update checks and downloads with explicit timeouts", async () => {
  const { tauriUpdateGateway } = await import("../../src/updater/gateway.ts");

  await tauriUpdateGateway.check();
  expect(mocks.check).toHaveBeenCalledWith({ timeout: 30_000 });

  await tauriUpdateGateway.downloadAndInstall(vi.fn());
  expect(mocks.downloadAndInstall).toHaveBeenCalledWith(expect.any(Function), {
    timeout: 10 * 60 * 1_000,
  });
});

test("serializes checks and keeps the newest checked update for installation", async () => {
  const firstDownloadAndInstall = vi.fn().mockResolvedValue(undefined);
  const secondDownloadAndInstall = vi.fn().mockResolvedValue(undefined);
  const firstClose = vi.fn();
  const firstCheck = deferred<{
    body: null;
    close: typeof firstClose;
    currentVersion: string;
    date: null;
    downloadAndInstall: typeof firstDownloadAndInstall;
    version: string;
  }>();
  mocks.check.mockReset();
  mocks.check.mockReturnValueOnce(firstCheck.promise).mockResolvedValueOnce({
    body: null,
    close: vi.fn(),
    currentVersion: "0.1.0",
    date: null,
    downloadAndInstall: secondDownloadAndInstall,
    version: "0.3.0",
  });

  const { tauriUpdateGateway } = await import("../../src/updater/gateway.ts");
  const automaticCheck = tauriUpdateGateway.check();
  const manualCheck = tauriUpdateGateway.check();

  await vi.waitFor(() => expect(mocks.check).toHaveBeenCalledTimes(1));
  firstCheck.resolve({
    body: null,
    close: firstClose,
    currentVersion: "0.1.0",
    date: null,
    downloadAndInstall: firstDownloadAndInstall,
    version: "0.2.0",
  });

  await expect(automaticCheck).resolves.toMatchObject({ version: "0.2.0" });
  await expect(manualCheck).resolves.toMatchObject({ version: "0.3.0" });
  expect(firstClose).toHaveBeenCalledOnce();

  await tauriUpdateGateway.downloadAndInstall(vi.fn());
  expect(firstDownloadAndInstall).not.toHaveBeenCalled();
  expect(secondDownloadAndInstall).toHaveBeenCalledOnce();
});

function deferred<T>() {
  let resolvePromise: (value: T) => void = () => undefined;
  const promise = new Promise<T>((resolve) => {
    resolvePromise = resolve;
  });
  return { promise, resolve: resolvePromise };
}

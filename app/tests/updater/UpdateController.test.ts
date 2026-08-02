import { act } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import {
  UpdateController,
  UpdatePhase,
  type AvailableUpdate,
  type UpdateDownloadProgress,
  type UpdateGateway,
} from "../../src/updater/index.ts";

const availableUpdate: AvailableUpdate = {
  currentVersion: "0.1.0",
  version: "0.2.0",
  notes: "A calmer updater.",
  publishedAt: "2026-07-31T12:00:00Z",
};

const gateway = {
  check: vi.fn<UpdateGateway["check"]>(),
  downloadAndInstall: vi.fn<UpdateGateway["downloadAndInstall"]>(),
  relaunch: vi.fn<UpdateGateway["relaunch"]>(),
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.useFakeTimers();
  window.localStorage.clear();
  gateway.check.mockResolvedValue(null);
  gateway.downloadAndInstall.mockResolvedValue(undefined);
  gateway.relaunch.mockResolvedValue(undefined);
});

afterEach(() => {
  vi.useRealTimers();
});

test("checks quietly after launch and every six hours", async () => {
  const controller = new UpdateController(gateway);
  const stop = controller.start();

  await act(async () => vi.advanceTimersByTimeAsync(4_999));
  expect(gateway.check).not.toHaveBeenCalled();

  await act(async () => vi.advanceTimersByTimeAsync(1));
  expect(gateway.check).toHaveBeenCalledTimes(1);
  expect(controller.getSnapshot()).toEqual({ phase: UpdatePhase.Idle });

  await act(async () => vi.advanceTimersByTimeAsync(6 * 60 * 60 * 1_000));
  expect(gateway.check).toHaveBeenCalledTimes(2);
  stop();
});

test("keeps automatic checks off until the persisted setting enables them", async () => {
  const controller = new UpdateController(gateway, {
    automaticChecksEnabled: false,
  });
  controller.start();

  await act(async () => vi.advanceTimersByTimeAsync(6 * 60 * 60 * 1_000));
  expect(gateway.check).not.toHaveBeenCalled();

  controller.setAutomaticChecksEnabled(true);
  await act(async () => vi.advanceTimersByTimeAsync(5_000));
  expect(gateway.check).toHaveBeenCalledTimes(1);

  controller.setAutomaticChecksEnabled(false);
  await act(async () => vi.advanceTimersByTimeAsync(6 * 60 * 60 * 1_000));
  expect(gateway.check).toHaveBeenCalledTimes(1);
});

test("manual checks report when Kosh is current", async () => {
  const controller = new UpdateController(gateway);

  await controller.checkManually();

  expect(controller.getSnapshot()).toEqual({ phase: UpdatePhase.Current });
});

test("manual checks explain when updates are unavailable in this build", async () => {
  const controller = new UpdateController(gateway, { enabled: false });

  await controller.checkManually();

  expect(gateway.check).not.toHaveBeenCalled();
  expect(controller.getSnapshot()).toEqual({
    phase: UpdatePhase.Error,
    message: "Update checks are available in packaged Kosh releases.",
  });
});

test("offers an update and dismisses that version for one day", async () => {
  const now = 1_000_000;
  gateway.check.mockResolvedValue(availableUpdate);
  const first = new UpdateController(gateway, { now: () => now });

  await first.checkManually();
  expect(first.getSnapshot()).toEqual({
    phase: UpdatePhase.Available,
    update: availableUpdate,
  });
  first.dismiss();

  const dismissed = new UpdateController(gateway, { now: () => now + 1 });
  dismissed.start();
  await act(async () => vi.advanceTimersByTimeAsync(5_000));
  expect(dismissed.getSnapshot()).toEqual({ phase: UpdatePhase.Idle });

  const expired = new UpdateController(gateway, {
    now: () => now + 24 * 60 * 60 * 1_000 + 1,
  });
  expired.start();
  await act(async () => vi.advanceTimersByTimeAsync(5_000));
  expect(expired.getSnapshot()).toEqual({
    phase: UpdatePhase.Available,
    update: availableUpdate,
  });
});

test("installs the checked update and relaunches after download", async () => {
  gateway.check.mockResolvedValue(availableUpdate);
  gateway.downloadAndInstall.mockImplementation(async (onProgress) => {
    const progress: UpdateDownloadProgress = {
      downloadedBytes: 75,
      totalBytes: 100,
    };
    onProgress(progress);
  });
  const controller = new UpdateController(gateway);

  await controller.checkManually();
  await controller.installAndRestart();

  expect(gateway.downloadAndInstall).toHaveBeenCalledTimes(1);
  expect(gateway.relaunch).toHaveBeenCalledTimes(1);
  expect(controller.getSnapshot()).toEqual({
    phase: UpdatePhase.Installing,
    update: availableUpdate,
  });
});

test("fences working copies before installing and relaunching", async () => {
  const prepareForRestart = vi.fn(async () => undefined);
  gateway.check.mockResolvedValue(availableUpdate);
  const controller = new UpdateController(gateway, { prepareForRestart });

  await controller.checkManually();
  await controller.installAndRestart();

  expect(prepareForRestart).toHaveBeenCalledOnce();
  expect(prepareForRestart.mock.invocationCallOrder[0]).toBeLessThan(
    gateway.downloadAndInstall.mock.invocationCallOrder[0] ?? Number.MAX_SAFE_INTEGER,
  );
});

test("does not install or relaunch when the working-copy fence fails", async () => {
  gateway.check.mockResolvedValue(availableUpdate);
  const controller = new UpdateController(gateway, {
    prepareForRestart: async () => {
      throw new Error("note could not be saved");
    },
  });

  await controller.checkManually();
  await controller.installAndRestart();

  expect(gateway.downloadAndInstall).not.toHaveBeenCalled();
  expect(gateway.relaunch).not.toHaveBeenCalled();
  expect(controller.getSnapshot()).toEqual({
    phase: UpdatePhase.Error,
    message: "note could not be saved",
  });
});

test("relaunches after an installed update even when the controller stops", async () => {
  let finishInstallation: (() => void) | undefined;
  gateway.check.mockResolvedValue(availableUpdate);
  gateway.downloadAndInstall.mockImplementation(
    () =>
      new Promise<void>((resolve) => {
        finishInstallation = resolve;
      }),
  );
  const controller = new UpdateController(gateway);

  await controller.checkManually();
  const installation = controller.installAndRestart();
  await vi.waitFor(() => expect(gateway.downloadAndInstall).toHaveBeenCalledOnce());
  controller.stop();
  finishInstallation?.();
  await installation;

  expect(gateway.relaunch).toHaveBeenCalledTimes(1);
});

test("automatic checks preserve an available update prompt", async () => {
  gateway.check.mockResolvedValueOnce(availableUpdate);
  const controller = new UpdateController(gateway);

  await controller.checkManually();
  gateway.check.mockRejectedValue(new Error("GitHub is unavailable"));
  controller.start();
  await act(async () => vi.advanceTimersByTimeAsync(5_000));

  expect(gateway.check).toHaveBeenCalledTimes(1);
  expect(controller.getSnapshot()).toEqual({
    phase: UpdatePhase.Available,
    update: availableUpdate,
  });
  controller.stop();
});

test.each([UpdatePhase.Current, UpdatePhase.Error])(
  "automatic checks preserve a visible %s result",
  async (phase) => {
    const controller = new UpdateController(gateway);
    if (phase === UpdatePhase.Current) {
      await controller.checkManually();
    } else {
      gateway.check.mockRejectedValueOnce(new Error("GitHub is unavailable"));
      await controller.checkManually();
    }
    const visibleState = controller.getSnapshot();
    gateway.check.mockResolvedValue(null);

    controller.start();
    await act(async () => vi.advanceTimersByTimeAsync(5_000));

    expect(gateway.check).toHaveBeenCalledTimes(1);
    expect(controller.getSnapshot()).toEqual(visibleState);
  },
);

test("dismisses a pending manual check without letting its result reappear", async () => {
  let finishCheck: ((update: AvailableUpdate | null) => void) | undefined;
  gateway.check.mockImplementation(
    () =>
      new Promise((resolve) => {
        finishCheck = resolve;
      }),
  );
  const controller = new UpdateController(gateway);

  const check = controller.checkManually();
  controller.dismiss();
  finishCheck?.(availableUpdate);
  await check;

  expect(controller.getSnapshot()).toEqual({ phase: UpdatePhase.Idle });
});

test("dismisses an available update even when storage is unavailable", async () => {
  const consoleWarning = vi.spyOn(console, "warn").mockImplementation(() => undefined);
  gateway.check.mockResolvedValue(availableUpdate);
  const controller = new UpdateController(gateway, {
    storage: {
      getItem: () => null,
      removeItem: () => undefined,
      setItem: () => {
        throw new Error("storage is unavailable");
      },
    },
  });

  await controller.checkManually();
  expect(() => controller.dismiss()).not.toThrow();
  expect(controller.getSnapshot()).toEqual({ phase: UpdatePhase.Idle });
  consoleWarning.mockRestore();
});

test("automatic failures stay quiet while manual failures are actionable", async () => {
  const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
  gateway.check.mockRejectedValue(new Error("GitHub is unavailable"));
  const controller = new UpdateController(gateway);
  controller.start();

  await act(async () => vi.advanceTimersByTimeAsync(5_000));
  expect(controller.getSnapshot()).toEqual({ phase: UpdatePhase.Idle });

  await controller.checkManually();
  expect(controller.getSnapshot()).toEqual({
    phase: UpdatePhase.Error,
    message: "GitHub is unavailable",
  });
  consoleError.mockRestore();
});

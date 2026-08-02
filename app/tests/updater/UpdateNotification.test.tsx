import { fireEvent, render, waitFor } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import {
  UpdateController,
  UpdateNotification,
  UpdatePhase,
  type AvailableUpdate,
  type UpdateGateway,
} from "../../src/updater/index.ts";

const availableUpdate: AvailableUpdate = {
  currentVersion: "0.1.0",
  version: "0.2.0",
  notes: null,
  publishedAt: null,
};

test("uses Kosh buttons for installing or deferring an update", async () => {
  const gateway: UpdateGateway = {
    check: vi.fn().mockResolvedValue(availableUpdate),
    downloadAndInstall: vi.fn().mockResolvedValue(undefined),
    relaunch: vi.fn().mockResolvedValue(undefined),
  };
  const controller = new UpdateController(gateway);
  await controller.checkManually();
  const { getByRole, queryByRole } = render(
    <UpdateNotification controller={controller} state={controller.getSnapshot()} />,
  );

  expect(getByRole("status").textContent).toContain("Kosh 0.2.0 is available");
  expect(
    getByRole("button", { name: "Install and restart" }).classList.contains("kosh-button"),
  ).toBe(true);
  fireEvent.click(getByRole("button", { name: "Not now" }));
  await waitFor(() => expect(controller.getSnapshot()).toEqual({ phase: UpdatePhase.Idle }));
  expect(queryByRole("alert")).toBeNull();
});

test("formats gigabyte-scale update progress clearly", () => {
  const controller = new UpdateController({
    check: vi.fn().mockResolvedValue(null),
    downloadAndInstall: vi.fn().mockResolvedValue(undefined),
    relaunch: vi.fn().mockResolvedValue(undefined),
  });
  const { getByRole } = render(
    <UpdateNotification
      controller={controller}
      state={{
        phase: UpdatePhase.Downloading,
        update: availableUpdate,
        downloadedBytes: 1_073_741_824,
        totalBytes: 2_147_483_648,
      }}
    />,
  );

  expect(getByRole("status").textContent).toContain("50% · 1.0 GB of 2.0 GB");
});

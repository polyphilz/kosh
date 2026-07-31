import { RouterProvider, createMemoryHistory } from "@tanstack/react-router";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { BackupRestorePreview } from "../../src/backend/contracts";
import { BackendProvider } from "../../src/backend/context";
import { FakeBackend } from "../../src/backend/fakeBackend";
import { AppearanceProvider } from "../../src/components/Appearance";
import { createAppRouter } from "../../src/router";

const ACCOUNT_ID = "0123456789abcdef0123456789abcdef";
const ACCESS_KEY_ID = "fedcba9876543210fedcba9876543210";
const SECRET_ACCESS_KEY = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

describe("offsite recovery settings", () => {
  it("keeps setup opt-in, verifies the target, and never renders saved secrets", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    renderSettings(backend);

    expect(await screen.findByRole("heading", { name: "Offsite recovery" })).toBeInTheDocument();
    expect(
      screen.getByText("This is backup, not multi-device sync.", { exact: false }),
    ).toBeVisible();
    await user.click(screen.getByText("Retention and recovery details"));
    expect(screen.getByText("30 days.", { exact: false })).toBeVisible();
    expect(
      screen.getByText("Complete checkpoint manifests are immutable", { exact: false }),
    ).toBeVisible();
    await fillTarget(user);

    await user.click(screen.getByRole("button", { name: "Test connection" }));
    expect(
      await screen.findByText("Connection verified. Kosh wrote, read, listed", { exact: false }),
    ).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Save target off" }));

    expect(await screen.findByText("kosh-local")).toBeVisible();
    expect(screen.getByText("Stored")).toBeVisible();
    expect(screen.getByRole("switch", { name: "Back up this library" })).not.toBeChecked();
    expect(screen.queryByDisplayValue(ACCESS_KEY_ID)).not.toBeInTheDocument();
    expect(screen.queryByDisplayValue(SECRET_ACCESS_KEY)).not.toBeInTheDocument();
    expect(JSON.stringify(await backend.loadBackupSettings())).not.toContain(SECRET_ACCESS_KEY);
  });

  it("shows separated health and supports a complete non-mutating recovery drill", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    await configure(backend);
    renderSettings(backend);

    await screen.findByRole("switch", { name: "Back up this library" });
    await user.click(screen.getByRole("button", { name: "Edit target" }));
    await user.type(screen.getByLabelText("R2 access key ID"), ACCESS_KEY_ID);
    await user.type(screen.getByLabelText("R2 secret access key"), SECRET_ACCESS_KEY);
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    await user.click(screen.getByRole("button", { name: "Edit target" }));
    expect(screen.getByLabelText("R2 access key ID")).toHaveValue("");
    expect(screen.getByLabelText("R2 secret access key")).toHaveValue("");
    await user.click(screen.getByRole("button", { name: "Cancel" }));

    const toggle = screen.getByRole("switch", { name: "Back up this library" });
    await user.click(toggle);
    await waitFor(() => expect(toggle).toBeChecked());
    expect(screen.getByText("Running")).toBeVisible();
    expect(screen.getByText("Current")).toBeVisible();
    expect(screen.getByText("Idle")).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Back up now" }));
    expect(await screen.findByText("A complete recovery point was published.")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Find recovery points" }));
    expect(await screen.findByText("Found 1 complete recovery point.")).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Preview restore" }));
    expect(await screen.findByText("Verified restore preview")).toBeVisible();
    expect(screen.getByText("Owned by this installation")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Run recovery drill" }));
    expect(
      await screen.findByText("Your live library was not changed.", { exact: false }),
    ).toBeVisible();
  });

  it("makes takeover explicit and sends the exact previewed owner", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const configured = await configure(backend);
    await backend.setBackupEnabled({
      expectedRevision: configured.config!.revision,
      enabled: true,
    });
    await backend.backupNow();
    const enabled = await backend.loadBackupSettings();
    await backend.setBackupEnabled({
      expectedRevision: enabled.config!.revision,
      enabled: false,
    });
    const [checkpoint] = await backend.listBackupCheckpoints();
    const foreignPreview: BackupRestorePreview = {
      checkpoint: checkpoint!,
      owner: {
        backupSetId: configured.config!.backupSetId,
        replicaEpochId: "019f547b-6200-7000-8000-000000000e99",
        writerId: "f".repeat(64),
        version: '"foreign-owner-v9"',
        isCurrentInstallation: false,
      },
      planFileCount: 2,
      planTotalBytes: 8192,
    };
    vi.spyOn(backend, "previewBackupRestore").mockResolvedValue(foreignPreview);
    const takeover = vi.spyOn(backend, "takeOverBackup").mockImplementation(async () => {
      const snapshot = await backend.loadBackupSettings();
      return {
        ...snapshot,
        config: snapshot.config
          ? {
              ...snapshot.config,
              revision: snapshot.config.revision + 1,
              replicaEpochId: "019f547b-6200-7000-8000-000000000e02",
            }
          : null,
      };
    });
    renderSettings(backend);

    await screen.findByRole("heading", { name: "Offsite recovery" });
    await user.click(screen.getByRole("button", { name: "Find recovery points" }));
    await user.click(await screen.findByRole("button", { name: "Preview restore" }));
    await user.click(await screen.findByRole("button", { name: "Take over backup set" }));
    const dialog = screen.getByRole("dialog", { name: "Take over this backup set?" });
    const transfer = within(dialog).getByRole("button", { name: "Transfer ownership" });
    expect(transfer).toBeDisabled();
    fireEvent.change(within(dialog).getByLabelText("Takeover confirmation"), {
      target: { value: "TAKE OVER" },
    });
    await waitFor(() =>
      expect(
        within(screen.getByRole("dialog", { name: "Take over this backup set?" })).getByRole(
          "button",
          { name: "Transfer ownership" },
        ),
      ).toBeEnabled(),
    );
    await user.click(
      within(screen.getByRole("dialog", { name: "Take over this backup set?" })).getByRole(
        "button",
        { name: "Transfer ownership" },
      ),
    );

    await waitFor(() =>
      expect(takeover).toHaveBeenCalledWith({
        expectedRevision: 3,
        expectedOwnerBackupSetId: foreignPreview.owner.backupSetId,
        expectedOwnerReplicaEpochId: foreignPreview.owner.replicaEpochId,
        expectedOwnerWriterId: foreignPreview.owner.writerId,
        expectedOwnerVersion: foreignPreview.owner.version,
        confirmation: "TAKE OVER",
      }),
    );
    expect(
      await screen.findByText("This installation now owns a fresh replica epoch.", {
        exact: false,
      }),
    ).toBeVisible();
  });

  it("announces connection failures without losing entered credentials", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    vi.spyOn(backend, "testBackupConnection").mockRejectedValueOnce(
      new Error("R2 is temporarily unavailable."),
    );
    renderSettings(backend);
    await screen.findByRole("heading", { name: "Offsite recovery" });
    await fillTarget(user);
    await user.click(screen.getByRole("button", { name: "Test connection" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("R2 is temporarily unavailable.");
    expect(screen.getByLabelText("R2 access key ID")).toHaveValue(ACCESS_KEY_ID);
    expect(screen.getByLabelText("R2 secret access key")).toHaveValue(SECRET_ACCESS_KEY);
  });
});

async function configure(backend: FakeBackend) {
  return backend.configureBackup({
    expectedRevision: 0,
    backupSetId: null,
    accountId: ACCOUNT_ID,
    jurisdiction: "DEFAULT",
    bucket: "kosh-local",
    accessKeyId: ACCESS_KEY_ID,
    secretAccessKey: SECRET_ACCESS_KEY,
  });
}

async function fillTarget(user: ReturnType<typeof userEvent.setup>) {
  await user.type(screen.getByLabelText("Cloudflare account ID"), ACCOUNT_ID);
  await user.type(screen.getByLabelText("R2 bucket"), "kosh-local");
  await user.type(screen.getByLabelText("R2 access key ID"), ACCESS_KEY_ID);
  await user.type(screen.getByLabelText("R2 secret access key"), SECRET_ACCESS_KEY);
}

function renderSettings(backend: FakeBackend) {
  const router = createAppRouter(
    createMemoryHistory({
      initialEntries: ["/settings"],
    }),
  );
  return render(
    <BackendProvider backend={backend}>
      <AppearanceProvider>
        <RouterProvider router={router} />
      </AppearanceProvider>
    </BackendProvider>,
  );
}

import { RouterProvider, createMemoryHistory } from "@tanstack/react-router";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../../src/backend/context";
import {
  DEFAULT_MAIN_WINDOW_ACCELERATOR,
  DEFAULT_QUICK_ADD_ACCELERATOR,
  KoshCommand,
} from "../../src/backend/contracts";
import { FakeBackend } from "../../src/backend/fakeBackend";
import { AppearanceProvider } from "../../src/components/Appearance";
import { createAppRouter } from "../../src/router";

describe("shortcut settings", () => {
  beforeEach(() => window.localStorage.clear());

  it("persists the automatic update preference", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const setAutomaticUpdateChecks = vi.spyOn(backend, "setAutomaticUpdateChecks");
    const router = createAppRouter(
      createMemoryHistory({
        initialEntries: ["/settings"],
      }),
    );
    render(
      <BackendProvider backend={backend}>
        <AppearanceProvider>
          <RouterProvider router={router} />
        </AppearanceProvider>
      </BackendProvider>,
    );

    const toggle = await screen.findByRole("switch", {
      name: "Automatically check for updates",
    });
    expect(toggle).toBeChecked();
    await user.click(toggle);

    await waitFor(() => expect(setAutomaticUpdateChecks).toHaveBeenCalledOnce());
    expect((await backend.loadShortcutSettings()).automaticUpdateChecksEnabled).toBe(false);
  });

  it("records, persists, rejects conflicts, and resets global shortcuts", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const setShortcutSettings = vi.spyOn(backend, "setShortcutSettings");
    const router = createAppRouter(
      createMemoryHistory({
        initialEntries: ["/settings"],
      }),
    );
    render(
      <BackendProvider backend={backend}>
        <AppearanceProvider>
          <RouterProvider router={router} />
        </AppearanceProvider>
      </BackendProvider>,
    );

    const quickAdd = await screen.findByRole("button", {
      name: "Quick Add shortcut: ⌃⌥⌘K",
    });
    await user.click(quickAdd);
    fireEvent.keyDown(window, {
      altKey: true,
      code: "KeyT",
      ctrlKey: true,
      key: "t",
      metaKey: true,
    });
    await waitFor(() => expect(setShortcutSettings).toHaveBeenCalledOnce());
    expect(
      (await backend.loadShortcutSettings()).keyboardBindings.find(
        (binding) => binding.command === KoshCommand.QuickAdd,
      )?.accelerator,
    ).toBe("control+alt+super+KeyT");

    await user.click(
      screen.getByRole("button", {
        name: "Main window shortcut: ⌃⌥⌘O",
      }),
    );
    fireEvent.keyDown(window, {
      altKey: true,
      code: "KeyT",
      ctrlKey: true,
      key: "t",
      metaKey: true,
    });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "two Kosh commands cannot use the same shortcut",
    );
    expect(
      (await backend.loadShortcutSettings()).keyboardBindings.find(
        (binding) => binding.command === KoshCommand.MainWindow,
      )?.accelerator,
    ).toBe(DEFAULT_MAIN_WINDOW_ACCELERATOR);

    await user.click(screen.getByRole("button", { name: "Reset shortcuts" }));
    await waitFor(async () => {
      const settings = await backend.loadShortcutSettings();
      expect(
        settings.keyboardBindings.find((binding) => binding.command === KoshCommand.QuickAdd)
          ?.accelerator,
      ).toBe(DEFAULT_QUICK_ADD_ACCELERATOR);
    });
  });

  it("configures local note-link shortcuts and rejects reserved commands", async () => {
    const user = userEvent.setup();
    const backend = new FakeBackend();
    const router = createAppRouter(
      createMemoryHistory({
        initialEntries: ["/settings"],
      }),
    );
    render(
      <BackendProvider backend={backend}>
        <AppearanceProvider>
          <RouterProvider router={router} />
        </AppearanceProvider>
      </BackendProvider>,
    );

    const copyNoteLink = await screen.findByRole("button", {
      name: "Copy note link shortcut: ⌘L",
    });
    await user.click(copyNoteLink);
    fireEvent.keyDown(window, {
      code: "KeyC",
      key: "c",
      metaKey: true,
      shiftKey: true,
    });
    expect(screen.getByRole("button", { name: "Copy note link shortcut: ⇧⌘C" })).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Copy exact link shortcut: ⇧⌘L" }));
    fireEvent.keyDown(window, {
      code: "KeyF",
      key: "f",
      metaKey: true,
    });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "That shortcut is reserved by another Kosh command.",
    );
    expect(screen.getByRole("button", { name: "Copy exact link shortcut: ⇧⌘L" })).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Main window shortcut: ⌃⌥⌘O" }));
    fireEvent.keyDown(window, {
      code: "KeyL",
      key: "l",
      metaKey: true,
    });
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Main window shortcut: ⌘L" })).toBeVisible(),
    );

    await user.click(screen.getByRole("button", { name: "Reset shortcuts" }));
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Copy note link shortcut: ⌘L" })).toBeVisible();
      expect(screen.getByRole("button", { name: "Main window shortcut: ⌃⌥⌘O" })).toBeVisible();
    });
  });
});

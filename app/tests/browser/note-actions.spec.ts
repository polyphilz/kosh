import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "./fixtures";

test("the minimal sidebar persists and its commands leave editor shortcuts alone", async ({
  page,
}) => {
  await page.goto("/#/");
  const navigation = page.getByRole("navigation", { name: "Primary" });

  await expect(navigation.locator(".app-nav-link")).toHaveCount(3);
  await expect(navigation.getByRole("button", { name: "New note" })).toBeVisible();
  await expect(navigation.getByRole("button", { name: "Search" })).toBeVisible();
  await expect(navigation.getByRole("link", { name: "Settings" })).toBeVisible();
  await expect(navigation.getByText(/Add|Library|Research/u)).toHaveCount(0);

  const editor = page.getByRole("textbox", { name: "Note" });
  await editor.focus();
  await page.keyboard.press("Meta+b");
  await expect(page.getByRole("button", { name: "Hide sidebar" })).toHaveAttribute(
    "aria-expanded",
    "true",
  );
  await page.evaluate(() => {
    window.dispatchEvent(
      new KeyboardEvent("keydown", {
        bubbles: true,
        code: "Slash",
        isComposing: true,
        key: "/",
        metaKey: true,
      }),
    );
  });
  await expect(page.getByRole("button", { name: "Hide sidebar" })).toBeVisible();

  await page.keyboard.press("Meta+/");
  await expect(page.getByRole("button", { name: "Show sidebar" })).toBeVisible();
  await expect(navigation).toBeHidden();
  await page.reload();
  await expect(page.getByRole("button", { name: "Show sidebar" })).toBeVisible();

  await page.getByRole("button", { name: "Show sidebar" }).click();
  await expect(navigation).toBeVisible();
  await page.keyboard.press("Meta+k");
  await expect(page.getByRole("dialog", { name: "Search notes" })).toBeVisible();
  await page.keyboard.press("Escape");

  await editor.fill("Find this local phrase. Another LOCAL phrase.");
  await page.keyboard.press("Meta+f");
  const findBar = page.getByRole("search", { name: "Find in note" });
  const find = findBar.getByRole("searchbox", { name: "Find in note" });
  await expect(find).toBeFocused();
  await find.fill("local");
  await expect(findBar.getByRole("status")).toContainText("1 of 2");
  await expect(page.locator('[data-kosh-find-match="true"]')).toHaveCount(2);
  await find.press("Enter");
  await expect(findBar.getByRole("status")).toContainText("2 of 2");
  await find.press("Shift+Enter");
  await expect(findBar.getByRole("status")).toContainText("1 of 2");
  await findBar.getByRole("button", { name: "Next match" }).click();
  await expect(findBar.getByRole("status")).toContainText("2 of 2");
  await page.keyboard.press("Escape");
  await expect(find).toHaveCount(0);
  await expect(page.locator('[data-kosh-find-match="true"]')).toHaveCount(0);
  await expect(editor).toBeFocused();

  const priorUrl = page.url();
  await page.keyboard.press("Meta+n");
  await expect(page).toHaveURL(/\/#\/new\/[0-9a-f-]{36}$/u);
  expect(page.url()).not.toBe(priorUrl);
});

test("Command-F waits for a loading note session", async ({ page }) => {
  await page.goto("/#/");
  await page.getByRole("textbox", { name: "Note" }).fill("A cold-loaded note to search.");
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });
  const noteId = page.url().split("/").at(-1)!;
  await page.keyboard.press("Meta+n");
  await expect(page).toHaveURL(/\/#\/new\/[0-9a-f-]{36}$/u);

  await page.evaluate((id) => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    const load = backend.loadTidbit.bind(backend);
    let release!: () => void;
    const loading = new Promise<void>((resolve) => {
      release = resolve;
    });
    Reflect.set(window, "__KOSH_RELEASE_NOTE_LOAD__", release);
    backend.loadTidbit = async (input) => {
      await loading;
      return load(input);
    };
    window.location.hash = `#/notes/${id}`;
  }, noteId);

  await expect(page).toHaveURL(new RegExp(`/#/notes/${noteId}$`, "u"));
  await expect(page.getByText("Opening note", { exact: true })).toBeAttached();
  await page.keyboard.press("Meta+f");
  await page.evaluate(() => Reflect.get(window, "__KOSH_RELEASE_NOTE_LOAD__")());

  await expect(page.getByRole("searchbox", { name: "Find in note" })).toBeFocused();
});

test("find survives the first-save route cutover", async ({ page }) => {
  await page.goto("/#/");
  await page.evaluate(() => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    const checkpoint = backend.checkpointWorkingCopy.bind(backend);
    let release!: () => void;
    const blocked = new Promise<void>((resolve) => {
      release = resolve;
    });
    Reflect.set(window, "__KOSH_RELEASE_FIND_CUTOVER__", release);
    Reflect.set(window, "__KOSH_FIND_CUTOVER_STARTED__", false);
    backend.checkpointWorkingCopy = async (input) => {
      Reflect.set(window, "__KOSH_FIND_CUTOVER_STARTED__", true);
      await blocked;
      return checkpoint(input);
    };
  });

  await page.getByRole("textbox", { name: "Note" }).fill("Keep this cutover needle visible.");
  await page.keyboard.press("Meta+f");
  const find = page.getByRole("searchbox", { name: "Find in note" });
  await find.fill("needle");
  await expect(page.getByRole("search", { name: "Find in note" })).toContainText("1 of 1");
  await expect
    .poll(() => page.evaluate(() => Reflect.get(window, "__KOSH_FIND_CUTOVER_STARTED__")))
    .toBe(true);

  await page.evaluate(() => Reflect.get(window, "__KOSH_RELEASE_FIND_CUTOVER__")());
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u);
  await expect(find).toHaveValue("needle");
  await expect(page.getByRole("search", { name: "Find in note" })).toContainText("1 of 1");
  await expect(page.locator('[data-kosh-find-match="true"]')).toHaveCount(1);
});

test("closing find cancels a pending first-save transfer", async ({ page }) => {
  await page.goto("/#/");
  await page.evaluate(() => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    const checkpoint = backend.checkpointWorkingCopy.bind(backend);
    let release!: () => void;
    const blocked = new Promise<void>((resolve) => {
      release = resolve;
    });
    Reflect.set(window, "__KOSH_RELEASE_CLOSED_FIND_CUTOVER__", release);
    Reflect.set(window, "__KOSH_CLOSED_FIND_CUTOVER_STARTED__", false);
    backend.checkpointWorkingCopy = async (input) => {
      Reflect.set(window, "__KOSH_CLOSED_FIND_CUTOVER_STARTED__", true);
      await blocked;
      return checkpoint(input);
    };
  });

  await page.getByRole("textbox", { name: "Note" }).fill("Close this pending find.");
  await page.keyboard.press("Meta+f");
  await page.getByRole("searchbox", { name: "Find in note" }).fill("pending");
  await expect
    .poll(() => page.evaluate(() => Reflect.get(window, "__KOSH_CLOSED_FIND_CUTOVER_STARTED__")))
    .toBe(true);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("search", { name: "Find in note" })).toHaveCount(0);

  await page.evaluate(() => Reflect.get(window, "__KOSH_RELEASE_CLOSED_FIND_CUTOVER__")());
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u);
  await expect(page.getByRole("search", { name: "Find in note" })).toHaveCount(0);
});

test("a deferred Command-F request is cancelled when its route closes", async ({ page }) => {
  await page.goto("/#/");
  await page.getByRole("textbox", { name: "Note" }).fill("Do not reopen stale find.");
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });
  const noteUrl = page.url();

  await page.evaluate(() => {
    const request = window.requestAnimationFrame.bind(window);
    const cancel = window.cancelAnimationFrame.bind(window);
    const heldId = 2_147_483_000;
    let held: FrameRequestCallback | null = null;
    window.requestAnimationFrame = (callback) => {
      if (held === null) {
        held = callback;
        return heldId;
      }
      return request(callback);
    };
    window.cancelAnimationFrame = (id) => {
      if (id === heldId) {
        held = null;
        return;
      }
      cancel(id);
    };
    Reflect.set(window, "__KOSH_RELEASE_DEFERRED_FIND__", () => {
      const callback = held;
      held = null;
      window.requestAnimationFrame = request;
      window.cancelAnimationFrame = cancel;
      if (callback) request(callback);
    });
  });

  await page.keyboard.press("Meta+f");
  await page.keyboard.press("Meta+n");
  await expect(page).toHaveURL(/\/#\/new\/[0-9a-f-]{36}$/u);
  await page.evaluate(() => Reflect.get(window, "__KOSH_RELEASE_DEFERRED_FIND__")());
  await page.goto(noteUrl);

  await expect(page.getByRole("textbox", { name: "Note" })).toContainText(
    "Do not reopen stale find.",
  );
  await expect(page.getByRole("search", { name: "Find in note" })).toHaveCount(0);
});

test("a queued Command-F request does not leak after leaving a loading note", async ({ page }) => {
  await page.goto("/#/");
  await page.getByRole("textbox", { name: "Note" }).fill("A delayed note to abandon.");
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });
  const delayedNoteId = page.url().split("/").at(-1)!;
  await page.keyboard.press("Meta+n");
  const initialNewNoteUrl = page.url();

  await page.evaluate((id) => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    const load = backend.loadTidbit.bind(backend);
    let release!: () => void;
    const loading = new Promise<void>((resolve) => {
      release = resolve;
    });
    Reflect.set(window, "__KOSH_RELEASE_ABANDONED_NOTE_LOAD__", release);
    backend.loadTidbit = async (input) => {
      await loading;
      return load(input);
    };
    window.location.hash = `#/notes/${id}`;
  }, delayedNoteId);

  await expect(page.getByText("Opening note", { exact: true })).toBeAttached();
  await page.keyboard.press("Meta+f");
  await page.keyboard.press("Meta+n");
  await expect(page).toHaveURL(/\/#\/new\/[0-9a-f-]{36}$/u);
  expect(page.url()).not.toBe(initialNewNoteUrl);
  await page.evaluate(() => Reflect.get(window, "__KOSH_RELEASE_ABANDONED_NOTE_LOAD__")());

  await expect(page.getByRole("textbox", { name: "Note" })).toBeVisible();
  await expect(page.getByRole("search", { name: "Find in note" })).toHaveCount(0);
});

test("note link shortcuts copy clean and exact routes", async ({ page }) => {
  await page.goto("/#/");
  const editor = page.getByRole("textbox", { name: "Note" });
  const noteId = page.url().match(/\/new\/([0-9a-f-]{36})$/u)?.[1];
  if (!noteId) throw new Error("the new-note route did not contain an id");
  await editor.fill("A linkable note with an exact search destination.");

  await page.keyboard.press("Meta+l");
  await expect(page.getByRole("status")).toHaveText("Note link copied");
  expect(await page.evaluate(() => window.__KOSH_FAKE_BACKEND__?.copiedTextForTest())).toBe(
    `kosh://note/${noteId}`,
  );
  await expect(page).toHaveURL(new RegExp(`/#/notes/${noteId}$`, "u"), { timeout: 5_000 });
  await page.evaluate(() => {
    window.history.replaceState(
      null,
      "",
      `${window.location.pathname}${window.location.search}${window.location.hash}?passage=passage-1&query=linkable`,
    );
  });
  const exactUrl = page.url();

  await page.keyboard.press("Meta+Shift+l");
  await expect(page.getByRole("status")).toHaveText("Exact note link copied");
  expect(await page.evaluate(() => window.__KOSH_FAKE_BACKEND__?.copiedTextForTest())).toBe(
    `kosh://note/${noteId}?passage=passage-1&query=linkable`,
  );
  await expect(page).toHaveURL(exactUrl);
});

test("valid sources autosave while invalid partial edits remain local", async ({ page }) => {
  await page.goto("/#/");
  const editor = page.getByRole("textbox", { name: "Note" });
  await expect(page.getByRole("button", { name: "Sources" })).toBeDisabled();
  await editor.fill("# Matrix notes\n\nA source-backed observation.");
  await expect(page.getByRole("button", { name: "Sources" })).toBeEnabled();
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });
  const noteId = page.url().split("/").at(-1)!;

  const sources = page.getByRole("button", { name: "Sources" });
  await sources.click();
  const sourceDialog = page.getByRole("dialog", { name: "Note sources" });
  await page.keyboard.press("Meta+f");
  await expect(sourceDialog).toHaveCount(0);
  await expect(page.getByRole("searchbox", { name: "Find in note" })).toBeFocused();
  await page.keyboard.press("Escape");
  await sources.click();
  await expect(sourceDialog).toBeVisible();
  await sourceDialog.getByLabel("Label").fill("NumPy guide");
  await sourceDialog
    .getByLabel("URL")
    .fill("https://numpy.org/doc/stable/user/absolute_beginners.html");
  await expect(page.getByRole("button", { name: "Sources 1" })).toBeVisible();
  await sourceDialog.getByLabel("URL").fill("not a complete URL");
  await expect(sourceDialog.getByRole("alert")).toHaveText("Enter a complete HTTP or HTTPS URL.");
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);

  await page.keyboard.press("Meta+k");
  await expect(page.getByRole("dialog", { name: "Search notes" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog", { name: "Search notes" })).toBeHidden();
  await expect(sourceDialog).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(sourceDialog).toBeHidden();
  await expect(sources).toBeFocused();
  await sources.click();
  await expect(sourceDialog.getByLabel("Label")).toHaveValue("NumPy guide");
  await expect(sourceDialog.getByLabel("URL")).toHaveValue(
    "https://numpy.org/doc/stable/user/absolute_beginners.html",
  );
  await page.keyboard.press("Escape");

  await page.getByRole("link", { name: "Settings" }).click();
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
  expect(
    await page.evaluate(async (id) => {
      const backend = window.__KOSH_FAKE_BACKEND__;
      if (!backend) throw new Error("fake backend is unavailable");
      return (await backend.loadTidbit(id)).sources;
    }, noteId),
  ).toEqual([
    {
      id: expect.any(String),
      label: "NumPy guide",
      url: "https://numpy.org/doc/stable/user/absolute_beginners.html",
    },
  ]);
});

test("delete flushes the latest note and Undo restores it immediately", async ({ page }) => {
  await page.goto("/#/");
  const editor = page.getByRole("textbox", { name: "Note" });
  await editor.fill("A durable opening line.");
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });
  const noteUrl = page.url();

  await editor.fill("The latest line must survive an immediate delete and restore.");
  await page.getByRole("button", { name: "Delete note" }).click();
  const confirmation = page.getByRole("dialog", { name: "Delete this note?" });
  await confirmation.getByRole("button", { name: "Delete note" }).click();

  await expect(page).toHaveURL(/\/#\/new\/[0-9a-f-]{36}$/u);
  await expect(page.getByRole("status")).toContainText("Deleted");
  expect(
    await page.evaluate(async () => {
      const backend = window.__KOSH_FAKE_BACKEND__;
      if (!backend) throw new Error("fake backend is unavailable");
      return (await backend.listNotesForTest({ cursor: null, limit: 10, scope: "DELETED" })).items;
    }),
  ).toEqual([
    expect.objectContaining({
      bodyPreview: expect.stringContaining("latest line must survive"),
      deletedAtMs: expect.any(Number),
    }),
  ]);

  await page.getByRole("button", { name: "Undo" }).click();
  await expect(page).toHaveURL(noteUrl);
  await expect(editor).toContainText(
    "The latest line must survive an immediate delete and restore.",
  );
  await expect(page.getByRole("button", { name: "Undo" })).toHaveCount(0);
});

test("Command-F stays contained by the delete confirmation", async ({ page }) => {
  await page.goto("/#/");
  const editor = page.getByRole("textbox", { name: "Note" });
  await editor.fill("A note that may be deleted.");
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });

  await page.getByRole("button", { name: "Delete note" }).click();
  const confirmation = page.getByRole("dialog", { name: "Delete this note?" });
  const cancel = confirmation.getByRole("button", { name: "Cancel" });
  await expect(cancel).toBeFocused();

  await page.keyboard.press("Meta+f");

  await expect(confirmation).toBeVisible();
  await expect(cancel).toBeFocused();
  await expect(page.getByRole("search", { name: "Find in note" })).toHaveCount(0);
});

test("history never reopens a deleted note as editable", async ({ page }) => {
  await page.goto("/#/");
  const editor = page.getByRole("textbox", { name: "Note" });
  await editor.fill("A deleted note must become a history tombstone.");
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });
  const deletedUrl = page.url();

  await page.getByRole("link", { name: "Settings" }).click();
  await page.evaluate((url) => {
    window.location.hash = new URL(url).hash;
  }, deletedUrl);
  await expect(page).toHaveURL(deletedUrl);
  await page.getByRole("button", { name: "Delete note" }).click();
  await page
    .getByRole("dialog", { name: "Delete this note?" })
    .getByRole("button", { name: "Delete note" })
    .click();
  await expect(page).toHaveURL(/\/#\/new\/[0-9a-f-]{36}$/u);

  await page.goBack();
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
  await page.goBack();
  await expect(page).toHaveURL(/\/#\/new\/[0-9a-f-]{36}$/u);
  expect(page.url()).not.toBe(deletedUrl);
  await page.getByRole("textbox", { name: "Note" }).fill("A fresh note remains writable.");
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });
});

test("a failed Undo stays visible and can be retried", async ({ page }) => {
  await page.goto("/#/");
  const editor = page.getByRole("textbox", { name: "Note" });
  await editor.fill("Retryable restoration evidence.");
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });
  const noteUrl = page.url();
  await page.evaluate(() => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    const restore = backend.restoreTidbit.bind(backend);
    let failed = false;
    backend.restoreTidbit = async (input) => {
      if (!failed) {
        failed = true;
        throw new Error("simulated restore outage");
      }
      return restore(input);
    };
  });

  await page.getByRole("button", { name: "Delete note" }).click();
  await page
    .getByRole("dialog", { name: "Delete this note?" })
    .getByRole("button", { name: "Delete note" })
    .click();
  await page.getByRole("button", { name: "Undo" }).click();
  await expect(page.getByRole("alert")).toContainText("simulated restore outage");
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();

  await page.getByRole("button", { name: "Undo" }).click();
  await expect(page).toHaveURL(noteUrl);
  await expect(editor).toContainText("Retryable restoration evidence.");
});

test("a failed delete keeps the note open with an actionable error", async ({ page }) => {
  await page.goto("/#/");
  const editor = page.getByRole("textbox", { name: "Note" });
  await editor.fill("Keep this note when deletion fails.");
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });
  const noteUrl = page.url();
  await page.evaluate(() => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    backend.deleteTidbit = async () => {
      throw new Error("simulated delete outage");
    };
  });

  await page.getByRole("button", { name: "Delete note" }).click();
  const confirmation = page.getByRole("dialog", { name: "Delete this note?" });
  await confirmation.getByRole("button", { name: "Delete note" }).click();

  await expect(confirmation).toContainText("Could not delete note: simulated delete outage");
  await expect(confirmation.getByRole("button", { name: "Delete note" })).toBeEnabled();
  await expect(page).toHaveURL(noteUrl);
  await expect(editor).toContainText("Keep this note when deletion fails.");
});

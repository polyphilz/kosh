import { expect, test } from "./fixtures";

test("cold launch opens an untouched ephemeral note and checkpoints the first edit", async ({
  page,
}) => {
  await page.goto("/#/");

  const editor = page.getByRole("textbox", { name: "Note" });
  await expect(editor).toBeFocused();
  await expect(page).toHaveURL(/\/#\/new\/[0-9a-f-]{36}$/u);
  await expect(page.getByRole("button", { name: /save/iu })).toHaveCount(0);
  await expect(page.getByRole("textbox", { name: /title/iu })).toHaveCount(0);
  expect(
    await page.evaluate(async () => {
      const backend = window.__KOSH_FAKE_BACKEND__;
      if (!backend) throw new Error("fake backend is unavailable");
      return {
        notes: (await backend.listTidbits({ cursor: null, limit: 10, scope: "ACTIVE" })).items
          .length,
        workingCopies: (await backend.listWorkingCopies()).length,
      };
    }),
  ).toEqual({ notes: 0, workingCopies: 0 });

  await editor.fill("A thought that should survive without a Save button.");
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });
  const persisted = await page.evaluate(async () => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    return (await backend.listTidbits({ cursor: null, limit: 10, scope: "ACTIVE" })).items;
  });
  expect(persisted).toHaveLength(1);
  expect(persisted[0]).toMatchObject({ title: null });

  await page.getByRole("button", { name: "Search", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Search notes" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("textbox", { name: "Note" })).toContainText(
    "A thought that should survive without a Save button.",
  );
});

test("new notes, settings, back, and forward use the transient route stack", async ({ page }) => {
  await page.goto("/#/");
  const editor = page.getByRole("textbox", { name: "Note" });
  await editor.fill("Note A anchors the navigation stack.");
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u);
  const noteAUrl = page.url();

  await page.keyboard.press("Meta+n");
  await expect(page).toHaveURL(/\/#\/new\/[0-9a-f-]{36}$/u);
  await expect(editor).toBeEmpty();
  await editor.fill("Note B replaces only its ephemeral route.");
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u);
  const noteBUrl = page.url();
  expect(noteBUrl).not.toBe(noteAUrl);

  await page.goBack();
  await expect(page).toHaveURL(noteAUrl);
  await expect(editor).toContainText("Note A anchors the navigation stack.");
  await page.goForward();
  await expect(page).toHaveURL(noteBUrl);
  await expect(editor).toContainText("Note B replaces only its ephemeral route.");

  await page.getByRole("button", { name: "Search", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Search notes" })).toBeVisible();
  await expect(page).toHaveURL(noteBUrl);
  await page.keyboard.press("Escape");
  await page.goBack();
  await expect(page).toHaveURL(noteAUrl);
  await page.goForward();
  await expect(page).toHaveURL(noteBUrl);

  await page.getByRole("link", { name: "Settings", exact: true }).click();
  await expect(page).toHaveURL(/\/#\/settings$/u);
  await page.goBack();
  await expect(page).toHaveURL(noteBUrl);
  await expect(editor).toContainText("Note B replaces only its ephemeral route.");
});

test("a failed navigation checkpoint keeps the active note open and recoverable", async ({
  page,
}) => {
  await page.goto("/#/");
  const editor = page.getByRole("textbox", { name: "Note" });
  await editor.fill("Do not leave until this reaches durable history.");
  await page.evaluate(() => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    backend.checkpointWorkingCopy = async () => {
      throw new Error("simulated navigation checkpoint failure");
    };
  });
  const noteUrl = page.url();

  await page.getByRole("link", { name: "Settings", exact: true }).click();

  await expect(page).toHaveURL(noteUrl);
  await expect(page.getByRole("alert")).toContainText("simulated navigation checkpoint failure");
  await expect(editor).toContainText("Do not leave until this reaches durable history.");
});

test("route navigation fences an edit before the working-copy debounce", async ({ page }) => {
  await page.goto("/#/");
  const editor = page.getByRole("textbox", { name: "Note" });
  await editor.fill("Navigate immediately, but keep every byte.");

  await page.getByRole("link", { name: "Settings", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();

  expect(
    await page.evaluate(async () => {
      const backend = window.__KOSH_FAKE_BACKEND__;
      if (!backend) throw new Error("fake backend is unavailable");
      return (await backend.listTidbits({ cursor: null, limit: 10, scope: "ACTIVE" })).items;
    }),
  ).toEqual([
    expect.objectContaining({
      bodyPreview: expect.stringContaining("Navigate immediately, but keep every byte."),
      title: null,
    }),
  ]);
});

test("lifecycle preparation locks note input until cancellation", async ({ page }) => {
  await page.goto("/#/");
  const editor = page.getByRole("textbox", { name: "Note" });
  await page.evaluate(() => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    const saveWorkingCopy = backend.saveWorkingCopy.bind(backend);
    backend.saveWorkingCopy = async (input) => {
      await new Promise<void>((resolve) => {
        Reflect.set(window, "__KOSH_RELEASE_LIFECYCLE_SAVE__", resolve);
      });
      return saveWorkingCopy(input);
    };
  });
  await editor.fill("Fence this note before the save debounce.");

  await page.evaluate(async () => {
    const modulePath = "/src/lifecycle/quit.tsx";
    const lifecycle = (await import(/* @vite-ignore */ modulePath)) as {
      prepareLifecycleParticipants(reason: "QUIT"): Promise<void>;
    };
    Reflect.set(
      window,
      "__KOSH_LIFECYCLE_PREPARATION__",
      lifecycle.prepareLifecycleParticipants("QUIT"),
    );
  });
  await expect(editor).toHaveAttribute("aria-disabled", "true");
  await expect(editor).toHaveAttribute("contenteditable", "false");

  await page.evaluate(() => {
    const release = Reflect.get(window, "__KOSH_RELEASE_LIFECYCLE_SAVE__") as
      | (() => void)
      | undefined;
    if (!release) throw new Error("lifecycle save did not start");
    release();
  });
  await page.evaluate(async () => {
    const preparation = Reflect.get(window, "__KOSH_LIFECYCLE_PREPARATION__") as
      | Promise<void>
      | undefined;
    if (!preparation) throw new Error("lifecycle preparation is unavailable");
    await preparation;
    const modulePath = "/src/lifecycle/quit.tsx";
    const lifecycle = (await import(/* @vite-ignore */ modulePath)) as {
      cancelLifecycleParticipants(): void;
    };
    lifecycle.cancelLifecycleParticipants();
  });
  await expect(editor).toHaveAttribute("aria-disabled", "false");
  await expect(editor).toHaveAttribute("contenteditable", "true");
});

test("a direct search route opens the search overlay", async ({ page }) => {
  await page.goto("/#/search");

  await expect(page.getByRole("dialog", { name: "Search notes" })).toBeVisible();
  await expect(page.getByRole("combobox", { name: "Search notes" })).toBeFocused();
});

test("an interrupted new note finishes recovery before accepting input", async ({ page }) => {
  const noteId = "019f547b-6200-7000-8000-00000000e001";
  await page.goto("/#/search");
  await page.evaluate(async (recoveredNoteId) => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    await backend.saveWorkingCopy({
      noteId: recoveredNoteId,
      baseRevisionId: null,
      editGeneration: 7,
      bodyMarkdown: "Recovered before the editor becomes interactive.",
      sources: [],
    });
    const loadWorkingCopy = backend.loadWorkingCopy.bind(backend);
    backend.loadWorkingCopy = async (requestedNoteId) => {
      if (requestedNoteId === recoveredNoteId) {
        await new Promise((resolve) => window.setTimeout(resolve, 250));
      }
      return loadWorkingCopy(requestedNoteId);
    };
    window.location.hash = `/new/${recoveredNoteId}`;
  }, noteId);

  await expect(page.locator("main.note-page[aria-busy='true']")).toBeVisible();
  await expect(page.getByRole("textbox", { name: "Note" })).toHaveCount(0);
  await expect(page.getByRole("textbox", { name: "Note" })).toContainText(
    "Recovered before the editor becomes interactive.",
  );
});

test("one stale working copy cannot block later recovery", async ({ page }) => {
  const trailingNoteId = "019f547b-6200-7000-8000-00000000e101";
  await page.goto("/#/search");
  const staleNoteId = await page.evaluate(async (recoverableNoteId) => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    const staleNote = await backend.createTidbit({
      title: null,
      bodyMarkdown: "Original durable note.",
      sources: [],
    });
    await backend.saveWorkingCopy({
      noteId: recoverableNoteId,
      baseRevisionId: null,
      editGeneration: 1,
      bodyMarkdown: "This trailing copy must still reconcile.",
      sources: [],
    });
    await backend.saveWorkingCopy({
      noteId: staleNote.id,
      baseRevisionId: staleNote.currentRevisionId,
      editGeneration: 1,
      bodyMarkdown: "This copy will become stale.",
      sources: [],
    });
    await backend.editTidbit({
      id: staleNote.id,
      expectedRevisionId: staleNote.currentRevisionId,
      title: null,
      bodyMarkdown: "A newer route already changed this note.",
      sources: [],
    });
    window.location.hash = "/";
    return staleNote.id;
  }, trailingNoteId);

  await expect
    .poll(() =>
      page.evaluate(async (noteId) => {
        const backend = window.__KOSH_FAKE_BACKEND__;
        if (!backend) throw new Error("fake backend is unavailable");
        const page = await backend.listTidbits({ cursor: null, limit: 20, scope: "ACTIVE" });
        return page.items.some((item) => item.id === noteId);
      }, trailingNoteId),
    )
    .toBe(true);
  expect(
    await page.evaluate(async () => {
      const backend = window.__KOSH_FAKE_BACKEND__;
      if (!backend) throw new Error("fake backend is unavailable");
      return (await backend.listWorkingCopies()).map((copy) => copy.noteId);
    }),
  ).toEqual([staleNoteId]);
});

test("startup recovery discards an abandoned existing-note media reservation", async ({ page }) => {
  await page.goto("/#/search");
  const noteId = await page.evaluate(async () => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    const note = await backend.createTidbit({
      title: null,
      bodyMarkdown: "Do not create a phantom revision for this note.",
      sources: [],
    });
    await backend.reserveWorkingCopyForMedia({
      noteId: note.id,
      baseRevisionId: note.currentRevisionId,
      editGeneration: 1,
      bodyMarkdown: note.bodyMarkdown,
      sources: [],
    });
    window.location.hash = "/";
    return note.id;
  });

  await expect
    .poll(() =>
      page.evaluate(async (id) => {
        const backend = window.__KOSH_FAKE_BACKEND__;
        if (!backend) throw new Error("fake backend is unavailable");
        return backend.loadWorkingCopy(id);
      }, noteId),
    )
    .toBeNull();
  expect(
    await page.evaluate(async (id) => {
      const backend = window.__KOSH_FAKE_BACKEND__;
      if (!backend) throw new Error("fake backend is unavailable");
      return (await backend.loadTidbit(id)).revisionNumber;
    }, noteId),
  ).toBe(1);
});

test("delayed reconciliation never checkpoints the note opened during its scan", async ({
  page,
}) => {
  const firstNoteId = "019f547b-6200-7000-8000-00000000e201";
  const openedNoteId = "019f547b-6200-7000-8000-00000000e202";
  await page.goto("/#/search");
  await page.evaluate(
    async ({ first, opened }) => {
      const backend = window.__KOSH_FAKE_BACKEND__;
      if (!backend) throw new Error("fake backend is unavailable");
      await backend.saveWorkingCopy({
        noteId: first,
        baseRevisionId: null,
        editGeneration: 2,
        bodyMarkdown: "The initially open interrupted note.",
        sources: [],
      });
      await backend.saveWorkingCopy({
        noteId: opened,
        baseRevisionId: null,
        editGeneration: 4,
        bodyMarkdown: "Open this while recovery scans.",
        sources: [],
      });
      const listWorkingCopies = backend.listWorkingCopies.bind(backend);
      backend.listWorkingCopies = async () => {
        (globalThis as unknown as Record<string, unknown>).__KOSH_RECONCILIATION_LISTING__ = true;
        await new Promise((resolve) => window.setTimeout(resolve, 400));
        return listWorkingCopies();
      };
      window.location.hash = `/new/${first}`;
    },
    { first: firstNoteId, opened: openedNoteId },
  );

  await expect(page.getByRole("textbox", { name: "Note" })).toContainText("initially open");
  await expect
    .poll(
      () =>
        page.evaluate(
          () =>
            (globalThis as unknown as Record<string, unknown>).__KOSH_RECONCILIATION_LISTING__ ===
            true,
        ),
      { timeout: 10_000 },
    )
    .toBe(true);
  await page.evaluate((noteId) => {
    window.location.hash = `/new/${noteId}`;
  }, openedNoteId);

  const editor = page.getByRole("textbox", { name: "Note" });
  await expect(editor).toContainText("Open this while recovery scans.");
  await page.waitForTimeout(500);
  await editor.fill("This edit belongs to the still-open recovered note.");
  await page.getByRole("link", { name: "Settings", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();

  await expect
    .poll(() =>
      page.evaluate(async (noteId) => {
        const backend = window.__KOSH_FAKE_BACKEND__;
        if (!backend) throw new Error("fake backend is unavailable");
        return backend.loadTidbit(noteId);
      }, openedNoteId),
    )
    .toMatchObject({ bodyMarkdown: "This edit belongs to the still-open recovered note." });
});

test("a legacy title is projected without a revision until the first authored edit", async ({
  page,
}) => {
  await page.goto("/#/search");
  const note = await page.evaluate(async () => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    return backend.createTidbit({
      title: "Legacy *vector* title",
      bodyMarkdown: "Original body.",
      sources: [],
    });
  });

  await page.goto(`/#/notes/${note.id}`);
  const editor = page.getByRole("textbox", { name: "Note" });
  await expect(editor).toContainText("Legacy *vector* title");
  await page.waitForTimeout(2_200);
  expect(
    await page.evaluate(async (noteId) => {
      const backend = window.__KOSH_FAKE_BACKEND__;
      if (!backend) throw new Error("fake backend is unavailable");
      return backend.listTidbitRevisions({
        beforeRevisionNumber: null,
        limit: 10,
        tidbitId: noteId,
      });
    }, note.id),
  ).toMatchObject({ items: [{ revisionNumber: 1 }] });

  await editor.press("Control+End");
  await editor.press("Enter");
  await editor.pressSequentially("New connection.");
  await expect
    .poll(async () =>
      page.evaluate(async (noteId) => {
        const backend = window.__KOSH_FAKE_BACKEND__;
        if (!backend) throw new Error("fake backend is unavailable");
        return backend.loadTidbit(noteId);
      }, note.id),
    )
    .toMatchObject({ revisionNumber: 2, title: null });
});

test("an image alone makes an ephemeral note contentful", async ({ page }) => {
  await page.route("kosh-media://**", async (route) => {
    await route.fulfill({
      body: Buffer.from(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
        "base64",
      ),
      contentType: "image/png",
    });
  });
  await page.goto("/#/");
  await page.evaluate(() => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    backend.selectImage = async () => "note-image-selection";
    backend.ingestSelectedImage = async () => ({
      id: "019f547b-6200-7000-8000-00000000d001",
      ingestLeaseId: "019f547b-6200-7000-8000-00000000d002",
      displayFilename: "diagram.png",
      mediaType: "image/png",
      byteLength: 256,
      kind: "IMAGE",
      naturalWidth: 640,
      naturalHeight: 480,
      ocrStatus: "READY",
      ocrError: null,
    });
  });

  const editor = page.getByRole("textbox", { name: "Note" });
  await editor.pressSequentially("/");
  await page.getByRole("option", { name: "Image", exact: true }).click();
  await expect(page.locator("[data-kosh-image='true']")).toBeVisible();
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });
  expect(
    await page.evaluate(async () => {
      const backend = window.__KOSH_FAKE_BACKEND__;
      if (!backend) throw new Error("fake backend is unavailable");
      return (await backend.listTidbits({ cursor: null, limit: 10, scope: "ACTIVE" })).items[0];
    }),
  ).toMatchObject({ title: null });
});

test("the page editor stays flat and usable in compact, dark, zoomed, and long layouts", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 640 });
  await page.goto("/#/");
  await page.evaluate(() => {
    document.documentElement.dataset.appearance = "DARK";
    const style = document.createElement("style");
    style.textContent = ".kosh-blocknote-editor--page .bn-editor { font-size: 30px !important; }";
    document.head.append(style);
  });
  const editor = page.getByRole("textbox", { name: "Note" });
  await editor.fill(
    Array.from(
      { length: 36 },
      (_, index) => `Long-form line ${index + 1} keeps the writing surface readable.`,
    ).join("\n"),
  );

  await expect(editor).toBeVisible();
  expect(
    await page.evaluate(() => ({
      backgroundImage: getComputedStyle(document.querySelector(".note-page")!).backgroundImage,
      horizontalOverflow: document.documentElement.scrollWidth > window.innerWidth,
    })),
  ).toEqual({ backgroundImage: "none", horizontalOverflow: false });
});

import { expect, test } from "./fixtures";

test("cold launch opens an untouched ephemeral note and checkpoints the first edit", async ({
  page,
}) => {
  await page.goto("/#/");

  const editor = page.getByRole("textbox", { name: "Note" });
  await expect(editor).toBeFocused();
  await expect
    .poll(() =>
      editor
        .locator(".bn-block-content")
        .evaluate((block) => getComputedStyle(block, "::after").content),
    )
    .toBe("\"Write something or press '/' for commands\"");
  await expect(page).toHaveURL(/\/#\/new\/[0-9a-f-]{36}$/u);
  await expect(page.getByRole("button", { name: /save/iu })).toHaveCount(0);
  await expect(page.getByRole("textbox", { name: /title/iu })).toHaveCount(0);
  expect(
    await page.evaluate(async () => {
      const backend = window.__KOSH_FAKE_BACKEND__;
      if (!backend) throw new Error("fake backend is unavailable");
      return {
        notes: (await backend.listNotesForTest({ cursor: null, limit: 10, scope: "ACTIVE" })).items
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
    return (await backend.listNotesForTest({ cursor: null, limit: 10, scope: "ACTIVE" })).items;
  });
  expect(persisted).toHaveLength(1);
  expect(persisted[0]?.displayTitle).toBe("A thought that should survive without a Save button.");

  await page.getByRole("button", { name: "Search", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Search notes" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("textbox", { name: "Note" })).toContainText(
    "A thought that should survive without a Save button.",
  );
});

test("autosave and reopen preserve canonical BlockNote block IDs", async ({ page }) => {
  await page.goto("/#/");
  const editor = page.getByRole("textbox", { name: "Note" });

  await editor.fill("First stable block");
  await editor.press("Enter");
  await editor.type("Second stable block");
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });

  const blocks = editor.locator(":scope > .bn-block-group > .bn-block-outer");
  await expect(blocks).toHaveCount(2);
  const idsBeforeReopen = await blocks.evaluateAll((elements) =>
    elements.map((element) => element.getAttribute("data-id")),
  );
  expect(idsBeforeReopen).toEqual([expect.any(String), expect.any(String)]);
  expect(new Set(idsBeforeReopen).size).toBe(2);

  const noteId = new URL(page.url()).hash.split("/").at(-1);
  if (!noteId) throw new Error("the durable note route has no note id");
  await expect
    .poll(async () =>
      page.evaluate(async (id) => {
        const backend = window.__KOSH_FAKE_BACKEND__;
        if (!backend) throw new Error("fake backend is unavailable");
        return (await backend.loadTidbit(id)).documentJson;
      }, noteId),
    )
    .toContain(idsBeforeReopen[0]);
  const savedDocument = await page.evaluate(async (id) => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    return (await backend.loadTidbit(id)).documentJson;
  }, noteId);
  expect(savedDocument).toContain(idsBeforeReopen[1]);

  await page.evaluate(() => {
    window.location.hash = "/settings";
  });
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
  await page.evaluate((id) => {
    window.location.hash = `/notes/${id}`;
  }, noteId);
  await expect(editor).toContainText("Second stable block");

  const idsAfterReopen = await blocks.evaluateAll((elements) =>
    elements.map((element) => element.getAttribute("data-id")),
  );
  expect(idsAfterReopen).toEqual(idsBeforeReopen);
});

test("stable block links flash once, remain shareable, and silently discard stale ids", async ({
  page,
}) => {
  await page.goto("/#/");
  const note = await page.evaluate(async () => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    return backend.seedNote({
      bodyMarkdown: "Opening context\n\nExact stable block\n\nClosing context",
      documentJson: JSON.stringify({
        schemaVersion: 1,
        blocks: [
          {
            id: "opening-block",
            type: "paragraph",
            props: {},
            content: [{ type: "text", text: "Opening context", styles: {} }],
            children: [],
          },
          {
            id: "exact-stable-block",
            type: "paragraph",
            props: {},
            content: [{ type: "text", text: "Exact stable block", styles: {} }],
            children: [],
          },
          {
            id: "closing-block",
            type: "paragraph",
            props: {},
            content: [{ type: "text", text: "Closing context", styles: {} }],
            children: [],
          },
        ],
      }),
      sources: [],
    });
  });

  await page.evaluate((noteId) => {
    window.location.hash = `/notes/${noteId}?blockId=exact-stable-block`;
  }, note.id);
  const hit = page.locator('[data-kosh-search-hit="true"]');
  await expect(hit).toContainText("Exact stable block");
  await expect(hit).toHaveCSS("animation-duration", "1.4s");
  await expect(page).toHaveURL(
    new RegExp(`/#/notes/${note.id}\\?blockId=exact-stable-block$`, "u"),
  );
  await expect(hit).toHaveCount(0, { timeout: 3_000 });
  await expect(page).toHaveURL(
    new RegExp(`/#/notes/${note.id}\\?blockId=exact-stable-block$`, "u"),
  );

  await page.evaluate((noteId) => {
    window.location.hash = `/notes/${noteId}?blockId=deleted-block`;
  }, note.id);
  await expect(page).toHaveURL(new RegExp(`/#/notes/${note.id}$`, "u"));
  await expect(page.locator('[data-kosh-search-hit="true"]')).toHaveCount(0);
});

test("deleting the first edit before checkpoint keeps the note ephemeral and empty", async ({
  page,
}) => {
  await page.goto("/#/");
  const editor = page.getByRole("textbox", { name: "Note" });

  await editor.pressSequentially("f");
  await page.waitForTimeout(500);
  await editor.press("Backspace");

  await expect(editor).toBeEmpty();
  await page.waitForTimeout(2_500);
  await expect(editor).toBeEmpty();
  await expect(page).toHaveURL(/\/#\/new\/[0-9a-f-]{36}$/u);
  expect(
    await page.evaluate(async () => {
      const backend = window.__KOSH_FAKE_BACKEND__;
      if (!backend) throw new Error("fake backend is unavailable");
      return {
        notes: (await backend.listNotesForTest({ cursor: null, limit: 10, scope: "ACTIVE" })).items
          .length,
        workingCopies: (await backend.listWorkingCopies()).length,
      };
    }),
  ).toEqual({ notes: 0, workingCopies: 0 });
});

test("the blank canvas below the last block continues the note", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/#/search");
  const note = await page.evaluate(async () => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    return backend.seedNote({
      bodyMarkdown: "Before the equation.\n\n$$\n\\sum_i a_i\n$$",
      sources: [],
    });
  });
  await page.evaluate((noteId) => {
    window.location.hash = `/notes/${noteId}`;
  }, note.id);

  const editor = page.getByRole("textbox", { name: "Note" });
  await expect(editor).toBeVisible();
  const trailingCanvas = editor.locator(".bn-trailing-block");
  await expect(trailingCanvas).toBeVisible();
  expect(
    await trailingCanvas.evaluate((element) => element.getBoundingClientRect().height),
  ).toBeGreaterThan(300);

  const canvasBox = await trailingCanvas.boundingBox();
  if (!canvasBox) throw new Error("the trailing writing canvas is not rendered");
  await page.mouse.click(canvasBox.x + 80, canvasBox.y + canvasBox.height - 40);
  await page.keyboard.type("Continue from anywhere below.");

  const blocks = editor.locator(":scope > .bn-block-group > .bn-block-outer");
  await expect(blocks).toHaveCount(3);
  await expect(blocks.last()).toContainText("Continue from anywhere below.");
});

test("the page gutter selects contiguous blocks beside the add and move controls", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/#/search");
  const note = await page.evaluate(async () => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    return backend.seedNote({
      bodyMarkdown:
        "First gutter block.\n\nSecond gutter block.\n\n$$\n\\sum_i a_i\n$$\n\nLast gutter block.",
      sources: [],
    });
  });
  await page.evaluate((noteId) => {
    window.location.hash = `/notes/${noteId}`;
  }, note.id);

  const editor = page.getByRole("textbox", { name: "Note" });
  const blocks = editor.locator(
    ":scope > .bn-block-group > .bn-block-outer:not(.bn-trailing-block)",
  );
  await expect(blocks).toHaveCount(4);
  await blocks.first().hover();
  const addBelow = page.getByRole("button", { name: "Click to add below" });
  const drag = page.getByRole("button", { name: "Drag to move" });
  await expect(addBelow).toBeVisible();
  await expect(drag).toBeVisible();

  const rail = page.getByTestId("note-gutter-selection-rail");
  const railBox = await rail.boundingBox();
  const sidebarBox = await page.locator(".app-sidebar").boundingBox();
  const addBox = await addBelow.boundingBox();
  const firstBox = await blocks.nth(0).boundingBox();
  const secondBox = await blocks.nth(1).boundingBox();
  const thirdBox = await blocks.nth(2).boundingBox();
  const lastBox = await blocks.nth(3).boundingBox();
  if (!railBox || !sidebarBox || !addBox || !firstBox || !secondBox || !thirdBox || !lastBox) {
    throw new Error("the gutter layout is not rendered");
  }
  expect(railBox.width).toBeGreaterThanOrEqual(40);
  expect(railBox.x).toBeGreaterThanOrEqual(sidebarBox.x + sidebarBox.width - 1);
  expect(railBox.x + railBox.width).toBeLessThan(addBox.x);

  const railX = railBox.x + railBox.width / 2;
  expect(
    await blocks.evaluateAll((elements) =>
      elements.map(
        (element) =>
          element.getAttribute("data-id") ??
          element.querySelector("[data-id]")?.getAttribute("data-id"),
      ),
    ),
  ).toEqual([expect.any(String), expect.any(String), expect.any(String), expect.any(String)]);
  expect(
    await page.evaluate(
      ({ x, y }) => document.elementFromPoint(x, y)?.getAttribute("data-testid"),
      { x: railX, y: firstBox.y + firstBox.height / 2 },
    ),
  ).toBe("note-gutter-selection-rail");

  const belowBlocksY = lastBox.y + lastBox.height + 80;
  expect(belowBlocksY).toBeLessThan(railBox.y + railBox.height);
  await page.mouse.click(railX, belowBlocksY);
  await expect(editor.locator('[data-kosh-gutter-selected="true"]')).toHaveCount(0);

  await page.mouse.move(railX, firstBox.y + firstBox.height / 2);
  await page.mouse.down();
  const selectionX = thirdBox.x + Math.min(250, thirdBox.width - 2);
  const thirdY = thirdBox.y + thirdBox.height / 2;
  await page.mouse.move(selectionX, thirdY, { steps: 12 });

  const marquee = page.getByTestId("note-gutter-selection-marquee");
  await expect(marquee).toBeVisible();
  const marqueeBox = await marquee.boundingBox();
  if (!marqueeBox) throw new Error("the gutter marquee is not rendered");
  expect(marqueeBox.x).toBeLessThan(firstBox.x);
  expect(marqueeBox.x + marqueeBox.width).toBeGreaterThan(firstBox.x);

  const selected = editor.locator('[data-kosh-gutter-selected="true"]');
  await expect(selected).toHaveCount(3);
  await page.mouse.move(selectionX, secondBox.y + secondBox.height / 2, { steps: 8 });
  await expect(selected).toHaveCount(2);
  await expect(selected.nth(0)).toContainText("First gutter block.");
  await expect(selected.nth(1)).toContainText("Second gutter block.");
  await page.mouse.move(selectionX, thirdY, { steps: 8 });
  await expect(selected).toHaveCount(3);
  await page.mouse.move(railX, thirdY, { steps: 8 });
  await expect(selected).toHaveCount(0);
  await page.mouse.move(selectionX, thirdY, { steps: 8 });
  await expect(selected).toHaveCount(3);
  await page.mouse.up();

  await expect(marquee).toBeHidden();
  await expect(selected).toHaveCount(3);
  await expect(selected.nth(0)).toContainText("First gutter block.");
  await expect(selected.nth(1)).toContainText("Second gutter block.");
  await expect(selected.nth(2)).toContainText("∑");
  const selectionText = await page.evaluate(() => window.getSelection()?.toString() ?? "");
  expect(selectionText).toContain("First gutter block.");
  expect(selectionText).toContain("Second gutter block.");
  expect(selectionText).not.toContain("Last gutter block.");

  await page.keyboard.press("Backspace");
  await expect(blocks).toHaveCount(1);
  await expect(blocks.first()).toContainText("Last gutter block.");
  await expect(selected).toHaveCount(0);
});

test("a gutter marquee keeps its anchor while auto-scrolling a long note", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 500 });
  await page.goto("/#/search");
  const note = await page.evaluate(async () => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    return backend.seedNote({
      bodyMarkdown: Array.from({ length: 48 }, (_, index) => `Long block ${index + 1}`).join(
        "\n\n",
      ),
      sources: [],
    });
  });
  await page.evaluate((noteId) => {
    window.location.hash = `/notes/${noteId}`;
  }, note.id);

  const editor = page.getByRole("textbox", { name: "Note" });
  const blocks = editor.locator(
    ":scope > .bn-block-group > .bn-block-outer:not(.bn-trailing-block)",
  );
  await expect(blocks).toHaveCount(48);
  const railBox = await page.getByTestId("note-gutter-selection-rail").boundingBox();
  const firstBox = await blocks.first().boundingBox();
  if (!railBox || !firstBox) throw new Error("the long-note gutter is not rendered");

  const railX = railBox.x + railBox.width / 2;
  await page.mouse.move(railX, firstBox.y + firstBox.height / 2);
  await page.mouse.down();
  await page.mouse.move(firstBox.x + 260, 492, { steps: 12 });
  await page.waitForFunction(() => window.scrollY > 150);

  const selected = editor.locator('[data-kosh-gutter-selected="true"]');
  await expect.poll(() => selected.count()).toBeGreaterThan(8);
  await expect(selected.first()).toContainText("Long block 1");
  await page.mouse.up();
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
      return (await backend.listNotesForTest({ cursor: null, limit: 10, scope: "ACTIVE" })).items;
    }),
  ).toEqual([
    expect.objectContaining({
      bodyPreview: expect.stringContaining("Navigate immediately, but keep every byte."),
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
  await page.goto("/#/settings");
  await page.evaluate(async (recoveredNoteId) => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    await backend.saveWorkingCopy({
      noteId: recoveredNoteId,
      baseContentVersionId: null,
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
  await page.goto("/#/settings");
  const staleNoteId = await page.evaluate(async (recoverableNoteId) => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    const staleNote = await backend.seedNote({
      bodyMarkdown: "Original durable note.",
      sources: [],
    });
    await backend.saveWorkingCopy({
      noteId: recoverableNoteId,
      baseContentVersionId: null,
      editGeneration: 1,
      bodyMarkdown: "This trailing copy must still reconcile.",
      sources: [],
    });
    await backend.saveWorkingCopy({
      noteId: staleNote.id,
      baseContentVersionId: staleNote.contentVersionId,
      editGeneration: 1,
      bodyMarkdown: "This copy will become stale.",
      sources: [],
    });
    await backend.replaceNoteForTest({
      id: staleNote.id,
      expectedContentVersionId: staleNote.contentVersionId,
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
        const page = await backend.listNotesForTest({ cursor: null, limit: 20, scope: "ACTIVE" });
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
  await page.goto("/#/settings");
  const noteId = await page.evaluate(async () => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    const note = await backend.seedNote({
      bodyMarkdown: "Do not create a phantom note state for this note.",
      sources: [],
    });
    await backend.reserveWorkingCopyForMedia({
      noteId: note.id,
      baseContentVersionId: note.contentVersionId,
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
      return (await backend.loadTidbit(id)).versionNumber;
    }, noteId),
  ).toBe(1);
});

test("delayed reconciliation never checkpoints the note opened during its scan", async ({
  page,
}) => {
  const firstNoteId = "019f547b-6200-7000-8000-00000000e201";
  const openedNoteId = "019f547b-6200-7000-8000-00000000e202";
  await page.goto("/#/settings");
  await page.evaluate(
    async ({ first, opened }) => {
      const backend = window.__KOSH_FAKE_BACKEND__;
      if (!backend) throw new Error("fake backend is unavailable");
      await backend.saveWorkingCopy({
        noteId: first,
        baseContentVersionId: null,
        editGeneration: 2,
        bodyMarkdown: "The initially open interrupted note.",
        sources: [],
      });
      await backend.saveWorkingCopy({
        noteId: opened,
        baseContentVersionId: null,
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
  const imageNote = await page.evaluate(async () => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    return (await backend.listNotesForTest({ cursor: null, limit: 10, scope: "ACTIVE" })).items[0];
  });
  expect(imageNote).toBeDefined();
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

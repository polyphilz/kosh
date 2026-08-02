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

  await page.getByRole("link", { name: "Search", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Search" })).toBeVisible();
  await page.goBack();
  await expect(page.getByRole("textbox", { name: "Note" })).toContainText(
    "A thought that should survive without a Save button.",
  );
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

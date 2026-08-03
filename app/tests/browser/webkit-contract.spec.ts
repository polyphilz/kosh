import { expect, test } from "./fixtures";

test("editor and search keyboard contracts hold in WebKit", async ({ page }) => {
  await page.goto("/#/");
  const editor = page.getByRole("textbox", { name: "Note" });
  await editor.fill("WebKit contract: a portable editor passage with `code` and $x^2$.");
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });

  await page
    .getByRole("navigation", { name: "Primary" })
    .getByRole("button", {
      name: "Search",
      exact: true,
    })
    .click();
  const search = page.getByRole("combobox", { name: "Search notes" });
  await search.fill("portable");
  const result = page.getByRole("option", { name: /WebKit contract/u });
  await expect(result).toBeVisible();
  await search.press("ArrowDown");
  await expect(search).toBeFocused();
  await search.press("Enter");
  await expect(page.getByText("Search match", { exact: true })).toBeVisible();
  await expect(page.locator('[data-kosh-search-hit="true"]')).toContainText(
    "a portable editor passage",
  );
});

test("the titleless note route focuses and checkpoints in WebKit", async ({ page }) => {
  await page.goto("/#/");
  const editor = page.getByRole("textbox", { name: "Note" });
  await expect(editor).toBeFocused();
  await editor.fill("WebKit preserves this titleless note automatically.");

  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });
  await expect(editor).toContainText("WebKit preserves this titleless note automatically.");
});

test("deleting the first edit stays empty across WebKit autosave boundaries", async ({ page }) => {
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

test("the trailing note canvas appends after an atomic block in WebKit", async ({ page }) => {
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
  const canvasBox = await trailingCanvas.boundingBox();
  if (!canvasBox) throw new Error("the trailing writing canvas is not rendered");
  await page.mouse.click(canvasBox.x + 80, canvasBox.y + canvasBox.height - 40);
  await page.keyboard.type("Continue below the equation.");

  const blocks = editor.locator(":scope > .bn-block-group > .bn-block-outer");
  await expect(blocks).toHaveCount(3);
  await expect(blocks.last()).toContainText("Continue below the equation.");
});

test("the block gutter selects a range containing an atomic block in WebKit", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/#/search");
  const note = await page.evaluate(async () => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    return backend.seedNote({
      bodyMarkdown: "First block.\n\nSecond block.\n\n$$\n\\sum_i a_i\n$$\n\nLast block.",
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
  const railBox = await page.getByTestId("note-gutter-selection-rail").boundingBox();
  const firstBox = await blocks.nth(0).boundingBox();
  const thirdBox = await blocks.nth(2).boundingBox();
  if (!railBox || !firstBox || !thirdBox) throw new Error("the WebKit gutter is not rendered");

  const railX = railBox.x + railBox.width / 2;
  const selectionX = thirdBox.x + Math.min(250, thirdBox.width - 2);
  await page.mouse.move(railX, firstBox.y + firstBox.height / 2);
  await page.mouse.down();
  await page.mouse.move(selectionX, thirdBox.y + thirdBox.height / 2, { steps: 12 });
  await expect(page.getByTestId("note-gutter-selection-marquee")).toBeVisible();
  await page.mouse.up();

  await expect(page.getByTestId("note-gutter-selection-marquee")).toBeHidden();
  await expect(editor.locator('[data-kosh-gutter-selected="true"]')).toHaveCount(3);
  await page.keyboard.press("Backspace");
  await expect(blocks).toHaveCount(1);
  await expect(blocks.first()).toContainText("Last block.");
});

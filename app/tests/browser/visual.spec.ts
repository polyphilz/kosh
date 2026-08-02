import { expect, test } from "./fixtures";

for (const theme of ["LIGHT", "DARK"] as const) {
  test(`full-page note stays visually stable in ${theme.toLowerCase()} mode`, async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 900 });
    await page.goto("/#/search");
    const note = await page.evaluate(async () => {
      const backend = window.__KOSH_FAKE_BACKEND__;
      if (!backend) throw new Error("fake backend is unavailable");
      return backend.seedNote({
        bodyMarkdown:
          "# NumPy scrap notes\n\nArrays keep shape and dtype together.\n\n- contiguous memory\n  - predictable strides\n- vectorized operations\n\n```python\na = np.array([[1, 2], [3, 4]])\n```\n\n$$a_{ij} = i + j$$",
        sources: [],
      });
    });
    await page.evaluate(
      ({ appearance, noteId }) => {
        document.documentElement.dataset.appearance = appearance;
        window.location.hash = `/notes/${noteId}`;
      },
      { appearance: theme, noteId: note.id },
    );
    await expect(page.getByRole("textbox", { name: "Note" })).toBeVisible();
    await page.evaluate(() => document.fonts.ready);

    await expect(page).toHaveScreenshot(`note-${theme.toLowerCase()}.png`, {
      animations: "disabled",
      caret: "hide",
      fullPage: true,
      maxDiffPixelRatio: 0.04,
      threshold: 0.35,
    });
  });
}

for (const theme of ["LIGHT", "DARK"] as const) {
  test(`search overlay stays visually stable in ${theme.toLowerCase()} mode`, async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 900 });
    await page.goto("/#/");
    await page.evaluate(async (appearance) => {
      document.documentElement.dataset.appearance = appearance;
      const backend = window.__KOSH_FAKE_BACKEND__;
      if (!backend) throw new Error("fake backend is unavailable");
      await backend.seedNote({
        bodyMarkdown: "# NumPy memory layout\n\nContiguous arrays make predictable strides.",
        sources: [{ label: "Array notes", url: "https://example.com/numpy" }],
      });
      await document.fonts.ready;
    }, theme);
    await page.keyboard.press("Meta+k");
    await page.getByRole("combobox", { name: "Search notes" }).fill("contiguous arrays");
    const dialog = page.getByRole("dialog", { name: "Search notes" });
    await expect(dialog.getByRole("option")).toBeVisible();

    await expect(dialog).toHaveScreenshot(`search-overlay-${theme.toLowerCase()}.png`, {
      animations: "disabled",
      caret: "hide",
      maxDiffPixelRatio: 0.04,
      threshold: 0.35,
    });
  });
}

test("note source and delete actions stay visually stable", async ({ page }) => {
  await page.goto("/#/");
  await page.getByRole("textbox", { name: "Note" }).fill("A note with compact actions.");
  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });

  await page.getByRole("button", { name: "Sources" }).click();
  const sources = page.getByRole("dialog", { name: "Note sources" });
  await sources.getByLabel("Label").fill("Reference");
  await sources.getByLabel("URL").fill("https://example.com/reference");
  await expect(sources).toHaveScreenshot("note-sources.png", {
    animations: "disabled",
    caret: "hide",
    maxDiffPixelRatio: 0.04,
    threshold: 0.35,
  });

  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Delete note" }).click();
  await expect(page.getByRole("dialog", { name: "Delete this note?" })).toHaveScreenshot(
    "note-delete-dialog.png",
    {
      animations: "disabled",
      caret: "hide",
      maxDiffPixelRatio: 0.04,
      threshold: 0.35,
    },
  );
});

test("diagnostics and maintenance settings stay visually stable", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/#/settings");
  const recovery = page.getByRole("region", { name: "Offsite recovery" });
  await expect(recovery.getByRole("button", { name: "Save target off" })).toBeVisible();
  await page.evaluate(() => window.scrollTo(0, 0));

  await expect(page).toHaveScreenshot("settings-diagnostics.png", {
    animations: "disabled",
    caret: "hide",
    maxDiffPixelRatio: 0.04,
    threshold: 0.35,
  });

  await expect(recovery).toHaveScreenshot("settings-recovery.png", {
    animations: "disabled",
    caret: "hide",
    maxDiffPixelRatio: 0.015,
    threshold: 0.3,
  });

  await page.getByRole("heading", { name: "Maintenance" }).scrollIntoViewIfNeeded();
  await expect(page).toHaveScreenshot("settings-maintenance.png", {
    animations: "disabled",
    caret: "hide",
    maxDiffPixelRatio: 0.04,
    threshold: 0.35,
  });
});

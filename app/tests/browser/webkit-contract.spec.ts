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

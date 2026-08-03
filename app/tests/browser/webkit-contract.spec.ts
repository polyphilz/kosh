import { expect, test } from "./fixtures";

test("editor and search keyboard contracts hold in WebKit", async ({ page }) => {
  await page.goto("/#/add");
  await page.getByRole("textbox", { name: /^Title/u }).fill("WebKit contract");
  const editor = page.getByRole("textbox", { name: "Tidbit" });
  await editor.fill("A portable editor passage with `code` and $x^2$.");
  await page.getByRole("button", { name: "Save tidbit" }).click();
  await expect(page.getByRole("heading", { name: "WebKit contract" })).toBeVisible();

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
    "A portable editor passage",
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

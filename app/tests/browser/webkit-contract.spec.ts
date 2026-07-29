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
    .getByRole("link", {
      name: "Search",
      exact: true,
    })
    .click();
  const search = page.getByRole("searchbox", { name: "Search tidbits" });
  await search.fill("portable");
  const result = page.getByRole("option", { name: /WebKit contract/u });
  await expect(result).toBeVisible();
  await search.press("ArrowDown");
  await expect(result).toBeFocused();
  await result.press("Enter");
  await expect(page.locator("#search-citation-detail")).toBeFocused();
  await expect(page.locator("#search-citation-detail")).toContainText("A portable editor passage");
});

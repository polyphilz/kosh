import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "./fixtures";

test("settings exposes local diagnostics and guarded maintenance", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/#/settings");

  await expect(page.getByRole("heading", { name: "Data & diagnostics" })).toBeVisible();
  await expect(page.getByText("Coming later")).toBeVisible();
  await page.getByText("Local paths").click();
  await expect(page.getByText("/tmp/kosh-browser-fixture/kosh.sqlite3")).toBeVisible();

  await page.getByRole("button", { name: "Check integrity" }).click();
  const dialog = page.getByRole("dialog", { name: "Check local data?" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText("Authored data will not change.", { exact: false })).toBeVisible();
  await dialog.getByRole("button", { name: "Run integrity check" }).click();
  await expect(
    page.getByText("Both databases and all referenced media passed integrity checks."),
  ).toBeVisible();

  const results = await new AxeBuilder({ page }).analyze();
  expect(results.violations).toEqual([]);
  await page.getByRole("heading", { name: "Maintenance" }).scrollIntoViewIfNeeded();
  await expect(page.getByRole("heading", { name: "Maintenance" })).toBeVisible();
});

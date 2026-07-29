import { expect, test } from "./fixtures";

test("runtime route crosses the typed fake backend", async ({ page }) => {
  await page.goto("/#/runtime");

  await expect(page.getByRole("heading", { name: "Runtime" })).toBeVisible();
  await expect(page.getByText("/tmp/kosh-browser-fixture")).toBeVisible();
  await expect(page.getByText("fixture-request-1")).toBeVisible();
  await expect(page.getByText("1785201600000")).toBeVisible();
});

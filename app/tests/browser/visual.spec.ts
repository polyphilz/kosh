import { expect, test, type Page } from "./fixtures";

for (const theme of ["LIGHT", "DARK"] as const) {
  test(`catalog and dialog stay stable in ${theme.toLowerCase()} mode`, async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 900 });
    await page.goto("/#/catalog");
    await page.evaluate(async (appearance) => {
      document.documentElement.dataset.appearance = appearance;
      await document.fonts.ready;
    }, theme);

    await expect(page).toHaveScreenshot(`catalog-${theme.toLowerCase()}.png`, {
      animations: "disabled",
      caret: "hide",
      fullPage: true,
      maxDiffPixelRatio: 0.04,
      threshold: 0.35,
    });

    await page.getByRole("button", { name: "Open dialog" }).click();
    await expect(page.getByRole("dialog", { name: "Remove this source?" })).toBeVisible();
    await expect(page).toHaveScreenshot(`dialog-${theme.toLowerCase()}.png`, {
      animations: "disabled",
      caret: "hide",
      maxDiffPixelRatio: 0.04,
      threshold: 0.35,
    });
  });
}

test("library surface stays visually stable", async ({ page }) => {
  await createTidbit(page, "Alpha note", "A compact thought.");
  await page.getByRole("link", { name: "Add" }).click();
  await page.getByRole("textbox", { name: /^Title/u }).fill("Beta chapter notes");
  await page
    .getByRole("textbox", { name: "Tidbit" })
    .fill("# Chapter 2\n\nA longer observation with `code` and $x^2$.");
  await page.getByRole("button", { name: "Save tidbit" }).click();
  await page.getByRole("link", { name: "Library", exact: true }).click();

  await expect(page.getByRole("heading", { name: "Library" })).toBeVisible();
  await expect(page).toHaveScreenshot("library-recent.png", {
    animations: "disabled",
    fullPage: true,
    mask: [page.locator(".library-list time")],
    maskColor: "#d8d2ca",
    maxDiffPixelRatio: 0.04,
    threshold: 0.35,
  });
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
    maxDiffPixelRatio: 0.001,
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

async function createTidbit(page: Page, title: string, body: string) {
  await page.goto("/#/add");
  await page.getByRole("textbox", { name: /^Title/u }).fill(title);
  await page.getByRole("textbox", { name: "Tidbit" }).fill(body);
  await page.getByRole("button", { name: "Save tidbit" }).click();
}

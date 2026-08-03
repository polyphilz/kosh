import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "./fixtures";

const themes = ["LIGHT", "DARK"] as const;

for (const theme of themes) {
  test(`catalog is accessible and stable in ${theme.toLowerCase()} mode`, async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 900 });
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("/#/catalog");
    await page.evaluate(async (appearance) => {
      document.documentElement.dataset.appearance = appearance;
      await document.fonts.ready;
    }, theme);

    const catalogResults = await new AxeBuilder({ page }).analyze();
    expect(catalogResults.violations).toEqual([]);
    const trigger = page.getByRole("button", { name: "Open dialog" });
    await trigger.click();
    await expect(page.getByRole("dialog", { name: "Remove this source?" })).toBeVisible();
    const dialogResults = await new AxeBuilder({ page }).analyze();
    expect(dialogResults.violations).toEqual([]);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toBeHidden();
    await expect(trigger).toBeFocused();
  });
}

test("primary destinations support keyboard navigation", async ({ page }) => {
  await page.goto("/#/");
  const add = page.getByRole("navigation", { name: "Primary" }).getByRole("link", {
    name: "Add",
  });

  await add.focus();
  await page.keyboard.press("Enter");

  await expect(page).toHaveURL(/#\/add$/);
  await expect(page.getByRole("heading", { name: "Add a tidbit" })).toBeVisible();
});

test("supported compact windows retain visible navigation labels", async ({ page }) => {
  await page.setViewportSize({ width: 720, height: 700 });
  await page.goto("/#/");

  const links = page.getByRole("navigation", { name: "Primary" }).getByRole("link");
  await expect(links).toHaveText(["Search", "Add", "Library", "Research", "Settings"]);

  for (const link of await links.all()) {
    await expect(link).not.toHaveCSS("color", "rgba(0, 0, 0, 0)");
  }
});

test("fixed appearance survives navigation and reload", async ({ page }) => {
  await page.goto("/#/settings");
  await page.getByRole("combobox", { name: "Appearance" }).selectOption("DARK");
  await expect(page.locator("html")).toHaveAttribute("data-appearance", "DARK");

  await page
    .getByRole("navigation", { name: "Primary" })
    .getByRole("link", {
      name: "Search",
      exact: true,
    })
    .click();
  await expect(page.locator("html")).toHaveAttribute("data-appearance", "DARK");

  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-appearance", "DARK");
});

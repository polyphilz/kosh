import { expect, test } from "./fixtures";

for (const appearance of ["LIGHT", "DARK"] as const) {
  test(`high-density catalog remains stable in ${appearance.toLowerCase()} mode`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 900, height: 720 });
    await page.goto("/#/catalog");
    await page.evaluate(async (value) => {
      document.documentElement.dataset.appearance = value;
      await document.fonts.ready;
    }, appearance);
    expect(await page.evaluate(() => window.devicePixelRatio)).toBe(2);
    await expect(page).toHaveScreenshot(`catalog-${appearance.toLowerCase()}-hidpi.png`, {
      animations: "disabled",
      caret: "hide",
      scale: "css",
    });
  });
}

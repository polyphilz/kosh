import { expect, test } from "./fixtures";

for (const appearance of ["LIGHT", "DARK"] as const) {
  test(`high-density note remains stable in ${appearance.toLowerCase()} mode`, async ({ page }) => {
    await page.setViewportSize({ width: 900, height: 720 });
    await page.goto("/#/");
    await page
      .getByRole("textbox", { name: "Note" })
      .fill("# High-density note\n\nA sharp, titleless writing surface at two device pixels.");
    await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });
    await page.evaluate(async (value) => {
      document.documentElement.dataset.appearance = value;
      await document.fonts.ready;
    }, appearance);
    expect(await page.evaluate(() => window.devicePixelRatio)).toBe(2);
    await expect(page).toHaveScreenshot(`note-${appearance.toLowerCase()}-hidpi.png`, {
      animations: "disabled",
      caret: "hide",
      maxDiffPixelRatio: 0.04,
      scale: "css",
      threshold: 0.35,
    });
  });
}

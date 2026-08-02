import { expect, test } from "./fixtures";

test("opaque local media blocks edit and serialize in WebKit", async ({ page }) => {
  await page.goto("/blocknote-spike.html?theme=dark");
  await page.waitForFunction(() => window.__KOSH_BLOCKNOTE_SPIKE__?.capability === "blocknote");
  await page.evaluate(() => window.__KOSH_BLOCKNOTE_SPIKE__!.insertMediaFixture());

  const image = page.locator("[data-kosh-image='true']");
  await image.getByLabel("Alt text").fill("WebKit diagram");
  await image.focus();
  await page.keyboard.press("Alt+ArrowLeft");

  const markdown = await page.evaluate(() => window.__KOSH_BLOCKNOTE_SPIKE__!.markdown());
  expect(markdown).toContain("alt=WebKit%20diagram");
  expect(markdown).toContain("{{kosh:pdf:019f547b-6200-7000-8000-000000000102}}");
  expect(markdown).not.toMatch(/(?:blob:|data:|file:|\/Users\/)/u);
  await expect(page.locator("[data-kosh-file='true']")).toContainText("appendix.txt");
});

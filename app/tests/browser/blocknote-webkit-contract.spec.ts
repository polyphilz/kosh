import { expect, test, type Page } from "./fixtures";

test("BlockNote keyboard, composition, and long-note contracts hold in WebKit", async ({
  page,
}) => {
  await page.goto("/blocknote-spike.html?theme=dark");
  await waitForSpike(page);
  await expect(page.locator(".bn-container")).toHaveAttribute("data-color-scheme", "dark");

  const listIds = await page.evaluate(() => window.__KOSH_BLOCKNOTE_SPIKE__!.installListPair());
  await page.keyboard.press("Tab");
  await expect
    .poll(async () => {
      const snapshot = await page.evaluate(() => window.__KOSH_BLOCKNOTE_SPIKE__!.snapshot());
      return (snapshot.blocks[0] as { children: Array<{ id: string }> }).children[0]?.id;
    })
    .toBe(listIds.secondId);
  await page.keyboard.press("Shift+Tab");
  await expect
    .poll(async () => {
      const snapshot = await page.evaluate(() => window.__KOSH_BLOCKNOTE_SPIKE__!.snapshot());
      return snapshot.blocks.length;
    })
    .toBe(2);

  const imeBlockId = await page.evaluate(() => window.__KOSH_BLOCKNOTE_SPIKE__!.appendParagraph());
  const imeBlock = page.locator(
    `.bn-block[data-id="${imeBlockId}"] .bn-block-content .bn-inline-content`,
  );
  await imeBlock.dispatchEvent("compositionstart", { data: "知識" });
  await page.keyboard.insertText("知識");
  await imeBlock.dispatchEvent("compositionend", { data: "知識" });
  await expect(imeBlock).toContainText("知識");

  await page.evaluate(() => window.__KOSH_BLOCKNOTE_SPIKE__!.installLongDocument(200));
  await expect(page.locator(".bn-block-outer")).toHaveCount(200);
  await page.locator(".bn-block-outer").last().scrollIntoViewIfNeeded();
  await page.keyboard.insertText(" webkit-tail");
  await expect(page.locator(".bn-block-outer").last()).toContainText("webkit-tail");
  expect(await page.evaluate(() => window.__KOSH_BLOCKNOTE_SPIKE__!.snapshot().focused)).toBe(true);
});

async function waitForSpike(page: Page) {
  await page.waitForFunction(() => window.__KOSH_BLOCKNOTE_SPIKE__?.capability === "blocknote");
}

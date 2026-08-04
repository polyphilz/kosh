import { expect, test, type Page } from "./fixtures";

test("BlockNote keyboard, composition, and long-note contracts hold in WebKit", async ({
  page,
}) => {
  await page.goto("/editor-harness.html?theme=dark");
  await waitForHarness(page);
  await expect(page.locator(".bn-container")).toHaveAttribute("data-color-scheme", "dark");

  const listIds = await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.installListPair());
  await page.keyboard.press("Tab");
  await expect
    .poll(async () => {
      const snapshot = await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.snapshot());
      return (snapshot.blocks[0] as { children: Array<{ id: string }> }).children[0]?.id;
    })
    .toBe(listIds.secondId);
  await expectInstantIndent(page, listIds.secondId);
  await page.keyboard.press("Shift+Tab");
  await expect
    .poll(async () => {
      const snapshot = await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.snapshot());
      return snapshot.blocks.length;
    })
    .toBe(2);
  await expectInstantIndent(page, listIds.secondId);

  const imeBlockId = await page.evaluate(() =>
    window.__KOSH_BLOCKNOTE_HARNESS__!.appendParagraph(),
  );
  const imeBlock = page.locator(
    `.bn-block[data-id="${imeBlockId}"] .bn-block-content .bn-inline-content`,
  );
  await imeBlock.dispatchEvent("compositionstart", { data: "知識" });
  await page.keyboard.insertText("知識");
  await imeBlock.dispatchEvent("compositionend", { data: "知識" });
  await expect(imeBlock).toContainText("知識");

  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.installLongDocument(200));
  await expect(page.locator(".bn-block-outer")).toHaveCount(200);
  await page.locator(".bn-block-outer").last().scrollIntoViewIfNeeded();
  await page.keyboard.insertText(" webkit-tail");
  await expect(page.locator(".bn-block-outer").last()).toContainText("webkit-tail");
  expect(await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.snapshot().focused)).toBe(
    true,
  );
});

async function waitForHarness(page: Page) {
  await page.waitForFunction(() => window.__KOSH_BLOCKNOTE_HARNESS__?.capability === "blocknote");
}

async function expectInstantIndent(page: Page, blockId: string) {
  const motion = await page.locator(`.bn-block-outer[data-id="${blockId}"]`).evaluate((outer) => {
    const content = outer.querySelector(":scope > .bn-block > .bn-block-content");
    if (!(content instanceof HTMLElement)) throw new Error("Block content is missing");

    return {
      content: getComputedStyle(content).transitionDuration,
      guide: getComputedStyle(outer, "::before").transitionDuration,
      marginLeft: getComputedStyle(outer).marginLeft,
      marker: getComputedStyle(content, "::before").transitionDuration,
      outer: getComputedStyle(outer).transitionDuration,
    };
  });

  expect(motion).toEqual({
    content: "0s",
    guide: "0s",
    marginLeft: "0px",
    marker: "0s",
    outer: "0s",
  });
}

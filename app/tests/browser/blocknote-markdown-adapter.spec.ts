import { expect, test, type Page } from "./fixtures";

const authoredMarkdown = [
  "# Math and code",
  "",
  "Inline $a_i$ remains editable.",
  "",
  "```python",
  "array = [1, 2, 3]",
  "```",
  "",
  "$$",
  "\\sum_i a_i",
  "$$",
].join("\n");

test("the production adapter edits math source and preserves canonical Markdown", async ({
  page,
}) => {
  await openHarness(page);
  await page.evaluate(
    (markdown) => window.__KOSH_BLOCKNOTE_HARNESS__!.loadMarkdown(markdown),
    authoredMarkdown,
  );
  await expect(page.getByLabel("Inline math source")).toHaveValue("a_i");
  await expect(page.getByLabel("Display math source")).toHaveValue("\\sum_i a_i");

  await page.getByLabel("Inline math source").fill("b^2");
  await page.getByLabel("Display math source").fill("\\begin{aligned}\nx &= 1\n\\end{aligned}");
  await expect.poll(() => editorMarkdown(page)).toContain("Inline $b^2$ remains editable.");
  await expect
    .poll(() => editorMarkdown(page))
    .toContain("\\begin{aligned}\nx &= 1\n\\end{aligned}");
  await expect(page.locator(".kosh-math-editor__preview .katex")).toHaveCount(2);

  const code = page.locator('.bn-block-content[data-content-type="codeBlock"]');
  await code.click();
  await page.keyboard.press("Tab");
  await expect
    .poll(() => page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.snapshot().focused))
    .toBe(true);
  await expect.poll(() => editorMarkdown(page)).toContain("array = [1, 2, 3]  \n```");
});

test("math previews bound user-controlled dimensions", async ({ page }) => {
  await openSpike(page);
  await page.evaluate(() =>
    window.__KOSH_BLOCKNOTE_SPIKE__!.loadMarkdown("$$\n\\rule{1000000em}{1em}\n$$"),
  );

  await expect(page.locator(".kosh-math-editor__preview .katex")).toHaveCount(1);
  expect(
    await page.locator(".kosh-math-editor__preview").evaluate((preview) => preview.scrollWidth),
  ).toBeLessThan(1_000);
});

test("rich paste cannot bypass the restricted schema or persist active content", async ({
  page,
}) => {
  await openHarness(page);
  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.loadMarkdown("Paste here: "));
  await page.locator(".bn-inline-content").click();
  await page.evaluate(() => {
    const clipboard = new DataTransfer();
    clipboard.setData(
      "text/html",
      '<table><tbody><tr><td>cell</td></tr></tbody></table><script>alert(1)</script><a href="javascript:alert(2)">unsafe link</a><h4>deep heading</h4>',
    );
    clipboard.setData("text/plain", "cell unsafe link deep heading");
    document.activeElement?.dispatchEvent(
      new ClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData: clipboard }),
    );
  });

  await expect.poll(async () => (await editorMarkdown(page)).length).toBeGreaterThan(12);
  const snapshot = await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.snapshot());
  const types = flatten(snapshot.blocks as HarnessBlock[]).map((block) => block.type);
  expect(types).not.toEqual(
    expect.arrayContaining(["table", "quote", "checkListItem", "image", "audio", "video"]),
  );
  expect(await editorMarkdown(page)).not.toContain("javascript:");
  expect(await page.locator(".bn-editor script").count()).toBe(0);
  expect(await page.locator('a[href^="javascript:"]').count()).toBe(0);
});

interface HarnessBlock {
  children: HarnessBlock[];
  type: string;
}

function flatten(blocks: HarnessBlock[]): HarnessBlock[] {
  return blocks.flatMap((block) => [block, ...flatten(block.children)]);
}

async function openHarness(page: Page) {
  await page.goto("/editor-harness.html");
  await page.waitForFunction(() => window.__KOSH_BLOCKNOTE_HARNESS__?.capability === "blocknote");
}

async function editorMarkdown(page: Page): Promise<string> {
  return page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.markdown());
}

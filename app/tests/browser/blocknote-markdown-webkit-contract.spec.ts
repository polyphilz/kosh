import { expect, test } from "./fixtures";

test("the Markdown adapter edits and serializes the restricted schema in WebKit", async ({
  page,
}) => {
  await page.goto("/editor-harness.html");
  await page.waitForFunction(() => window.__KOSH_BLOCKNOTE_HARNESS__?.capability === "blocknote");
  await page.evaluate(() =>
    window.__KOSH_BLOCKNOTE_HARNESS__!.loadMarkdown(
      "# WebKit\n\nInline $x$ survives.\n\n```typescript\nconst value = 1;\n```\n\n$$\ny = x + 1\n$$",
    ),
  );

  await page.getByLabel("Inline math source").fill("x^2");
  await page.getByLabel("Display math source").fill("y = x^2");

  await expect
    .poll(() => page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.markdown()))
    .toContain("Inline $x^2$ survives.");
  await expect
    .poll(() => page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.markdown()))
    .toContain("y = x^2");
  await expect(page.locator(".kosh-math-editor__preview .katex")).toHaveCount(2);
  await expect(page.locator('.bn-block-content[data-content-type="codeBlock"]')).toContainText(
    "const value = 1;",
  );
});

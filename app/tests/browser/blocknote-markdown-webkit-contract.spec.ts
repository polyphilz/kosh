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

  await page.getByRole("button", { name: "Edit inline math: x" }).click();
  await page.getByLabel("Inline math source").fill("x^2");
  await page.getByRole("button", { name: /Done/u }).click();
  await expect(page.getByLabel("Inline math source")).toHaveCount(0);
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

test("paired-dollar input and slash insertion create editable inline math in WebKit", async ({
  page,
}) => {
  await page.goto("/editor-harness.html");
  await page.waitForFunction(() => window.__KOSH_BLOCKNOTE_HARNESS__?.capability === "blocknote");
  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.appendParagraph("Energy "));
  await page.keyboard.type("$$E = mc^2$$");

  await expect(page.getByRole("button", { name: "Edit inline math: E = mc^2" })).toBeVisible();
  await expect
    .poll(() => page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.markdown()))
    .toContain("Energy $E = mc^2$");

  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.appendParagraph());
  await page.keyboard.type("/inline");
  await page.getByRole("listbox").getByRole("option", { name: "Inline math" }).click();
  await expect(page.getByLabel("Inline math source")).toBeFocused();
});

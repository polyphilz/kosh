import { expect, test, type Page } from "./fixtures";

test("local image, PDF, and file blocks preserve only opaque Markdown references", async ({
  page,
}) => {
  await openHarness(page);
  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.insertMediaFixture());
  expect((await readHarnessSnapshot(page)).blocks.map((block) => block.type)).toEqual(
    expect.arrayContaining(["koshImage", "koshPdf", "koshFileAttachment"]),
  );

  const image = page.locator("[data-kosh-image='true']");
  await expect(image).toBeVisible();
  await expect(page.locator("[data-kosh-pdf='true']")).toContainText("chapter.pdf");
  await expect(page.locator("[data-kosh-file='true']")).toContainText("appendix.txt");
  await page.locator("[data-kosh-pdf='true']").getByRole("button", { name: "Open" }).click();
  await page.locator("[data-kosh-file='true']").getByRole("button", { name: "Reveal" }).click();

  await image.getByLabel("Alt text").fill("Architecture diagram");
  await image.getByLabel("Caption").fill("Chapter overview");
  const beforeWidth = await image.evaluate((element) => Number.parseInt(element.style.width, 10));
  await image.focus();
  await page.keyboard.press("Alt+ArrowLeft");
  await expect
    .poll(() => image.evaluate((element) => Number.parseInt(element.style.width, 10)))
    .toBe(beforeWidth - 5);
  const resizeHandle = image.getByRole("button", { name: "Resize image" });
  const handleBox = await resizeHandle.boundingBox();
  if (!handleBox) throw new Error("Image resize handle has no bounding box");
  await page.mouse.move(handleBox.x + handleBox.width / 2, handleBox.y + handleBox.height / 2);
  await page.mouse.down();
  await page.mouse.move(handleBox.x + handleBox.width / 2 + 60, handleBox.y + handleBox.height / 2);
  await page.mouse.up();
  const resizedWidth = await image.evaluate((element) => Number.parseInt(element.style.width, 10));
  expect(resizedWidth).toBeGreaterThan(beforeWidth - 5);

  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.setEditable(false));
  await image.focus();
  await page.keyboard.press("Alt+ArrowRight");
  await expect
    .poll(() => image.evaluate((element) => Number.parseInt(element.style.width, 10)))
    .toBe(resizedWidth);
  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.setEditable(true));

  const snapshot = await readHarnessSnapshot(page);
  const pdfBlock = snapshot.blocks.find((block) => block.type === "koshPdf")!;
  await page.locator(`.bn-block-outer[data-id="${pdfBlock.id}"]`).hover();
  await page.getByRole("button", { name: "Drag to move" }).click();
  await page.getByRole("menuitem", { name: "Move block up" }).click();
  await expect
    .poll(async () => {
      const blocks = (await readHarnessSnapshot(page)).blocks;
      return blocks.findIndex((block) => block.id === pdfBlock.id);
    })
    .toBe(snapshot.blocks.findIndex((block) => block.id === pdfBlock.id) - 1);
  const pdf = page.locator("[data-kosh-pdf='true']");
  await pdf.getByRole("button", { name: "Remove" }).click();
  await expect(pdf).toHaveCount(0);
  await page.locator(".bn-inline-content").last().click();
  await page.keyboard.press("ControlOrMeta+z");
  await expect(page.locator("[data-kosh-pdf='true']")).toHaveCount(1);

  const file = page.locator("[data-kosh-file='true']");
  await file.getByRole("button", { name: "Replace" }).click();
  await expect(file).toHaveCount(0);
  await expect(page.locator("[data-kosh-image='true']")).toHaveCount(2);
  await page.locator(".bn-inline-content").last().click();
  await page.keyboard.press("ControlOrMeta+z");
  await expect(page.locator("[data-kosh-file='true']")).toHaveCount(1);

  const markdown = await editorMarkdown(page);
  expect(markdown).toContain(
    "{{kosh:image:019f547b-6200-7000-8000-000000000101;width=" +
      `${resizedWidth}%;alt=Architecture%20diagram;caption=Chapter%20overview}}`,
  );
  expect(markdown).toContain("{{kosh:pdf:019f547b-6200-7000-8000-000000000102}}");
  expect(markdown).toContain("{{kosh:attachment:019f547b-6200-7000-8000-000000000103}}");
  expect(markdown).not.toMatch(/(?:blob:|data:|file:|\/Users\/)/u);

  await page.evaluate((value) => window.__KOSH_BLOCKNOTE_HARNESS__!.loadMarkdown(value), markdown);
  await expect(page.locator("[data-kosh-image='true']")).toHaveCount(1);
  await expect(page.locator("[data-kosh-pdf='true']")).toHaveCount(1);
  await expect(page.locator("[data-kosh-file='true']")).toHaveCount(1);
  await expect.poll(() => editorMarkdown(page)).toBe(markdown);
});

test("paste, native-drop insertion, cancellation, and failure retain authored content", async ({
  page,
}) => {
  await openHarness(page);
  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.loadMarkdown("Keep this text"));
  await page.locator(".bn-inline-content").click();
  await page.evaluate(() => {
    const clipboard = new DataTransfer();
    clipboard.items.add(new File([new Uint8Array([1, 2, 3])], "paste.png", { type: "image/png" }));
    document.activeElement?.dispatchEvent(
      new ClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData: clipboard }),
    );
  });
  await expect(page.locator("[data-kosh-image='true']")).toHaveCount(1);

  await page.evaluate(() => {
    const transfer = new DataTransfer();
    transfer.setData("application/x-kosh-media", "validated-native-drop");
    document
      .querySelector("main")!
      .dispatchEvent(
        new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer: transfer }),
      );
  });
  await expect(page.locator("[data-kosh-image='true']")).toHaveCount(2);
  await expect(page.locator("[data-kosh-pdf='true']")).toHaveCount(1);
  await expect(page.locator("[data-kosh-file='true']")).toHaveCount(1);

  for (const outcome of ["cancel", "failure"] as const) {
    const requestId = await page.evaluate(() =>
      window.__KOSH_BLOCKNOTE_HARNESS__!.beginDeferredMedia(),
    );
    await expect(page.getByRole("status", { name: "Adding deferred image" })).toBeVisible();
    await page.evaluate(
      ({ id, result }) => window.__KOSH_BLOCKNOTE_HARNESS__!.resolveDeferredMedia(id, result),
      { id: requestId, result: outcome },
    );
    await expect(page.getByRole("status", { name: "Adding deferred image" })).toHaveCount(0);
  }

  const markdown = await editorMarkdown(page);
  expect(markdown.replace(/\{\{kosh:[^}]+\}\}|\s/gu, "")).toContain("Keepthistext");
  expect(markdown).not.toContain("koshPendingMedia");
});

test("read-only editors reject direct, deferred, pasted, and dropped media", async ({ page }) => {
  await openHarness(page);
  const original = "Read-only authored text";
  await page.evaluate(
    (markdown) => window.__KOSH_BLOCKNOTE_HARNESS__!.loadMarkdown(markdown),
    original,
  );
  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.setEditable(false));

  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.insertMediaFixture());
  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.beginDeferredMedia());
  await page.evaluate(() => {
    const clipboard = new DataTransfer();
    clipboard.items.add(new File([new Uint8Array([1])], "paste.png", { type: "image/png" }));
    document.activeElement?.dispatchEvent(
      new ClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData: clipboard }),
    );
    const transfer = new DataTransfer();
    transfer.setData("application/x-kosh-media", "validated-native-drop");
    document
      .querySelector("main")!
      .dispatchEvent(
        new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer: transfer }),
      );
  });

  await expect(page.getByRole("status", { name: "Adding deferred image" })).toHaveCount(0);
  await expect(page.locator("[data-kosh-image='true']")).toHaveCount(0);
  await expect.poll(() => editorMarkdown(page)).toBe(original);
});

test("slow media ingest preserves the active writing cursor", async ({ page }) => {
  await openHarness(page);
  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.loadMarkdown("Original thought"));
  const requestId = await page.evaluate(() =>
    window.__KOSH_BLOCKNOTE_HARNESS__!.beginDeferredMedia(),
  );
  await expect(page.getByRole("status", { name: "Adding deferred image" })).toBeVisible();

  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.appendParagraph("Keep writing"));
  await page.keyboard.type(" while loading");
  await page.evaluate(
    (id) => window.__KOSH_BLOCKNOTE_HARNESS__!.resolveDeferredMedia(id, "success"),
    requestId,
  );
  await expect(page.getByRole("status", { name: "Adding deferred image" })).toHaveCount(0);
  await expect(page.locator("[data-kosh-image='true']")).toHaveCount(1);
  await page.keyboard.type(" after completion");

  const markdown = await editorMarkdown(page);
  expect(markdown).toContain("Keep writing while loading after completion");
});

test("deferred media inserts at the active caret without stealing it on completion", async ({
  page,
}) => {
  await openHarness(page);
  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.loadMarkdown("Before after"));
  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.setCursorOffset(6));

  const requestId = await page.evaluate(() =>
    window.__KOSH_BLOCKNOTE_HARNESS__!.beginDeferredMedia(),
  );
  await expect
    .poll(async () => (await readHarnessSnapshot(page)).blocks.map((block) => block.type))
    .toEqual(["paragraph", "koshPendingMedia", "paragraph"]);
  await page.keyboard.type("inserted");
  await page.evaluate(
    (id) => window.__KOSH_BLOCKNOTE_HARNESS__!.resolveDeferredMedia(id, "success"),
    requestId,
  );
  await expect(page.locator("[data-kosh-image='true']")).toHaveCount(1);
  await page.keyboard.type(" still here");

  const markdown = await editorMarkdown(page);
  expect(markdown).toContain("Before\n\n{{kosh:image:");
  expect(markdown).toContain("inserted still here after");
});

test("cancelled or failed media restores the unsplit authored text", async ({ page }) => {
  await openHarness(page);
  for (const outcome of ["cancel", "failure"] as const) {
    const original = "Before selected text after";
    await page.evaluate(
      (markdown) => window.__KOSH_BLOCKNOTE_HARNESS__!.loadMarkdown(markdown),
      original,
    );
    if (outcome === "cancel") {
      await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.setCursorOffset(6));
    } else {
      await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.setTextSelectionOffsets(7, 20));
    }

    const requestId = await page.evaluate(() =>
      window.__KOSH_BLOCKNOTE_HARNESS__!.beginDeferredMedia(),
    );
    await expect(page.getByRole("status", { name: "Adding deferred image" })).toBeVisible();
    await page.evaluate(
      ({ id, result }) => window.__KOSH_BLOCKNOTE_HARNESS__!.resolveDeferredMedia(id, result),
      { id: requestId, result: outcome },
    );

    await expect(page.getByRole("status", { name: "Adding deferred image" })).toHaveCount(0);
    await expect.poll(() => editorMarkdown(page)).toBe(original);
    expect((await readHarnessSnapshot(page)).blocks.map((block) => block.type)).toEqual([
      "paragraph",
    ]);
  }
});

test("overlapping deferred media restores one paragraph or preserves committed media", async ({
  page,
}) => {
  await openHarness(page);
  const original = "Before after";

  await page.evaluate(
    (markdown) => window.__KOSH_BLOCKNOTE_HARNESS__!.loadMarkdown(markdown),
    original,
  );
  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.setCursorOffset(6));
  const cancelled = await page.evaluate(() => [
    window.__KOSH_BLOCKNOTE_HARNESS__!.beginDeferredMedia(),
    window.__KOSH_BLOCKNOTE_HARNESS__!.beginDeferredMedia(),
  ]);
  await page.evaluate(([first, second]) => {
    window.__KOSH_BLOCKNOTE_HARNESS__!.resolveDeferredMedia(first, "cancel");
    window.__KOSH_BLOCKNOTE_HARNESS__!.resolveDeferredMedia(second, "cancel");
  }, cancelled);
  await expect.poll(() => editorMarkdown(page)).toBe(original);
  expect((await readHarnessSnapshot(page)).blocks.map((block) => block.type)).toEqual([
    "paragraph",
  ]);

  await page.evaluate(
    (markdown) => window.__KOSH_BLOCKNOTE_HARNESS__!.loadMarkdown(markdown),
    original,
  );
  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.setCursorOffset(6));
  const mixed = await page.evaluate(() => [
    window.__KOSH_BLOCKNOTE_HARNESS__!.beginDeferredMedia(),
    window.__KOSH_BLOCKNOTE_HARNESS__!.beginDeferredMedia(),
  ]);
  await page.evaluate(([first, second]) => {
    window.__KOSH_BLOCKNOTE_HARNESS__!.resolveDeferredMedia(second, "success");
    window.__KOSH_BLOCKNOTE_HARNESS__!.resolveDeferredMedia(first, "cancel");
  }, mixed);
  await expect(page.locator("[data-kosh-image='true']")).toHaveCount(1);
  await expect.poll(() => editorMarkdown(page)).toContain("Before\n\n{{kosh:image:");
  await expect.poll(() => editorMarkdown(page)).toContain("}}\n\n&#x20;after");
});

test("image and PDF retries restart status polling", async ({ page }) => {
  await openHarness(page);

  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.installRetryMediaFixture("image"));
  const imageRetry = page.getByRole("button", { name: "Retry text recognition" });
  await expect(imageRetry).toBeVisible();
  const imageCallsBeforeRetry = await page.evaluate(() =>
    window.__KOSH_BLOCKNOTE_HARNESS__!.mediaStatusCalls("image"),
  );
  await imageRetry.click();
  await expect(page.locator("[data-kosh-image='true']")).toContainText("Image text indexed");
  await expect
    .poll(() => page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.mediaStatusCalls("image")))
    .toBeGreaterThan(imageCallsBeforeRetry);

  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.installRetryMediaFixture("pdf"));
  const pdfRetry = page.getByRole("button", { name: "Retry extraction" });
  await expect(pdfRetry).toBeVisible();
  const pdfCallsBeforeRetry = await page.evaluate(() =>
    window.__KOSH_BLOCKNOTE_HARNESS__!.mediaStatusCalls("pdf"),
  );
  await pdfRetry.click();
  await expect(page.locator("[data-kosh-pdf='true']")).toContainText("12 pages · 12 searchable");
  await expect
    .poll(() => page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.mediaStatusCalls("pdf")))
    .toBeGreaterThan(pdfCallsBeforeRetry);
});

test("the restricted slash menu inserts media through the local controller", async ({ page }) => {
  await openHarness(page);
  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.appendParagraph());
  await page.keyboard.type("/image");
  await page.getByRole("option", { name: "Image" }).click();

  await expect(page.locator("[data-kosh-image='true']")).toHaveCount(1);
  await expect
    .poll(() => editorMarkdown(page))
    .toContain("{{kosh:image:019f547b-6200-7000-8000-000000000101");
});

interface HarnessBlock {
  id: string;
  props: Record<string, unknown>;
  type: string;
}

async function openHarness(page: Page) {
  await page.goto("/editor-harness.html");
  await page.waitForFunction(() => window.__KOSH_BLOCKNOTE_HARNESS__?.capability === "blocknote");
}

async function readHarnessSnapshot(page: Page): Promise<{ blocks: HarnessBlock[] }> {
  return page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.snapshot()) as Promise<{
    blocks: HarnessBlock[];
  }>;
}

async function editorMarkdown(page: Page): Promise<string> {
  return page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.markdown());
}

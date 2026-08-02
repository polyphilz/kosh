import type { Locator } from "@playwright/test";
import { expect, test, type Page } from "./fixtures";

interface SpikeInlineContent {
  props?: Record<string, unknown>;
  styles?: Record<string, boolean>;
  text?: string;
  type: string;
}

interface SpikeBlock {
  children: SpikeBlock[];
  content?: SpikeInlineContent[];
  id: string;
  props: Record<string, unknown>;
  type: string;
}

interface SpikeSnapshot {
  blocks: SpikeBlock[];
  focused: boolean;
  selectedBlockIds: string[];
}

const expectedSchema = {
  blocks: [
    "paragraph",
    "heading",
    "bulletListItem",
    "numberedListItem",
    "codeBlock",
    "displayMath",
  ],
  inlineContent: ["text", "link", "inlineMath"],
  styles: ["bold", "italic", "strike", "code"],
};

test("the restricted BlockNote schema rejects the plain-editor approximation", async ({ page }) => {
  await page.goto("/blocknote-spike.html?mode=plain");
  await expect(page.getByRole("textbox", { name: "Plain editor" })).toBeVisible();
  expect(await page.evaluate(() => window.__KOSH_BLOCKNOTE_SPIKE__ ?? null)).toBeNull();

  await openSpike(page);
  const schema = await page.evaluate(() => window.__KOSH_BLOCKNOTE_SPIKE__!.schema);
  expect(schema).toEqual(expectedSchema);

  const snapshot = await readSnapshot(page);
  expect(snapshot.blocks.map((block) => block.type)).toEqual([
    "heading",
    "heading",
    "heading",
    "paragraph",
    "bulletListItem",
    "numberedListItem",
    "codeBlock",
    "displayMath",
    "paragraph",
  ]);
  expect(snapshot.blocks.slice(0, 3).map((block) => block.props.level)).toEqual([1, 2, 3]);
  expect(snapshot.blocks[4]?.children[0]?.type).toBe("bulletListItem");
  expect(snapshot.blocks[5]?.children[0]?.type).toBe("numberedListItem");
  expect(snapshot.blocks[6]?.props.language).toBe("python");
  expect(snapshot.blocks[7]?.props.latex).toBe("\\sum_i a_i");
  expect(snapshot.blocks[3]?.content).toEqual([
    { type: "text", text: "Bold", styles: { bold: true } },
    { type: "text", text: ", italic", styles: { italic: true } },
    { type: "text", text: ", strike", styles: { strike: true } },
    { type: "text", text: ", and code", styles: { code: true } },
    { type: "text", text: " with ", styles: {} },
    { type: "inlineMath", props: { latex: "a_i" } },
    { type: "text", text: ".", styles: {} },
  ]);
  expect(flatten(snapshot.blocks).map((block) => block.type)).not.toEqual(
    expect.arrayContaining(["table", "quote", "checkListItem", "image", "audio", "video"]),
  );

  await page.evaluate(() => window.__KOSH_BLOCKNOTE_SPIKE__!.appendParagraph());
  await page.keyboard.type("/");
  const options = page.getByRole("option");
  await expect(options).toHaveText([
    "Paragraph",
    "Heading 1",
    "Heading 2",
    "Heading 3",
    "Bullet list",
    "Ordered list",
    "Code block",
    "Display math",
    "Inline math",
  ]);
  await page.getByRole("option", { name: "Display math" }).click();
  await expect.poll(async () => (await readSnapshot(page)).blocks.at(-1)?.type).toBe("displayMath");
});

test("real keyboard input covers undo, redo, IME, and list nesting", async ({ page }) => {
  await openSpike(page);
  const undoBlockId = await page.evaluate(() =>
    window.__KOSH_BLOCKNOTE_SPIKE__!.appendParagraph("Undo seed "),
  );
  await page.keyboard.type("change");
  await expect
    .poll(async () => blockText(await readSnapshot(page), undoBlockId))
    .toBe("Undo seed change");
  await page.keyboard.press("Meta+z");
  await expect.poll(async () => blockText(await readSnapshot(page), undoBlockId)).toBe("");
  await page.keyboard.press("Meta+Shift+z");
  await expect
    .poll(async () => blockText(await readSnapshot(page), undoBlockId))
    .toBe("Undo seed change");

  const imeBlockId = await page.evaluate(() => window.__KOSH_BLOCKNOTE_SPIKE__!.appendParagraph());
  const imeBlock = blockContent(page, imeBlockId).locator(".bn-inline-content");
  await imeBlock.dispatchEvent("compositionstart", { data: "かな" });
  await page.keyboard.insertText("かな");
  await imeBlock.dispatchEvent("compositionend", { data: "かな" });
  await expect.poll(async () => blockText(await readSnapshot(page), imeBlockId)).toBe("かな");
  await expect(page.getByRole("option")).toHaveCount(0);

  const listIds = await page.evaluate(() => window.__KOSH_BLOCKNOTE_SPIKE__!.installListPair());
  await page.keyboard.press("Tab");
  await expect.poll(async () => (await readSnapshot(page)).blocks.length).toBe(1);
  expect((await readSnapshot(page)).blocks[0]?.children[0]?.id).toBe(listIds.secondId);
  await page.keyboard.press("Shift+Tab");
  const unnested = await readSnapshot(page);
  expect(unnested.blocks.map((block) => block.id)).toEqual([listIds.firstId, listIds.secondId]);
  expect(unnested.focused).toBe(true);
});

test("the gutter selects, deletes, reorders, and restores editor focus", async ({ page }) => {
  await openSpike(page);
  let snapshot = await readSnapshot(page);

  await pointerSelect(page, blockContent(page, snapshot.blocks[1]!.id));
  expect((await readSnapshot(page)).selectedBlockIds).toEqual([snapshot.blocks[1]!.id]);

  await page.reload();
  await waitForSpike(page);
  snapshot = await readSnapshot(page);
  const selectionStart = blockContent(page, snapshot.blocks[1]!.id);
  const selectionEnd = blockContent(page, snapshot.blocks[3]!.id);
  await pointerSelect(page, selectionStart, selectionEnd);
  const selected = (await readSnapshot(page)).selectedBlockIds;
  expect(selected).toEqual(snapshot.blocks.slice(1, 4).map((block) => block.id));

  await page.locator(`.bn-block-outer[data-id="${selected[0]}"]`).hover();
  await page.getByRole("button", { name: "Open block menu" }).click();
  await page.getByRole("menuitem", { name: "Delete selected blocks" }).click();
  await expect.poll(async () => (await readSnapshot(page)).focused).toBe(true);
  snapshot = await readSnapshot(page);
  expect(snapshot.blocks).toHaveLength(6);
  expect(flatten(snapshot.blocks).map((block) => block.id)).not.toEqual(
    expect.arrayContaining(selected),
  );

  await page.reload();
  await waitForSpike(page);
  snapshot = await readSnapshot(page);
  const nestedBlockId = snapshot.blocks[4]!.children[0]!.id;
  const targetBlockId = snapshot.blocks[0]!.id;
  await dragBlockBefore(page, nestedBlockId, targetBlockId);
  await expect.poll(async () => (await readSnapshot(page)).blocks[0]?.id).toBe(nestedBlockId);
  snapshot = await readSnapshot(page);
  expect(snapshot.blocks.find((block) => block.type === "bulletListItem")?.children).toEqual([]);
  expect(snapshot.focused).toBe(true);
});

test("long documents remain editable in both appearances", async ({ page }) => {
  for (const theme of ["light", "dark"] as const) {
    await page.goto(`/blocknote-spike.html?theme=${theme}`);
    await waitForSpike(page);
    await expect(page.locator(".bn-container")).toHaveAttribute("data-color-scheme", theme);
    await page.evaluate(() => window.__KOSH_BLOCKNOTE_SPIKE__!.installLongDocument(500));
    await expect(page.locator(".bn-block-outer")).toHaveCount(500);
    const last = page.locator(".bn-block-outer").last();
    await last.scrollIntoViewIfNeeded();
    await page.keyboard.insertText(` ${theme}-tail`);
    const snapshot = await readSnapshot(page);
    expect(blockText(snapshot, snapshot.blocks.at(-1)!.id)).toContain(`${theme}-tail`);
    expect(snapshot.focused).toBe(true);
  }
});

async function openSpike(page: Page) {
  await page.goto("/blocknote-spike.html");
  await waitForSpike(page);
}

async function waitForSpike(page: Page) {
  await page.waitForFunction(() => window.__KOSH_BLOCKNOTE_SPIKE__?.capability === "blocknote");
}

async function readSnapshot(page: Page): Promise<SpikeSnapshot> {
  return (await page.evaluate(() => window.__KOSH_BLOCKNOTE_SPIKE__!.snapshot())) as SpikeSnapshot;
}

function flatten(blocks: SpikeBlock[]): SpikeBlock[] {
  return blocks.flatMap((block) => [block, ...flatten(block.children)]);
}

function blockText(snapshot: SpikeSnapshot, blockId: string): string {
  return (
    flatten(snapshot.blocks)
      .find((block) => block.id === blockId)
      ?.content?.map((content) => content.text ?? "")
      .join("") ?? ""
  );
}

function blockContent(page: Page, blockId: string): Locator {
  return page.locator(`.bn-block[data-id="${blockId}"] .bn-block-content`);
}

async function pointerSelect(page: Page, start: Locator, end = start) {
  const startBox = await start.boundingBox();
  const endBox = await end.boundingBox();
  if (!startBox || !endBox) throw new Error("selection blocks are not rendered");
  await page.mouse.move(startBox.x + 2, startBox.y + startBox.height / 2);
  await page.mouse.down();
  await page.mouse.move(endBox.x + Math.min(250, endBox.width - 2), endBox.y + endBox.height / 2, {
    steps: 12,
  });
  await page.mouse.up();
}

async function dragBlockBefore(page: Page, sourceId: string, targetId: string) {
  await page.locator(`.bn-block-outer[data-id="${sourceId}"]`).hover();
  const handleBox = await page.getByRole("button", { name: "Open block menu" }).boundingBox();
  const targetBox = await page.locator(`.bn-block[data-id="${targetId}"]`).boundingBox();
  if (!handleBox || !targetBox) throw new Error("drag source or target is not rendered");
  const sourceX = handleBox.x + handleBox.width / 2;
  const sourceY = handleBox.y + handleBox.height / 2;
  await page.mouse.move(sourceX, sourceY);
  await page.mouse.down();
  await page.waitForTimeout(75);
  await page.mouse.move(sourceX + 10, sourceY, { steps: 3 });
  await page.mouse.move(targetBox.x + targetBox.width / 2, targetBox.y + 2, { steps: 20 });
  await page.waitForTimeout(75);
  await page.mouse.up();
}

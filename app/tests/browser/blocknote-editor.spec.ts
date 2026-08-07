import type { Locator } from "@playwright/test";
import { expect, test, type Page } from "./fixtures";

interface HarnessInlineContent {
  props?: Record<string, unknown>;
  styles?: Record<string, boolean>;
  text?: string;
  type: string;
}

interface HarnessBlock {
  children: HarnessBlock[];
  content?: HarnessInlineContent[];
  id: string;
  props: Record<string, unknown>;
  type: string;
}

interface HarnessSnapshot {
  blocks: HarnessBlock[];
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
    "koshImage",
    "koshFileAttachment",
  ],
  inlineContent: ["text", "link", "inlineMath"],
  styles: ["bold", "italic", "strike", "code"],
};

test("the editor exposes only Kosh's restricted BlockNote schema", async ({ page }) => {
  await openHarness(page);
  const schema = await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.schema);
  expect(schema).toEqual(expectedSchema);

  const snapshot = await readHarnessSnapshot(page);
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

  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.appendParagraph());
  await page.keyboard.type("/");
  const slashMenu = page.getByRole("listbox");
  const options = slashMenu.getByRole("option");
  await expect(slashMenu.getByText("Kosh blocks", { exact: true })).toHaveCount(0);
  await expect(slashMenu.getByText("Kosh media", { exact: true })).toHaveCount(0);
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
    "Image",
    "File",
  ]);
  await slashMenu.getByRole("option", { name: "Display math" }).click();
  await expect
    .poll(async () => (await readHarnessSnapshot(page)).blocks.at(-1)?.type)
    .toBe("displayMath");
});

test("slash menu rows keep one height while filtering", async ({ page }) => {
  await openHarness(page);
  await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.appendParagraph());
  await page.keyboard.type("/");

  const options = page.getByRole("listbox").getByRole("option");
  await expect(options).toHaveCount(11);
  const fullMenuHeights = await options.evaluateAll((rows) =>
    rows.map((row) => row.getBoundingClientRect().height),
  );
  expect(new Set(fullMenuHeights)).toEqual(new Set([40]));

  await page.keyboard.type("pa");
  await expect(options).toHaveCount(1);
  await expect(options).toHaveText(["Paragraph"]);
  expect(await options.first().evaluate((row) => row.getBoundingClientRect().height)).toBe(40);
});

test("real keyboard input covers undo, redo, IME, and list nesting", async ({ page }) => {
  await openHarness(page);
  const undoBlockId = await page.evaluate(() =>
    window.__KOSH_BLOCKNOTE_HARNESS__!.appendParagraph("Undo seed "),
  );
  await page.keyboard.type("change");
  await expect
    .poll(async () => blockText(await readHarnessSnapshot(page), undoBlockId))
    .toBe("Undo seed change");
  await page.keyboard.press("ControlOrMeta+z");
  await expect.poll(async () => blockText(await readHarnessSnapshot(page), undoBlockId)).toBe("");
  await page.keyboard.press("ControlOrMeta+Shift+z");
  await expect
    .poll(async () => blockText(await readHarnessSnapshot(page), undoBlockId))
    .toBe("Undo seed change");

  const imeBlockId = await page.evaluate(() =>
    window.__KOSH_BLOCKNOTE_HARNESS__!.appendParagraph(),
  );
  const imeBlock = blockContent(page, imeBlockId).locator(".bn-inline-content");
  await imeBlock.dispatchEvent("compositionstart", { data: "かな" });
  await page.keyboard.insertText("かな");
  await imeBlock.dispatchEvent("compositionend", { data: "かな" });
  await expect
    .poll(async () => blockText(await readHarnessSnapshot(page), imeBlockId))
    .toBe("かな");
  await expect(page.getByRole("listbox")).toHaveCount(0);

  const listIds = await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.installListPair());
  await page.keyboard.press("Tab");
  await expect.poll(async () => (await readHarnessSnapshot(page)).blocks.length).toBe(1);
  expect((await readHarnessSnapshot(page)).blocks[0]?.children[0]?.id).toBe(listIds.secondId);
  expect(await blockIndentMotion(page, listIds.secondId)).toEqual({
    contentTransitionDuration: "0s",
    guideTransitionDuration: "0s",
    marginLeft: "0px",
    markerTransitionDuration: "0s",
    outerTransitionDuration: "0s",
  });
  await page.keyboard.press("Shift+Tab");
  const unnested = await readHarnessSnapshot(page);
  expect(unnested.blocks.map((block) => block.id)).toEqual([listIds.firstId, listIds.secondId]);
  expect(unnested.focused).toBe(true);
  expect(await blockIndentMotion(page, listIds.secondId)).toEqual({
    contentTransitionDuration: "0s",
    guideTransitionDuration: "0s",
    marginLeft: "0px",
    markerTransitionDuration: "0s",
    outerTransitionDuration: "0s",
  });
});

test("the gutter controls add below and expose stable hover guidance", async ({ page }) => {
  await openHarness(page);
  await page.evaluate(() => {
    document.documentElement.dataset.appearance = "DARK";
  });
  const before = await readHarnessSnapshot(page);
  const targetIndex = 3;
  const targetId = before.blocks[targetIndex]!.id;
  await page.locator(`.bn-block-outer[data-id="${targetId}"]`).hover();

  const addBelow = page.getByRole("button", { name: "Click to add below" });
  const drag = page.getByRole("button", { name: "Drag to move" });
  await expect(addBelow).toBeVisible();
  await expect(drag).toBeVisible();
  for (const button of [addBelow, drag]) {
    await button.hover();
    const highlight = await button.evaluate((element) => {
      const icon = element.firstElementChild;
      if (!(icon instanceof HTMLElement)) throw new Error("gutter icon wrapper is missing");
      const accentProbe = document.createElement("span");
      accentProbe.style.color = "var(--accent)";
      element.append(accentProbe);
      const accent = getComputedStyle(accentProbe).color;
      accentProbe.remove();
      const buttonStyle = getComputedStyle(element);
      const iconStyle = getComputedStyle(icon);
      return {
        accent,
        buttonBackground: buttonStyle.backgroundColor,
        buttonWidth: element.getBoundingClientRect().width,
        iconBackground: iconStyle.backgroundColor,
        iconColor: iconStyle.color,
        iconWidth: icon.getBoundingClientRect().width,
      };
    });
    expect(highlight).toMatchObject({
      buttonBackground: "rgba(0, 0, 0, 0)",
      buttonWidth: 24,
      iconWidth: 20,
    });
    expect(highlight.iconBackground).not.toBe("rgba(0, 0, 0, 0)");
    expect(highlight.iconColor).toBe(highlight.accent);
  }
  await addBelow.hover();
  await expect
    .poll(() =>
      addBelow.evaluate((button) => ({
        content: getComputedStyle(button, "::after").content,
        opacity: getComputedStyle(button, "::after").opacity,
      })),
    )
    .toEqual({ content: '"Click to add below"', opacity: "1" });
  await drag.hover();
  await expect
    .poll(() =>
      drag.evaluate((button) => ({
        content: getComputedStyle(button, "::after").content,
        opacity: getComputedStyle(button, "::after").opacity,
      })),
    )
    .toEqual({ content: '"Drag to move"', opacity: "1" });

  await addBelow.hover();
  await addBelow.click();
  await expect
    .poll(async () => (await readHarnessSnapshot(page)).blocks.length)
    .toBe(before.blocks.length + 1);
  const after = await readHarnessSnapshot(page);
  expect(after.blocks[targetIndex + 1]).toMatchObject({ content: [], type: "paragraph" });
  expect(after.focused).toBe(true);
});

test("the gutter selects, deletes, reorders, and restores editor focus", async ({ page }) => {
  await openHarness(page);
  let snapshot = await readHarnessSnapshot(page);

  await pointerSelect(page, blockContent(page, snapshot.blocks[1]!.id));
  expect((await readHarnessSnapshot(page)).selectedBlockIds).toEqual([snapshot.blocks[1]!.id]);

  await page.reload();
  await waitForHarness(page);
  snapshot = await readHarnessSnapshot(page);
  const selectionStart = blockContent(page, snapshot.blocks[1]!.id);
  const selectionEnd = blockContent(page, snapshot.blocks[3]!.id);
  await pointerSelect(page, selectionStart, selectionEnd);
  const selected = (await readHarnessSnapshot(page)).selectedBlockIds;
  expect(selected).toEqual(snapshot.blocks.slice(1, 4).map((block) => block.id));

  await page.locator(`.bn-block-outer[data-id="${selected[0]}"]`).hover();
  await page.getByRole("button", { name: "Drag to move" }).click();
  await page.getByRole("menuitem", { name: "Delete selected blocks" }).click();
  await expect.poll(async () => (await readHarnessSnapshot(page)).focused).toBe(true);
  snapshot = await readHarnessSnapshot(page);
  expect(snapshot.blocks).toHaveLength(6);
  expect(flatten(snapshot.blocks).map((block) => block.id)).not.toEqual(
    expect.arrayContaining(selected),
  );

  await page.reload();
  await waitForHarness(page);
  snapshot = await readHarnessSnapshot(page);
  const nestedBlockId = snapshot.blocks[4]!.children[0]!.id;
  const targetBlockId = snapshot.blocks[0]!.id;
  await dragBlockBefore(page, nestedBlockId, targetBlockId);
  await expect
    .poll(async () => (await readHarnessSnapshot(page)).blocks[0]?.id)
    .toBe(nestedBlockId);
  snapshot = await readHarnessSnapshot(page);
  expect(snapshot.blocks.find((block) => block.type === "bulletListItem")?.children).toEqual([]);
  expect(snapshot.focused).toBe(true);
});

test("long documents remain editable in both appearances", async ({ page }) => {
  for (const theme of ["light", "dark"] as const) {
    await page.goto(`/editor-harness.html?theme=${theme}`);
    await waitForHarness(page);
    await expect(page.locator(".bn-container")).toHaveAttribute("data-color-scheme", theme);
    await page.evaluate(() => window.__KOSH_BLOCKNOTE_HARNESS__!.installLongDocument(500));
    await expect(page.locator(".bn-block-outer")).toHaveCount(500);
    const last = page.locator(".bn-block-outer").last();
    await last.scrollIntoViewIfNeeded();
    await page.keyboard.insertText(` ${theme}-tail`);
    const snapshot = await readHarnessSnapshot(page);
    expect(blockText(snapshot, snapshot.blocks.at(-1)!.id)).toContain(`${theme}-tail`);
    expect(snapshot.focused).toBe(true);
  }
});

async function openHarness(page: Page) {
  await page.goto("/editor-harness.html");
  await waitForHarness(page);
}

async function blockIndentMotion(page: Page, blockId: string) {
  return page.locator(`.bn-block-outer[data-id="${blockId}"]`).evaluate((outer) => {
    const content = outer.querySelector(":scope > .bn-block > .bn-block-content");
    if (!(content instanceof HTMLElement)) throw new Error("Block content is missing");

    return {
      contentTransitionDuration: getComputedStyle(content).transitionDuration,
      guideTransitionDuration: getComputedStyle(outer, "::before").transitionDuration,
      marginLeft: getComputedStyle(outer).marginLeft,
      markerTransitionDuration: getComputedStyle(content, "::before").transitionDuration,
      outerTransitionDuration: getComputedStyle(outer).transitionDuration,
    };
  });
}

async function waitForHarness(page: Page) {
  await page.waitForFunction(() => window.__KOSH_BLOCKNOTE_HARNESS__?.capability === "blocknote");
}

async function readHarnessSnapshot(page: Page): Promise<HarnessSnapshot> {
  return (await page.evaluate(() =>
    window.__KOSH_BLOCKNOTE_HARNESS__!.snapshot(),
  )) as HarnessSnapshot;
}

function flatten(blocks: HarnessBlock[]): HarnessBlock[] {
  return blocks.flatMap((block) => [block, ...flatten(block.children)]);
}

function blockText(snapshot: HarnessSnapshot, blockId: string): string {
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
  const handleBox = await page.getByRole("button", { name: "Drag to move" }).boundingBox();
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

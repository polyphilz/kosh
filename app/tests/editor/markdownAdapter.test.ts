import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { koshBlocksToMarkdown, markdownToKoshBlocks } from "../../src/editor/markdownAdapter";
import { koshBlockNoteSchema } from "../../src/editor/schema";

interface MarkdownFixture {
  canonical: string;
  id: string;
}

const fixtures = JSON.parse(
  readFileSync("tests/fixtures/markdown/kosh-markdown-v1.json", "utf8"),
) as MarkdownFixture[];

describe("restricted BlockNote Markdown adapter", () => {
  it.each(fixtures)("round-trips the canonical $id fixture", ({ canonical }) => {
    expect(koshBlocksToMarkdown(markdownToKoshBlocks(canonical))).toBe(canonical);
  });

  it("maps the complete authored schema without BlockNote generic export", () => {
    const markdown = [
      "# Heading one",
      "",
      "## Heading two",
      "",
      "### Heading three",
      "",
      "Plain **bold**, *italic*, ~~strike~~, `code`, [safe](https://example.com/path), and $a_i$.",
      "",
      "- parent",
      "  - child",
      "",
      "1. first",
      "2. second",
      "",
      "```python",
      "array = [1, 2, 3]",
      "```",
      "",
      "$$",
      "\\sum_i a_i",
      "$$",
    ].join("\n");
    const blocks = markdownToKoshBlocks(markdown);

    expect(blocks.map((block) => block.type)).toEqual([
      "heading",
      "heading",
      "heading",
      "paragraph",
      "bulletListItem",
      "numberedListItem",
      "numberedListItem",
      "codeBlock",
      "displayMath",
    ]);
    expect(blocks[4]?.children?.[0]?.type).toBe("bulletListItem");
    expect(koshBlocksToMarkdown(blocks)).toBe(markdown);
  });

  it("keeps legacy constructs available but outside the creation schema", () => {
    expect(Object.keys(koshBlockNoteSchema.blockSchema)).toEqual([
      "paragraph",
      "heading",
      "bulletListItem",
      "numberedListItem",
      "codeBlock",
      "displayMath",
      "legacyMarkdown",
    ]);
    expect(Object.keys(koshBlockNoteSchema.blockSchema)).not.toEqual(
      expect.arrayContaining(["table", "quote", "checkListItem", "image", "audio", "video"]),
    );

    const legacy = markdownToKoshBlocks(
      "> quote\n\n---\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n- [x] retained",
    );
    expect(legacy.every((block) => block.type === "legacyMarkdown")).toBe(true);
  });

  it("neutralizes unsafe links and inline or block HTML", () => {
    const once = koshBlocksToMarkdown(
      markdownToKoshBlocks(
        "[click](javascript:alert(1)) <img src=x onerror=alert(1)>\n\n<script>alert(2)</script>",
      ),
    );
    const twice = koshBlocksToMarkdown(markdownToKoshBlocks(once));

    expect(once).not.toContain("](javascript:");
    expect(once).toContain("\\<img");
    expect(once).not.toMatch(/(^|[^\\])<img/u);
    expect(once).toContain("\\<script");
    expect(once).not.toMatch(/(^|[^\\])<script/u);
    expect(twice).toBe(once);
  });

  it("is idempotent across deterministic mixed documents", () => {
    const snippets = [
      "A shower thought with **weight**.",
      "- one\n  - nested",
      "```rust\nlet answer = 42;\n```",
      "Inline $x^2$ and ~~old~~ context.",
      "#### Preserved legacy heading",
    ];
    for (let offset = 0; offset < snippets.length; offset += 1) {
      const source = [...snippets.slice(offset), ...snippets.slice(0, offset)].join("\n\n");
      const canonical = koshBlocksToMarkdown(markdownToKoshBlocks(source));
      expect(koshBlocksToMarkdown(markdownToKoshBlocks(canonical))).toBe(canonical);
    }
  });

  it("retains styled or linked inline math without silently dropping its context", () => {
    for (const markdown of ["**$x$**", "~~$x$~~", "[$x$](https://example.com)"]) {
      const blocks = markdownToKoshBlocks(markdown);
      expect(blocks).toEqual([{ type: "legacyMarkdown", props: { markdown } }]);
      expect(koshBlocksToMarkdown(blocks)).toBe(markdown);
    }
  });
});

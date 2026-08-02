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
      "koshImage",
      "koshPendingMedia",
      "koshPdf",
      "koshFileAttachment",
      "koshLegacyMedia",
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

  it("round-trips canonical media as opaque Kosh blocks", () => {
    const markdown = [
      "{{kosh:image:019f547b-6200-7000-8000-000000000201;width=70%;alt=Diagram;caption=Overview}}",
      "",
      "{{kosh:pdf:019f547b-6200-7000-8000-000000000202}}",
      "",
      "{{kosh:attachment:019f547b-6200-7000-8000-000000000203;caption=Appendix}}",
    ].join("\n");
    const blocks = markdownToKoshBlocks(markdown);

    expect(blocks.map((block) => block.type)).toEqual([
      "koshImage",
      "koshPdf",
      "koshFileAttachment",
    ]);
    expect(blocks[0]?.props).toMatchObject({
      attachmentId: "019f547b-6200-7000-8000-000000000201",
      altText: "Diagram",
      caption: "Overview",
      widthPercent: 70,
    });
    expect(koshBlocksToMarkdown(blocks)).toBe(markdown);
    expect(JSON.stringify(blocks)).not.toMatch(/(?:blob:|data:|file:|\/Users\/)/u);
  });

  it("keeps malformed media references explicit and never persists pending placeholders", () => {
    const malformed = "{{kosh:image:not-a-uuid;width=70%}}";
    expect(markdownToKoshBlocks(malformed)).toEqual([
      { type: "koshLegacyMedia", props: { markdown: malformed } },
    ]);
    expect(
      koshBlocksToMarkdown([
        { type: "paragraph", content: "Before" },
        { type: "koshPendingMedia", props: { label: "Adding", requestId: "request" } },
        { type: "paragraph", content: "After" },
      ]),
    ).toBe("Before\n\nAfter");
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

  it("preserves reference links and fenced-code metadata losslessly", () => {
    expect(
      koshBlocksToMarkdown(
        markdownToKoshBlocks("Read [the docs][docs].\n\n[docs]: https://example.com/reference"),
      ),
    ).toBe("Read [the docs](https://example.com/reference).");
    const code = '```js title="example"\nconst answer = 42;\n```';
    expect(koshBlocksToMarkdown(markdownToKoshBlocks(code))).toBe(code);
  });

  it("preserves titled links as legacy Markdown instead of degrading them to text", () => {
    for (const markdown of [
      'Read [the docs](https://example.com/reference "Reference").',
      'Read [the docs][docs].\n\n[docs]: https://example.com/reference "Reference"',
    ]) {
      const blocks = markdownToKoshBlocks(markdown);
      expect(blocks[0]?.type).toBe("legacyMarkdown");
      expect(koshBlocksToMarkdown(blocks)).toBe(markdown);
    }
  });

  it("flattens nested non-list blocks without dropping their authored content", () => {
    expect(
      koshBlocksToMarkdown([
        {
          type: "paragraph",
          content: "Parent",
          children: [
            { type: "paragraph", content: "Nested paragraph" },
            { type: "heading", props: { level: 2 }, content: "Nested heading" },
          ],
        },
      ]),
    ).toBe("Parent\n\nNested paragraph\n\n## Nested heading");
  });

  it("keeps punctuation inside its authored style", () => {
    const markdown = koshBlocksToMarkdown([
      {
        type: "paragraph",
        content: [{ type: "text", text: "Hello!", styles: { bold: true } }],
      },
    ]);

    expect(markdown).toBe("**Hello!**");
    expect(koshBlocksToMarkdown(markdownToKoshBlocks(markdown))).toBe(markdown);
  });

  it("omits structural empty cursor paragraphs from persisted Markdown", () => {
    expect(
      koshBlocksToMarkdown([
        { type: "paragraph", content: "Before" },
        { type: "paragraph" },
        { type: "paragraph", content: "After" },
      ]),
    ).toBe("Before\n\nAfter");
  });

  it("canonicalizes adjacent styled runs without ambiguous Markdown delimiters", () => {
    const blocks = [
      {
        type: "paragraph",
        content: [
          { type: "text", text: "Bold", styles: { bold: true } },
          { type: "text", text: ", italic", styles: { italic: true } },
          { type: "text", text: ", strike", styles: { strike: true } },
          { type: "text", text: ", and code", styles: { code: true } },
        ],
      },
    ] as const;
    const markdown = koshBlocksToMarkdown(blocks);

    expect(markdown).toBe("**Bold**_, italic_~~, strike~~`, and code`");
    expect(koshBlocksToMarkdown(markdownToKoshBlocks(markdown))).toBe(markdown);
  });

  it("canonicalizes adjacent styled runs nested inside links", () => {
    const markdown = koshBlocksToMarkdown([
      {
        type: "paragraph",
        content: [
          {
            type: "link",
            href: "https://example.com",
            content: [
              { type: "text", text: "Bold", styles: { bold: true } },
              { type: "text", text: ", italic", styles: { italic: true } },
            ],
          },
        ],
      },
    ]);

    expect(markdown).toBe("[**Bold**_, italic_](https://example.com/)");
    expect(koshBlocksToMarkdown(markdownToKoshBlocks(markdown))).toBe(markdown);
  });
});

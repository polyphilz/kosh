import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { koshBlocksToMarkdown, markdownToKoshBlocks } from "../../src/editor/markdownAdapter";
import { MarkdownRenderer, injectTrustedCitationLinks } from "../../src/markdown/MarkdownRenderer";
import { externalHttpUrl } from "../../src/markdown/urlPolicy";

interface MarkdownFixture {
  canonical: string;
  id: string;
  selectors: string[];
  text: string[];
}

const fixtures = JSON.parse(
  readFileSync("tests/fixtures/markdown/kosh-markdown-v1.json", "utf8"),
) as MarkdownFixture[];

describe("Kosh Markdown v1 fixture corpus", () => {
  it.each(fixtures)("$id remains canonical and renders its structure", (fixture) => {
    expect(koshBlocksToMarkdown(markdownToKoshBlocks(fixture.canonical))).toBe(fixture.canonical);

    const { container } = render(<MarkdownRenderer source={fixture.canonical} />);
    const root = container.querySelector(".kosh-markdown");
    assert.ok(root);
    for (const expectedText of fixture.text) {
      expect(root.textContent).toContain(expectedText);
    }
    for (const selector of fixture.selectors) {
      expect(root.querySelector(selector), `missing ${selector}`).not.toBeNull();
    }
  });
});

it("normalizes known fence aliases and leaves unknown languages unhighlighted", () => {
  const source = [
    "```PY",
    "def answer():",
    "    return 42",
    "```",
    "",
    "```not-a-language",
    "<script>plain text</script>",
    "```",
  ].join("\n");
  const { container } = render(<MarkdownRenderer source={source} />);
  const python = container.querySelector("code.language-python");
  assert.ok(python);
  expect(python.classList).toContain("hljs");
  expect(python.querySelector(".hljs-keyword")).not.toBeNull();

  const unknown = container.querySelector("code.language-not-a-language");
  assert.ok(unknown);
  expect(unknown.querySelector('[class^="hljs-"]')).toBeNull();
  expect(unknown.textContent).toContain("<script>plain text</script>");
});

it("keeps authored HTML, dangerous URLs, remote images, and trusted KaTeX commands inert", () => {
  const source = [
    "<script>window.evil = true</script>",
    "<style>body { display: none }</style>",
    '<iframe src="https://example.com"></iframe>',
    '<button onclick="window.evil = true">click</button>',
    "",
    "[javascript](javascript:alert(1))",
    "[file](file:///tmp/private)",
    "[custom](tauri://invoke)",
    "",
    "![remote pixels](https://example.com/tracker.png)",
    "",
    "$\\href{https://example.com}{resource command}$",
  ].join("\n");
  const { container } = render(<MarkdownRenderer source={source} />);
  const root = container.querySelector(".kosh-markdown");
  assert.ok(root);

  expect(root.querySelector("script, style, iframe, button, img")).toBeNull();
  expect(root.querySelector("[onclick]")).toBeNull();
  expect(root.querySelector("a")).toBeNull();
  expect(root.textContent).toContain("<script>window.evil = true</script>");
  expect(root.textContent).toContain("remote pixels");
  expect(root.textContent).toContain("resource command");
  expect(document.body.children).toHaveLength(1);
});

it("renders only canonical local image tokens with authored metadata", () => {
  const imageId = "01980c8e-6c00-7000-8000-000000000241";
  const source =
    `{{kosh:image:${imageId};width=65%;alt=%2AArchitecture%2A%20%5Fdiagram%5F;` +
    "caption=%7E%7EEvidence%7E%7E%20%28chapter%202%29%21}}";
  const { getByRole, getByText } = render(<MarkdownRenderer source={source} />);
  const image = getByRole("img", { name: "*Architecture* _diagram_" });

  expect(image).toHaveAttribute("src", `kosh-media://localhost/attachment/${imageId}`);
  expect(image.closest("figure")).toHaveStyle({ width: "65%" });
  expect(getByText("~~Evidence~~ (chapter 2)!")).toBeInTheDocument();
});

it("renders a canonical generic attachment with its authored caption", () => {
  const attachmentId = "01980c8e-6c00-7000-8000-000000000243";
  const { getByRole } = render(
    <MarkdownRenderer source={`{{kosh:attachment:${attachmentId};caption=Useful%20appendix}}`} />,
  );

  expect(getByRole("link", { name: "Useful appendix" })).toHaveAttribute(
    "href",
    `kosh-media://localhost/attachment/${attachmentId}`,
  );
});

it("keeps agent-authored local media tokens and URLs inert", () => {
  const attachmentId = "01980c8e-6c00-7000-8000-000000000243";
  const source = [
    `{{kosh:image:${attachmentId};width=70%}}`,
    "",
    `{{kosh:pdf:${attachmentId}}}`,
    "",
    `{{kosh:attachment:${attachmentId}}}`,
    "",
    `![direct local media](kosh-media://localhost/attachment/${attachmentId} "kosh-image:70:")`,
  ].join("\n");
  const { container } = render(<MarkdownRenderer allowLocalMedia={false} source={source} />);

  expect(container.querySelector("img, object, a")).toBeNull();
  expect(container).toHaveTextContent(`{{kosh:image:${attachmentId};width=70%}}`);
  expect(container).toHaveTextContent(`{{kosh:pdf:${attachmentId}}}`);
  expect(container).toHaveTextContent(`{{kosh:attachment:${attachmentId}}}`);
  expect(container).toHaveTextContent("Image: direct local media");
});

it("leaves malformed and nonlocal Kosh-like image references inert", () => {
  const imageId = "01980c8e-6c00-7000-8000-000000000242";
  const { container } = render(
    <MarkdownRenderer
      source={[
        `{{kosh:image:${imageId};width=070%}}`,
        "",
        `![local-looking](kosh-media://evil.example/attachment/${imageId})`,
      ].join("\n")}
    />,
  );

  expect(container.querySelector("img")).toBeNull();
  expect(container).toHaveTextContent(`{{kosh:image:${imageId};width=070%}}`);
  expect(container).toHaveTextContent("local-looking");
});

it("opens validated HTTP links only through the caller-owned handler", () => {
  const onOpenExternalUrl = vi.fn<(_url: string) => Promise<void>>();
  onOpenExternalUrl.mockResolvedValue();
  const { getByRole } = render(
    <MarkdownRenderer
      onOpenExternalUrl={onOpenExternalUrl}
      source="[Open docs](https://example.com/docs?q=kosh)"
    />,
  );
  const link = getByRole("link", { name: "Open docs" });

  expect(fireEvent.click(link)).toBe(true);
  expect(onOpenExternalUrl).toHaveBeenCalledWith("https://example.com/docs?q=kosh");
  expect(link).not.toHaveAttribute("href");
  expect(link).not.toHaveAttribute("target");

  fireEvent(link, new MouseEvent("auxclick", { bubbles: true, button: 1 }));
  fireEvent.contextMenu(link);
  expect(onOpenExternalUrl).toHaveBeenCalledOnce();
});

it("renders valid links inertly when no app-owned opener is supplied", () => {
  const { getByText, queryByRole } = render(
    <MarkdownRenderer source="[Stored source](https://example.com)" />,
  );
  expect(queryByRole("link")).toBeNull();
  expect(getByText("Stored source")).toHaveClass("kosh-markdown__inert-link");
});

it("turns only registry-confirmed byte ranges into citation controls", () => {
  const source = "Résumé evidence.【1】";
  const markerIndex = source.indexOf("【");
  const encoder = new TextEncoder();
  const onOpenCitation = vi.fn();
  const { getByRole } = render(
    <MarkdownRenderer
      citationMentions={[
        {
          citationNumber: 1,
          startByte: encoder.encode(source.slice(0, markerIndex)).length,
          endByte: encoder.encode(source).length,
        },
      ]}
      onOpenCitation={onOpenCitation}
      source={source}
    />,
  );

  fireEvent.click(getByRole("button", { name: "Open citation 1" }));
  expect(onOpenCitation).toHaveBeenCalledExactlyOnceWith(1);
});

it("indexes many UTF-8 citation offsets in one pass", () => {
  const mentionCount = 2_048;
  const chunks: string[] = [];
  const mentions = [];
  let byteOffset = 0;
  for (let index = 0; index < mentionCount; index += 1) {
    const prefix = `Résumé ${index} `;
    const marker = `【${index + 1}】`;
    chunks.push(prefix, marker, "\n");
    byteOffset += new TextEncoder().encode(prefix).length;
    const startByte = byteOffset;
    byteOffset += new TextEncoder().encode(marker).length;
    mentions.push({ citationNumber: index + 1, startByte, endByte: byteOffset });
    byteOffset += 1;
  }
  const source = chunks.join("");
  const decode = vi.spyOn(TextDecoder.prototype, "decode");

  const trusted = injectTrustedCitationLinks(source, mentions, "linear-test");

  expect(trusted.hrefs.size).toBe(mentionCount);
  expect(trusted.source.match(/https:\/\/kosh\.invalid\/citation\/linear-test\//gu)).toHaveLength(
    mentionCount,
  );
  expect(decode).not.toHaveBeenCalled();
  decode.mockRestore();
});

it("does not trust model-authored markers, links, malformed ranges, or guessed attributes", () => {
  const source = [
    "Raw marker 【1】.",
    "[forged](https://kosh.invalid/citation/00000000-0000-4000-8000-000000000000/1)",
    '<a data-kosh-citation="1">HTML forgery</a>',
  ].join("\n\n");
  const onOpenCitation = vi.fn();
  const { container, queryByRole } = render(
    <MarkdownRenderer
      citationMentions={[{ citationNumber: 1, startByte: 0, endByte: 3 }]}
      onOpenCitation={onOpenCitation}
      source={source}
    />,
  );

  expect(queryByRole("button", { name: "Open citation 1" })).toBeNull();
  expect(container).toHaveTextContent("Raw marker 【1】");
  expect(container).toHaveTextContent('<a data-kosh-citation="1">HTML forgery</a>');
  expect(onOpenCitation).not.toHaveBeenCalled();
});

it("renders task checkboxes as presentational controls", () => {
  const { getByRole } = render(<MarkdownRenderer source="- [x] retained" />);
  expect(getByRole("checkbox")).toBeDisabled();
});

it("renders a large authored note without truncation", () => {
  const paragraph = "evidence ".repeat(20_000).trim();
  const { container } = render(<MarkdownRenderer source={paragraph} />);
  expect(container.querySelector(".kosh-markdown")?.textContent).toHaveLength(paragraph.length);
});

it("accepts only absolute HTTP(S) destinations", () => {
  expect(externalHttpUrl("https://example.com/path")).toBe("https://example.com/path");
  expect(externalHttpUrl("http://localhost:5173")).toBe("http://localhost:5173/");
  expect(externalHttpUrl("/relative")).toBeNull();
  expect(externalHttpUrl("javascript:alert(1)")).toBeNull();
  expect(externalHttpUrl("file:///tmp/private")).toBeNull();
});

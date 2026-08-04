import { describe, expect, it } from "vitest";
import { withoutKoshStructureMarkers } from "../../src/markdown/structureMarkers";
import { hasMeaningfulAuthoredContent } from "../../src/notes/content";

describe("editor structure markers", () => {
  it("removes standalone editor markers but preserves identical fenced code", () => {
    const markdown = [
      "Visible prose",
      "",
      "<!-- kosh:block:empty -->",
      "",
      "```html",
      "<!-- kosh:block:empty -->",
      "```",
    ].join("\n");

    expect(withoutKoshStructureMarkers(markdown)).toBe(
      ["Visible prose", "", "", "```html", "<!-- kosh:block:empty -->", "```"].join("\n"),
    );
  });

  it("preserves reserved-looking indented code as authored content", () => {
    const markdown = "    <!-- kosh:children:start -->";

    expect(withoutKoshStructureMarkers(markdown)).toBe(markdown);
    expect(hasMeaningfulAuthoredContent(markdown)).toBe(true);
  });

  it("treats code containing only reserved-looking text as meaningful", () => {
    expect(hasMeaningfulAuthoredContent("```html\n<!-- kosh:block:empty -->\n```")).toBe(true);
    expect(hasMeaningfulAuthoredContent("```html\n\n```")).toBe(false);
  });
});

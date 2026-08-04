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

  it("removes deeply indented structure markers emitted by nested lists", () => {
    const markdown = "    <!-- kosh:children:start -->";

    expect(withoutKoshStructureMarkers(markdown)).toBe("");
    expect(hasMeaningfulAuthoredContent(markdown)).toBe(false);
  });

  it("treats code containing only reserved-looking text as meaningful", () => {
    expect(hasMeaningfulAuthoredContent("```html\n<!-- kosh:block:empty -->\n```")).toBe(true);
    expect(hasMeaningfulAuthoredContent("```html\n\n```")).toBe(false);
  });
});

import { describe, expect, it } from "vitest";
import { highlightedSegments } from "../../src/search/presentation";

describe("search presentation", () => {
  it("maps Unicode scalar highlight offsets without breaking the original text", () => {
    expect(
      highlightedSegments(
        "A naïve café note",
        [
          { field: "BODY", startChar: 2, endChar: 7 },
          { field: "BODY", startChar: 8, endChar: 12 },
        ],
        ["BODY"],
      ),
    ).toEqual([
      { highlighted: false, text: "A " },
      { highlighted: true, text: "naïve" },
      { highlighted: false, text: " " },
      { highlighted: true, text: "café" },
      { highlighted: false, text: " note" },
    ]);
  });

  it("merges overlapping ranges and clamps malformed derived offsets", () => {
    expect(
      highlightedSegments(
        "citation",
        [
          { field: "BODY", startChar: -3, endChar: 4 },
          { field: "BODY", startChar: 3, endChar: 200 },
        ],
        ["BODY"],
      ),
    ).toEqual([{ highlighted: true, text: "citation" }]);
  });
});

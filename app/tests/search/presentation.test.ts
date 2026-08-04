import { describe, expect, it } from "vitest";
import type { CitationResolution } from "../../src/backend/contracts";
import {
  citationCopyText,
  citationLocation,
  highlightedSegments,
} from "../../src/search/presentation";

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

  it("formats typed locators and copies only resolved provenance", () => {
    const citation: CitationResolution = {
      passageId: "passage-1",
      excerpt: "Supported text.",
      headingContext: ["Chapter", "Evidence"],
      constructionVersion: "v1",
      state: "HISTORICAL",
      locator: { kind: "PDF_PAGE", page: 7 },
      tidbit: {
        id: "tidbit-1",
        revisionId: "revision-3",
        revisionNumber: 3,
        displayTitle: "Proof",
        deleted: false,
      },
      attachment: {
        id: "attachment-1",
        extractionId: "extraction-1",
        displayFilename: "paper.pdf",
        mediaType: "application/pdf",
        deleted: false,
      },
    };

    expect(citationLocation(citation)).toBe("Chapter › Evidence · page 7");
    expect(citationCopyText(citation)).toBe(
      [
        "paper.pdf",
        "Chapter › Evidence · page 7",
        "Revision 3 · Historical",
        "Kosh passage: passage-1",
        "Supported text.",
      ].join("\n"),
    );
  });

  it("preserves whitespace-sensitive excerpts in copied citations", () => {
    const citation: CitationResolution = {
      passageId: "passage-code",
      excerpt: "  const answer = 42;\n",
      headingContext: ["Example"],
      constructionVersion: "v1",
      state: "CURRENT",
      locator: {
        kind: "MARKDOWN_BLOCKS",
        startBlock: 0,
        endBlock: 0,
        startChar: 0,
        endChar: 21,
        startLine: 1,
        endLine: 1,
      },
      tidbit: {
        id: "tidbit-code",
        revisionId: "revision-code",
        revisionNumber: 1,
        displayTitle: "Code sample",
        deleted: false,
      },
      attachment: null,
    };

    expect(citationCopyText(citation)).toBe(
      [
        "Code sample",
        "Example · line 1",
        "Revision 1 · Current",
        "Kosh passage: passage-code",
        "  const answer = 42;\n",
      ].join("\n"),
    );
  });
});
